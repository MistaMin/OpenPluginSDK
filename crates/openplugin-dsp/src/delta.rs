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

/// Latency-aligned `processed - dry`, gliding in and out. No allocation after
/// [`Delta::prepare`].
pub struct Delta {
    rings: Vec<Vec<f32>>,
    write: usize,
    latency: usize,
    amount: f32,
    target: f32,
    step: f32,
}

impl Delta {
    /// `max_latency` is the longest delay [`Self::set_latency`] may ask for, in
    /// samples; `glide_ms` is the switching time (20 ms is a good default).
    pub fn prepare(channels: usize, max_latency: usize, sample_rate: f32, glide_ms: f32) -> Self {
        let channels = channels.clamp(1, 64);
        let cap = max_latency.saturating_add(1).max(1);
        let rate = if sample_rate.is_finite() && sample_rate >= 8_000.0 { sample_rate } else { 48_000.0 };
        let glide = if glide_ms.is_finite() { glide_ms.max(0.0) } else { 20.0 };
        Self {
            rings: vec![vec![0.0; cap]; channels],
            write: 0,
            latency: 0,
            amount: 0.0,
            target: 0.0,
            step: 1.0 / (rate * glide * 0.001).max(1.0),
        }
    }

    pub fn set_latency(&mut self, latency: usize) {
        let cap = self.rings.first().map(|ring| ring.len()).unwrap_or(1);
        self.latency = latency.min(cap.saturating_sub(1));
    }

    pub fn latency(&self) -> usize {
        self.latency
    }

    /// Glide toward the difference (`true`) or the processed signal.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.target = if enabled { 1.0 } else { 0.0 };
    }

    pub fn is_enabled(&self) -> bool {
        self.target > 0.5
    }

    /// Jump to the target at once.
    pub fn snap(&mut self) {
        self.amount = self.target;
    }

    /// How much of the dry signal is being subtracted, 0 to 1.
    pub fn amount(&self) -> f32 {
        self.amount
    }

    pub fn reset(&mut self) {
        for ring in &mut self.rings {
            ring.fill(0.0);
        }
        self.write = 0;
    }

    /// Replace `processed` with `processed - amount * delayed dry`. Every
    /// block must pass `dry`, even while disabled, so the delay stays full.
    pub fn process(&mut self, dry: &[&[f32]], processed: &mut [&mut [f32]]) {
        let Some(length) = self.rings.first().map(|ring| ring.len()) else { return };
        let frames = processed.first().map(|channel| channel.len()).unwrap_or(0);
        let latency = self.latency.min(length - 1);
        let idle = self.amount == 0.0 && self.target == 0.0;
        for frame in 0..frames {
            let read = (self.write + length - latency) % length;
            for (index, ring) in self.rings.iter_mut().enumerate() {
                let input = dry.get(index).and_then(|c| c.get(frame)).copied().unwrap_or(0.0);
                ring[self.write] = if input.is_finite() { input } else { 0.0 };
                if idle {
                    continue;
                }
                let delayed = ring[read];
                if let Some(sample) = processed.get_mut(index).and_then(|c| c.get_mut(frame)) {
                    let wet = if sample.is_finite() { *sample } else { 0.0 };
                    *sample = wet - self.amount * delayed;
                }
            }
            self.write += 1;
            if self.write == length {
                self.write = 0;
            }
            if self.amount < self.target {
                self.amount = (self.amount + self.step).min(self.target);
            } else if self.amount > self.target {
                self.amount = (self.amount - self.step).max(self.target);
            }
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
