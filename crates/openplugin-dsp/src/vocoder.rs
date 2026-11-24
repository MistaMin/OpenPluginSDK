//! Phase vocoder: time stretch and pitch shift, with identity phase locking
//! and onset-triggered phase reset.
//!
//! # Method
//! Analysis frames of `frame` samples (Hann window) are taken at hops of
//! `hop_a` input samples and resynthesised at a fixed hop `hop_s = frame / 4`
//! (Hann synthesis window, weighted overlap-add normalised as in
//! [`crate::stft`]); the time-stretch ratio is `S = hop_s / hop_a`. For every
//! bin the phase advance between two analysis frames gives the bin's
//! instantaneous frequency
//! `w_inst = w_k + princarg(phi_k - phi'_k - w_k hop_a) / hop_a`
//! and the synthesis phase advances by `w_inst * hop_s`
//! (J. L. Flanagan and R. M. Golden, "Phase vocoder", Bell Syst. Tech. J.
//! 45, 1966; M. Dolson, "The phase vocoder: a tutorial", Computer Music J.
//! 10(4), 1986). The analysis positions are fractional-accurate (the frame
//! start is rounded, the accumulated position is not), so the output length is
//! `S` times the input to the sample.
//!
//! # Identity phase locking
//! [`PhaseLocking::Identity`] follows J. Laroche and M. Dolson, "Improved phase
//! vocoder time-scale modification of audio", IEEE Trans. Speech and Audio
//! Processing 7(3), 1999, section III-B ("identity phase locking"): spectral
//! peaks (a bin larger than its two neighbours on each side) get the phase
//! propagation above, and every other bin takes its peak's synthesis phase
//! plus its own analysis phase relative to the peak,
//! `psi_k = psi_p + (phi_k - phi_p)`. The region of each peak ends midway to the
//! next one. This keeps the phase relations between the bins of one partial
//! that the plain vocoder breaks ("phasiness").
//! [`PhaseLocking::None`] is the plain, per-bin vocoder.
//!
//! # Transients
//! With `transients` on, an onset is detected by spectral flux (J. P. Bello et
//! al., "A tutorial on onset detection in music signals", IEEE Trans. Speech
//! and Audio Processing 13(5), 2005): the half-wave-rectified magnitude
//! increase summed over bins, divided by the summed magnitude, so the flux is
//! 0..1 whatever the level. When it exceeds `flux_threshold`, phases are reset
//! to the analysis phases (the idea of phase reset at onsets: A. Roebel, "A new
//! approach to transient processing in the phase vocoder", DAFx-03, and
//! C. Duxbury, M. Davies, M. Sandler, "Improved time-scaling of musical audio
//! using phase locking at transients", AES 112th Conv., 2002), and the frame's
//! energy centroid (where the click sits in the frame) is moved by a linear
//! phase ramp from `c` to `c * S` so the transient lands where it belongs in
//! the stretched time. The move is limited to the frame, so a transient that
//! enters a frame late when stretching by more than 1 is clipped, and its
//! frames fade through the synthesis window (that is the weak point; see the
//! cookbook for measured numbers).
//!
//! # Pitch shift
//! [`PitchShifter`] uses stretch plus resample: stretch by `p` (the pitch
//! ratio) and read the stretched stream `p` samples per output sample with a
//! 24-tap Kaiser-windowed sinc (cutoff 0.46, beta 9, 512 phases with linear
//! interpolation). For `p > 1` the bins above `Nyquist / p` are removed in the
//! vocoder frame first, so nothing folds. The alternative, direct bin
//! shifting (Laroche and Dolson, "New phase-vocoder techniques for
//! pitch-shifting, harmonizing and other exotic effects", WASPAA 1999), shifts
//! the formants with the pitch only through the peaks; stretch plus resample
//! was chosen because it reuses the time-stretch path, has an exact pitch
//! ratio, and keeps one code path. Formants move with the pitch.
//! Channels are processed independently (no inter-channel phase locking).
//!
//! # Latency and real time
//! Streaming pitch shift has a fixed latency [`PitchShifter::latency`] =
//! `frame / 2 + frame / (2 min_ratio) + 64` samples (1.5 frames + 64 over
//! -12..+12 semitones), the same at every ratio. Every buffer is allocated by `new`; `process`, `push`
//! and `pop` do not allocate, never panic, and treat non-finite input as 0.
//! The FFT work comes in bursts of one frame per `hop_a` samples.

use crate::stft::check_ola_windows;
use crate::rate::effective_rate;
use crate::windows::WindowKind;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::f64::consts::PI;
use std::sync::Arc;

/// Pitch shifter margin added to the frame in the latency, in samples.
const MARGIN: usize = 64;
/// Interpolator taps and half-width.
const TAPS: usize = 24;
const HALF: usize = 12;
const PHASES: usize = 512;
/// Internal chunk of the streaming pitch shifter.
const CHUNK: usize = 256;

/// How the phases of bins near a partial are kept together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PhaseLocking {
    /// Each bin advances on its own (the plain phase vocoder).
    None,
    /// Laroche and Dolson identity phase locking.
    Identity,
}

/// Vocoder settings.
/// # Example
///
/// ```
/// use openplugin_dsp::{PhaseLocking, VocoderConfig};
///
/// let config = VocoderConfig { transients: true, ..VocoderConfig::for_rate(48_000.0) };
/// assert_eq!(config.frame, 2048);
/// assert_eq!(config.locking, PhaseLocking::Identity);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VocoderConfig {
    /// Frame size, a power of two in 256..=16384.
    pub frame: usize,
    /// Plain phase vocoder or identity phase locking.
    pub locking: PhaseLocking,
    /// Reset phases at detected onsets.
    pub transients: bool,
    /// Normalised spectral flux (0..1) above which a frame is an onset.
    pub flux_threshold: f32,
}

impl VocoderConfig {
    /// A frame of about 40 ms (2048 at 44.1 and 48 kHz, 4096 at 96 kHz),
    /// identity locking, no transient handling, flux threshold 0.5.
    pub fn for_rate(sample_rate: f32) -> Self {
        let fs = effective_rate(sample_rate);
        let frame = ((0.04 * fs) as usize).next_power_of_two().clamp(256, 16384);
        Self { frame, locking: PhaseLocking::Identity, transients: false, flux_threshold: 0.5 }
    }
}

struct Core {
    inbuf: Vec<f32>,
    in_count: i64,
    next_start: f64,
    last_start: i64,
    have_prev: bool,
    acc: Vec<f32>,
    wbase: i64,
    released: i64,
    prev_phase: Vec<f32>,
    prev_mag: Vec<f32>,
    psi: Vec<f32>,
    onsets: u64,
    in_onset: bool,
}

struct Shared {
    n: usize,
    hs: usize,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    buf: Vec<Complex<f32>>,
    analysis: Vec<f32>,
    synthesis: Vec<f32>,
    mag: Vec<f32>,
    phase: Vec<f32>,
    peaks: Vec<usize>,
}

struct Engine {
    sh: Shared,
    cores: Vec<Core>,
    cfg: VocoderConfig,
    in_cap: usize,
    out_cap: usize,
}

#[inline]
fn princarg(x: f64) -> f64 {
    x - 2.0 * PI * (x / (2.0 * PI)).round()
}

impl Engine {
    fn new(channels: usize, sample_rate: f32, mut cfg: VocoderConfig) -> Self {
        let _ = effective_rate(sample_rate);
        let n = cfg.frame.clamp(256, 16384).next_power_of_two();
        cfg.frame = n;
        if !cfg.flux_threshold.is_finite() {
            cfg.flux_threshold = 0.5;
        }
        cfg.flux_threshold = cfg.flux_threshold.clamp(0.01, 1.0);
        let hs = n / 4;
        let analysis = WindowKind::Hann.to_vec(n, true);
        let mut synthesis = WindowKind::Hann.to_vec(n, true);
        let gain = check_ola_windows(&analysis, &synthesis, hs).gain;
        let scale = (1.0 / (gain * n as f64)) as f32;
        for v in &mut synthesis {
            *v *= scale;
        }
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        let ifft = planner.plan_fft_inverse(n);
        let scratch_len = fft.get_inplace_scratch_len().max(ifft.get_inplace_scratch_len());
        let nb = n / 2 + 1;
        let in_cap = (3 * n + 4 * CHUNK).next_power_of_two();
        let out_cap = (8 * n).max(n + 4096).next_power_of_two();
        let mut e = Self {
            sh: Shared {
                n,
                hs,
                fft,
                ifft,
                scratch: vec![Complex::new(0.0, 0.0); scratch_len],
                buf: vec![Complex::new(0.0, 0.0); n],
                analysis,
                synthesis,
                mag: vec![0.0; nb],
                phase: vec![0.0; nb],
                peaks: Vec::with_capacity(nb),
            },
            cores: Vec::new(),
            cfg,
            in_cap,
            out_cap,
        };
        for _ in 0..channels.clamp(1, 64) {
            e.cores.push(Core {
                inbuf: vec![0.0; in_cap],
                in_count: 0,
                next_start: 0.0,
                last_start: 0,
                have_prev: false,
                acc: vec![0.0; out_cap],
                wbase: 0,
                released: 0,
                prev_phase: vec![0.0; nb],
                prev_mag: vec![0.0; nb],
                psi: vec![0.0; nb],
                onsets: 0,
                in_onset: false,
            });
        }
        e.reset();
        e
    }

    fn reset_core(c: &mut Core, n: usize, hs: usize) {
        c.inbuf.fill(0.0);
        c.acc.fill(0.0);
        c.prev_phase.fill(0.0);
        c.prev_mag.fill(0.0);
        c.psi.fill(0.0);
        c.in_count = 0;
        // Start before time zero so the first output samples are fully overlapped.
        c.next_start = -((n - hs) as f64);
        c.last_start = 0;
        c.have_prev = false;
        c.in_onset = false;
        c.wbase = 0;
        c.released = 0;
    }

    fn reset(&mut self) {
        let (n, hs) = (self.sh.n, self.sh.hs);
        for c in &mut self.cores {
            Self::reset_core(c, n, hs);
        }
    }

    /// Accept as much of `input` as fits, then run every frame that is ready.
    fn push(&mut self, ch: usize, input: &[f32], stretch: f64, cut: usize) -> usize {
        let Some(c) = self.cores.get_mut(ch) else { return 0 };
        let mask = self.in_cap - 1;
        // Samples from the next frame start to the newest must fit.
        let lag = (c.in_count - c.next_start.floor() as i64).max(0) as usize;
        let space = self.in_cap.saturating_sub(lag + 1);
        let take = input.len().min(space);
        for &x in &input[..take] {
            c.inbuf[(c.in_count as usize) & mask] = if x.is_finite() { x } else { 0.0 };
            c.in_count += 1;
        }
        Self::run(&mut self.sh, c, &self.cfg, self.in_cap, self.out_cap, stretch, cut);
        take
    }

    fn run(sh: &mut Shared, c: &mut Core, cfg: &VocoderConfig, in_cap: usize, out_cap: usize, stretch: f64, cut: usize) {
        let n = sh.n as i64;
        loop {
            let start = c.next_start.round() as i64;
            if start + n > c.in_count {
                break;
            }
            if c.wbase + n - c.released > out_cap as i64 {
                break;
            }
            if c.in_count - start > in_cap as i64 {
                // Data already overwritten: skip ahead (only after a ratio jump).
                c.next_start = (c.in_count - n) as f64;
                continue;
            }
            Self::frame(sh, c, cfg, in_cap, start, stretch, cut);
            c.last_start = start;
            c.have_prev = true;
            c.wbase += sh.hs as i64;
            c.next_start += sh.hs as f64 / stretch;
        }
    }

    fn frame(sh: &mut Shared, c: &mut Core, cfg: &VocoderConfig, in_cap: usize, start: i64, stretch: f64, cut: usize) {
        let n = sh.n;
        let nb = n / 2 + 1;
        let imask = in_cap - 1;
        let want_centroid = cfg.transients;
        let (mut num, mut den) = (0.0f64, 0.0f64);
        for k in 0..n {
            let x = c.inbuf[((start + k as i64) as usize) & imask] * sh.analysis[k];
            sh.buf[k] = Complex::new(x, 0.0);
            if want_centroid {
                let e = f64::from(x) * f64::from(x);
                num += k as f64 * e;
                den += e;
            }
        }
        sh.fft.process_with_scratch(&mut sh.buf, &mut sh.scratch);
        let mut sum_mag = 0.0f64;
        let mut rise = 0.0f64;
        for k in 0..nb {
            let z = sh.buf[k];
            let m = z.norm();
            sh.mag[k] = if m.is_finite() { m } else { 0.0 };
            sh.phase[k] = if m > 0.0 { z.im.atan2(z.re) } else { 0.0 };
            sum_mag += f64::from(sh.mag[k]);
            rise += f64::from((sh.mag[k] - c.prev_mag[k]).max(0.0));
        }
        let ha = (start - c.last_start).max(1) as f64;
        let hs = sh.hs as f64;
        // Onset with hysteresis: it starts when the flux passes the threshold
        // and lasts while the frames still gain more than 0.4 of it, so the
        // frames that carry the transient near the middle of the window (the
        // magnitude grows by less per hop there, most of all at small hops)
        // are reset too.
        let flux = if sum_mag > 1e-6 { rise / sum_mag } else { 0.0 };
        let thr = f64::from(cfg.flux_threshold);
        let was = c.in_onset;
        c.in_onset = cfg.transients && (flux > thr || (was && flux > 0.4 * thr));
        let onset = !c.have_prev || c.in_onset;
        let w1 = 2.0 * PI / n as f64;
        if onset {
            // Phase reset, moving the energy centroid from c to c * S.
            let mut delta = 0.0;
            if cfg.transients && c.have_prev && den > 1e-20 {
                let cen = num / den;
                let mid = n as f64 * 0.5;
                delta = (mid + stretch * (cen - mid)).clamp(0.0, (n - 1) as f64) - cen;
            }
            if c.have_prev && !was {
                c.onsets += 1;
            }
            for k in 0..nb {
                let p = f64::from(sh.phase[k]) - w1 * k as f64 * delta;
                c.psi[k] = princarg(p) as f32;
            }
        } else {
            let advance = |k: usize, sh: &Shared, c: &Core| -> f32 {
                let wk = w1 * k as f64;
                let dphi = princarg(
                    f64::from(sh.phase[k]) - f64::from(c.prev_phase[k]) - wk * ha,
                );
                let winst = wk + dphi / ha;
                princarg(f64::from(c.psi[k]) + winst * hs) as f32
            };
            let mut peaks = std::mem::take(&mut sh.peaks);
            peaks.clear();
            if cfg.locking == PhaseLocking::Identity {
                let maxm = sh.mag.iter().cloned().fold(0.0f32, f32::max);
                let floor = maxm * 1e-4;
                for k in 0..nb {
                    let m = sh.mag[k];
                    if m <= floor || m <= 0.0 {
                        continue;
                    }
                    let lo = k.saturating_sub(2);
                    let hi = (k + 2).min(nb - 1);
                    if (lo..=hi).all(|j| j == k || sh.mag[j] < m) {
                        peaks.push(k);
                    }
                }
            }
            if peaks.is_empty() {
                for k in 0..nb {
                    let v = advance(k, sh, c);
                    c.psi[k] = v;
                }
            } else {
                for (j, &p) in peaks.iter().enumerate() {
                    let lo = if j == 0 { 0 } else { (peaks[j - 1] + p) / 2 + 1 };
                    let hi = if j + 1 == peaks.len() { nb - 1 } else { (p + peaks[j + 1]) / 2 };
                    let psi_p = advance(p, sh, c);
                    let phi_p = sh.phase[p];
                    for k in lo..=hi {
                        c.psi[k] = princarg(f64::from(psi_p) + f64::from(sh.phase[k]) - f64::from(phi_p)) as f32;
                    }
                }
            }
            sh.peaks = peaks;
        }
        c.prev_phase[..nb].copy_from_slice(&sh.phase[..nb]);
        c.prev_mag[..nb].copy_from_slice(&sh.mag[..nb]);
        // Resynthesis, with a short taper above the anti-alias cutoff.
        let cut = cut.min(nb);
        for k in 0..nb {
            let mut m = sh.mag[k];
            if k >= cut {
                m = 0.0;
            } else if k + 4 > cut {
                m *= (cut - k) as f32 / 4.0;
            }
            let (s, co) = c.psi[k].sin_cos();
            sh.buf[k] = Complex::new(m * co, m * s);
        }
        sh.buf[0].im = 0.0;
        sh.buf[n / 2].im = 0.0;
        for k in 1..n / 2 {
            sh.buf[n - k] = sh.buf[k].conj();
        }
        sh.ifft.process_with_scratch(&mut sh.buf, &mut sh.scratch);
        let omask = c.acc.len() - 1;
        for k in 0..n {
            let v = sh.buf[k].re * sh.synthesis[k];
            c.acc[((c.wbase + k as i64) as usize) & omask] += if v.is_finite() { v } else { 0.0 };
        }
    }

    fn available(&self, ch: usize) -> usize {
        self.cores.get(ch).map_or(0, |c| (c.wbase - c.released).max(0) as usize)
    }

    fn release_to(c: &mut Core, to: i64) {
        let to = to.min(c.wbase);
        let mask = c.acc.len() - 1;
        while c.released < to {
            c.acc[(c.released as usize) & mask] = 0.0;
            c.released += 1;
        }
    }
}

/// Time stretch by a ratio, streaming or offline.
///
/// `ratio` is output length over input length: 2 is twice as long (slower),
/// 0.5 half as long. Clamped to 0.25..=4 and changeable at any time.
/// Pitch is preserved.
///
/// Streaming use: `push` input (it accepts what fits and returns the count),
/// `pop` what is [`available`](Self::available). Output sample `q` corresponds
/// to input time `(q - lead) / ratio`, so the first [`TimeStretch::lead`]
/// output samples belong to the time before the first input sample. [`stretch_offline`](Self::stretch_offline)
/// handles that and the flush; it allocates and is for offline rendering.
/// `new` allocates every buffer; `push`, `pop` and the setters do not.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{TimeStretch, VocoderConfig};
///
/// let rate = 48_000.0;
/// let mut ts = TimeStretch::new(1, rate, VocoderConfig::for_rate(rate));
/// ts.set_ratio(1.5);
/// let input: Vec<f32> = (0..24_000)
///     .map(|n| 0.5 * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / rate).sin())
///     .collect();
/// let out = ts.stretch_offline(0, &input); // offline: allocates the result
/// assert_eq!(out.len(), 36_000);
/// ```
pub struct TimeStretch {
    e: Engine,
    ratio: f64,
}

impl TimeStretch {
    /// Allocates every buffer.
    pub fn new(channels: usize, sample_rate: f32, config: VocoderConfig) -> Self {
        Self { e: Engine::new(channels, sample_rate, config), ratio: 1.0 }
    }

    /// Output length over input length, clamped to 0.25..=4; a non-finite ratio
    /// sets 1. Takes effect at the next frame. Realtime-safe.
    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = if ratio.is_finite() { f64::from(ratio).clamp(0.25, 4.0) } else { 1.0 };
    }

    /// The ratio in use.
    pub fn ratio(&self) -> f32 {
        self.ratio as f32
    }

    /// The analysis frame size, samples.
    pub fn frame(&self) -> usize {
        self.e.sh.n
    }

    /// The synthesis hop, samples.
    pub fn hop(&self) -> usize {
        self.e.sh.hs
    }

    /// Output samples that precede the first input sample at the current ratio.
    pub fn lead(&self) -> usize {
        // Frames are anchored by their centres, so a frame's content sits at
        // the right stretched time: lead = frame / 2 + ratio * frame / 4.
        (self.e.sh.n as f64 * (0.5 + 0.25 * self.ratio)).round() as usize
    }

    /// Input latency of the streaming path at ratio 1, in samples.
    pub fn latency(&self) -> usize {
        self.e.sh.n
    }

    /// Onsets at which phases were reset (transient mode) on `channel`.
    pub fn onsets(&self, channel: usize) -> u64 {
        self.e.cores.get(channel).map_or(0, |c| c.onsets)
    }

    /// Clears every channel's history and queued output.
    pub fn reset(&mut self) {
        self.e.reset();
    }

    /// Feed input; returns how many samples were taken (all, unless the output is full).
    pub fn push(&mut self, channel: usize, input: &[f32]) -> usize {
        self.e.push(channel, input, self.ratio, usize::MAX)
    }

    /// Finished output samples waiting on `channel` (0 for a missing channel).
    pub fn available(&self, channel: usize) -> usize {
        self.e.available(channel)
    }

    /// Take up to `out.len()` finished output samples; returns the count.
    pub fn pop(&mut self, channel: usize, out: &mut [f32]) -> usize {
        let n = out.len().min(self.e.available(channel));
        let Some(c) = self.e.cores.get_mut(channel) else { return 0 };
        let mask = c.acc.len() - 1;
        for (i, o) in out[..n].iter_mut().enumerate() {
            *o = c.acc[((c.released + i as i64) as usize) & mask];
        }
        Engine::release_to(c, c.released + n as i64);
        n
    }

    /// Stretch a whole signal on `channel` (offline): resets the channel,
    /// processes, flushes and returns exactly `round(len * ratio)` samples.
    /// Allocates the result.
    pub fn stretch_offline(&mut self, channel: usize, input: &[f32]) -> Vec<f32> {
        let (n, hs) = (self.e.sh.n, self.e.sh.hs);
        if let Some(c) = self.e.cores.get_mut(channel) {
            Engine::reset_core(c, n, hs);
        }
        let want = (input.len() as f64 * self.ratio).round() as usize;
        let lead = self.lead();
        let mut out = Vec::with_capacity(want + lead + 2 * n);
        let mut tmp = vec![0.0f32; 1024];
        let zeros = vec![0.0f32; 1024];
        let mut fed = 0usize;
        let mut guard = 0usize;
        while out.len() < lead + want && guard < 1_000_000 {
            guard += 1;
            let chunk = if fed < input.len() {
                let end = (fed + 1024).min(input.len());
                let t = self.push(channel, &input[fed..end]);
                fed += t;
                t
            } else {
                self.push(channel, &zeros)
            };
            loop {
                let got = self.pop(channel, &mut tmp);
                if got == 0 {
                    break;
                }
                out.extend_from_slice(&tmp[..got]);
            }
            let _ = chunk;
        }
        out.drain(..lead.min(out.len()));
        out.truncate(want);
        out
    }
}

struct Shift {
    rpos: f64,
    started: bool,
    underruns: u64,
}

/// Real-time pitch shifter: phase-vocoder stretch plus resample. See the
/// module docs. Fixed latency [`PitchShifter::latency`].
/// `new` allocates every buffer; `process` does not.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{PitchShifter, VocoderConfig};
///
/// let rate = 48_000.0;
/// let mut shifter = PitchShifter::new(2, rate, VocoderConfig::for_rate(rate));
/// shifter.set_semitones(7.0); // a fifth up
/// let mut left = vec![0.0f32; 512];
/// let mut right = vec![0.0f32; 512];
/// shifter.process(0, &mut left);
/// shifter.process(1, &mut right);
/// assert_eq!(shifter.latency(), 2048 / 2 + 2048 + 64);
/// ```
pub struct PitchShifter {
    e: Engine,
    ratio: f64,
    min_ratio: f64,
    shift: Vec<Shift>,
    table: Vec<f32>,
}

impl PitchShifter {
    /// Shifts from -12 to +12 semitones. Allocates every buffer.
    pub fn new(channels: usize, sample_rate: f32, config: VocoderConfig) -> Self {
        Self::with_min_ratio(channels, sample_rate, config, 0.5)
    }

    /// Like [`PitchShifter::new`] with the lowest ratio set to `min_ratio`
    /// (clamped to 0.5..=1; a higher value lowers the latency, which is
    /// `frame / 2 + frame / (2 min_ratio) + 64`).
    pub fn with_min_ratio(channels: usize, sample_rate: f32, config: VocoderConfig, min_ratio: f32) -> Self {
        let min_ratio = if min_ratio.is_finite() { f64::from(min_ratio).clamp(0.5, 1.0) } else { 0.5 };
        let e = Engine::new(channels, sample_rate, config);
        let mut table = vec![0.0f32; (PHASES + 1) * TAPS];
        let fc = 0.46f64;
        let beta = 9.0;
        let i0b = crate::windows::bessel_i0(beta);
        for u in 0..=PHASES {
            let f = u as f64 / PHASES as f64;
            let mut sum = 0.0;
            let mut row = [0.0f64; TAPS];
            for (k, r) in row.iter_mut().enumerate() {
                let x = (k as f64 - (HALF as f64 - 1.0)) - f;
                let t = x / HALF as f64;
                let w = if t.abs() < 1.0 {
                    crate::windows::bessel_i0(beta * (1.0 - t * t).sqrt()) / i0b
                } else {
                    0.0
                };
                let a = 2.0 * fc * x;
                let s = if a.abs() < 1e-12 { 1.0 } else { (PI * a).sin() / (PI * a) };
                *r = 2.0 * fc * s * w;
                sum += *r;
            }
            for (k, r) in row.iter().enumerate() {
                table[u * TAPS + k] = (*r / sum) as f32;
            }
        }
        let shift = (0..e.cores.len()).map(|_| Shift { rpos: 0.0, started: false, underruns: 0 }).collect();
        let mut s = Self { e, ratio: 1.0, min_ratio, shift, table };
        s.reset();
        s
    }

    /// Pitch ratio `2^(semitones / 12)`, limited to the supported range
    /// (+12 semitones, and down to the `min_ratio` of the constructor).
    pub fn set_semitones(&mut self, semitones: f32) {
        let s = if semitones.is_finite() { semitones } else { 0.0 };
        self.set_ratio(2f32.powf(s / 12.0));
    }

    /// Pitch ratio directly, limited to `min_ratio..=2`.
    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = if ratio.is_finite() { f64::from(ratio).clamp(self.min_ratio, 2.0) } else { 1.0 };
    }

    /// The lowest ratio accepted (0.5 for [`PitchShifter::new`]).
    pub fn min_ratio(&self) -> f32 {
        self.min_ratio as f32
    }

    /// Fixed latency in samples for `Plugin::latency`,
    /// `frame / 2 + frame / (2 min_ratio) + 64`: 1.5 frames + 64 for the
    /// default range of -12 to +12 semitones.
    ///
    /// It is the same at every ratio: a stretch-then-resample shifter needs
    /// `frame / (2 p)` samples of look-ahead at ratio `p`, so the latency
    /// is sized for the lowest ratio allowed and held there.
    pub fn latency(&self) -> usize {
        let n = self.e.sh.n as f64;
        (n * 0.5 + n / (2.0 * self.min_ratio)).ceil() as usize + MARGIN
    }

    /// The analysis frame size, samples.
    pub fn frame(&self) -> usize {
        self.e.sh.n
    }

    /// Times the reader caught up with the vocoder (output held at zero).
    /// Zero in normal use; can count after an abrupt ratio change.
    pub fn underruns(&self, channel: usize) -> u64 {
        self.shift.get(channel).map_or(0, |s| s.underruns)
    }

    /// Onsets at which phases were reset (transient mode) on `channel`.
    pub fn onsets(&self, channel: usize) -> u64 {
        self.e.cores.get(channel).map_or(0, |c| c.onsets)
    }

    /// Clears every channel's history; the latency stays the same.
    pub fn reset(&mut self) {
        self.e.reset();
        for s in &mut self.shift {
            s.rpos = 0.0;
            s.started = false;
        }
    }

    /// Process one channel in place (any length).
    pub fn process(&mut self, channel: usize, buffer: &mut [f32]) {
        if channel >= self.shift.len() {
            return;
        }
        let p = self.ratio;
        let nb = self.e.sh.n / 2 + 1;
        let cut = if p > 1.0 { ((nb - 1) as f64 / p) as usize } else { usize::MAX };
        let hs = self.e.sh.hs as i64;
        let gap_limit = (self.e.out_cap - self.e.sh.n) as i64 - 4 * hs - 64;
        let t_lat = self.latency() as f64;
        let n = self.e.sh.n as f64;
        // Stretched position read for the output at time t:
        // p (t - latency) + frame / 2 + p frame / 4 (the vocoder centres
        // frames, so a frame's content sits `lead` samples into the stream).
        let lead = n * (0.5 + 0.25 * p);
        // Steady-state distance between the newest finished sample and the reader.
        let gap_nom = (p * (t_lat - hs as f64) - lead).max(p * 16.0);
        for chunk in buffer.chunks_mut(CHUNK) {
            let taken = self.e.push(channel, chunk, p, cut);
            let c = &mut self.e.cores[channel];
            let st = &mut self.shift[channel];
            let _ = taken;
            if !st.started {
                st.started = true;
                st.rpos = lead - p * t_lat;
            }
            for s in chunk.iter_mut() {
                let i = st.rpos.floor() as i64;
                if i + HALF as i64 + 1 >= c.wbase {
                    st.underruns += 1;
                    *s = 0.0;
                    continue;
                }
                let f = st.rpos - i as f64;
                let fp = f * PHASES as f64;
                let u = (fp as usize).min(PHASES - 1);
                let a = (fp - u as f64) as f32;
                let mask = c.acc.len() - 1;
                let (r0, r1) = (&self.table[u * TAPS..(u + 1) * TAPS], &self.table[(u + 1) * TAPS..(u + 2) * TAPS]);
                let mut y = 0.0f32;
                for k in 0..TAPS {
                    let idx = i - (HALF as i64 - 1) + k as i64;
                    let w = r0[k] + a * (r1[k] - r0[k]);
                    y += w * c.acc[(idx as usize) & mask];
                }
                *s = if y.is_finite() { y } else { 0.0 };
                st.rpos += p;
                Engine::release_to(c, st.rpos.floor() as i64 - HALF as i64 - 2);
            }
            // After an abrupt ratio change the reader may lag far behind: re-sync.
            if c.wbase - (st.rpos as i64) > gap_limit {
                st.rpos = c.wbase as f64 - gap_nom;
                Engine::release_to(c, st.rpos.floor() as i64 - HALF as i64 - 2);
            }
        }
    }
}
