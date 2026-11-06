//! Feed-forward log-domain dynamics processor.

#[derive(Debug, Clone)]
pub struct Compressor {
    sample_rate: f32,
    envelope_db: f32,
    gain_reduction_db: f32,
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
                        max_abs = max_abs.max(s.abs());
                    }
                }
            }

            let input_db = 20.0 * max_abs.log10();
            let coef = if input_db > self.envelope_db { att_coef } else { rel_coef };
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
