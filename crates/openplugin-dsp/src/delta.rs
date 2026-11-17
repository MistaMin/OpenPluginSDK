//! Delta listening: play what the processing adds or takes away.
//!
//! The output becomes `processed - dry`, with the dry signal delayed by the
//! plugin's reported latency so the two line up, and switching glides over a
//! short time so it does not click. With nothing processed (a flat EQ, a
//! compressor that is not compressing) Delta is silent.
//!
//! Which dry signal to subtract is the plugin's choice: usually the input as
//! it arrives. With an automatic make-up gain, the plugin may want the
//! difference before that gain, so Delta shows the processing and not the
//! level change; call [`Delta::process`] at that point in the chain.

use crate::bypass::DelayFade;

/// Latency-aligned `processed - dry`, gliding in and out. No allocation after
/// [`Delta::prepare`].
pub struct Delta {
    line: DelayFade,
}

impl Delta {
    /// `max_latency` is the longest delay [`Self::set_latency`] may ask for, in
    /// samples; `glide_ms` is the switching time (20 ms is a good default).
    pub fn prepare(channels: usize, max_latency: usize, sample_rate: f32, glide_ms: f32) -> Self {
        let glide = if glide_ms.is_finite() { glide_ms.max(0.0) } else { 20.0 };
        Self { line: DelayFade::new(channels, max_latency, sample_rate, glide * 0.001, 0.0) }
    }

    pub fn set_latency(&mut self, latency: usize) {
        self.line.set_latency(latency);
    }

    pub fn latency(&self) -> usize {
        self.line.latency()
    }

    /// Glide toward the difference (`true`) or the processed signal.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.line.target = if enabled { 1.0 } else { 0.0 };
    }

    pub fn is_enabled(&self) -> bool {
        self.line.target > 0.5
    }

    /// Jump to the target at once.
    pub fn snap(&mut self) {
        self.line.value = self.line.target;
    }

    /// How much of the dry signal is being subtracted, 0 to 1.
    pub fn amount(&self) -> f32 {
        self.line.value
    }

    pub fn reset(&mut self) {
        self.line.reset();
    }

    /// Replace `processed` with `processed - amount * delayed dry`. Every
    /// block must pass `dry`, even while disabled, so the delay stays full.
    pub fn process(&mut self, dry: &[&[f32]], processed: &mut [&mut [f32]]) {
        let frames = processed.first().map(|channel| channel.len()).unwrap_or(0);
        let idle = self.line.value == 0.0 && self.line.target == 0.0;
        for frame in 0..frames {
            let amount = self.line.value;
            for index in 0..self.line.channels() {
                let input = dry.get(index).and_then(|c| c.get(frame)).copied().unwrap_or(0.0);
                let delayed = self.line.push(index, input);
                if idle {
                    continue;
                }
                if let Some(sample) = processed.get_mut(index).and_then(|c| c.get_mut(frame)) {
                    let wet = if sample.is_finite() { *sample } else { 0.0 };
                    *sample = wet - amount * delayed;
                }
            }
            self.line.advance();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_signal_is_silent_and_a_gain_change_is_heard() {
        let mut delta = Delta::prepare(2, 16, 48_000.0, 20.0);
        delta.set_enabled(true);
        delta.snap();
        let dry: Vec<f32> = (0..256).map(|n| (n as f32 * 0.1).sin()).collect();
        let mut l = dry.clone();
        let mut r: Vec<f32> = dry.iter().map(|x| x * 1.5).collect();
        delta.process(&[&dry, &dry], &mut [&mut l, &mut r]);
        assert!(l.iter().all(|v| v.abs() < 1e-6), "identity cancels");
        for (out, x) in r.iter().zip(&dry) {
            assert!((out - 0.5 * x).abs() < 1e-6);
        }
    }

    #[test]
    fn dry_is_aligned_to_the_latency() {
        let mut delta = Delta::prepare(1, 8, 48_000.0, 0.0);
        delta.set_latency(3);
        delta.set_enabled(true);
        delta.snap();
        let dry = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        // A plugin that only delays by three samples.
        let mut out = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        delta.process(&[&dry], &mut [&mut out]);
        assert!(out.iter().all(|v| v.abs() < 1e-6), "{out:?}");
    }

    #[test]
    fn switching_glides_and_disabled_passes_through() {
        let mut delta = Delta::prepare(1, 0, 48_000.0, 20.0);
        let dry = vec![1.0f32; 2_000];
        let mut out = vec![0.0f32; 2_000];
        delta.process(&[&dry], &mut [&mut out]);
        assert!(out.iter().all(|v| *v == 0.0), "off: untouched");
        delta.set_enabled(true);
        delta.process(&[&dry], &mut [&mut out]);
        assert!(out[0] > -0.01, "starts near the processed signal");
        assert!((out[1_999] + 1.0).abs() < 1e-6, "ends at the full difference");
        assert!(out.windows(2).all(|w| w[1] <= w[0] + 1e-7), "monotonic glide");
        assert_eq!(delta.amount(), 1.0);
    }
}
