//! True-peak estimation by 4x oversampled interpolation.
//!
//! Source: Recommendation ITU-R BS.1770-4 (2015), Annex 2, "True-peak
//! measurement". The sample-peak of a PCM signal can understate the peak of the
//! reconstructed analogue waveform (an fs/4 sine sampled at 45 degrees reads
//! 3.01 dB low). The Annex estimates the true peak by up-sampling by 4 with a
//! 48-tap low-pass (12 taps in each of 4 polyphase branches) and taking the
//! largest absolute value over the interpolated samples.
//!
//! The filters here are designed for this crate rather than copied from the
//! Annex's coefficient table: Kaiser-windowed sincs with the cut-off at the
//! original Nyquist rate, each polyphase branch normalised to unity DC gain.
//!
//! - [`TruePeak::new`] has the Annex's structure, 4 branches of 12 taps (48
//!   multiplies per input sample, delay 6). With only four points per sample
//!   the true crest can fall between two of them, so a full-scale sine reads
//!   low by up to a factor cos(pi f / 8): measured in this module's tests, at
//!   most 0.35 dB low and 0.10 dB high for sines up to 0.4 fs (more near
//!   0.5 fs, as the Annex itself notes).
//! - [`TruePeak::precise`] uses 8 branches of 16 taps (128 multiplies per
//!   sample, delay 9) and a parabolic estimate of the crest between grid
//!   points; the same sweep reads between 0.03 dB low and 0.02 dB high.
//!
//! Both are accurate for sines up to 0.4 fs and read low above that: with 12
//! or 16 taps per phase the transition band is wide, so on full-band white
//! noise [`TruePeak::precise`] reads up to 2.6 dB below a 96-tap interpolator.
//! The [`crate::Limiter`] therefore no longer uses it; its detector
//! (`intersample.rs`) evaluates a 96-tap filter together with the filter of
//! [`crate::TruePeakMeter`] and the taps of [`TruePeak::new`].

use crate::windows::bessel_i0;

const MAX_PHASES: usize = 8;
const MAX_TAPS: usize = 16;
const STANDARD_TAPS: usize = 12;
const STANDARD_BETA: f64 = 4.0;
const PRECISE_BETA: f64 = 6.0;
const PRECISE_TAPS: usize = 16;
const PRECISE_PHASES: usize = 8;

/// Delay, in input samples, of [`TruePeak::new`] (12 taps per phase).
pub const TRUE_PEAK_DELAY: usize = STANDARD_TAPS / 2;

/// Kaiser-windowed sinc, `taps` per phase, window shape `beta`; each phase
/// normalised to unity DC gain. Phase `p`, tap `k` is coefficient `4k + p`.
fn design(phases: usize, taps: usize, beta: f64) -> [[f32; MAX_TAPS]; MAX_PHASES] {
    let n = taps * phases;
    let centre = (n as f64 - 1.0) / 2.0;
    let denom = bessel_i0(beta);
    let mut h = [[0.0f64; MAX_TAPS]; MAX_PHASES];
    for m in 0..n {
        let t = (m as f64 - centre) / phases as f64;
        let sinc = if t == 0.0 { 1.0 } else { (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t) };
        let r = (m as f64 - centre) / centre;
        let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / denom;
        h[m % phases][m / phases] = sinc * w;
    }
    let mut out = [[0.0f32; MAX_TAPS]; MAX_PHASES];
    for p in 0..phases {
        let sum: f64 = h[p][..taps].iter().sum();
        for k in 0..taps {
            out[p][k] = (h[p][k] / sum) as f32;
        }
    }
    out
}

/// The taps of [`TruePeak::new`], for the limiter's detector
/// ([`crate::intersample`]): `[p][k]` multiplies `x[n - k]`, 12 taps a phase.
pub(crate) fn standard_taps() -> [[f32; MAX_TAPS]; MAX_PHASES] {
    design(4, STANDARD_TAPS, STANDARD_BETA)
}

/// One channel's 4x true-peak detector.
#[derive(Debug, Clone)]
pub struct TruePeak {
    taps: [[f32; MAX_TAPS]; MAX_PHASES],
    phases: usize,
    refine: bool,
    tail: [f32; 2],
    n: usize,
    /// `hist[0]` is the newest input sample.
    hist: [f32; MAX_TAPS],
}

impl Default for TruePeak {
    fn default() -> Self {
        Self::new()
    }
}

impl TruePeak {
    /// The BS.1770-4 structure: 4 phases of 12 taps. See the module docs for
    /// its measured accuracy.
    pub fn new() -> Self {
        Self { taps: design(4, STANDARD_TAPS, STANDARD_BETA), phases: 4, refine: false, tail: [0.0; 2], n: STANDARD_TAPS, hist: [0.0; MAX_TAPS] }
    }

    /// 8 phases of 16 taps with crest refinement: about 2.7 times the cost and
    /// 3 more samples of delay for much better accuracy below 0.4 fs.
    pub fn precise() -> Self {
        Self { taps: design(PRECISE_PHASES, PRECISE_TAPS, PRECISE_BETA), phases: PRECISE_PHASES, refine: true, tail: [0.0; 2], n: PRECISE_TAPS, hist: [0.0; MAX_TAPS] }
    }

    /// Delay in input samples: a reading returned for sample `n` describes the
    /// waveform around sample `n - delay()`.
    pub fn delay(&self) -> usize {
        self.n / 2 + usize::from(self.refine)
    }

    pub fn reset(&mut self) {
        self.hist = [0.0; MAX_TAPS];
        self.tail = [0.0; 2];
    }

    /// Feed one sample; returns the largest absolute value among the four
    /// interpolated points (linear amplitude, finite, never negative). A
    /// non-finite input counts as 0.
    #[inline]
    pub fn push(&mut self, x: f32) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        let n = self.n;
        self.hist.copy_within(0..n - 1, 1);
        self.hist[0] = x;
        let mut peak = 0.0f32;
        if !self.refine {
            for phase in &self.taps[..self.phases] {
                let acc: f32 = phase[..n].iter().zip(&self.hist[..n]).map(|(a, b)| a * b).sum();
                peak = peak.max(acc.abs());
            }
            return peak;
        }
        // Refined: sequence = two carried values, then this sample's outputs.
        // Every centre whose neighbours are known is tested; a local maximum
        // gets a parabolic vertex estimate, which recovers the peak that falls
        // between two grid points.
        let mut seq = [0.0f32; MAX_PHASES + 2];
        seq[0] = self.tail[0];
        seq[1] = self.tail[1];
        for (p, phase) in self.taps[..self.phases].iter().enumerate() {
            let acc: f32 = phase[..n].iter().zip(&self.hist[..n]).map(|(a, b)| a * b).sum();
            seq[2 + p] = acc.abs();
        }
        let total = self.phases + 2;
        for c in 1..total - 1 {
            let (a, b, d) = (seq[c - 1], seq[c], seq[c + 1]);
            let mut v = b;
            if b >= a && b >= d {
                let den = a - 2.0 * b + d;
                if den < -1.0e-12 {
                    let vertex = b - (a - d) * (a - d) / (8.0 * den);
                    // The vertex of a sampled band-limited peak cannot exceed
                    // the neighbours' envelope by much; cap the estimate.
                    v = vertex.min(b * 1.2);
                }
            }
            peak = peak.max(v);
        }
        self.tail = [seq[total - 2], seq[total - 1]];
        peak
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_gain_is_unity_per_phase() {
        for (phases, taps, beta) in [(4, STANDARD_TAPS, STANDARD_BETA), (PRECISE_PHASES, PRECISE_TAPS, PRECISE_BETA)] {
            for phase in design(phases, taps, beta).iter().take(phases) {
                let s: f32 = phase[..taps].iter().sum();
                assert!((s - 1.0).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn fs_over_4_sine_at_45_degrees_reads_full_amplitude() {
        for (mut tp, tol) in [(TruePeak::new(), 0.02f32), (TruePeak::precise(), 0.005)] {
            let mut best = 0.0f32;
            for n in 0..400 {
                let x = (std::f32::consts::FRAC_PI_2 * n as f32 + std::f32::consts::FRAC_PI_4).sin();
                let p = tp.push(x);
                if n > 60 {
                    best = best.max(p);
                }
            }
            // The sample peak is 0.7071; the true peak is 1.0.
            assert!((best - 1.0).abs() < tol, "{best}");
        }
    }

    /// Worst under- and over-reading of unit sines from 0.02 to 0.40 fs, 8 phases each.
    fn sweep(mut make: impl FnMut() -> TruePeak) -> (f64, f64) {
        let mut under = 0.0f64;
        let mut over = 0.0f64;
        let mut f = 0.02f64;
        while f < 0.401 {
            for ph in 0..8 {
                let mut tp = make();
                let mut best = 0.0f32;
                for n in 0..600 {
                    let x = (std::f64::consts::TAU * f * n as f64 + ph as f64 * 0.8).sin() as f32;
                    let p = tp.push(x);
                    if n > 80 {
                        best = best.max(p);
                    }
                }
                let d = 20.0 * (best as f64).log10();
                under = under.min(d);
                over = over.max(d);
            }
            f += 0.01;
        }
        (under, over)
    }

    #[test]
    fn measured_accuracy() {
        let (u, o) = sweep(TruePeak::new);
        eprintln!("standard: under {u:.3} dB, over {o:+.3} dB");
        assert!(u > -0.40 && o < 0.12, "{u} {o}");
        let (u, o) = sweep(TruePeak::precise);
        eprintln!("precise:  under {u:.3} dB, over {o:+.3} dB");
        assert!(u > -0.05 && o < 0.03, "{u} {o}");
    }

    #[test]
    fn impulse_delay_matches_delay() {
        for mut tp in [TruePeak::new(), TruePeak::precise()] {
            let mut at = 0;
            let mut best = 0.0;
            for n in 0..60 {
                let p = tp.push(if n == 0 { 1.0 } else { 0.0 });
                if p > best {
                    best = p;
                    at = n;
                }
            }
            let d = tp.delay();
            assert!(at + 1 >= d && at <= d, "peak at {at}, delay {d}");
        }
    }
}
