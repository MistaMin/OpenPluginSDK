//! Feedback delay network reverb after Jot and Chaigne.
//!
//! Sources:
//! - J.-M. Jot and A. Chaigne, "Digital delay networks for designing
//!   artificial reverberators", 90th AES Convention, preprint 3030, 1991:
//!   the network `x(n+1) = A * (delay lines of x)` with a lossless (here
//!   orthogonal) feedback matrix `A`, and the absorption filters that give
//!   each line the gain its delay length and the target reverberation time
//!   call for.
//! - J.-M. Jot, "Efficient models for reverberation and distance rendering
//!   in computer music and virtual audio reality", ICMC 1997: the
//!   frequency-dependent form of that rule.
//! - Householder reflection `A = I - (2/N) * 1 * 1^T` and the normalised
//!   Walsh-Hadamard matrix `H / sqrt(N)` are both orthogonal, so the network
//!   is lossless until the absorption filters are inserted (Jot's
//!   "unitary feedback matrix" condition; Rocchesso and Smith, "Circulant and
//!   elliptic feedback delay networks for artificial reverberation", IEEE
//!   Trans. Speech and Audio 5(1), 1997, treat the same family).
//! - First-order allpass (Thiran) fractional delay for the optional modulation,
//!   chosen because its gain is exactly one at every frequency; gentle delay modulation breaks up the fixed modes of the network, the
//!   remedy of Griesinger and of Dattorro for metallic ringing.
//!
//! # Structure
//!
//! `N` (8 or 16) delay lines whose lengths are distinct primes (hence
//! mutually prime) near geometrically spaced targets of 20 to 60 ms times
//! the size setting, computed for the real sample rate. Each line passes an
//! absorption filter, then the orthogonal matrix recirculates the vector.
//! Stereo input is injected with orthogonal sign patterns and the two outputs
//! are tapped with two other orthogonal patterns, so identical inputs still
//! yield decorrelated outputs.
//!
//! # Decay time (Jot's rule)
//!
//! A line of `m` samples must lose `60 * m / (fs * RT60)` dB per pass, i.e.
//! have gain `10^(-3 m / (fs * RT60))` (`-60 dB` after `RT60` seconds). Here
//! the target is given at three reference frequencies (default 125 Hz,
//! 1 kHz, 8 kHz) and realised per line as a mid gain times two first-order
//! shelving filters (a low shelf with its pole at four times the low
//! reference, a high shelf at a quarter of the high reference). The shelf
//! gains are solved so the cascade's magnitude equals the Jot target at the
//! three reference frequencies exactly (a short fixed-point iteration whose
//! magnitudes are the exact digital responses), and the whole cascade is then
//! capped below unity at every frequency, so the loop gain is below one and the
//! network is stable at any setting. Between and beyond the references the
//! decay time is that of the smooth first-order interpolation.
//!
//! Everything is allocated in [`FdnReverb::new`] / [`FdnReverb::set_sample_rate`].
//! Setters and `process*` never allocate.

use crate::early::EarlyReflections;
use crate::rate::effective_rate;
use crate::reverb_common::{PreDelay, Ring, apply_width, clean, flush, flush64, next_prime};

use std::f64::consts::PI;

/// Lowest and highest decay time accepted, in seconds.
pub const FDN_RT60_RANGE: (f32, f32) = (0.05, 600.0);
/// Size (delay length scale) range.
pub const FDN_SIZE_RANGE: (f32, f32) = (0.2, 4.0);
/// Longest pre-delay, in milliseconds.
pub const FDN_MAX_PREDELAY_MS: f32 = 500.0;
/// Decay time used for every band while frozen. Finite, so the loop gain stays
/// below one and the output is bounded; 60 dB takes under three hours.
pub const FDN_FREEZE_RT60: f32 = 10_000.0;

const BASE_MIN_MS: f64 = 20.0;
const BASE_MAX_MS: f64 = 60.0;
const MOD_MAX_MS: f32 = 0.15;
const SLEW: f32 = 0.02;
/// Output gain: with input gain `1/sqrt(N)` a diffuse field of line variance
/// `1/(N (1 - G^2))` sums to `0.25 / (1 - G^2)`, about 0 dB re the input for
/// a per-pass energy loss `G^2 = 0.75`.
const OUT_GAIN: f32 = 0.5;

/// Feedback matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FdnMatrix {
    /// `I - (2/N) 1 1^T`: a reflection, O(N) per sample.
    Householder,
    /// Normalised Walsh-Hadamard transform: O(N log N), fully mixing.
    Hadamard,
}

#[derive(Clone, Copy, Debug)]
struct Shelf {
    b0: f64,
    b1: f64,
    a1: f64,
    s: f64,
}

impl Shelf {
    const IDENTITY: Self = Self { b0: 1.0, b1: 0.0, a1: 0.0, s: 0.0 };

    /// `H(s) = (s + k)/(s + 1)` in units of the pole frequency, bilinear with
    /// `kk = tan(pi fc / fs)`: gain `k` at DC, 1 at Nyquist.
    fn low(k: f64, kk: f64) -> Self {
        if (k - 1.0).abs() < 1e-9 {
            return Self::IDENTITY;
        }
        let n = 1.0 / (1.0 + kk);
        Self { b0: (1.0 + k * kk) * n, b1: (k * kk - 1.0) * n, a1: (kk - 1.0) * n, s: 0.0 }
    }

    /// `H(s) = (k s + 1)/(s + 1)`: gain 1 at DC, `k` at Nyquist.
    fn high(k: f64, kk: f64) -> Self {
        if (k - 1.0).abs() < 1e-9 {
            return Self::IDENTITY;
        }
        let n = 1.0 / (1.0 + kk);
        Self { b0: (k + kk) * n, b1: (kk - k) * n, a1: (kk - 1.0) * n, s: 0.0 }
    }

    #[inline]
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.s;
        self.s = flush64(self.b1 * x - self.a1 * y);
        y
    }
}

#[derive(Clone, Copy, Debug)]
struct Absorb {
    g: f64,
    lo: Shelf,
    hi: Shelf,
}

impl Absorb {
    const NEUTRAL: Self = Self { g: 1.0, lo: Shelf::IDENTITY, hi: Shelf::IDENTITY };

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.hi.tick(self.lo.tick(f64::from(x)));
        (y * self.g) as f32
    }
}

fn mag_low(k: f64, w: f64) -> f64 {
    ((w * w + k * k) / (w * w + 1.0)).sqrt()
}

fn mag_high(k: f64, w: f64) -> f64 {
    ((k * k * w * w + 1.0) / (w * w + 1.0)).sqrt()
}

/// Jot's per-line absorption for a line of `delay` samples: gain `10^(-3 delay/(fs RT))`
/// at each of the three reference frequencies, realised as `g * low shelf * high shelf`.
fn design_absorption(fs: f64, delay: f64, rt: [f64; 3], refs: [f64; 3]) -> Absorb {
    let fh = refs[2].min(0.45 * fs);
    let fm = refs[1].min(fh / 1.2);
    let fl = refs[0].min(fm / 1.2).max(1.0);
    // Natural log of the target gain at each reference.
    let t: Vec3 = [
        -3.0 * delay / (fs * rt[0]) * std::f64::consts::LN_10,
        -3.0 * delay / (fs * rt[1]) * std::f64::consts::LN_10,
        -3.0 * delay / (fs * rt[2]) * std::f64::consts::LN_10,
    ];
    let kl_pole = (PI * (4.0 * fl).min(0.45 * fs) / fs).tan();
    let kh_pole = (PI * (fh / 4.0) / fs).tan();
    let w = |f: f64, kk: f64| (PI * f / fs).tan() / kk;
    let (wl_l, wl_m, wl_h) = (w(fl, kl_pole), w(fm, kl_pole), w(fh, kl_pole));
    let (wh_l, wh_m, wh_h) = (w(fl, kh_pole), w(fm, kh_pole), w(fh, kh_pole));

    let mut kl = (t[0] - t[1]).exp();
    let mut kh = (t[2] - t[1]).exp();
    let mut ln_g = t[1];
    for _ in 0..24 {
        ln_g = t[1] - mag_low(kl, wl_m).ln() - mag_high(kh, wh_m).ln();
        let want_l = (t[0] - ln_g - mag_high(kh, wh_l).ln()).exp();
        let k2 = want_l * want_l * (wl_l * wl_l + 1.0) - wl_l * wl_l;
        kl = k2.max(4.0e-4).sqrt();
        let want_h = (t[2] - ln_g - mag_low(kl, wl_h).ln()).exp();
        let k2 = (want_h * want_h * (wh_h * wh_h + 1.0) - 1.0) / (wh_h * wh_h);
        kh = k2.max(4.0e-4).sqrt();
    }
    let mut g = ln_g.exp();

    // Passivity: the cascade must stay below unity everywhere.
    let mut worst = (g * kl).max(g * kh);
    for i in 0..96 {
        let f = 5.0 * ((0.4999 * fs / 5.0).ln() * i as f64 / 95.0).exp();
        let m = g * mag_low(kl, w(f, kl_pole)) * mag_high(kh, w(f, kh_pole));
        worst = worst.max(m);
    }
    const CAP: f64 = 0.999_99;
    if worst > CAP {
        g *= CAP / worst;
    }
    Absorb { g, lo: Shelf::low(kl, kl_pole), hi: Shelf::high(kh, kh_pole) }
}

type Vec3 = [f64; 3];

fn parity(x: usize) -> f32 {
    if x.count_ones() & 1 == 0 { 1.0 } else { -1.0 }
}

/// 8 or 16 line feedback delay network reverb, stereo in and out.
///
/// The output is the wet signal only (early reflections plus the tail); mix
/// it with the dry path yourself.
#[derive(Clone, Debug)]
pub struct FdnReverb {
    sample_rate: f32,
    n: usize,
    lines: Vec<Ring>,
    delays: Vec<usize>,
    d_cur: Vec<f32>,
    interp: Vec<f32>,
    absorb: Vec<Absorb>,
    lfo_phase: Vec<f32>,
    lfo_inc: Vec<f32>,
    in_l: [f32; 16],
    in_r: [f32; 16],
    out_l: [f32; 16],
    out_r: [f32; 16],
    pre: PreDelay,
    early: EarlyReflections,
    // settings
    matrix: FdnMatrix,
    size: f32,
    rt60: [f32; 3],
    refs: [f32; 3],
    mod_depth: f32,
    mod_rate: f32,
    mod_samples: f32,
    width: f32,
    frozen: bool,
    in_gain: f32,
    in_coef: f32,
}

impl FdnReverb {
    /// Builds a network of `lines` delay lines (8 or 16; any other value is
    /// rounded to the nearest of the two) for `sample_rate`. Defaults: size 1,
    /// decay 2.5 s / 2 s / 1 s at 125 Hz / 1 kHz / 8 kHz, Householder matrix,
    /// modulation depth 0.25 at 0.8 Hz, width 1, pre-delay 0, early level 0.
    /// Allocates.
    pub fn new(sample_rate: f32, lines: usize) -> Self {
        let n = if lines >= 12 { 16 } else { 8 };
        let mut s = Self {
            sample_rate: 48_000.0,
            n,
            lines: Vec::new(),
            delays: Vec::new(),
            d_cur: Vec::new(),
            interp: Vec::new(),
            absorb: Vec::new(),
            lfo_phase: Vec::new(),
            lfo_inc: Vec::new(),
            in_l: [0.0; 16],
            in_r: [0.0; 16],
            out_l: [0.0; 16],
            out_r: [0.0; 16],
            pre: PreDelay::new(48_000.0, 1.0, 0.0),
            early: EarlyReflections::new(48_000.0),
            matrix: FdnMatrix::Householder,
            size: 1.0,
            rt60: [2.5, 2.0, 1.0],
            refs: [125.0, 1000.0, 8000.0],
            mod_depth: 0.25,
            mod_rate: 0.8,
            mod_samples: 0.0,
            width: 1.0,
            frozen: false,
            in_gain: 1.0,
            in_coef: 0.0,
        };
        s.early.set_level(0.0);
        s.build(sample_rate);
        s
    }

    /// Re-targets the network to a new rate: reallocates the buffers and
    /// clears the tail, keeps every setting. Not for the audio thread.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let pre_ms = self.pre_delay_ms();
        let level = self.early.level();
        self.build(sample_rate);
        self.set_predelay_ms(pre_ms);
        self.early.set_level(level);
    }

    fn pre_delay_ms(&self) -> f32 {
        self.pre.target_samples() as f32 * 1000.0 / self.sample_rate
    }

    fn build(&mut self, sample_rate: f32) {
        let fs = effective_rate(sample_rate);
        self.sample_rate = fs;
        let n = self.n;
        let max_len = (f64::from(FDN_SIZE_RANGE.1) * BASE_MAX_MS * f64::from(fs) / 1000.0 * 1.1) as usize
            + 96
            + (MOD_MAX_MS * 0.001 * fs) as usize * 2;
        self.lines = (0..n).map(|_| Ring::new(max_len)).collect();
        self.delays = vec![0; n];
        self.d_cur = vec![0.0; n];
        self.interp = vec![0.0; n];
        self.absorb = vec![Absorb::NEUTRAL; n];
        self.lfo_phase = (0..n).map(|i| i as f32 / n as f32).collect();
        self.lfo_inc = vec![0.0; n];
        let mut early = EarlyReflections::new(fs);
        early.set_level(self.early.level());
        early.set_pattern(crate::early::EarlyPattern::SmallRoom);
        self.early = early;
        self.pre = PreDelay::new(fs, FDN_MAX_PREDELAY_MS, 0.0);
        for i in 0..n {
            self.in_l[i] = parity(i & 3) / (n as f32).sqrt();
            self.in_r[i] = parity(i & 5) / (n as f32).sqrt();
            self.out_l[i] = parity(i & 1) * OUT_GAIN;
            self.out_r[i] = parity((i >> 1) & 1) * OUT_GAIN;
        }
        self.in_coef = 1.0 - (-1.0 / (0.01 * fs)).exp();
        self.in_gain = if self.frozen { 0.0 } else { 1.0 };
        self.layout_delays();
        for i in 0..n {
            self.d_cur[i] = self.delays[i] as f32;
        }
        self.update_mod();
        self.design();
    }

    /// Picks prime delay lengths for the current size and rate.
    fn layout_delays(&mut self) {
        let n = self.n;
        let fs = f64::from(self.sample_rate);
        let mut prev = 0usize;
        for i in 0..n {
            let frac = i as f64 / (n - 1) as f64;
            let ms = BASE_MIN_MS * (BASE_MAX_MS / BASE_MIN_MS).powf(frac) * f64::from(self.size);
            let target = ((ms * fs / 1000.0).round() as usize).max(8);
            let mut p = next_prime(target);
            if p <= prev {
                p = next_prime(prev + 1);
            }
            self.delays[i] = p;
            prev = p;
        }
    }

    fn update_mod(&mut self) {
        let fs = self.sample_rate;
        let shortest = self.delays.first().copied().unwrap_or(8) as f32;
        let max_excursion = (shortest * 0.25 - 2.0).max(0.0);
        self.mod_samples = (self.mod_depth * MOD_MAX_MS * 0.001 * fs).min(max_excursion);
        let n = self.n;
        for i in 0..n {
            let spread = 0.7 + 0.6 * i as f32 / (n - 1) as f32;
            self.lfo_inc[i] = std::f32::consts::TAU * self.mod_rate * spread / fs;
        }
    }

    fn effective_rt(&self) -> [f64; 3] {
        if self.frozen {
            [f64::from(FDN_FREEZE_RT60); 3]
        } else {
            [f64::from(self.rt60[0]), f64::from(self.rt60[1]), f64::from(self.rt60[2])]
        }
    }

    fn design(&mut self) {
        let rt = self.effective_rt();
        let refs = [f64::from(self.refs[0]), f64::from(self.refs[1]), f64::from(self.refs[2])];
        let fs = f64::from(self.sample_rate);
        for i in 0..self.n {
            // Keep the filter states across redesigns; only the coefficients move.
            let new = design_absorption(fs, self.delays[i] as f64, rt, refs);
            let old = self.absorb[i];
            self.absorb[i] = Absorb {
                g: new.g,
                lo: Shelf { s: old.lo.s, ..new.lo },
                hi: Shelf { s: old.hi.s, ..new.hi },
            };
        }
    }

    /// Number of delay lines (8 or 16).
    pub fn lines(&self) -> usize {
        self.n
    }

    /// The real sample rate in use.
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Current delay line lengths in samples (distinct primes, so mutually prime).
    pub fn delay_lengths(&self) -> &[usize] {
        &self.delays
    }

    /// Reverberation time, in seconds for -60 dB, at the three reference
    /// frequencies. Values are clamped to [`FDN_RT60_RANGE`]; NaN is ignored.
    pub fn set_rt60(&mut self, low: f32, mid: f32, high: f32) {
        let mut v = self.rt60;
        for (slot, x) in v.iter_mut().zip([low, mid, high]) {
            if x.is_finite() {
                *slot = x.clamp(FDN_RT60_RANGE.0, FDN_RT60_RANGE.1);
            }
        }
        self.rt60 = v;
        self.design();
    }

    /// The decay time settings (low, mid, high), seconds.
    pub fn rt60(&self) -> (f32, f32, f32) {
        (self.rt60[0], self.rt60[1], self.rt60[2])
    }

    /// The frequencies at which the three decay times hold (Hz, increasing;
    /// they are kept at least 1.2x apart and below 0.45 of the sample rate).
    pub fn set_reference_hz(&mut self, low: f32, mid: f32, high: f32) {
        let mut v = self.refs;
        for (slot, x) in v.iter_mut().zip([low, mid, high]) {
            if x.is_finite() && x > 1.0 {
                *slot = x;
            }
        }
        self.refs = v;
        self.design();
    }

    /// Delay length scale, `0.2..=4.0`. The lines glide to their new lengths
    /// (at most 2 % pitch bend), so turning it does not click.
    pub fn set_size(&mut self, size: f32) {
        if size.is_finite() {
            self.size = size.clamp(FDN_SIZE_RANGE.0, FDN_SIZE_RANGE.1);
            self.layout_delays();
            self.update_mod();
            self.design();
        }
    }

    pub fn set_matrix(&mut self, matrix: FdnMatrix) {
        self.matrix = matrix;
    }

    /// Modulation depth `0.0..=1.0` (up to 0.15 ms peak excursion, limited to a
    /// quarter of the shortest line) and rate in hertz (`0.05..=5`; each line
    /// runs at 0.7 to 1.3 times the rate).
    pub fn set_modulation(&mut self, depth: f32, rate_hz: f32) {
        if depth.is_finite() {
            self.mod_depth = depth.clamp(0.0, 1.0);
        }
        if rate_hz.is_finite() {
            self.mod_rate = rate_hz.clamp(0.05, 5.0);
        }
        self.update_mod();
    }

    /// Pre-delay in milliseconds, `0..=500`; glides when changed.
    pub fn set_predelay_ms(&mut self, ms: f32) {
        self.pre.set_ms(self.sample_rate, ms);
    }

    /// Stereo width of the wet output: `0.0` mono, `1.0` as built, up to `2.0`.
    pub fn set_width(&mut self, width: f32) {
        if width.is_finite() {
            self.width = width.clamp(0.0, 2.0);
        }
    }

    /// Freeze: input is faded out over about 10 ms and every band decays with
    /// [`FDN_FREEZE_RT60`] so the tail holds. Loop gain stays below one, so the
    /// output is bounded.
    pub fn set_freeze(&mut self, frozen: bool) {
        if self.frozen != frozen {
            self.frozen = frozen;
            self.design();
        }
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// The early reflections stage (added to the wet output, fed by the
    /// pre-delayed input). Its level starts at 0, i.e. off.
    pub fn early_mut(&mut self) -> &mut EarlyReflections {
        &mut self.early
    }

    /// Samples after the input stops until the wet output is 60 dB down: the
    /// longest decay time, the pre-delay, and the longest line. While
    /// [`Self::is_frozen`] the tail is infinite; report `Tail::Infinite`.
    pub fn tail_samples(&self) -> usize {
        let rt = self.rt60.iter().fold(0.0f32, |a, b| a.max(*b));
        let longest = self.delays.last().copied().unwrap_or(0);
        let early = self.early.longest_tap_samples();
        (rt * self.sample_rate).ceil() as usize + self.pre.target_samples() + longest.max(early)
    }

    /// Clears every delay and filter state.
    pub fn reset(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        self.interp.fill(0.0);
        for a in &mut self.absorb {
            a.lo.s = 0.0;
            a.hi.s = 0.0;
        }
        self.pre.reset();
        self.early.reset();
        self.in_gain = if self.frozen { 0.0 } else { 1.0 };
    }

    /// One stereo frame in, the wet frame out.
    #[allow(clippy::needless_range_loop)] // lines index several parallel arrays
    pub fn process_frame(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let n = self.n;
        let target = if self.frozen { 0.0 } else { 1.0 };
        self.in_gain += (target - self.in_gain) * self.in_coef;
        let (pl, pr) = self.pre.process(clean(in_l), clean(in_r));
        let (pl, pr) = (pl * self.in_gain, pr * self.in_gain);
        let (el, er) = self.early.process_frame(pl, pr);

        let mut y = [0.0f32; 16];
        let mut wl = 0.0;
        let mut wr = 0.0;
        for i in 0..n {
            let c = self.d_cur[i];
            let t = self.delays[i] as f32;
            self.d_cur[i] = c + (t - c).clamp(-SLEW, SLEW);
            let shift = if self.mod_samples > 0.0 {
                let ph = self.lfo_phase[i];
                let mut next = ph + self.lfo_inc[i];
                if next > std::f32::consts::TAU {
                    next -= std::f32::consts::TAU;
                }
                self.lfo_phase[i] = next;
                ph.sin() * self.mod_samples
            } else {
                0.0
            };
            let raw = self.lines[i].at_allpass(self.d_cur[i] + shift, &mut self.interp[i]);
            let v = self.absorb[i].tick(raw);
            y[i] = v;
            wl += v * self.out_l[i];
            wr += v * self.out_r[i];
        }

        match self.matrix {
            FdnMatrix::Householder => {
                let mut s = 0.0;
                for v in &y[..n] {
                    s += *v;
                }
                s *= 2.0 / n as f32;
                for v in &mut y[..n] {
                    *v -= s;
                }
            }
            FdnMatrix::Hadamard => {
                let mut h = 1;
                while h < n {
                    let mut i = 0;
                    while i < n {
                        for j in i..i + h {
                            let (a, b) = (y[j], y[j + h]);
                            y[j] = a + b;
                            y[j + h] = a - b;
                        }
                        i += 2 * h;
                    }
                    h *= 2;
                }
                let norm = 1.0 / (n as f32).sqrt();
                for v in &mut y[..n] {
                    *v *= norm;
                }
            }
        }

        for i in 0..n {
            let inj = self.in_l[i] * pl + self.in_r[i] * pr;
            self.lines[i].push(flush(y[i] + inj));
        }

        let (ol, or) = (wl + el, wr + er);
        if !(ol.is_finite() && or.is_finite()) {
            self.reset();
            return (0.0, 0.0);
        }
        apply_width(ol, or, self.width)
    }

    /// In place: the buffers (equal length expected; the shorter governs) are
    /// replaced by the wet signal.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (a, b) = self.process_frame(*l, *r);
            *l = a;
            *r = b;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gcd(a: usize, b: usize) -> usize {
        if b == 0 { a } else { gcd(b, a % b) }
    }

    #[test]
    fn lengths_are_mutually_prime_at_every_rate_and_size() {
        for &lines in &[8usize, 16] {
            for &fs in &[1_000.0f32, 8_000.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
                for &size in &[0.2f32, 1.0, 4.0] {
                    let mut r = FdnReverb::new(fs, lines);
                    r.set_size(size);
                    let d = r.delay_lengths();
                    assert_eq!(d.len(), lines);
                    for i in 0..d.len() {
                        for j in i + 1..d.len() {
                            assert_eq!(gcd(d[i], d[j]), 1, "{fs} {size}: {} {}", d[i], d[j]);
                        }
                    }
                    assert!(d.windows(2).all(|w| w[0] < w[1]));
                }
            }
        }
    }

    #[test]
    fn lengths_scale_with_rate_and_size() {
        let a = FdnReverb::new(48_000.0, 8);
        let b = FdnReverb::new(96_000.0, 8);
        let ratio = b.delay_lengths()[7] as f32 / a.delay_lengths()[7] as f32;
        assert!((ratio - 2.0).abs() < 0.01);
        let mut c = FdnReverb::new(48_000.0, 8);
        c.set_size(2.0);
        let ratio = c.delay_lengths()[3] as f32 / a.delay_lengths()[3] as f32;
        assert!((ratio - 2.0).abs() < 0.02);
    }

    #[test]
    fn absorption_hits_the_jot_gain_at_the_references() {
        let fs = 48_000.0;
        let d = 1999.0;
        let rt = [3.0, 2.0, 0.8];
        let a = design_absorption(fs, d, rt, [125.0, 1000.0, 8000.0]);
        // Evaluate the digital response of the realised filters.
        let resp = |f: f64| {
            let wv = 2.0 * PI * f / fs;
            let z1 = (wv.cos(), -wv.sin());
            let sh = |s: &Shelf| {
                let num = (s.b0 + s.b1 * z1.0, s.b1 * z1.1);
                let den = (1.0 + s.a1 * z1.0, s.a1 * z1.1);
                ((num.0 * num.0 + num.1 * num.1) / (den.0 * den.0 + den.1 * den.1)).sqrt()
            };
            a.g * sh(&a.lo) * sh(&a.hi)
        };
        for (f, r) in [(125.0, 3.0), (1000.0, 2.0), (8000.0, 0.8)] {
            let want = 10f64.powf(-3.0 * d / (fs * r));
            assert!((resp(f) / want - 1.0).abs() < 2e-3, "{f}: {} vs {want}", resp(f));
        }
    }

    #[test]
    fn output_is_wet_only_and_nan_safe() {
        let mut r = FdnReverb::new(48_000.0, 8);
        let (a, b) = r.process_frame(1.0, 1.0);
        assert!(a == 0.0 && b == 0.0, "no direct path");
        let mut r = FdnReverb::new(48_000.0, 8);
        for _ in 0..2000 {
            let (a, b) = r.process_frame(f32::NAN, f32::INFINITY);
            assert!(a == 0.0 && b == 0.0);
        }
    }

    #[test]
    fn setters_ignore_nan() {
        let mut r = FdnReverb::new(48_000.0, 16);
        r.set_rt60(f32::NAN, f32::INFINITY, -4.0);
        let (l, m, h) = r.rt60();
        assert_eq!((l, m), (2.5, 2.0));
        assert_eq!(h, FDN_RT60_RANGE.0);
        r.set_size(f32::NAN);
        r.set_width(f32::NAN);
        r.set_modulation(f32::NAN, f32::NAN);
        r.set_predelay_ms(f32::NAN);
    }
}
