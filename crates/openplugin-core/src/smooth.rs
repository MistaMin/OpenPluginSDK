//! One-pole parameter smoother.
//!
//! A 10 ms one-pole on a gain keeps a host parameter event from stepping the
//! audio. The coefficient is computed in `set_time`,
//! which is not realtime. [`OnePole::next_value`] only multiplies and adds.

/// A one-pole low-pass on a control value: each sample moves a fixed share
/// of the way to the target. Denormal-safe in practice: it settles on the
/// target rather than decaying towards zero.
///
/// ```
/// use openplugin_core::OnePole;
/// let mut gain = OnePole::new();
/// gain.set_time(48_000.0, 10.0);
/// gain.reset(0.0);
/// let after_10_ms = (0..480).map(|_| gain.next_value(1.0)).last().unwrap();
/// assert!((after_10_ms - 0.632).abs() < 0.01);
/// ```
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
    /// At 0, with no smoothing (the next value is the target) until
    /// [`Self::set_time`].
    pub fn new() -> Self {
        Self { y: 0.0, coeff: 1.0 }
    }

    /// `time_ms` is the 63% time constant at `sample_rate` hertz. Zero,
    /// negative or not finite snaps on the next sample; a sample rate that is
    /// not finite counts as 48 kHz, and one below 1 Hz as 1 Hz. Calls `exp`:
    /// call it from `prepare`, not per sample.
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

    /// Jump to `value` with no glide (0 if it is not finite).
    pub fn reset(&mut self, value: f32) {
        self.y = if value.is_finite() { value } else { 0.0 };
    }

    /// The value last returned.
    pub fn current(&self) -> f32 {
        self.y
    }

    /// One sample towards `target`, and the new value. A target that is not
    /// finite holds the current value. Audio thread; no allocation.
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
