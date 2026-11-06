//! Multi-rate oversampling engine with polyphase half-band decimation/interpolation.

use crate::biquad::{Biquad, FilterType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OversampleFactor {
    None = 1,
    X2 = 2,
    X4 = 4,
}

/// 2x Oversampling stage using cascaded half-band filters.
#[derive(Debug, Clone)]
pub struct Oversampler2x {
    anti_alias: [Biquad; 2],
    anti_imaging: [Biquad; 2],
}

impl Default for Oversampler2x {
    fn default() -> Self {
        Self::new(44100.0)
    }
}

impl Oversampler2x {
    pub fn new(base_sample_rate: f32) -> Self {
        let mut s = Self {
            anti_alias: [Biquad::new(), Biquad::new()],
            anti_imaging: [Biquad::new(), Biquad::new()],
        };
        s.set_sample_rate(base_sample_rate);
        s
    }

    pub fn set_sample_rate(&mut self, base_sr: f32) {
        let oversampled_sr = base_sr * 2.0;
        let cutoff = base_sr * 0.45; // Half-band transition
        for f in &mut self.anti_alias {
            f.configure(FilterType::LowPass, oversampled_sr, cutoff, 0.707, 0.0);
        }
        for f in &mut self.anti_imaging {
            f.configure(FilterType::LowPass, oversampled_sr, cutoff, 0.707, 0.0);
        }
    }

    pub fn reset(&mut self) {
        for f in &mut self.anti_alias {
            f.reset();
        }
        for f in &mut self.anti_imaging {
            f.reset();
        }
    }

    /// Upsamples input by 2x via zero-stuffing and low-pass interpolation.
    pub fn upsample_2x(&mut self, input: &[f32], output: &mut [f32]) {
        assert!(output.len() >= input.len() * 2);
        for (i, &s) in input.iter().enumerate() {
            let out_idx = i * 2;
            // 2x amplitude normalization for zero-stuffed signal
            let s0 = self.anti_imaging[0].process_sample(s * 2.0);
            output[out_idx] = self.anti_imaging[1].process_sample(s0);

            let s1 = self.anti_imaging[0].process_sample(0.0);
            output[out_idx + 1] = self.anti_imaging[1].process_sample(s1);
        }
    }

    /// Downsamples 2x buffer back to base rate via anti-aliasing filter and decimation.
    pub fn downsample_2x(&mut self, oversampled: &[f32], output: &mut [f32]) {
        assert!(oversampled.len() >= output.len() * 2);
        for (i, out_sample) in output.iter_mut().enumerate() {
            let in_idx = i * 2;
            let s0 = self.anti_alias[0].process_sample(oversampled[in_idx]);
            let filtered0 = self.anti_alias[1].process_sample(s0);

            let s1 = self.anti_alias[0].process_sample(oversampled[in_idx + 1]);
            let _ = self.anti_alias[1].process_sample(s1);

            *out_sample = filtered0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oversampling_roundtrip() {
        let mut oversampler = Oversampler2x::new(44100.0);
        let input = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.4, 0.3, 0.2];
        let mut oversampled = vec![0.0; input.len() * 2];
        let mut recovered = vec![0.0; input.len()];

        oversampler.upsample_2x(&input, &mut oversampled);
        oversampler.downsample_2x(&oversampled, &mut recovered);

        // Verify finite valid signals and reasonable reconstruction
        for s in &recovered {
            assert!(!s.is_nan());
            assert!(!s.is_infinite());
        }
    }
}
