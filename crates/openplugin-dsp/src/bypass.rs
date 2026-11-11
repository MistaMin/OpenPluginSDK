//! Latency-aligned dry path for soft bypass and wet/dry mix.
//!
//! The dry signal is delayed by the plugin's current latency, then crossfaded
//! with the processed buffer. `mix` does not allocate. A hard bypass that drops
//! reported latency is the host's choice. This block keeps the delay.

/// Dry delay and a 10 ms crossfade. `wet == 1` is the processed signal.
pub struct SoftBypass {
    rings: Vec<Vec<f32>>,
    write: usize,
    latency: usize,
    wet: f32,
    target: f32,
    step: f32,
}

impl SoftBypass {
    /// `max_latency` is the longest delay `set_latency` may ask for, in samples.
    pub fn prepare(channels: usize, max_latency: usize, sample_rate: f32) -> Self {
        let channels = channels.clamp(1, 64);
        let cap = max_latency.saturating_add(1).max(1);
        let rate = if sample_rate.is_finite() && sample_rate >= 8_000.0 {
            sample_rate
        } else {
            48_000.0
        };
        Self {
            rings: vec![vec![0.0; cap]; channels],
            write: 0,
            latency: 0,
            wet: 1.0,
            target: 1.0,
            step: 1.0 / (rate * 0.01).max(1.0),
        }
    }

    pub fn set_latency(&mut self, latency: usize) {
        let cap = self.rings.first().map(|ring| ring.len()).unwrap_or(1);
        self.latency = latency.min(cap.saturating_sub(1));
    }

    pub fn latency(&self) -> usize {
        self.latency
    }

    /// `true` fades toward the delayed dry signal.
    pub fn set_bypassed(&mut self, bypassed: bool) {
        self.target = if bypassed { 0.0 } else { 1.0 };
    }

    /// Jump to the fade target. Tests and a transport stop use this.
    pub fn snap(&mut self) {
        self.wet = self.target;
    }

    pub fn wet(&self) -> f32 {
        self.wet
    }

    pub fn reset(&mut self) {
        for ring in &mut self.rings {
            ring.fill(0.0);
        }
        self.write = 0;
    }

    /// Replace `wet` with the crossfade. `dry` is the unprocessed input for this block.
    pub fn mix(&mut self, dry: &[&[f32]], wet: &mut [&mut [f32]]) {
        let Some(first) = self.rings.first() else {
            return;
        };
        let length = first.len();
        if length == 0 {
            return;
        }
        let frames = wet.first().map(|channel| channel.len()).unwrap_or(0);
        let latency = self.latency.min(length - 1);
        for frame in 0..frames {
            let dry_gain = 1.0 - self.wet;
            let wet_gain = self.wet;
            for (index, ring) in self.rings.iter_mut().enumerate() {
                let input = dry
                    .get(index)
                    .and_then(|channel| channel.get(frame))
                    .copied()
                    .unwrap_or(0.0);
                let input = if input.is_finite() { input } else { 0.0 };
                ring[self.write] = input;
                let read = (self.write + length - latency) % length;
                let delayed = ring[read];
                if let Some(sample) = wet.get_mut(index).and_then(|channel| channel.get_mut(frame))
                {
                    let processed = if sample.is_finite() { *sample } else { 0.0 };
                    *sample = delayed * dry_gain + processed * wet_gain;
                }
            }
            self.write += 1;
            if self.write == length {
                self.write = 0;
            }
            if self.wet < self.target {
                self.wet = (self.wet + self.step).min(self.target);
            } else if self.wet > self.target {
                self.wet = (self.wet - self.step).max(self.target);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypassed_dry_is_delayed_by_the_reported_latency() {
        let mut bypass = SoftBypass::prepare(1, 8, 48_000.0);
        bypass.set_latency(4);
        bypass.set_bypassed(true);
        bypass.snap();
        let dry = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut wet = [0.0; 8];
        bypass.mix(&[&dry], &mut [&mut wet]);
        assert!(wet[..4].iter().all(|sample| sample.abs() < 1e-6));
        assert!((wet[4] - 1.0).abs() < 1e-5);
        assert!(wet[5..].iter().all(|sample| sample.abs() < 1e-6));
    }

    #[test]
    fn engaged_path_keeps_the_processed_signal() {
        let mut bypass = SoftBypass::prepare(1, 4, 48_000.0);
        bypass.set_latency(4);
        let dry = [1.0; 4];
        let mut wet = [0.25; 4];
        bypass.mix(&[&dry], &mut [&mut wet]);
        assert!(wet.iter().all(|sample| (*sample - 0.25).abs() < 1e-5));
    }
}
