//! Monophonic pitch detection: YIN and McLeod's pitch method (MPM).
//!
//! Both detectors look at one block of the most recent `window()` samples and
//! report a [`Pitch`]: a frequency in Hz and a confidence in `0..=1`. They are
//! block based: construct once (this allocates and plans the transforms), then
//! call [`PitchDetector::detect`] as often as you like. `detect` does not
//! allocate, does not panic, and treats non-finite samples as zero.
//!
//! # YIN
//! A. de Cheveigne and H. Kawahara, "YIN, a fundamental frequency estimator
//! for speech and music", J. Acoust. Soc. Am. 111(4), 2002. Steps used:
//! the difference function `d(t) = sum_j (x_j - x_{j+t})^2` (step 2), the
//! cumulative mean normalised difference `d'(t) = d(t) / ((1/t) sum_{k<=t} d(k))`
//! (step 3), the absolute threshold (step 4: the first `t` whose `d'` is under
//! the threshold, then the lowest point of that dip; the global minimum when
//! none is), and parabolic interpolation of the minimum (step 5). Steps 1 and
//! 6 (autocorrelation method, best local estimate) are not used. `d(t)` is
//! computed as `E_0 + E_t - 2 r(t)` with the cross-correlation `r` from an
//! FFT, so the cost is `O(n log n)` instead of `O(n * lag)`.
//! Confidence is `1 - d'(t)`, so aperiodic input (noise, `d'` near 1) is near 0.
//!
//! # MPM
//! P. McLeod and G. Wyvill, "A smarter way to find pitch", Proc. ICMC 2005.
//! The normalised square difference function
//! `n'(t) = 2 r'(t) / m'(t)`, `r'(t) = sum_j x_j x_{j+t}`,
//! `m'(t) = sum_j (x_j^2 + x_{j+t}^2)` (both over the same overlap) is
//! computed from an FFT autocorrelation (Wiener-Khinchin) with the recursive
//! `m'`. Key maxima are the highest point between each positive-going and the
//! next negative-going zero crossing; the first one above `k` times the
//! highest is the period (`k` = 0.93 by default, the paper suggests 0.8 to
//! 1.0), refined by a parabola. Confidence (the paper's "clarity") is the
//! interpolated `n'` at that peak.
//!
//! Both need a window of about two periods of the lowest frequency
//! ([`PitchDetector::window`]). A tone needs a few periods inside the window
//! to be found. Frequencies are reported in Hz at the real sample rate.

use crate::rate::effective_rate;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// One pitch estimate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pitch {
    /// Estimated fundamental in Hz, or 0 when there was no candidate (silence).
    pub frequency: f32,
    /// How periodic the block is, `0..=1`. Noise reads low, a clean tone near 1.
    pub confidence: f32,
}

impl Pitch {
    /// No estimate.
    pub const NONE: Pitch = Pitch { frequency: 0.0, confidence: 0.0 };

    /// True when a frequency was found with at least `threshold` confidence.
    pub fn is_voiced(&self, threshold: f32) -> bool {
        self.frequency > 0.0 && self.confidence >= threshold
    }
}

/// Common interface of the detectors.
pub trait PitchDetector {
    /// Number of samples `detect` uses (the most recent ones of its argument).
    fn window(&self) -> usize;
    /// Estimate the pitch of the last [`window`](Self::window) samples of
    /// `samples`. A shorter slice gives [`Pitch::NONE`].
    fn detect(&mut self, samples: &[f32]) -> Pitch;
}

/// Parabolic interpolation through three points; returns (offset, value) of
/// the extremum relative to the middle point, offset within +-1.
fn parabola(a: f64, b: f64, c: f64) -> (f64, f64) {
    let den = a - 2.0 * b + c;
    if den.abs() < 1e-300 {
        return (0.0, b);
    }
    let off = 0.5 * (a - c) / den;
    if !off.is_finite() || off.abs() > 1.0 {
        return (0.0, b);
    }
    (off, b - 0.25 * (a - c) * off)
}

fn range_lags(rate: f32, min_hz: f32, max_hz: f32) -> (f32, usize, usize) {
    let fs = effective_rate(rate);
    let lo = if min_hz.is_finite() { min_hz.clamp(1.0, fs * 0.25) } else { 50.0 };
    let hi = if max_hz.is_finite() { max_hz.clamp(lo * 1.5, fs * 0.45) } else { 2000.0 };
    let tau_max = (fs / lo).ceil() as usize + 1;
    let tau_min = ((fs / hi).floor() as usize).max(2);
    (fs, tau_min, tau_max)
}

/// YIN detector. See the module docs.
pub struct YinDetector {
    fs: f64,
    threshold: f32,
    tau_min: usize,
    tau_max: usize,
    /// Integration window `W`.
    w: usize,
    n: usize,
    fft: Arc<dyn Fft<f64>>,
    ifft: Arc<dyn Fft<f64>>,
    scratch: Vec<Complex<f64>>,
    a: Vec<Complex<f64>>,
    b: Vec<Complex<f64>>,
    cum: Vec<f64>,
    cmnd: Vec<f64>,
    diff: Vec<f64>,
}

impl YinDetector {
    /// A detector for `min_hz..=max_hz` at `sample_rate`. The threshold
    /// defaults to 0.15 (the paper uses 0.1 to 0.15). Allocates.
    pub fn new(sample_rate: f32, min_hz: f32, max_hz: f32) -> Self {
        let (fs, tau_min, tau_max) = range_lags(sample_rate, min_hz, max_hz);
        let w = tau_max;
        let n = w + tau_max;
        let m = n.next_power_of_two();
        let mut planner = FftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(m);
        let ifft = planner.plan_fft_inverse(m);
        let scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len().max(ifft.get_inplace_scratch_len())];
        Self {
            fs: f64::from(fs),
            threshold: 0.15,
            tau_min,
            tau_max,
            w,
            n,
            fft,
            ifft,
            scratch,
            a: vec![Complex::new(0.0, 0.0); m],
            b: vec![Complex::new(0.0, 0.0); m],
            cum: vec![0.0; n + 1],
            cmnd: vec![0.0; tau_max + 1],
            diff: vec![0.0; tau_max + 1],
        }
    }

    /// Absolute threshold of step 4, clamped to 0.01..=0.5.
    pub fn set_threshold(&mut self, t: f32) {
        if t.is_finite() {
            self.threshold = t.clamp(0.01, 0.5);
        }
    }
}

impl PitchDetector for YinDetector {
    fn window(&self) -> usize {
        self.n
    }

    fn detect(&mut self, samples: &[f32]) -> Pitch {
        if samples.len() < self.n {
            return Pitch::NONE;
        }
        let x = &samples[samples.len() - self.n..];
        let m = self.a.len();
        for v in self.a.iter_mut().chain(self.b.iter_mut()) {
            *v = Complex::new(0.0, 0.0);
        }
        self.cum[0] = 0.0;
        for (i, &s) in x.iter().enumerate() {
            let s = if s.is_finite() { f64::from(s) } else { 0.0 };
            self.b[i].re = s;
            if i < self.w {
                self.a[i].re = s;
            }
            self.cum[i + 1] = self.cum[i] + s * s;
        }
        let e0 = self.cum[self.w];
        if e0 < 1e-20 {
            return Pitch::NONE;
        }
        self.fft.process_with_scratch(&mut self.a, &mut self.scratch);
        self.fft.process_with_scratch(&mut self.b, &mut self.scratch);
        for i in 0..m {
            self.a[i] = self.a[i].conj() * self.b[i];
        }
        self.ifft.process_with_scratch(&mut self.a, &mut self.scratch);
        let scale = 1.0 / m as f64;
        // d(t) -> CMND.
        self.cmnd[0] = 1.0;
        let mut running = 0.0;
        for t in 1..=self.tau_max {
            let et = self.cum[t + self.w] - self.cum[t];
            let d = (e0 + et - 2.0 * self.a[t].re * scale).max(0.0);
            running += d;
            self.diff[t] = d;
            self.cmnd[t] = if running > 1e-30 { d * t as f64 / running } else { 1.0 };
        }
        // Step 4.
        let thr = f64::from(self.threshold);
        let mut pick = 0usize;
        let mut t = self.tau_min;
        while t < self.tau_max {
            if self.cmnd[t] < thr {
                // Lowest point of the whole dip (a noisy dip has ripples on its
                // flank; stopping at the first local minimum biases the lag).
                let mut best = t;
                while t + 1 < self.tau_max && self.cmnd[t + 1] < thr {
                    t += 1;
                    if self.cmnd[t] < self.cmnd[best] {
                        best = t;
                    }
                }
                pick = best;
                break;
            }
            t += 1;
        }
        if pick == 0 {
            let mut best = f64::INFINITY;
            for (t, &d) in self.cmnd.iter().enumerate().take(self.tau_max).skip(self.tau_min) {
                if d < best {
                    best = d;
                    pick = t;
                }
            }
            if pick == 0 {
                return Pitch::NONE;
            }
        }
        // Step 5: parabolic interpolation. Interior points only.
        let (tau, val) = if pick > 1 && pick < self.tau_max {
            // The paper interpolates the difference function itself (the
            // normalisation by the running mean tilts the dip towards short lags).
            let (off, _) = parabola(self.diff[pick - 1], self.diff[pick], self.diff[pick + 1]);
            (pick as f64 + off, self.cmnd[pick])
        } else {
            (pick as f64, self.cmnd[pick])
        };
        let conf = (1.0 - val).clamp(0.0, 1.0);
        Pitch { frequency: (self.fs / tau) as f32, confidence: conf as f32 }
    }
}

/// McLeod pitch method detector. See the module docs.
pub struct MpmDetector {
    fs: f64,
    cutoff: f32,
    tau_min: usize,
    w: usize,
    fft: Arc<dyn Fft<f64>>,
    ifft: Arc<dyn Fft<f64>>,
    scratch: Vec<Complex<f64>>,
    a: Vec<Complex<f64>>,
    nsdf: Vec<f64>,
    sq: Vec<f64>,
}

impl MpmDetector {
    /// A detector for `min_hz..=max_hz` at `sample_rate`. The key-maximum
    /// cutoff `k` defaults to 0.93. Allocates.
    pub fn new(sample_rate: f32, min_hz: f32, max_hz: f32) -> Self {
        let (fs, tau_min, tau_max) = range_lags(sample_rate, min_hz, max_hz);
        // n'(t) is useful up to about W/2; two periods of the lowest tone.
        let w = 2 * tau_max;
        let m = (2 * w).next_power_of_two();
        let mut planner = FftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(m);
        let ifft = planner.plan_fft_inverse(m);
        let scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len().max(ifft.get_inplace_scratch_len())];
        Self {
            fs: f64::from(fs),
            cutoff: 0.93,
            tau_min,
            w,
            fft,
            ifft,
            scratch,
            a: vec![Complex::new(0.0, 0.0); m],
            nsdf: vec![0.0; tau_max + 2],
            sq: vec![0.0; w],
        }
    }

    /// The key-maximum cutoff `k`, clamped to 0.5..=1.
    pub fn set_cutoff(&mut self, k: f32) {
        if k.is_finite() {
            self.cutoff = k.clamp(0.5, 1.0);
        }
    }
}

impl MpmDetector {
    fn key_maxima(&self, mut f: impl FnMut(usize, f64)) {
        let n = self.nsdf.len();
        let mut i = 1;
        while i < n && self.nsdf[i] > 0.0 {
            i += 1;
        }
        while i < n {
            while i < n && self.nsdf[i] <= 0.0 {
                i += 1;
            }
            let mut pk = 0usize;
            let mut pv = f64::NEG_INFINITY;
            while i < n && self.nsdf[i] > 0.0 {
                if self.nsdf[i] > pv {
                    pv = self.nsdf[i];
                    pk = i;
                }
                i += 1;
            }
            if pk >= self.tau_min && pk + 1 < n && pv.is_finite() {
                f(pk, pv);
            }
        }
    }
}

impl PitchDetector for MpmDetector {
    fn window(&self) -> usize {
        self.w
    }

    fn detect(&mut self, samples: &[f32]) -> Pitch {
        if samples.len() < self.w {
            return Pitch::NONE;
        }
        let x = &samples[samples.len() - self.w..];
        let m = self.a.len();
        for v in self.a.iter_mut() {
            *v = Complex::new(0.0, 0.0);
        }
        let mut e = 0.0;
        for (i, &s) in x.iter().enumerate() {
            let s = if s.is_finite() { f64::from(s) } else { 0.0 };
            self.a[i].re = s;
            self.sq[i] = s * s;
            e += s * s;
        }
        if e < 1e-20 {
            return Pitch::NONE;
        }
        self.fft.process_with_scratch(&mut self.a, &mut self.scratch);
        for v in self.a.iter_mut() {
            *v = Complex::new(v.norm_sqr(), 0.0);
        }
        self.ifft.process_with_scratch(&mut self.a, &mut self.scratch);
        let scale = 1.0 / m as f64;
        let n_lags = self.nsdf.len();
        let mut mp = 2.0 * e;
        self.nsdf[0] = 1.0;
        for t in 1..n_lags {
            mp -= self.sq[t - 1] + self.sq[self.w - t];
            self.nsdf[t] = if mp > 1e-20 { 2.0 * self.a[t].re * scale / mp } else { 0.0 };
        }
        // Key maxima: the highest point of each positive region after the
        // first (lag 0) lobe has fallen below zero.
        let mut best_val = f64::NEG_INFINITY;
        self.key_maxima(|_, v| {
            if v > best_val {
                best_val = v;
            }
        });
        if !best_val.is_finite() {
            return Pitch::NONE;
        }
        let thr = f64::from(self.cutoff) * best_val;
        let mut chosen: Option<usize> = None;
        self.key_maxima(|k, v| {
            if chosen.is_none() && v >= thr {
                chosen = Some(k);
            }
        });
        let Some(k) = chosen else { return Pitch::NONE };
        let (off, val) = parabola(self.nsdf[k - 1], self.nsdf[k], self.nsdf[k + 1]);
        let tau = k as f64 + off;
        Pitch { frequency: (self.fs / tau) as f32, confidence: val.clamp(0.0, 1.0) as f32 }
    }
}
