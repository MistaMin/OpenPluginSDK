//! Slow stereo RMS loudness match.
//!
//! Water Channel's gain compensation: a two-second power estimate, a silence
//! gate, and a correction clamped to ±12 dB. It is not LUFS and it is not a
//! limiter. While disabled, the correction releases back to unity.

#[derive(Clone, Debug)]
pub struct GainComp {
    input_power: f64,
    output_power: f64,
    correction_db: f64,
    power_alpha: f64,
    match_alpha: f64,
    off_alpha: f64,
}

impl GainComp {
    pub fn new(sample_rate: f32) -> Self {
        let rate = if sample_rate.is_finite() {
            f64::from(sample_rate.max(8_000.0))
        } else {
            48_000.0
        };
        Self {
            input_power: 0.0,
            output_power: 0.0,
            correction_db: 0.0,
            power_alpha: 1.0 - (-1.0 / (rate * 2.0)).exp(),
            match_alpha: 1.0 - (-1.0 / (rate * 2.0)).exp(),
            off_alpha: 1.0 - (-1.0 / (rate * 0.05)).exp(),
        }
    }

    pub fn correction_db(&self) -> f32 {
        self.correction_db as f32
    }

    pub fn reset(&mut self) {
        self.input_power = 0.0;
        self.output_power = 0.0;
        self.correction_db = 0.0;
    }

    /// Linear gain that brings `output` toward the loudness of `input`.
    pub fn process(&mut self, input: [f32; 2], output: [f32; 2], enabled: bool) -> f32 {
        let power = |frame: [f32; 2]| {
            let left = f64::from(frame[0]);
            let right = f64::from(frame[1]);
            if left.is_finite() && right.is_finite() {
                (left * left + right * right) * 0.5
            } else {
                0.0
            }
        };
        self.input_power += (power(input) - self.input_power) * self.power_alpha;
        self.output_power += (power(output) - self.output_power) * self.power_alpha;
        let target = if !enabled {
            0.0
        } else if self.input_power > 1e-6 && self.output_power > 1e-12 {
            (10.0 * (self.input_power / self.output_power).log10()).clamp(-12.0, 12.0)
        } else {
            self.correction_db
        };
        let alpha = if enabled {
            self.match_alpha
        } else {
            self.off_alpha
        };
        self.correction_db += (target - self.correction_db) * alpha;
        if self.correction_db.abs() < 1e-12 {
            self.correction_db = 0.0;
        }
        10f64.powf(self.correction_db / 20.0) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_a_steady_drop_and_ignores_a_short_dip() {
        let mut comp = GainComp::new(48_000.0);
        let mut gain = 1.0;
        for _ in 0..48_000 * 20 {
            gain = comp.process([0.25; 2], [0.125; 2], true);
        }
        assert!((gain - 2.0).abs() < 0.001);
        let before = comp.correction_db();
        for _ in 0..240 {
            comp.process([0.25; 2], [0.025; 2], true);
        }
        assert!((comp.correction_db() - before).abs() < 0.03);
    }

    #[test]
    fn silence_holds_and_bypass_returns_to_unity() {
        let mut comp = GainComp::new(48_000.0);
        for _ in 0..48_000 {
            assert_eq!(comp.process([0.0; 2], [0.0; 2], true), 1.0);
        }
        for _ in 0..48_000 * 10 {
            comp.process([0.5; 2], [0.001; 2], true);
        }
        assert!(comp.correction_db() <= 12.0);
        let held = comp.correction_db();
        assert!(held > 1.0);
        for _ in 0..48_000 {
            comp.process([0.0; 2], [0.0; 2], false);
        }
        assert!(comp.correction_db() < 0.001);
    }
}
