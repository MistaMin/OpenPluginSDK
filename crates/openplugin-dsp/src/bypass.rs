//! Latency-aligned dry path for soft bypass and wet/dry mix.
//!
//! The dry signal is delayed by the plugin's current latency, then crossfaded
//! with the processed buffer. `mix` does not allocate. A hard bypass that drops
//! reported latency is the host's choice. This block keeps the delay.

/// Per-channel dry delay rings and a linear fade, shared by [`SoftBypass`]
/// and [`crate::Delta`]. Allocates only in [`Self::new`].
pub(crate) struct DelayFade {
    rings: Vec<Vec<f32>>,
    write: usize,
    latency: usize,
    pub(crate) value: f32,
    pub(crate) target: f32,
    step: f32,
}

impl DelayFade {
    /// Up to 64 channels, delays up to `max_latency` samples, a fade of
    /// `fade_seconds`, starting (and resting) at `start`.
    pub(crate) fn new(channels: usize, max_latency: usize, sample_rate: f32, fade_seconds: f32, start: f32) -> Self {
        let channels = channels.clamp(1, 64);
        let cap = max_latency.saturating_add(1).max(1);
        let rate = crate::rate::effective_rate(sample_rate);
        Self {
            rings: vec![vec![0.0; cap]; channels],
            write: 0,
            latency: 0,
            value: start,
            target: start,
            step: 1.0 / (rate * fade_seconds).max(1.0),
        }
    }

    pub(crate) fn set_latency(&mut self, latency: usize) {
        let cap = self.rings.first().map(|ring| ring.len()).unwrap_or(1);
        self.latency = latency.min(cap.saturating_sub(1));
    }

    pub(crate) fn latency(&self) -> usize {
        self.latency
    }

    pub(crate) fn channels(&self) -> usize {
        self.rings.len()
    }

    pub(crate) fn reset(&mut self) {
        for ring in &mut self.rings {
            ring.fill(0.0);
        }
        self.write = 0;
    }

    /// Store this frame's dry `input` for `channel` (non-finite as 0) and
    /// return the dry sample from `latency` frames ago.
    #[inline]
    pub(crate) fn push(&mut self, channel: usize, input: f32) -> f32 {
        let ring = &mut self.rings[channel];
        let length = ring.len();
        ring[self.write] = if input.is_finite() { input } else { 0.0 };
        ring[(self.write + length - self.latency) % length]
    }

    /// End of a frame: move the write position and the fade one step on.
    #[inline]
    pub(crate) fn advance(&mut self) {
        self.write += 1;
        if self.write == self.rings[0].len() {
            self.write = 0;
        }
        if self.value < self.target {
            self.value = (self.value + self.step).min(self.target);
        } else if self.value > self.target {
            self.value = (self.value - self.step).max(self.target);
        }
    }
}

/// Dry delay and a 10 ms crossfade. `wet == 1` is the processed signal.
///
/// [`prepare`](Self::prepare) allocates; nothing after it does. Non-finite dry
/// samples count as `0.0`.
///
/// # Example
///
/// ```
/// use openplugin_dsp::SoftBypass;
///
/// let mut bypass = SoftBypass::prepare(1, 64, 48_000.0);
/// bypass.set_latency(0);
/// bypass.set_bypassed(true);
/// bypass.snap(); // no fade: fully bypassed now
///
/// let dry = [0.5f32; 8];
/// let mut wet = [0.0f32; 8]; // what the plugin's processing produced
/// bypass.mix(&[&dry[..]], &mut [&mut wet[..]]);
/// assert_eq!(wet, dry);
/// ```
pub struct SoftBypass {
    line: DelayFade,
}

impl SoftBypass {
    /// `max_latency` is the longest delay `set_latency` may ask for, in samples.
    pub fn prepare(channels: usize, max_latency: usize, sample_rate: f32) -> Self {
        Self { line: DelayFade::new(channels, max_latency, sample_rate, 0.01, 1.0) }
    }

    /// Delay of the dry path in samples: the latency the plugin reports. Clamped
    /// to the `max_latency` given to [`prepare`](Self::prepare). Does not allocate.
    pub fn set_latency(&mut self, latency: usize) {
        self.line.set_latency(latency);
    }

    /// The dry-path delay in samples.
    pub fn latency(&self) -> usize {
        self.line.latency()
    }

    /// `true` fades toward the delayed dry signal.
    pub fn set_bypassed(&mut self, bypassed: bool) {
        self.line.target = if bypassed { 0.0 } else { 1.0 };
    }

    /// Jump to the fade target. Tests and a transport stop use this.
    pub fn snap(&mut self) {
        self.line.value = self.line.target;
    }

    /// Current crossfade position: 1.0 is fully processed, 0.0 fully bypassed.
    pub fn wet(&self) -> f32 {
        self.line.value
    }

    /// Clears the dry delay. The crossfade position is kept.
    pub fn reset(&mut self) {
        self.line.reset();
    }

    /// Replace `wet` with the crossfade. `dry` is the unprocessed input for this block.
    pub fn mix(&mut self, dry: &[&[f32]], wet: &mut [&mut [f32]]) {
        let frames = wet.first().map(|channel| channel.len()).unwrap_or(0);
        for frame in 0..frames {
            let wet_gain = self.line.value;
            let dry_gain = 1.0 - wet_gain;
            for index in 0..self.line.channels() {
                let input = dry.get(index).and_then(|channel| channel.get(frame)).copied().unwrap_or(0.0);
                let delayed = self.line.push(index, input);
                if let Some(sample) = wet.get_mut(index).and_then(|channel| channel.get_mut(frame)) {
                    let processed = if sample.is_finite() { *sample } else { 0.0 };
                    *sample = delayed * dry_gain + processed * wet_gain;
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
