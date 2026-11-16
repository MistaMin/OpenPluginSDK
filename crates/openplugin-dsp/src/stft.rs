//! Short-time Fourier transform frame processor with overlap-add resynthesis.
//!
//! Method: J. B. Allen, "Short term spectral analysis, synthesis, and
//! modification by discrete Fourier transform", IEEE Trans. ASSP 25(3), 1977,
//! and D. W. Griffin and J. S. Lim, "Signal estimation from modified
//! short-time Fourier transform", IEEE Trans. ASSP 32(2), 1984 (weighted
//! overlap-add, WOLA). Every `hop` samples the last `frame` samples are
//! multiplied by the analysis window, transformed, handed to a
//! [`SpectralProcessor`], transformed back, multiplied by the synthesis
//! window and added into an output accumulator.
//!
//! Reconstruction needs the sum over frames of `analysis[n] * synthesis[n]` to
//! be a constant for every sample (the COLA condition, or WOLA when the two
//! windows differ). [`check_ola`] measures how far a window/hop pair is from
//! that, and [`Stft`] divides by the mean so the constant is exactly one. With
//! a pair that has ripple, resynthesis is modulated by it; read
//! [`Stft::ola_error`].
//!
//! Latency is exactly `frame` samples, for every host block size including 1:
//! the output at time `t` is the resynthesised sample of time `t - frame`,
//! which is final one sample before it is read.
//!
//! The transform is a `rustfft` complex transform of the real frame, planned in
//! [`Stft::new`] (the prepare call). The callback sees the non-redundant half,
//! bins `0..=frame/2`; the conjugate half is rebuilt before the inverse, so the
//! output is real by construction. (A real-to-complex packing at half the
//! length would halve the cost; it is not done here.) Bin `k` is at
//! `k * sample_rate / frame`. Bin phase is referred to the first (oldest)
//! sample of the frame, not its centre. `process*` does not allocate.

use crate::windows::WindowKind;
pub use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// Receives each spectrum, in place.
pub trait SpectralProcessor {
    /// `bins` holds frequency bins `0..=frame/2` of `channel`'s windowed frame.
    /// Bins 0 and `frame/2` are real by definition; their imaginary parts are ignored.
    fn process_frame(&mut self, channel: usize, bins: &mut [Complex<f32>]);
}

impl<F: FnMut(usize, &mut [Complex<f32>])> SpectralProcessor for F {
    fn process_frame(&mut self, channel: usize, bins: &mut [Complex<f32>]) {
        self(channel, bins)
    }
}

/// A processor that leaves the spectrum alone.
pub struct Identity;

impl SpectralProcessor for Identity {
    fn process_frame(&mut self, _: usize, _: &mut [Complex<f32>]) {}
}

/// Why an [`Stft`] could not be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StftError {
    /// Frame size is not a power of two of at least 4.
    FrameSize,
    /// Hop is zero or larger than the frame.
    Hop,
    /// The windows have no overlap-add gain (all zero), so no resynthesis exists.
    NoGain,
}

/// Overlap-add quality of a window pair at a hop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OlaReport {
    /// Mean of `sum_k analysis * synthesis` over one hop. [`Stft`] divides by it.
    pub gain: f64,
    /// `(max - min) / mean` of that sum over the hop phases. Zero is exact COLA.
    pub ripple: f64,
    /// `20 log10(ripple)`, or -inf when exact.
    pub ripple_db: f64,
}

impl OlaReport {
    /// True when the ripple is at most `tolerance` (for example `1e-4`).
    pub fn is_cola(&self, tolerance: f64) -> bool {
        self.ripple <= tolerance
    }
}

/// Measure the weighted overlap-add condition for window pair `analysis`,
/// `synthesis` (both length `frame`) at `hop`. For equal windows this is the
/// COLA check of the windows squared; with `synthesis` all ones it is the
/// plain COLA check of `analysis`.
pub fn check_ola_windows(analysis: &[f32], synthesis: &[f32], hop: usize) -> OlaReport {
    let n = analysis.len().min(synthesis.len());
    let hop = hop.clamp(1, n.max(1));
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut total = 0.0;
    for p in 0..hop {
        let mut sum = 0.0f64;
        let mut i = p;
        while i < n {
            sum += analysis[i] as f64 * synthesis[i] as f64;
            i += hop;
        }
        min = min.min(sum);
        max = max.max(sum);
        total += sum;
    }
    let mean = total / hop as f64;
    let ripple = if mean.abs() > 0.0 {
        (max - min) / mean.abs()
    } else {
        f64::INFINITY
    };
    OlaReport {
        gain: mean,
        ripple,
        ripple_db: 20.0 * ripple.log10(),
    }
}

/// [`check_ola_windows`] for named windows (periodic form).
pub fn check_ola(frame: usize, hop: usize, analysis: WindowKind, synthesis: WindowKind) -> OlaReport {
    check_ola_windows(
        &analysis.to_vec(frame, true),
        &synthesis.to_vec(frame, true),
        hop,
    )
}

/// STFT settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StftConfig {
    /// Frame size, a power of two of at least 4.
    pub frame: usize,
    /// Samples between frames, `1..=frame`.
    pub hop: usize,
    pub analysis: WindowKind,
    pub synthesis: WindowKind,
}

impl StftConfig {
    /// Hann analysis, rectangular synthesis, 75 % overlap: exact COLA.
    pub fn hann(frame: usize) -> Self {
        Self {
            frame,
            hop: frame / 4,
            analysis: WindowKind::Hann,
            synthesis: WindowKind::Rectangular,
        }
    }
}

struct Channel {
    input: Vec<f32>,
    acc: Vec<f32>,
    pos: usize,
    since: usize,
}

/// Multichannel STFT analysis/resynthesis.
pub struct Stft {
    n: usize,
    hop: usize,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    buf: Vec<Complex<f32>>,
    analysis: Vec<f32>,
    /// Synthesis window already divided by the OLA gain and by `n`.
    synthesis: Vec<f32>,
    report: OlaReport,
    channels: Vec<Channel>,
}

impl Stft {
    /// Prepare: plans the transforms and allocates every buffer.
    pub fn new(channels: usize, config: StftConfig) -> Result<Self, StftError> {
        let n = config.frame;
        if n < 4 || !n.is_power_of_two() {
            return Err(StftError::FrameSize);
        }
        if config.hop == 0 || config.hop > n {
            return Err(StftError::Hop);
        }
        let analysis = config.analysis.to_vec(n, true);
        let mut synthesis = config.synthesis.to_vec(n, true);
        let report = check_ola_windows(&analysis, &synthesis, config.hop);
        if report.gain.is_nan() || report.gain.abs() <= 1e-9 {
            return Err(StftError::NoGain);
        }
        let scale = (1.0 / (report.gain * n as f64)) as f32;
        for v in &mut synthesis {
            *v *= scale;
        }
        let mut planner = FftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(n);
        let inverse = planner.plan_fft_inverse(n);
        let scratch_len = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());
        Ok(Self {
            n,
            hop: config.hop,
            forward,
            inverse,
            scratch: vec![Complex::default(); scratch_len],
            buf: vec![Complex::default(); n],
            analysis,
            synthesis,
            report,
            channels: (0..channels.max(1))
                .map(|_| Channel {
                    input: vec![0.0; n],
                    acc: vec![0.0; n],
                    pos: 0,
                    since: 0,
                })
                .collect(),
        })
    }

    pub fn channels(&self) -> usize {
        self.channels.len()
    }

    pub fn frame(&self) -> usize {
        self.n
    }

    pub fn hop(&self) -> usize {
        self.hop
    }

    /// Number of bins the callback sees, `frame / 2 + 1`.
    pub fn bins(&self) -> usize {
        self.n / 2 + 1
    }

    /// Latency in samples for `Plugin::latency`: equal to the frame size.
    pub fn latency(&self) -> usize {
        self.n
    }

    /// Overlap-add quality of the configured pair (already compensated for gain).
    pub fn ola_error(&self) -> OlaReport {
        self.report
    }

    /// Clear all signal history.
    pub fn reset(&mut self) {
        for c in &mut self.channels {
            c.input.fill(0.0);
            c.acc.fill(0.0);
            c.pos = 0;
            c.since = 0;
        }
    }

    /// Process `input` into `output` (same length) for one channel.
    pub fn process<P: SpectralProcessor>(
        &mut self,
        channel: usize,
        input: &[f32],
        output: &mut [f32],
        processor: &mut P,
    ) {
        for (x, y) in input.iter().zip(output.iter_mut()) {
            *y = self.tick(channel, *x, processor);
        }
    }

    /// Process a buffer in place for one channel.
    pub fn process_in_place<P: SpectralProcessor>(
        &mut self,
        channel: usize,
        buffer: &mut [f32],
        processor: &mut P,
    ) {
        for s in buffer.iter_mut() {
            *s = self.tick(channel, *s, processor);
        }
    }

    /// One sample in, one sample out (delayed by [`Stft::latency`]). A
    /// `channel` the transform was not made with gives 0.
    pub fn tick<P: SpectralProcessor>(&mut self, channel: usize, x: f32, processor: &mut P) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        let n = self.n;
        let Some(c) = self.channels.get_mut(channel) else { return 0.0 };
        let pos = c.pos;
        c.input[pos] = x;
        let y = c.acc[pos];
        c.acc[pos] = 0.0;
        c.pos = if pos + 1 == n { 0 } else { pos + 1 };
        c.since += 1;
        if c.since == self.hop {
            c.since = 0;
            // Oldest sample of the frame is at `c.pos`.
            let start = c.pos;
            for k in 0..n {
                let idx = if start + k >= n { start + k - n } else { start + k };
                self.buf[k] = Complex::new(c.input[idx] * self.analysis[k], 0.0);
            }
            self.forward
                .process_with_scratch(&mut self.buf, &mut self.scratch);
            processor.process_frame(channel, &mut self.buf[..=n / 2]);
            self.buf[0].im = 0.0;
            self.buf[n / 2].im = 0.0;
            for k in 1..n / 2 {
                self.buf[n - k] = self.buf[k].conj();
            }
            self.inverse
                .process_with_scratch(&mut self.buf, &mut self.scratch);
            for k in 0..n {
                let idx = if start + k >= n { start + k - n } else { start + k };
                c.acc[idx] += self.buf[k].re * self.synthesis[k];
            }
        }
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_settings() {
        let ok = StftConfig::hann(256);
        assert_eq!(Stft::new(1, StftConfig { frame: 100, ..ok }).err(), Some(StftError::FrameSize));
        assert_eq!(Stft::new(1, StftConfig { hop: 0, ..ok }).err(), Some(StftError::Hop));
        assert_eq!(Stft::new(1, StftConfig { hop: 512, ..ok }).err(), Some(StftError::Hop));
    }

    #[test]
    fn cola_checker_accepts_good_and_flags_bad() {
        use WindowKind::*;
        let one = Rectangular;
        assert!(check_ola(1024, 256, Hann, one).is_cola(1e-6));
        assert!(check_ola(1024, 512, Hann, one).is_cola(1e-6));
        assert!(check_ola(1024, 512, Hamming, one).is_cola(1e-6));
        assert!(check_ola(1024, 256, BlackmanHarris, one).is_cola(1e-6));
        assert!(check_ola(1024, 512, SqrtHann, SqrtHann).is_cola(1e-6));
        assert!(check_ola(1024, 128, Rectangular, one).is_cola(1e-9));
        // Bad pairs.
        let bad = check_ola(1024, 768, Hann, one);
        assert!(bad.ripple > 0.5, "{bad:?}");
        assert!(check_ola(1024, 1024, Hann, one).ripple > 1.0);
        assert!(check_ola(1024, 512, BlackmanHarris, one).ripple > 0.1);
        assert!(check_ola(1024, 512, Kaiser { beta: 8.0 }, one).ripple > 0.01);
    }

    #[test]
    fn sixteen_samples_of_latency_for_a_frame_of_sixteen() {
        let mut s = Stft::new(1, StftConfig::hann(16)).unwrap();
        assert_eq!(s.latency(), 16);
        let mut out = Vec::new();
        for t in 0..40 {
            out.push(s.tick(0, if t == 3 { 1.0 } else { 0.0 }, &mut Identity));
        }
        assert!((out[19] - 1.0).abs() < 1e-5, "{out:?}");
    }

    #[test]
    fn a_channel_that_does_not_exist_gives_silence() {
        let mut s = Stft::new(2, StftConfig::hann(16)).unwrap();
        assert_eq!(s.tick(2, 1.0, &mut Identity), 0.0);
        let mut out = [1.0_f32; 40];
        s.process(2, &[1.0; 40], &mut out, &mut Identity);
        assert_eq!(out, [0.0; 40]);
        s.process_in_place(usize::MAX, &mut out, &mut Identity);
    }
}
