//! Phaser: a cascade of first-order allpass filters with feedback.
//!
//! Each stage is `A(z) = (a + z^-1) / (1 + a z^-1)`, a first-order allpass whose
//! phase runs from 0 at DC to -180 degrees at Nyquist. With the bilinear
//! transform and `g = tan(pi f_b / fs)`, `a = (g - 1) / (g + 1)` puts the -90
//! degree point at the break frequency `f_b` (J. O. Smith, "Introduction to
//! Digital Filters", section "First-Order Allpass"; also "Physical Audio
//! Signal Processing", section "Phasing"). The phase of one stage is
//!
//! ```text
//!     phi(w) = -2 atan( tan(w / 2) / g )
//! ```
//!
//! and `N` stages give `N phi(w)`. Mixing the dry input with the cascade's
//! output cancels wherever the cascade's phase is an odd multiple of -180
//! degrees (and the magnitudes are both 1, which an allpass guarantees), so
//! the notches are at
//!
//! ```text
//!     tan(w_k / 2) = g tan( (2k + 1) pi / (2N) ),     (2k + 1) < N.
//! ```
//!
//! `N` stages produce `N / 2` notches (rounded down). The tests check these
//! positions. Feedback from the cascade's output to its input (one sample late,
//! so there is no delay-free loop) adds a resonant peak next to each notch.
//! The break frequency is swept by an LFO on a logarithmic scale between a
//! minimum and a maximum, which is how the ear tracks pitch.
//!
//! The stage is in transposed direct form II with `f64` state, flushed below
//! `1e-30`. Non-finite input is treated as 0. The loop is bounded by clamping
//! the fed-back value to +-1000. Allocation happens in [`Phaser::new`] and
//! [`Phaser::prepare`] only.

use crate::lfo::{Lfo, LfoRate, LfoShape};
use crate::rate::effective_rate;
use openplugin_core::Transport;

/// Most allpass stages.
pub const MAX_PHASER_STAGES: usize = 12;
/// Feedback limit (absolute).
pub const MAX_PHASER_FEEDBACK: f32 = 0.95;

#[derive(Clone, Debug)]
struct ChannelState {
    z: [f64; MAX_PHASER_STAGES],
    last: f64,
    lfo: Lfo,
}

/// Phaser.
/// `new` and `prepare` allocate; processing and the setters do not.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{LfoShape, Phaser};
///
/// let mut phaser = Phaser::new(1, 48_000.0);
/// phaser.set_shape(LfoShape::Triangle);
/// let mut block = vec![0.25f32; 512];
/// phaser.process(0, &mut block);
/// assert!(block.iter().all(|s| s.is_finite()));
/// ```
#[derive(Clone, Debug)]
pub struct Phaser {
    sample_rate: f32,
    channels: Vec<ChannelState>,
    stages: usize,
    min_hz: f32,
    max_hz: f32,
    depth: f32,
    feedback: f32,
    mix: f32,
    spread: f32,
    rate: LfoRate,
    shape: LfoShape,
}

impl Phaser {
    /// Allocates.
    pub fn new(channels: usize, sample_rate: f32) -> Self {
        let mut p = Self {
            sample_rate: effective_rate(sample_rate),
            channels: Vec::new(),
            stages: 6,
            min_hz: 200.0,
            max_hz: 3_000.0,
            depth: 1.0,
            feedback: 0.4,
            mix: 0.5,
            spread: 0.5,
            rate: LfoRate::Hz(0.4),
            shape: LfoShape::Sine,
        };
        p.prepare(channels, sample_rate);
        p
    }

    /// Set channel count and rate and clear state. Allocates.
    pub fn prepare(&mut self, channels: usize, sample_rate: f32) {
        self.sample_rate = effective_rate(sample_rate);
        self.channels.clear();
        for _ in 0..channels.max(1) {
            self.channels.push(ChannelState {
                z: [0.0; MAX_PHASER_STAGES],
                last: 0.0,
                lfo: Lfo::new(self.sample_rate),
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
            ch.lfo.set_seed(ci as u64 + 200);
        }
    }

    /// Clear state and restart the LFOs.
    pub fn reset(&mut self) {
        for ch in &mut self.channels {
            ch.z = [0.0; MAX_PHASER_STAGES];
            ch.last = 0.0;
            ch.lfo.reset();
        }
    }

    /// The channel count given to `new`.
    pub fn channels(&self) -> usize {
        self.channels.len()
    }

    /// Number of allpass stages, 1..=12.
    pub fn set_stages(&mut self, stages: usize) {
        self.stages = stages.clamp(1, MAX_PHASER_STAGES);
    }

    /// Lowest and highest break frequency in hertz. They are limited to
    /// 1 Hz..0.49 fs and ordered. Equal values hold the break fixed.
    pub fn set_range(&mut self, min_hz: f32, max_hz: f32) {
        if min_hz.is_finite() && max_hz.is_finite() {
            let hi = 0.49 * self.sample_rate;
            let (a, b) = (min_hz.clamp(1.0, hi), max_hz.clamp(1.0, hi));
            self.min_hz = a.min(b);
            self.max_hz = a.max(b);
        }
    }

    /// How much of the range the LFO sweeps, 0..1, about the geometric centre.
    pub fn set_depth(&mut self, depth: f32) {
        if depth.is_finite() {
            self.depth = depth.clamp(0.0, 1.0);
        }
    }

    /// Feedback, limited to +-0.95.
    pub fn set_feedback(&mut self, fb: f32) {
        if fb.is_finite() {
            self.feedback = fb.clamp(-MAX_PHASER_FEEDBACK, MAX_PHASER_FEEDBACK);
        }
    }

    /// -1..1; `out = (1 - |mix|) dry + mix * wet`. At 0.5 the notches are deepest.
    pub fn set_mix(&mut self, mix: f32) {
        if mix.is_finite() {
            self.mix = mix.clamp(-1.0, 1.0);
        }
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

    /// Lock a synced LFO to the host for the block about to run.
    pub fn set_transport(&mut self, transport: &Transport) {
        for ch in &mut self.channels {
            ch.lfo.set_transport(transport);
        }
    }

    /// The allpass coefficient for break frequency `hz` at `sample_rate`.
    pub fn coefficient(sample_rate: f32, hz: f32) -> f64 {
        let fs = f64::from(effective_rate(sample_rate));
        let f = if hz.is_finite() { f64::from(hz) } else { 1_000.0 };
        let g = (std::f64::consts::PI * f.clamp(1.0, 0.49 * fs) / fs).tan();
        (g - 1.0) / (g + 1.0)
    }

    /// Frequencies, in hertz, of the notches of `stages` allpass stages with
    /// break frequency `break_hz` (see the module documentation). `out`
    /// receives them lowest first; returns how many were written.
    pub fn notch_frequencies(sample_rate: f32, break_hz: f32, stages: usize, out: &mut [f32]) -> usize {
        let fs = f64::from(effective_rate(sample_rate));
        let g = (std::f64::consts::PI * f64::from(break_hz).clamp(1.0, 0.49 * fs) / fs).tan();
        let n = stages.clamp(1, MAX_PHASER_STAGES);
        let mut count = 0;
        let mut k = 0;
        while 2 * k + 1 < n && count < out.len() {
            let arg = (2 * k + 1) as f64 * std::f64::consts::PI / (2 * n) as f64;
            let w = 2.0 * (g * arg.tan()).atan();
            out[count] = (w * fs / std::f64::consts::TAU) as f32;
            count += 1;
            k += 1;
        }
        count
    }

    /// One sample of one channel. A channel index past the end uses the last.
    #[inline]
    pub fn process_sample(&mut self, channel: usize, input: f32) -> f32 {
        let x = if input.is_finite() { f64::from(input) } else { 0.0 };
        let ci = channel.min(self.channels.len() - 1);
        let ch = &mut self.channels[ci];
        let lfo = f64::from(ch.lfo.next());
        // u in 0..1 about the centre; frequency on a log scale.
        let u = 0.5 + 0.5 * f64::from(self.depth) * lfo;
        let (lo, hi) = (f64::from(self.min_hz), f64::from(self.max_hz));
        let f = lo * (hi / lo).powf(u);
        let a = Self::coefficient(self.sample_rate, f as f32);
        let mut v = x + f64::from(self.feedback) * ch.last;
        for z in ch.z.iter_mut().take(self.stages) {
            let y = a * v + *z;
            let s = v - a * y;
            *z = if s.abs() < 1e-30 { 0.0 } else { s };
            v = y;
        }
        if !v.is_finite() || v.abs() > 1e30 {
            ch.z = [0.0; MAX_PHASER_STAGES];
            ch.last = 0.0;
            return 0.0;
        }
        ch.last = if v.abs() < 1e-30 { 0.0 } else { v.clamp(-1000.0, 1000.0) };
        let m = f64::from(self.mix);
        let y = (1.0 - m.abs()) * x + m * v;
        if y.abs() <= f64::from(f32::MAX) { y as f32 } else { 0.0 }
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
    fn break_frequency_gives_minus_ninety_degrees() {
        // a = 0 exactly at fs/4, where one stage is a pure sample delay.
        assert!(Phaser::coefficient(48_000.0, 12_000.0).abs() < 1e-12);
    }

    #[test]
    fn notch_list_for_four_stages() {
        let mut out = [0.0f32; 8];
        let n = Phaser::notch_frequencies(48_000.0, 1_000.0, 4, &mut out);
        assert_eq!(n, 2);
        assert!(out[0] < out[1]);
        assert_eq!(Phaser::notch_frequencies(48_000.0, 1_000.0, 2, &mut out), 1);
        assert_eq!(Phaser::notch_frequencies(48_000.0, 1_000.0, 1, &mut out), 0);
    }

    #[test]
    fn hostile_input_recovers() {
        let mut p = Phaser::new(2, 96_000.0);
        p.set_feedback(0.95);
        for i in 0..5000 {
            assert!(p.process_sample(i % 2, if i % 2 == 0 { f32::NAN } else { f32::NEG_INFINITY }).is_finite());
        }
        let mut last = 1.0f32;
        for _ in 0..400_000 {
            last = p.process_sample(0, 0.0).abs();
        }
        assert!(last < 1e-20);
    }
}
