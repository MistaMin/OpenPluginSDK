//! Flanger with feedback and an optional through-zero mode.
//!
//! A flanger adds the input to a copy delayed by a short, swept time. The sum
//! of a signal and its delayed copy is a comb filter, `|1 + g e^{-j w d}|`,
//! with notches at odd multiples of `fs / (2 d)`; sweeping `d` sweeps the
//! notches. Feeding the delayed signal back (`wet = x(n - d) + fb * wet(n - d)`)
//! turns notches into narrow peaks as well. The feedback comb is the one in
//! J. O. Smith, "Physical Audio Signal Processing", section "Flanging", and
//! J. Dattorro, "Effect Design, Part 2", J. Audio Eng. Soc. 45(10), 1997.
//!
//! The loop delay equals the direct delay `d` exactly: the line is read
//! `d - 1` samples back before the new sample is written, so the delay-free
//! step of the write is not added to the loop. The smallest `d` is therefore
//! the interpolator's minimum plus one sample (2 samples for Hermite).
//!
//! **Through-zero** mode copies what a pair of tape machines does. The "dry"
//! path is no longer immediate but delayed by a fixed `delay_ms`, and the swept
//! path moves around it, from `delay_ms - depth_ms` to `delay_ms + depth_ms`.
//! The two delays cross, which is not possible when the dry path is
//! immediate. With negative `mix` (the wet path inverted) the output is
//! cancelled at the crossing, the characteristic "jet" sound. The cost is a
//! fixed latency of `delay_ms`, reported by [`Flanger::latency_samples`].
//!
//! `mix` runs from -1 to 1: `out = (1 - |mix|) dry + mix * wet`. At 0.5 the
//! notches are deepest. Negative mix inverts the wet path, moving the notches
//! to even multiples of `fs / (2 d)` and putting one at DC.
//!
//! Non-finite input is treated as 0; the feedback path is limited to +-1000
//! and flushed below `1e-30`, so a hostile input cannot leave the loop
//! stuck in a non-finite or subnormal state. Allocation happens in
//! [`Flanger::new`] and [`Flanger::prepare`] only.

use crate::interp::Interpolation;
use crate::lfo::{Lfo, LfoRate, LfoShape};
use crate::moddelay::{Glide, ModDelay};
use crate::rate::effective_rate;
use openplugin_core::Transport;

/// Longest centre delay or depth, in milliseconds.
pub const MAX_FLANGER_MS: f32 = 50.0;
/// Feedback limit (absolute).
pub const MAX_FLANGER_FEEDBACK: f32 = 0.98;

#[derive(Clone, Debug)]
struct ChannelState {
    line: ModDelay,
    dry: ModDelay,
    lfo: Lfo,
    delay: Glide,
    depth: Glide,
}

/// Flanger.
/// `new` and `prepare` allocate; processing and the setters do not.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{Flanger, LfoRate};
///
/// let mut flanger = Flanger::new(2, 48_000.0);
/// flanger.set_delay_ms(3.0);
/// flanger.set_depth_ms(2.0);
/// flanger.set_feedback(0.6);
/// flanger.set_rate(LfoRate::Hz(0.25));
/// let mut left = vec![0.25f32; 512];
/// let mut right = vec![0.25f32; 512];
/// flanger.process(0, &mut left);
/// flanger.process(1, &mut right);
/// assert!(left.iter().all(|s| s.is_finite()));
/// ```
#[derive(Clone, Debug)]
pub struct Flanger {
    sample_rate: f32,
    channels: Vec<ChannelState>,
    delay_ms: f32,
    depth_ms: f32,
    feedback: f32,
    mix: f32,
    through_zero: bool,
    spread: f32,
    rate: LfoRate,
    shape: LfoShape,
}

impl Flanger {
    /// Allocates.
    pub fn new(channels: usize, sample_rate: f32) -> Self {
        let mut f = Self {
            sample_rate: effective_rate(sample_rate),
            channels: Vec::new(),
            delay_ms: 3.0,
            depth_ms: 2.0,
            feedback: 0.5,
            mix: 0.5,
            through_zero: false,
            spread: 0.5,
            rate: LfoRate::Hz(0.3),
            shape: LfoShape::Triangle,
        };
        f.prepare(channels, sample_rate);
        f
    }

    /// Set channel count and rate and clear state. Allocates.
    pub fn prepare(&mut self, channels: usize, sample_rate: f32) {
        self.sample_rate = effective_rate(sample_rate);
        let fs = self.sample_rate;
        self.channels.clear();
        for _ in 0..channels.max(1) {
            self.channels.push(ChannelState {
                line: ModDelay::new(fs, 2.0 * MAX_FLANGER_MS + 2.0, Interpolation::Hermite),
                dry: ModDelay::new(fs, MAX_FLANGER_MS + 2.0, Interpolation::Linear),
                lfo: Lfo::new(fs),
                delay: Glide::new(fs, 10.0),
                depth: Glide::new(fs, 10.0),
            });
        }
        self.apply_lfo_setup();
    }

    fn apply_lfo_setup(&mut self) {
        let n = self.channels.len();
        for (ci, ch) in self.channels.iter_mut().enumerate() {
            ch.lfo.set_sample_rate(self.sample_rate);
            ch.lfo.set_shape(self.shape);
            ch.lfo.set_rate(self.rate);
            let off = if n > 1 { 0.5 * self.spread * ci as f32 / (n - 1) as f32 } else { 0.0 };
            ch.lfo.set_phase_offset(off);
            ch.lfo.set_seed(ci as u64 + 100);
        }
    }

    /// Clear state and restart the LFOs.
    pub fn reset(&mut self) {
        let fs = self.sample_rate;
        for ch in &mut self.channels {
            ch.line.reset();
            ch.dry.reset();
            ch.lfo.reset();
            ch.delay = Glide::new(fs, 10.0);
            ch.depth = Glide::new(fs, 10.0);
        }
    }

    /// The channel count given to `new`.
    pub fn channels(&self) -> usize {
        self.channels.len()
    }

    /// Centre delay in milliseconds, 0..=50.
    pub fn set_delay_ms(&mut self, ms: f32) {
        if ms.is_finite() {
            self.delay_ms = ms.clamp(0.0, MAX_FLANGER_MS);
        }
    }

    /// Sweep half-width in milliseconds, 0..=50.
    pub fn set_depth_ms(&mut self, ms: f32) {
        if ms.is_finite() {
            self.depth_ms = ms.clamp(0.0, MAX_FLANGER_MS);
        }
    }

    /// Feedback, limited to +-0.98. Negative feedback moves the peaks.
    pub fn set_feedback(&mut self, fb: f32) {
        if fb.is_finite() {
            self.feedback = fb.clamp(-MAX_FLANGER_FEEDBACK, MAX_FLANGER_FEEDBACK);
        }
    }

    /// -1..1, see the module documentation.
    pub fn set_mix(&mut self, mix: f32) {
        if mix.is_finite() {
            self.mix = mix.clamp(-1.0, 1.0);
        }
    }

    /// Through-zero flanging: the dry path is delayed so the sweep can cross it.
    pub fn set_through_zero(&mut self, on: bool) {
        self.through_zero = on;
    }

    /// The sweep rate, free in Hz or synced to the host tempo.
    pub fn set_rate(&mut self, rate: LfoRate) {
        self.rate = rate;
        self.apply_lfo_setup();
    }

    /// The sweep waveform.
    pub fn set_shape(&mut self, shape: LfoShape) {
        self.shape = shape;
        self.apply_lfo_setup();
    }

    /// 0..1, offset of the last channel's LFO in half cycles.
    pub fn set_stereo_spread(&mut self, spread: f32) {
        if spread.is_finite() {
            self.spread = spread.clamp(0.0, 1.0);
            self.apply_lfo_setup();
        }
    }

    /// Interpolator for the swept tap. Thiran is replaced by Hermite.
    pub fn set_interpolation(&mut self, interp: Interpolation) {
        let interp = match interp {
            Interpolation::Thiran(_) => Interpolation::Hermite,
            other => other,
        };
        for ch in &mut self.channels {
            ch.line.set_interpolation(interp);
        }
    }

    /// Lock a synced LFO to the host for the block about to run.
    pub fn set_transport(&mut self, transport: &Transport) {
        for ch in &mut self.channels {
            ch.lfo.set_transport(transport);
        }
    }

    /// Latency in samples: the fixed dry delay in through-zero mode, else 0.
    pub fn latency_samples(&self) -> usize {
        if self.through_zero {
            (f64::from(self.delay_ms) * f64::from(self.sample_rate) / 1000.0).round() as usize
        } else {
            0
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
        let lfo = ch.lfo.next();
        let floor = ch.line.min_delay() + 1.0;
        let d = (base + depth * lfo).max(floor);
        // Read d - 1 back before the write: the loop delay is exactly d.
        let wet = ch.line.read(d - 1.0);
        let fb = (wet * self.feedback).clamp(-1000.0, 1000.0);
        ch.line.write(x + fb);
        let dry = if self.through_zero {
            ch.dry.write(x);
            ch.dry.read(base.round().max(0.0))
        } else {
            x
        };
        let y = (1.0 - self.mix.abs()) * dry + self.mix * wet;
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

    fn impulse_response(f: &mut Flanger, n: usize) -> Vec<f32> {
        (0..n).map(|i| f.process_sample(0, if i == 0 { 1.0 } else { 0.0 })).collect()
    }

    #[test]
    fn static_comb_puts_echoes_at_multiples_of_the_delay() {
        let mut f = Flanger::new(1, 48_000.0);
        f.set_delay_ms(1.0); // 48 samples
        f.set_depth_ms(0.0);
        f.set_feedback(0.5);
        f.set_mix(1.0);
        let ir = impulse_response(&mut f, 400);
        // wet = x(n-d) + fb * wet(n-d): echoes d, 2d, 3d with 1, .5, .25.
        assert!((ir[48] - 1.0).abs() < 1e-5);
        assert!((ir[96] - 0.5).abs() < 1e-5);
        assert!((ir[144] - 0.25).abs() < 1e-5);
    }

    #[test]
    fn through_zero_reports_latency_and_dry_is_delayed() {
        let mut f = Flanger::new(1, 48_000.0);
        f.set_through_zero(true);
        f.set_delay_ms(2.0);
        f.set_depth_ms(0.0);
        f.set_feedback(0.0);
        f.set_mix(0.0);
        assert_eq!(f.latency_samples(), 96);
        let ir = impulse_response(&mut f, 300);
        assert!((ir[96] - 1.0).abs() < 1e-5);
        assert!(ir.iter().enumerate().all(|(i, v)| i == 96 || v.abs() < 1e-6));
    }

    #[test]
    fn hostile_input_recovers() {
        let mut f = Flanger::new(2, 44_100.0);
        f.set_feedback(0.95);
        for i in 0..5000 {
            let x = if i % 3 == 0 { f32::NAN } else { f32::INFINITY };
            assert!(f.process_sample(i % 2, x).is_finite());
        }
        let mut peak = 1.0f32;
        for _ in 0..200_000 {
            peak = f.process_sample(0, 0.0).abs();
        }
        assert!(peak < 1e-20);
    }
}
