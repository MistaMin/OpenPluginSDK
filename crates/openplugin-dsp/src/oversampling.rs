//! 2x oversampling through a designed minimum-phase FIR, in polyphase form.
//!
//! [`Oversampler2x`] is [`MinPhaseHalfband`] designed for a passband to 0.45 of
//! the base rate and 80 dB of rejection: the image of a tone and content that
//! would fold about the base Nyquist are 80 dB down, and the passband is flat
//! (below 1e-6 dB of ripple). It was two RBJ low-passes before, which lost
//! 8.8 dB at 20 kHz in a 48 kHz round trip and left a 20 kHz image only 16 dB
//! down. Delay is low and frequency dependent, as with any minimum-phase filter;
//! use [`crate::LinearPhaseOversampler`] where the phase must stay linear.
//!
//! `new` and `set_sample_rate` allocate (they design the filter). The two
//! process calls do not, and process the frames both buffers can hold.

use crate::halfband_min::MinPhaseHalfband;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OversampleFactor {
    None = 1,
    X2 = 2,
    X4 = 4,
}

/// 2x up and down sampling with one minimum-phase anti-imaging / anti-alias filter.
#[derive(Debug, Clone)]
pub struct Oversampler2x {
    filter: MinPhaseHalfband,
}

impl Default for Oversampler2x {
    fn default() -> Self {
        Self::new(44100.0)
    }
}

impl Oversampler2x {
    pub fn new(base_sample_rate: f32) -> Self {
        Self { filter: design(base_sample_rate) }
    }

    /// Redesigns the filter for a new base rate. Allocates; call it from `prepare`.
    pub fn set_sample_rate(&mut self, base_sr: f32) {
        self.filter = design(base_sr);
    }

    pub fn reset(&mut self) {
        self.filter.reset();
    }

    /// Upsamples by 2x via zero-stuffing and low-pass interpolation. Processes
    /// `min(input.len(), output.len() / 2)` frames.
    pub fn upsample_2x(&mut self, input: &[f32], output: &mut [f32]) {
        let frames = input.len().min(output.len() / 2);
        self.filter.upsample_2x(&input[..frames], &mut output[..2 * frames]);
    }

    /// Filters and decimates a 2x buffer back to the base rate. Processes
    /// `min(output.len(), oversampled.len() / 2)` frames.
    pub fn downsample_2x(&mut self, oversampled: &[f32], output: &mut [f32]) {
        let frames = output.len().min(oversampled.len() / 2);
        self.filter.downsample_2x(&oversampled[..2 * frames], &mut output[..frames]);
    }
}

fn design(base_sample_rate: f32) -> MinPhaseHalfband {
    let rate = f64::from(crate::rate::effective_rate(base_sample_rate));
    MinPhaseHalfband::design(rate, 0.45 * rate, 80.0).expect("0.45 fs and 80 dB is a valid half-band spec")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, rate: f32, frames: usize) -> Vec<f32> {
        let step = std::f64::consts::TAU * f64::from(freq) / f64::from(rate);
        (0..frames).map(|n| (step * n as f64).sin() as f32).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn round_trip_keeps_the_highs_and_rejects_the_image() {
        let rate = 48_000.0;
        for freq in [1_000.0, 10_000.0, 15_000.0, 20_000.0] {
            let mut os = Oversampler2x::new(rate);
            let input = tone(freq, rate, 8192);
            let mut up = vec![0.0; 2 * input.len()];
            let mut back = vec![0.0; input.len()];
            os.upsample_2x(&input, &mut up);
            os.downsample_2x(&up, &mut back);
            let loss = 20.0 * (rms(&back[1024..]) / rms(&input[1024..])).log10();
            assert!(loss.abs() < 0.01, "{freq} Hz round trip: {loss} dB");
        }
        // A 2x-rate tone above the base Nyquist must not fold back.
        let mut os = Oversampler2x::new(rate);
        let high = tone(30_000.0, 2.0 * rate, 16384);
        let mut out = vec![0.0; 8192];
        os.downsample_2x(&high, &mut out);
        let fold = 20.0 * (rms(&out[1024..]) / rms(&high)).log10();
        assert!(fold < -78.0, "30 kHz folds back at {fold} dB");
    }

    #[test]
    fn short_buffers_process_the_shorter_length() {
        let mut os = Oversampler2x::new(44_100.0);
        let input = [0.5f32; 8];
        let mut up = [0.0f32; 9];
        os.upsample_2x(&input, &mut up);
        assert_eq!(up[8], 0.0);
        let mut out = [0.0f32; 8];
        os.downsample_2x(&up, &mut out);
        assert!(out.iter().all(|s| s.is_finite()));
    }
}
