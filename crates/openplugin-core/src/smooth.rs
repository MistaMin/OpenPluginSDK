//! One-pole parameter smoother.
//!
//! A 10 ms one-pole on a gain keeps a host parameter event from stepping the
//! audio. The coefficient is computed in `set_time`,
//! which is not realtime. [`OnePole::next_value`] only multiplies and adds.

#[derive(Clone, Copy, Debug)]
pub struct OnePole {
    y: f32,
    coeff: f32,
}

impl Default for OnePole {
    fn default() -> Self {
        Self::new()
    }
}

impl OnePole {
    pub fn new() -> Self {
        Self { y: 0.0, coeff: 1.0 }
    }

    /// `time_ms` is the 63% time constant. Zero snaps on the next sample.
    pub fn set_time(&mut self, sample_rate: f32, time_ms: f32) {
        let rate = if sample_rate.is_finite() {
            sample_rate.max(1.0)
        } else {
            48_000.0
        };
        if !time_ms.is_finite() || time_ms <= 0.0 {
            self.coeff = 1.0;
            return;
        }
        self.coeff = 1.0 - (-1.0 / (time_ms * 0.001 * rate)).exp();
    }

    pub fn reset(&mut self, value: f32) {
        self.y = if value.is_finite() { value } else { 0.0 };
    }

    pub fn current(&self) -> f32 {
        self.y
    }

    #[inline]
    pub fn next_value(&mut self, target: f32) -> f32 {
        let target = if target.is_finite() { target } else { self.y };
        self.y += self.coeff * (target - self.y);
        self.y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaches_the_target_and_ignores_non_finite() {
        let mut pole = OnePole::new();
        pole.set_time(48_000.0, 10.0);
        pole.reset(0.0);
        let mut last = 0.0;
        for _ in 0..48_000 {
            last = pole.next_value(1.0);
        }
        assert!((last - 1.0).abs() < 1e-3);
        let held = pole.next_value(f32::NAN);
        assert!((held - last).abs() < 1e-4);
    }

    #[test]
    fn zero_time_snaps() {
        let mut pole = OnePole::new();
        pole.set_time(48_000.0, 0.0);
        assert_eq!(pole.next_value(0.25), 0.25);
    }
}
