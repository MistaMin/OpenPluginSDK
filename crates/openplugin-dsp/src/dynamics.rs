//! Feed-forward log-domain dynamics processor.
#![allow(deprecated)] // this module defines the deprecated items

#[derive(Debug, Clone)]
#[deprecated(since = "1.0.0", note = "use `Dynamics`, the compressor, expander and gate with a soft knee and a sidechain")]
pub struct Compressor {
    sample_rate: f32,
    envelope_db: f32,
    gain_reduction_db: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor {
    pub fn new() -> Self {
        Self {
            sample_rate: 44100.0,
            envelope_db: -120.0,
            gain_reduction_db: 0.0,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr.max(1000.0);
    }

    pub fn reset(&mut self) {
        self.envelope_db = -120.0;
        self.gain_reduction_db = 0.0;
    }

    pub fn current_gain_reduction_db(&self) -> f32 {
        self.gain_reduction_db
    }

    /// Process a block with threshold (dB), ratio, attack (ms), release (ms), knee (dB), makeup (dB).
    #[allow(clippy::too_many_arguments)] // public API; grouping would break callers
    pub fn process_block(
        &mut self,
        io: &mut [Vec<f32>],
        frames: usize,
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
        knee_db: f32,
        makeup_db: f32,
    ) {
        let att_coef = (-1.0 / (attack_ms.max(0.1) * 0.001 * self.sample_rate)).exp();
        let rel_coef = (-1.0 / (release_ms.max(1.0) * 0.001 * self.sample_rate)).exp();
        let makeup_linear = 10.0_f32.powf(makeup_db / 20.0);
        let num_channels = io.len();

        for i in 0..frames {
            // Detector: peak across channels
            let mut max_abs = 1e-6_f32;
            for ch in 0..num_channels {
                if let Some(c) = io.get(ch) {
                    if let Some(&s) = c.get(i) {
                        // An infinite sample would leave the envelope NaN for good.
                        if s.is_finite() {
                            max_abs = max_abs.max(s.abs());
                        }
                    }
                }
            }

            let input_db = 20.0 * max_abs.log10();
            let coef = if input_db > self.envelope_db {
                att_coef
            } else {
                rel_coef
            };
            self.envelope_db = input_db + coef * (self.envelope_db - input_db);

            // Compute target gain reduction with soft knee
            let over_db = self.envelope_db - threshold_db;
            let gr_db = if knee_db > 0.0 && over_db > -knee_db / 2.0 && over_db < knee_db / 2.0 {
                let x = over_db + knee_db / 2.0;
                let excess = (x * x) / (2.0 * knee_db);
                excess * (1.0 - 1.0 / ratio.max(1.0))
            } else if over_db > 0.0 {
                over_db * (1.0 - 1.0 / ratio.max(1.0))
            } else {
                0.0
            };

            self.gain_reduction_db = gr_db.max(0.0);
            let gain_linear = 10.0_f32.powf(-self.gain_reduction_db / 20.0) * makeup_linear;

            for ch in 0..num_channels {
                if let Some(c) = io.get_mut(ch) {
                    if let Some(s) = c.get_mut(i) {
                        *s *= gain_linear;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_infinite_sample_does_not_stop_the_compressor() {
        let mut c = Compressor::new();
        c.set_sample_rate(48_000.0);
        let run = |c: &mut Compressor, first: f32| {
            let mut io = vec![vec![1.0_f32; 48_000]];
            io[0][0] = first;
            c.process_block(&mut io, 48_000, -30.0, 4.0, 5.0, 50.0, 0.0, 0.0);
            c.current_gain_reduction_db()
        };
        let before = run(&mut c, 1.0);
        assert!(before > 10.0, "{before}");
        let after = run(&mut c, f32::INFINITY);
        assert!((after - before).abs() < 0.01, "{after} after +inf, {before} before");
    }
}
