//! Spectral repair: a spectral gate and noise reduction from a learned noise
//! profile, on the [`Stft`] frame of this crate.
//!
//! # Method
//!
//! Every hop the STFT hands this module one spectrum per channel. For each bin
//! `k` the module has the frame power `P = |X_k|^2` and, once learned, the
//! noise power `N_k` (the mean of `|X_k|^2` over the frames of a learn period).
//! A real gain `g_k` in `[floor, 1]` is computed, smoothed, and applied to the
//! bin. Phase is untouched.
//!
//! * **Gate** ([`RepairMode::Gate`]): `g = 1` where `P >= N * threshold`, else
//!   the floor. The threshold is set in dB above the learned noise.
//! * **Spectral subtraction** ([`GainRule::SpectralSubtraction`]): S. F. Boll,
//!   "Suppression of acoustic noise in speech using spectral subtraction",
//!   IEEE Trans. ASSP 27(2), 1979, with the over-subtraction factor `alpha`
//!   and spectral floor `beta` of M. Berouti, R. Schwartz and J. Makhoul,
//!   "Enhancement of speech corrupted by acoustic noise", ICASSP 1979:
//!   `|S|^2 = max(P - alpha N, beta N)`, so `g = sqrt(|S|^2 / P)`.
//! * **Wiener** ([`GainRule::Wiener`]): `g = xi / (1 + xi)` with the a-priori
//!   SNR `xi`.
//! * **MMSE short-time spectral amplitude** ([`GainRule::MmseStsa`]): Y.
//!   Ephraim and D. Malah, "Speech enhancement using a minimum mean-square
//!   error short-time spectral amplitude estimator", IEEE Trans. ASSP 32(6),
//!   1984, eq. 7: with `v = xi / (1 + xi) * gamma` and `gamma = P / N`,
//!   `g = (sqrt(pi) / 2) (sqrt(v) / gamma) exp(-v/2) [(1+v) I0(v/2) + v I1(v/2)]`.
//!   The exponential is folded into exponentially scaled Bessel functions
//!   (Abramowitz and Stegun 9.8.1 to 9.8.4) so large `v` does not overflow.
//!
//! The a-priori SNR is either the maximum-likelihood `xi = max(gamma - 1, 0)`
//! or the decision-directed estimate of Ephraim and Malah (1984, eq. 51):
//! `xi_t = a * A_{t-1}^2 / N + (1 - a) * max(gamma_t - 1, 0)`, where
//! `A_{t-1}^2 = g_{t-1}^2 P_{t-1}` is the previous frame's clean-power estimate
//! (the rule's own gain, before time smoothing) and `a` is about 0.98. The
//! over-subtraction factor scales the noise power seen by Wiener and MMSE too
//! (`N' = alpha N`). `xi` is floored at -25 dB as in the paper.
//!
//! Gains are then smoothed to limit musical noise (isolated spectral peaks
//! that come and go): a box average across `2r + 1` bins, then a one-pole
//! smoother over frames per bin with separate attack (gain rising, the gate
//! opening) and release (gain falling) times. The result is clamped to
//! `[floor, 1]` where `floor = 10^(-amount_db / 20)`: the reduction never
//! exceeds `amount_db`, so noise is lowered rather than muted.
//!
//! # Profile
//!
//! [`SpectralRepair::start_learning`] zeroes an accumulator;
//! every frame while learning adds `|X_k|^2` per channel (frames still
//! containing the zeros from start-up are skipped), and
//! [`SpectralRepair::stop_learning`] replaces the profile with the mean. Power
//! is averaged rather than magnitude: it is the expectation the rules need,
//! and the square of a mean magnitude of Gaussian noise is `pi/4` of the mean
//! power. The error of the mean falls as `1 / sqrt(frames)` (frames at 75 %
//! overlap are correlated, so by a little less). While learning the audio
//! passes through. The profile is in the units of this STFT (sqrt-Hann both
//! sides), so it is only valid for the same frame size and hop;
//! [`SpectralRepair::save_profile`] and [`SpectralRepair::load_profile`] carry
//! it as versioned bytes and refuse anything else.
//!
//! # Identity
//!
//! When disabled, when `amount_db` is zero, while learning, or with no profile,
//! the spectra are not touched at all, so the output equals the plain [`Stft`]
//! identity path (the input delayed by [`SpectralRepair::latency`]) bit for
//! bit. "Listen to removed" has no effect then.
//!
//! Nothing allocates after `new` except `save_profile`. Non-finite input is
//! zeroed by the STFT; non-finite parameters are ignored.

// Per-bin loops index several parallel arrays; the Bessel coefficients are the
// published A&S digits.
#![allow(clippy::needless_range_loop, clippy::excessive_precision)]

use crate::stft::{Complex, SpectralProcessor, Stft, StftConfig, StftError};
use crate::windows::WindowKind;

const MAGIC: [u8; 4] = *b"OPNP";
const VERSION: u32 = 1;
/// A-priori SNR floor, -25 dB (Ephraim and Malah 1984).
const XI_MIN: f32 = 0.003_162_277_7;
const TINY: f32 = 1e-30;
/// Largest reduction depth in dB.
pub const MAX_AMOUNT_DB: f32 = 96.0;
/// Largest frequency-smoothing radius in bins.
pub const MAX_SMOOTH_BINS: usize = 32;

/// What the module does with the learned profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RepairMode {
    /// Per-bin gate: open where the frame exceeds the profile by the threshold.
    Gate,
    /// Noise reduction with a [`GainRule`].
    Reduce,
}

/// Gain rule for [`RepairMode::Reduce`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GainRule {
    /// Boll 1979 with Berouti et al. 1979 over-subtraction and floor.
    SpectralSubtraction,
    /// `xi / (1 + xi)`.
    Wiener,
    /// Ephraim and Malah 1984 MMSE short-time spectral amplitude.
    MmseStsa,
}

/// Parameters. Out-of-range values are clamped and non-finite ones ignored by
/// [`SpectralRepair::set_params`].
/// # Example
///
/// ```
/// use openplugin_dsp::spectral_repair::{GainRule, RepairMode, RepairParams};
///
/// let p = RepairParams {
///     mode: RepairMode::Reduce,
///     rule: GainRule::Wiener,
///     amount_db: 12.0,
///     ..RepairParams::default()
/// };
/// assert!(p.enabled);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RepairParams {
    /// Master switch; off is the STFT identity path.
    pub enabled: bool,
    /// Gate or noise reduction.
    pub mode: RepairMode,
    /// Greatest reduction in dB, `0..=96`. Zero is identity.
    pub amount_db: f32,
    /// Output what was removed (`X (1 - g)`) instead of what remains.
    pub listen_removed: bool,
    /// Gate: threshold above the learned noise, dB (`-20..=60`).
    pub threshold_db: f32,
    /// Gain rule used by [`RepairMode::Reduce`].
    pub rule: GainRule,
    /// Over-subtraction factor `alpha` (`0.1..=10`); scales the noise power.
    pub over_subtraction: f32,
    /// Spectral floor `beta`, a fraction of the noise power kept (`0..=1`).
    pub spectral_floor: f32,
    /// Decision-directed a-priori SNR (Wiener and MMSE rules).
    pub decision_directed: bool,
    /// Decision-directed smoothing `a` (`0..=0.999`).
    pub dd_alpha: f32,
    /// Time for the gain to rise (gate opens), ms (`0..=2000`).
    pub attack_ms: f32,
    /// Time for the gain to fall (gate closes), ms (`0..=2000`).
    pub release_ms: f32,
    /// Frequency smoothing radius in bins (`0..=32`).
    pub freq_smooth_bins: usize,
}

impl Default for RepairParams {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: RepairMode::Reduce,
            amount_db: 18.0,
            listen_removed: false,
            threshold_db: 6.0,
            rule: GainRule::SpectralSubtraction,
            over_subtraction: 2.0,
            spectral_floor: 0.02,
            decision_directed: false,
            dd_alpha: 0.98,
            attack_ms: 5.0,
            release_ms: 60.0,
            freq_smooth_bins: 1,
        }
    }
}

/// Why profile bytes were refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProfileError {
    /// Shorter than the header or than its own counts say.
    Truncated,
    /// Not a profile (wrong magic).
    BadMagic,
    /// A version this build does not read.
    UnsupportedVersion(u32),
    /// Frame size, hop or channel count differ from this instance.
    Mismatch,
    /// Checksum differs, or extra bytes follow.
    Corrupt,
    /// A power is NaN, infinite or negative, or a zero count has non-zero powers.
    BadValue,
}

struct ChannelState {
    profile: Vec<f32>,
    profile_frames: u32,
    sum: Vec<f64>,
    sum_frames: u32,
    gain: Vec<f32>,
    prev_clean: Vec<f32>,
    warm: u32,
}

struct Core {
    bins: usize,
    warm_frames: u32,
    ch: Vec<ChannelState>,
    raw: Vec<f32>,
    sm: Vec<f32>,
    params: RepairParams,
    att: f32,
    rel: f32,
    g_min: f32,
    thr: f32,
    learning: bool,
    sample_rate: f32,
    hop: usize,
}

impl Core {
    fn apply_params(&mut self, p: RepairParams) {
        let d = self.params;
        let f = |v: f32, lo: f32, hi: f32, old: f32| if v.is_finite() { v.clamp(lo, hi) } else { old };
        let q = RepairParams {
            enabled: p.enabled,
            mode: p.mode,
            amount_db: f(p.amount_db, 0.0, MAX_AMOUNT_DB, d.amount_db),
            listen_removed: p.listen_removed,
            threshold_db: f(p.threshold_db, -20.0, 60.0, d.threshold_db),
            rule: p.rule,
            over_subtraction: f(p.over_subtraction, 0.1, 10.0, d.over_subtraction),
            spectral_floor: f(p.spectral_floor, 0.0, 1.0, d.spectral_floor),
            decision_directed: p.decision_directed,
            dd_alpha: f(p.dd_alpha, 0.0, 0.999, d.dd_alpha),
            attack_ms: f(p.attack_ms, 0.0, 2000.0, d.attack_ms),
            release_ms: f(p.release_ms, 0.0, 2000.0, d.release_ms),
            freq_smooth_bins: p.freq_smooth_bins.min(MAX_SMOOTH_BINS),
        };
        let coef = |ms: f32| {
            if ms <= 0.0 {
                0.0
            } else {
                (-(self.hop as f32) / (self.sample_rate * ms * 1e-3)).exp()
            }
        };
        self.att = coef(q.attack_ms);
        self.rel = coef(q.release_ms);
        self.g_min = 10f32.powf(-q.amount_db / 20.0);
        self.thr = 10f32.powf(q.threshold_db / 10.0);
        self.params = q;
    }

    fn active(&self) -> bool {
        self.params.enabled && self.params.amount_db > 0.0
    }

    fn bypass_frame(&mut self, channel: usize, bins: &[Complex<f32>]) {
        let s = &mut self.ch[channel];
        for ((g, c), x) in s.gain.iter_mut().zip(s.prev_clean.iter_mut()).zip(bins) {
            *g = 1.0;
            *c = x.re * x.re + x.im * x.im;
        }
    }
}

impl SpectralProcessor for Core {
    fn process_frame(&mut self, channel: usize, bins: &mut [Complex<f32>]) {
        let nb = self.bins;
        if bins.len() != nb || channel >= self.ch.len() {
            return;
        }
        let warm = self.ch[channel].warm;
        if warm < self.warm_frames {
            self.ch[channel].warm = warm + 1;
        }
        if self.learning {
            let s = &mut self.ch[channel];
            // Frames that still hold the zeros of start-up are not noise.
            if warm >= self.warm_frames {
                for (a, x) in s.sum.iter_mut().zip(bins.iter()) {
                    *a += (x.re * x.re + x.im * x.im) as f64;
                }
                s.sum_frames = s.sum_frames.saturating_add(1);
            }
            self.bypass_frame(channel, bins);
            return;
        }
        if !self.active() || self.ch[channel].profile_frames == 0 {
            self.bypass_frame(channel, bins);
            return;
        }

        let p = self.params;
        let g_min = self.g_min;
        let thr = self.thr;
        let alpha = p.over_subtraction;
        let beta = p.spectral_floor;
        let s = &mut self.ch[channel];
        for k in 0..nb {
            let x = bins[k];
            let pw = x.re * x.re + x.im * x.im;
            let n = s.profile[k].max(1e-20);
            let g = if p.mode == RepairMode::Gate {
                if pw >= n * thr {
                    1.0
                } else {
                    g_min
                }
            } else if pw <= TINY {
                1.0
            } else {
                let an = alpha * n;
                let g = match p.rule {
                    GainRule::SpectralSubtraction => ((pw - an) / pw).max(0.0).sqrt(),
                    GainRule::Wiener | GainRule::MmseStsa => {
                        let gamma = pw / an;
                        let ml = (gamma - 1.0).max(0.0);
                        let xi = if p.decision_directed {
                            p.dd_alpha * s.prev_clean[k] / an + (1.0 - p.dd_alpha) * ml
                        } else {
                            ml
                        }
                        .max(XI_MIN);
                        if p.rule == GainRule::Wiener {
                            xi / (1.0 + xi)
                        } else {
                            let v = xi / (1.0 + xi) * gamma;
                            let h = 0.5 * v;
                            0.886_226_9 * v.sqrt() / gamma * ((1.0 + v) * i0e(h) + v * i1e(h))
                        }
                    }
                };
                let g = g.max((beta * n / pw).sqrt()).min(1.0);
                s.prev_clean[k] = g * g * pw;
                g
            };
            self.raw[k] = if g.is_finite() { g.clamp(g_min, 1.0) } else { 1.0 };
        }
        // Frequency smoothing.
        let r = p.freq_smooth_bins;
        if r == 0 {
            self.sm[..nb].copy_from_slice(&self.raw[..nb]);
        } else {
            for k in 0..nb {
                let lo = k.saturating_sub(r);
                let hi = (k + r).min(nb - 1);
                let mut acc = 0.0f32;
                for v in &self.raw[lo..=hi] {
                    acc += *v;
                }
                self.sm[k] = acc / (hi - lo + 1) as f32;
            }
        }
        // Time smoothing and application.
        for k in 0..nb {
            let target = self.sm[k];
            let cur = s.gain[k];
            let c = if target > cur { self.att } else { self.rel };
            let g = (c * cur + (1.0 - c) * target).clamp(g_min, 1.0);
            s.gain[k] = g;
            let m = if p.listen_removed { 1.0 - g } else { g };
            bins[k] = Complex::new(bins[k].re * m, bins[k].im * m);
        }
    }
}

/// Exponentially scaled `exp(-x) I0(x)`, `x >= 0` (Abramowitz and Stegun 9.8.1, 9.8.2).
fn i0e(x: f32) -> f32 {
    if x < 3.75 {
        let t = x / 3.75;
        let y = t * t;
        let p = 1.0
            + y * (3.515_622_9
                + y * (3.089_942_4
                    + y * (1.206_749_2 + y * (0.265_973_2 + y * (0.036_076_8 + y * 0.004_581_3)))));
        p * (-x).exp()
    } else {
        let t = 3.75 / x;
        let p = 0.398_942_28
            + t * (0.013_285_92
                + t * (0.002_253_19
                    + t * (-0.001_575_65
                        + t * (0.009_162_81
                            + t * (-0.020_577_06
                                + t * (0.026_355_37 + t * (-0.016_476_33 + t * 0.003_923_77)))))));
        p / x.sqrt()
    }
}

/// Exponentially scaled `exp(-x) I1(x)`, `x >= 0` (A&S 9.8.3, 9.8.4).
fn i1e(x: f32) -> f32 {
    if x < 3.75 {
        let t = x / 3.75;
        let y = t * t;
        let p = 0.5
            + y * (0.878_905_94
                + y * (0.514_988_69
                    + y * (0.150_849_34 + y * (0.026_587_33 + y * (0.003_015_32 + y * 0.000_324_11)))));
        x * p * (-x).exp()
    } else {
        let t = 3.75 / x;
        let p = 0.398_942_28
            + t * (-0.039_880_24
                + t * (-0.003_620_18
                    + t * (0.001_638_01
                        + t * (-0.010_315_55
                            + t * (0.022_829_67
                                + t * (-0.028_953_12 + t * (0.017_876_54 + t * -0.004_200_59)))))));
        p / x.sqrt()
    }
}

/// Settings for [`SpectralRepair::new`].
/// # Example
///
/// ```
/// use openplugin_dsp::spectral_repair::RepairConfig;
///
/// let config = RepairConfig::new(48_000.0, 2, 2048);
/// assert_eq!(config.hop, 512);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RepairConfig {
    /// Sample rate, Hz; one that is not finite and positive counts as 48 kHz.
    pub sample_rate: f32,
    /// Channel count; 0 counts as 1.
    pub channels: usize,
    /// STFT frame, a power of two of at least 4 (2048 at 48 kHz is typical).
    pub frame: usize,
    /// STFT hop; `frame / 4` keeps the sqrt-Hann pair exactly COLA.
    pub hop: usize,
}

impl RepairConfig {
    /// A configuration with the hop at `frame / 4`.
    pub fn new(sample_rate: f32, channels: usize, frame: usize) -> Self {
        Self { sample_rate, channels, frame, hop: frame / 4 }
    }
}

/// Spectral gate and learned-profile noise reduction.
/// `new` allocates everything; processing, learning and `load_profile` do
/// not. Feed it on the audio thread; `save_profile` allocates.
///
/// # Example
///
/// ```
/// use openplugin_dsp::spectral_repair::{RepairConfig, SpectralRepair};
///
/// let mut repair = SpectralRepair::new(RepairConfig::new(48_000.0, 1, 1024)).unwrap();
///
/// // Learn the noise from a passage of noise only.
/// let noise: Vec<f32> = (0..24_000u32)
///     .map(|n| (n.wrapping_mul(2_654_435_761) >> 8) as f32 / 16_777_216.0 * 0.02 - 0.01)
///     .collect();
/// let mut out = vec![0.0f32; noise.len()];
/// repair.start_learning();
/// repair.process(0, &noise, &mut out);
/// assert!(repair.stop_learning());
///
/// // Now the same noise is reduced.
/// repair.process(0, &noise, &mut out);
/// let energy = |x: &[f32]| x.iter().map(|s| s * s).sum::<f32>();
/// assert!(energy(&out[12_000..]) < energy(&noise[12_000..]));
/// assert_eq!(repair.latency(), 1024);
/// ```
pub struct SpectralRepair {
    stft: Stft,
    core: Core,
    frame: usize,
    hop: usize,
}

impl SpectralRepair {
    /// Prepare: allocates everything. Sqrt-Hann analysis and synthesis.
    pub fn new(config: RepairConfig) -> Result<Self, StftError> {
        let channels = config.channels.max(1);
        let stft = Stft::new(channels, Self::stft_config(config.frame, config.hop))?;
        let bins = stft.bins();
        let sr = if config.sample_rate.is_finite() && config.sample_rate > 0.0 {
            config.sample_rate
        } else {
            48_000.0
        };
        let mut core = Core {
            bins,
            warm_frames: (config.frame / config.hop.max(1)) as u32,
            ch: (0..channels)
                .map(|_| ChannelState {
                    profile: vec![0.0; bins],
                    profile_frames: 0,
                    sum: vec![0.0; bins],
                    sum_frames: 0,
                    gain: vec![1.0; bins],
                    prev_clean: vec![0.0; bins],
                    warm: 0,
                })
                .collect(),
            raw: vec![1.0; bins],
            sm: vec![1.0; bins],
            params: RepairParams::default(),
            att: 0.0,
            rel: 0.0,
            g_min: 1.0,
            thr: 1.0,
            learning: false,
            sample_rate: sr,
            hop: config.hop,
        };
        core.apply_params(RepairParams::default());
        Ok(Self { stft, core, frame: config.frame, hop: config.hop })
    }

    /// The STFT configuration used for `frame` and `hop`. A plain [`Stft`] built
    /// from it with [`crate::Identity`] is the reference for the identity path.
    pub fn stft_config(frame: usize, hop: usize) -> StftConfig {
        StftConfig {
            frame,
            hop,
            analysis: WindowKind::SqrtHann,
            synthesis: WindowKind::SqrtHann,
        }
    }

    /// The parameters in use (after clamping).
    pub fn params(&self) -> RepairParams {
        self.core.params
    }

    /// Applies parameters: out-of-range values are clamped and non-finite ones
    /// ignored. Realtime-safe.
    pub fn set_params(&mut self, p: RepairParams) {
        self.core.apply_params(p);
    }

    /// Latency in samples for `Plugin::latency`: the STFT's.
    pub fn latency(&self) -> usize {
        self.stft.latency()
    }

    /// The channel count (at least 1).
    pub fn channels(&self) -> usize {
        self.core.ch.len()
    }

    /// Bins per spectrum, `frame / 2 + 1`.
    pub fn bins(&self) -> usize {
        self.core.bins
    }

    /// The STFT frame size, samples.
    pub fn frame(&self) -> usize {
        self.frame
    }

    /// The STFT hop, samples.
    pub fn hop(&self) -> usize {
        self.hop
    }

    /// Clear signal history and gain state; the profile is kept.
    pub fn reset(&mut self) {
        self.stft.reset();
        for c in &mut self.core.ch {
            c.gain.fill(1.0);
            c.prev_clean.fill(0.0);
            c.warm = 0;
        }
    }

    /// Begin a learn period: the accumulators are cleared, the old profile
    /// stays in use until [`Self::stop_learning`]. Audio passes unchanged.
    pub fn start_learning(&mut self) {
        for c in &mut self.core.ch {
            c.sum.fill(0.0);
            c.sum_frames = 0;
        }
        self.core.learning = true;
    }

    /// End the learn period. A channel that saw at least one frame gets the mean
    /// power as its profile; others keep theirs. Returns whether channel 0 did.
    pub fn stop_learning(&mut self) -> bool {
        self.core.learning = false;
        for c in &mut self.core.ch {
            if c.sum_frames > 0 {
                let inv = 1.0 / c.sum_frames as f64;
                for (p, s) in c.profile.iter_mut().zip(&c.sum) {
                    let v = (*s * inv) as f32;
                    *p = if v.is_finite() { v } else { 0.0 };
                }
                c.profile_frames = c.sum_frames;
            }
        }
        self.core.ch[0].sum_frames > 0
    }

    /// Whether a learn period is running.
    pub fn is_learning(&self) -> bool {
        self.core.learning
    }

    /// Frames averaged into the profile of `channel` (0 when there is none).
    pub fn profile_frames(&self, channel: usize) -> u32 {
        self.core.ch.get(channel).map_or(0, |c| c.profile_frames)
    }

    /// The learned mean power per bin of `channel` (windowed-DFT units).
    pub fn profile(&self, channel: usize) -> Option<&[f32]> {
        self.core
            .ch
            .get(channel)
            .filter(|c| c.profile_frames > 0)
            .map(|c| &c.profile[..])
    }

    /// Forget every profile.
    pub fn clear_profile(&mut self) {
        for c in &mut self.core.ch {
            c.profile.fill(0.0);
            c.profile_frames = 0;
        }
    }

    /// Install a profile for `channel` from powers per bin (`bins()` values,
    /// finite and not negative). Returns false and changes nothing otherwise.
    pub fn set_profile(&mut self, channel: usize, power: &[f32]) -> bool {
        let bins = self.core.bins;
        let Some(c) = self.core.ch.get_mut(channel) else { return false };
        if power.len() != bins || power.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return false;
        }
        c.profile.copy_from_slice(power);
        c.profile_frames = 1;
        true
    }

    /// Profile bytes: `"OPNP"`, version 1, frame, hop, channels, then per
    /// channel a frame count and `bins` f32 powers, then an FNV-1a checksum of
    /// everything before it. All little-endian. Allocates; not for the audio thread.
    pub fn save_profile(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(24 + self.core.ch.len() * (4 + 4 * self.core.bins));
        out.extend_from_slice(&MAGIC);
        for v in [VERSION, self.frame as u32, self.hop as u32, self.core.ch.len() as u32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for c in &self.core.ch {
            out.extend_from_slice(&c.profile_frames.to_le_bytes());
            for p in &c.profile {
                out.extend_from_slice(&p.to_le_bytes());
            }
        }
        let sum = fnv1a(&out);
        out.extend_from_slice(&sum.to_le_bytes());
        out
    }

    /// Load bytes from [`Self::save_profile`]. All-or-nothing: on any error the
    /// current profile is unchanged. Does not allocate.
    pub fn load_profile(&mut self, bytes: &[u8]) -> Result<(), ProfileError> {
        let bins = self.core.bins;
        let channels = self.core.ch.len();
        if bytes.len() < 20 {
            return Err(ProfileError::Truncated);
        }
        if bytes[..4] != MAGIC {
            return Err(ProfileError::BadMagic);
        }
        let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        let version = u32_at(4);
        if version != VERSION {
            return Err(ProfileError::UnsupportedVersion(version));
        }
        if u32_at(8) as usize != self.frame
            || u32_at(12) as usize != self.hop
            || u32_at(16) as usize != channels
        {
            return Err(ProfileError::Mismatch);
        }
        let want = 20 + channels * (4 + 4 * bins) + 4;
        if bytes.len() < want {
            return Err(ProfileError::Truncated);
        }
        if bytes.len() > want || fnv1a(&bytes[..want - 4]) != u32_at(want - 4) {
            return Err(ProfileError::Corrupt);
        }
        // Validate everything before writing anything.
        let mut at = 20;
        for _ in 0..channels {
            let frames = u32_at(at);
            at += 4;
            for _ in 0..bins {
                let v = f32::from_bits(u32_at(at));
                at += 4;
                if !v.is_finite() || v < 0.0 || (frames == 0 && v != 0.0) {
                    return Err(ProfileError::BadValue);
                }
            }
        }
        let mut at = 20;
        for c in &mut self.core.ch {
            c.profile_frames = u32_at(at);
            at += 4;
            for p in c.profile.iter_mut() {
                *p = f32::from_bits(u32_at(at));
                at += 4;
            }
        }
        Ok(())
    }

    /// Process `input` into `output` (same length) for one channel.
    pub fn process(&mut self, channel: usize, input: &[f32], output: &mut [f32]) {
        let channel = channel.min(self.core.ch.len() - 1);
        self.stft.process(channel, input, output, &mut self.core);
    }

    /// Process a buffer in place for one channel.
    pub fn process_in_place(&mut self, channel: usize, buffer: &mut [f32]) {
        let channel = channel.min(self.core.ch.len() - 1);
        self.stft.process_in_place(channel, buffer, &mut self.core);
    }

    /// One sample in, one out, delayed by [`Self::latency`].
    pub fn tick(&mut self, channel: usize, x: f32) -> f32 {
        let channel = channel.min(self.core.ch.len() - 1);
        self.stft.tick(channel, x, &mut self.core)
    }
}

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for b in bytes {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Power series for I_nu(x) in f64.
    fn series(nu: u32, x: f64) -> f64 {
        let mut term = (x / 2.0).powi(nu as i32) / (1..=nu).map(|k| k as f64).product::<f64>();
        let mut sum = term;
        for k in 1..400 {
            term *= (x * x / 4.0) / (k as f64 * (k + nu) as f64);
            sum += term;
        }
        sum
    }

    #[test]
    fn scaled_bessel_matches_series() {
        for &x in &[0.0f32, 0.1, 0.5, 1.0, 2.0, 3.0, 3.7, 3.8, 5.0, 10.0, 25.0, 60.0] {
            let want0 = series(0, x as f64) * (-(x as f64)).exp();
            let want1 = series(1, x as f64) * (-(x as f64)).exp();
            assert!((i0e(x) as f64 - want0).abs() < 2e-6 * want0.max(1e-3), "i0e({x})");
            assert!((i1e(x) as f64 - want1).abs() < 2e-6 * want1.max(1e-3) + 1e-7, "i1e({x})");
        }
    }

    #[test]
    fn mmse_gain_limits() {
        let gain = |xi: f32, gamma: f32| {
            let v = xi / (1.0 + xi) * gamma;
            0.886_226_9 * v.sqrt() / gamma * ((1.0 + v) * i0e(0.5 * v) + v * i1e(0.5 * v))
        };
        // High SNR: the gain tends to the Wiener gain, which tends to 1.
        assert!((gain(1000.0, 1001.0) - 1.0).abs() < 0.01);
        assert!(gain(0.003, 1.0) < 0.25);
        // Hand value: xi = 1, gamma = 1 gives v = 0.5 and 0.886 sqrt(0.5) (1.5 I0e(.25) + .5 I1e(.25)).
        let g = gain(1.0, 1.0);
        let v = 0.5f64;
        let want = 0.5 * std::f64::consts::PI.sqrt() * v.sqrt()
            * (-v / 2.0).exp()
            * ((1.0 + v) * series(0, v / 2.0) + v * series(1, v / 2.0));
        assert!((g as f64 - want).abs() < 1e-5, "{g} vs {want}");
    }

    #[test]
    fn bad_bytes_refused() {
        let mut r = SpectralRepair::new(RepairConfig::new(48_000.0, 1, 64)).unwrap();
        assert_eq!(r.load_profile(&[]), Err(ProfileError::Truncated));
        assert_eq!(r.load_profile(&[0u8; 40]), Err(ProfileError::BadMagic));
        assert!(r.set_profile(0, &vec![1.0; r.bins()]));
        let good = r.save_profile();
        assert_eq!(r.load_profile(&good), Ok(()));
    }
}
