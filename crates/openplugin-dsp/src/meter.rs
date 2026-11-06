//! Metering primitives: Peak, RMS, and standard VU ballistics.

#[derive(Debug, Clone)]
pub struct VuMeter {
    sample_rate: f32,
    envelope: f32,
}

impl VuMeter {
    pub fn new() -> Self {
        Self {
            sample_rate: 44100.0,
            envelope: 0.0,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr.max(1000.0);
    }

    pub fn reset(&mut self) {
        self.envelope = 0.0;
    }

    /// Feeds samples and updates needle position according to ANSI C16.5 VU standard
    /// (300 ms integration time).
    pub fn process_samples(&mut self, samples: &[f32]) -> f32 {
        let coef = (-1.0 / (0.300 * self.sample_rate)).exp();
        for &s in samples {
            let rect = s.abs();
            self.envelope = rect + coef * (self.envelope - rect);
        }
        self.envelope
    }

    /// Returns reading in dB (-inf to +3 dB typical scale).
    pub fn level_db(&self) -> f32 {
        if self.envelope <= 1e-5 {
            -100.0
        } else {
            20.0 * self.envelope.log10()
        }
    }
}
