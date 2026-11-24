//! Multi-voice chorus.
//!
//! A chorus mixes the input with copies of itself delayed by a few tens of
//! milliseconds, where each copy's delay is swept slowly by its own LFO. The
//! copies detune very slightly against the dry signal and each other, which
//! the ear hears as several players. The structure is the one described in
//! J. Dattorro, "Effect Design, Part 2: Delay-Line Modulation and Chorus",
//! J. Audio Eng. Soc. 45(10), 1997, and in J. O. Smith, "Physical Audio Signal
//! Processing", section "Chorus": `y = (1 - mix) x + mix * g * sum_v x(n - d_v(n))`
//! with `d_v(n) = delay + depth * lfo_v(n)`.
//!
//! Voices of one channel are spaced evenly in LFO phase (`v / voices`), and a
//! channel is offset from the first by `spread / 2` of a cycle (times its
//! index over the channel count minus one), which widens a stereo image. The
//! voice sum is scaled by `1 / sqrt(voices)` so that decorrelated voices keep
//! the wet level steady as voices are added.
//!
//! Delay and depth are glided (10 ms) so a parameter jump does not click. The
//! read delay is clamped into the interpolator's valid range, so `delay_ms` less
//! than `depth_ms` is safe. The LFO can sync to the host: see [`Lfo`].
//!
//! Allocation happens in [`Chorus::new`] and [`Chorus::prepare`] only.

use crate::interp::Interpolation;
use crate::lfo::{Lfo, LfoRate, LfoShape};
use crate::moddelay::{Glide, ModDelay};
use crate::rate::effective_rate;
use openplugin_core::Transport;

/// Most voices per channel.
pub const MAX_CHORUS_VOICES: usize = 8;
/// Longest base delay plus depth, in milliseconds.
pub const MAX_CHORUS_MS: f32 = 100.0;

#[derive(Clone, Debug)]
struct ChannelState {
    line: ModDelay,
    lfos: Vec<Lfo>,
    delay: Glide,
    depth: Glide,
}

/// Chorus with up to [`MAX_CHORUS_VOICES`] voices per channel.
#[derive(Clone, Debug)]
///
/// [`new`](Self::new) and [`prepare`](Self::prepare) allocate; the setters and
/// `process*` do not.
///
/// # Example
///
/// ```
/// use openplugin_dsp::Chorus;
///
/// let mut chorus = Chorus::new(2, 48_000.0);
/// chorus.set_voices(3);
/// chorus.set_delay_ms(12.0);
/// chorus.set_depth_ms(3.0);
/// chorus.set_mix(0.5);
///
/// let mut left = [0.1f32; 256];
/// chorus.process(0, &mut left);
/// assert!(left.iter().all(|s| s.is_finite()));
/// ```
pub struct Chorus {
    sample_rate: f32,
    channels: Vec<ChannelState>,
    voices: usize,
    rate: LfoRate,
    shape: LfoShape,
    delay_ms: f32,
    depth_ms: f32,
    mix: f32,
    spread: f32,
}

impl Chorus {
    /// A chorus for `channels` channels at `sample_rate`. Allocates.
    pub fn new(channels: usize, sample_rate: f32) -> Self {
        let mut c = Self {
            sample_rate: effective_rate(sample_rate),
            channels: Vec::new(),
            voices: 3,
            rate: LfoRate::Hz(0.8),
            shape: LfoShape::Sine,
            delay_ms: 18.0,
            depth_ms: 4.0,
            mix: 0.5,
            spread: 1.0,
        };
        c.prepare(channels, sample_rate);
        c
    }

    /// Set the channel count and rate, clear all state. Allocates.
    pub fn prepare(&mut self, channels: usize, sample_rate: f32) {
        self.sample_rate = effective_rate(sample_rate);
        let channels = channels.max(1);
        let fs = self.sample_rate;
        self.channels.clear();
        for _ in 0..channels {
            self.channels.push(ChannelState {
                line: ModDelay::new(fs, MAX_CHORUS_MS + 1.0, Interpolation::Hermite),
                lfos: (0..MAX_CHORUS_VOICES).map(|_| Lfo::new(fs)).collect(),
                delay: Glide::new(fs, 10.0),
                depth: Glide::new(fs, 10.0),
            });
        }
        self.apply_lfo_setup();
        self.reset();
    }

    fn apply_lfo_setup(&mut self) {
        let n = self.channels.len();
        let voices = self.voices;
        for (ci, ch) in self.channels.iter_mut().enumerate() {
            let chan_off = if n > 1 { 0.5 * self.spread * ci as f32 / (n - 1) as f32 } else { 0.0 };
            for (vi, lfo) in ch.lfos.iter_mut().enumerate() {
                lfo.set_shape(self.shape);
                lfo.set_rate(self.rate);
                lfo.set_sample_rate(self.sample_rate);
                lfo.set_phase_offset(vi as f32 / voices as f32 + chan_off);
                lfo.set_seed((ci * MAX_CHORUS_VOICES + vi) as u64 + 1);
            }
        }
    }

    /// Number of channels prepared.
    pub fn channels(&self) -> usize {
        self.channels.len()
    }

    /// Clear delay lines and restart the LFOs.
    pub fn reset(&mut self) {
        for ch in &mut self.channels {
            ch.line.reset();
            for l in &mut ch.lfos {
                l.reset();
            }
            ch.delay = Glide::new(self.sample_rate, 10.0);
            ch.depth = Glide::new(self.sample_rate, 10.0);
        }
    }

    /// Voices per channel, clamped to 1..=8.
    pub fn set_voices(&mut self, voices: usize) {
        self.voices = voices.clamp(1, MAX_CHORUS_VOICES);
        self.apply_lfo_setup();
    }

    /// LFO rate of every voice, in Hz or locked to the tempo (see [`LfoRate`]).
    pub fn set_rate(&mut self, rate: LfoRate) {
        self.rate = rate;
        self.apply_lfo_setup();
    }

    /// LFO waveform of every voice.
    pub fn set_shape(&mut self, shape: LfoShape) {
        self.shape = shape;
        self.apply_lfo_setup();
    }

    /// Centre delay in milliseconds (ignored if not finite; limited to 0..=100).
    pub fn set_delay_ms(&mut self, ms: f32) {
        if ms.is_finite() {
            self.delay_ms = ms.clamp(0.0, MAX_CHORUS_MS);
        }
    }

    /// Sweep half-width in milliseconds (ignored if not finite; 0..=100).
    pub fn set_depth_ms(&mut self, ms: f32) {
        if ms.is_finite() {
            self.depth_ms = ms.clamp(0.0, MAX_CHORUS_MS);
        }
    }

    /// 0 is dry, 1 is wet only.
    pub fn set_mix(&mut self, mix: f32) {
        if mix.is_finite() {
            self.mix = mix.clamp(0.0, 1.0);
        }
    }

    /// 0 puts every channel in phase, 1 puts the last channel half a cycle away.
    pub fn set_stereo_spread(&mut self, spread: f32) {
        if spread.is_finite() {
            self.spread = spread.clamp(0.0, 1.0);
            self.apply_lfo_setup();
        }
    }

    /// Interpolator for the voice taps (Hermite by default). Not Thiran: a
    /// recursive interpolator misbehaves under a sweep.
    pub fn set_interpolation(&mut self, interp: Interpolation) {
        let interp = match interp {
            Interpolation::Thiran(_) => Interpolation::Hermite,
            other => other,
        };
        for ch in &mut self.channels {
            ch.line.set_interpolation(interp);
        }
    }

    /// Lock synced LFOs to the host for the block about to run.
    pub fn set_transport(&mut self, transport: &Transport) {
        for ch in &mut self.channels {
            for l in &mut ch.lfos {
                l.set_transport(transport);
            }
        }
    }

    /// One sample of one channel. A channel index past the end uses the last.
    #[inline]
    pub fn process_sample(&mut self, channel: usize, input: f32) -> f32 {
        let x = if input.is_finite() { input } else { 0.0 };
        let fs = f64::from(self.sample_rate);
        let ci = channel.min(self.channels.len() - 1);
        let ch = &mut self.channels[ci];
        let base = ch.delay.next((f64::from(self.delay_ms) * fs / 1000.0) as f32);
        let depth = ch.depth.next((f64::from(self.depth_ms) * fs / 1000.0) as f32);
        ch.line.write(x);
        let mut sum = 0.0;
        for lfo in ch.lfos.iter_mut().take(self.voices) {
            let d = base + depth * lfo.next();
            sum += ch.line.read(d);
        }
        let wet = sum / (self.voices as f32).sqrt();
        let y = (1.0 - self.mix) * x + self.mix * wet;
        if y.is_finite() { y } else { 0.0 }
    }

    /// Process a block of one channel in place.
    pub fn process(&mut self, channel: usize, block: &mut [f32]) {
        for s in block {
            *s = self.process_sample(channel, *s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_mix_is_transparent_and_state_is_finite() {
        let mut c = Chorus::new(2, 48_000.0);
        c.set_mix(0.0);
        for n in 0..1000 {
            let x = (n as f32 * 0.05).sin();
            assert_eq!(c.process_sample(0, x), x);
        }
        c.set_mix(1.0);
        for _ in 0..1000 {
            assert!(c.process_sample(1, f32::NAN).is_finite());
        }
    }

    #[test]
    fn zero_depth_is_a_pure_delay() {
        let mut c = Chorus::new(1, 48_000.0);
        c.set_depth_ms(0.0);
        c.set_delay_ms(10.0);
        c.set_voices(1);
        c.set_mix(1.0);
        let mut out = Vec::new();
        for n in 0..2000 {
            out.push(c.process_sample(0, if n == 0 { 1.0 } else { 0.0 }));
        }
        let peak = out.iter().position(|v| v.abs() > 0.5).unwrap();
        assert_eq!(peak, 480);
    }
}
