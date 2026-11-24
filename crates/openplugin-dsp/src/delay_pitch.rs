//! Low-latency pitch shifter from modulated delay taps.
//!
//! Method: the delay-line modulation pitch shifter of J. Dattorro, "Effect
//! Design, Part 2: Delay-Line Modulation and Chorus", J. Audio Eng. Soc.
//! 45(10), 1997, and the classic two-tap "rotating tape head" arrangement. A
//! tap whose delay shrinks at `r - 1` samples per sample reads the signal
//! `r` times faster, so a sine of frequency `f` comes out at `r f`. A delay
//! cannot shrink for ever: each tap's delay is a sawtooth, and the taps are
//! crossfaded so that a tap's weight is zero exactly where it wraps (the jump
//! is never heard) while the weights always sum to one. Reads use the
//! fractional [`ModDelay`] with Hermite interpolation.
//!
//! - **Two taps** (the default), half a cycle apart. Each tap is at full
//!   weight for most of its pass and crossfades with the other over a
//!   fraction `crossfade` of it (`sin^2` / `cos^2`, so the weights sum to
//!   one). A short crossfade keeps the two copies of the signal, which are
//!   `W` apart in time and so out of phase for most frequencies, mixed for
//!   as short a time as possible.
//! - **3 to 6 taps** use the smooth `sin^2` weights over the whole cycle
//!   (sum `M / 2`, normalised). More of the signal is a mix of delayed copies
//!   at any moment, so tonal material is rougher, not smoother.
//!
//! The delay of the active taps spans `W` (the window), so the latency is
//! the average, `d_min + W / 2` ([`DelayPitchShifter::latency`], about 15 ms
//! for a 30 ms window), and the cost is one or two interpolated reads per
//! sample for two taps. There is no analysis frame.
//!
//! Where it falls short, measured in `tests/pitch_shift_latency.rs`: a
//! sustained tone `f` crossfading between taps `W / 2` apart sums to
//! `|w1 + w2 exp(j pi f W)|`, so the output is amplitude-modulated at the
//! wrap rate `|r - 1| W^-1 * 2` unless `f W` is an even integer (taps in
//! phase). This is the "roughness" of every delay-line shifter. Choosing `W`
//! as a multiple of the period of a known pitch removes it for that pitch; a
//! longer window lowers the wrap rate but raises the latency. The formants
//! move with the pitch (no spectral-envelope preservation). Shifting up folds
//! content above Nyquist over `r` (no anti-alias filter), so feed it
//! band-limited material or shift down. A ratio of exactly 1 is a single tap
//! at the average delay.
//!
//! `process` never allocates, treats non-finite input as zero, clamps the
//! ratio, and is panic-free.

use crate::interp::Interpolation;
use crate::moddelay::ModDelay;
use crate::rate::effective_rate;
use std::f64::consts::PI;

const D_MIN: f32 = 2.0;
/// Most taps.
pub const MAX_DELAY_PITCH_TAPS: usize = 6;

struct Channel {
    line: ModDelay,
    phase: f64,
    /// Cycle length in samples, smoothed towards its target.
    cycle: f64,
}

/// Delay-line pitch shifter.
///
/// # Example
///
/// ```
/// use openplugin_dsp::DelayPitchShifter;
///
/// let mut shifter = DelayPitchShifter::new(1, 48_000.0, 40.0, 2);
/// shifter.set_semitones(7.0);
///
/// let mut block = [0.0f32; 256];
/// shifter.process(0, &mut block);
/// assert!(block.iter().all(|s| s.is_finite()));
/// ```
pub struct DelayPitchShifter {
    sample_rate: f32,
    window: f32,
    taps: usize,
    crossfade: f64,
    ratio: f64,
    /// Period of the input's pitch in samples, when synchronised.
    period: Option<f64>,
    channels: Vec<Channel>,
}

impl DelayPitchShifter {
    /// `window_ms` (5..=100) is the span of the delay, `taps` is 2..=6.
    /// The crossfade starts at 0.3. Allocates.
    pub fn new(channels: usize, sample_rate: f32, window_ms: f32, taps: usize) -> Self {
        let fs = effective_rate(sample_rate);
        let ms = if window_ms.is_finite() { window_ms.clamp(5.0, 100.0) } else { 30.0 };
        let window = (ms * fs / 1000.0).round().max(16.0);
        let taps = taps.clamp(2, MAX_DELAY_PITCH_TAPS);
        let total_ms = (D_MIN + window * 2.0 + 8.0) * 1000.0 / fs;
        Self {
            sample_rate: fs,
            window,
            taps,
            crossfade: 0.3,
            ratio: 1.0,
            period: None,
            channels: (0..channels.clamp(1, 64))
                .map(|_| Channel { line: ModDelay::new(fs, total_ms, Interpolation::Hermite), phase: 0.0, cycle: 0.0 })
                .collect(),
        }
    }

    /// Crossfade length of the two-tap shifter as a fraction (0.05..=1) of a
    /// tap's pass. 1 is the smooth `sin^2` weighting; smaller values mix the
    /// two copies for less of the time. Ignored with three or more taps.
    pub fn set_crossfade(&mut self, fraction: f32) {
        self.crossfade = if fraction.is_finite() { f64::from(fraction).clamp(0.05, 1.0) } else { 0.3 };
    }

    /// Pitch-synchronous mode. With the fundamental of a monophonic input in
    /// Hz (from [`crate::MpmDetector`] or [`crate::YinDetector`]), the
    /// distance between the taps is rounded to a whole number of periods, so
    /// the copies being crossfaded are in phase: the amplitude ripple goes
    /// away and the pitch is exact for that tone, for any `W` and ratio. The
    /// cycle length follows the pitch with a time constant of about 100 ms.
    /// `None` (or a non-positive or non-finite value) goes back to a fixed
    /// window. The reported latency stays that of the fixed window; the true
    /// mean delay differs from it by at most one period.
    pub fn set_period_hz(&mut self, hz: Option<f32>) {
        self.period = hz
            .filter(|h| h.is_finite() && *h > 20.0)
            .map(|h| f64::from(self.sample_rate) / f64::from(h))
            .filter(|t| *t < f64::from(self.window));
    }

    /// Pitch ratio `2^(semitones / 12)`; semitones are clamped to +-24.
    pub fn set_semitones(&mut self, semitones: f32) {
        let s = if semitones.is_finite() { semitones.clamp(-24.0, 24.0) } else { 0.0 };
        self.ratio = 2f64.powf(f64::from(s) / 12.0);
    }

    /// Pitch ratio directly, clamped to 0.25..=4.
    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = if ratio.is_finite() { f64::from(ratio).clamp(0.25, 4.0) } else { 1.0 };
    }

    /// Latency in samples for `Plugin::latency`: the average tap delay.
    pub fn latency(&self) -> usize {
        (D_MIN + self.window * 0.5).round() as usize
    }

    /// Span of the delay `W` in samples.
    pub fn window_samples(&self) -> usize {
        self.window as usize
    }

    /// The sample rate in Hz the shifter was built for.
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Clear the delay lines and restart the taps.
    pub fn reset(&mut self) {
        for c in &mut self.channels {
            c.line.reset();
            c.phase = 0.0;
        }
    }

    /// Weight of a two-tap pass at position `x` in 0..2 (units of half a cycle).
    #[inline]
    fn weight2(x: f64, fade: f64) -> f64 {
        if x < fade {
            let s = (0.5 * PI * x / fade).sin();
            s * s
        } else if x < 1.0 {
            1.0
        } else if x < 1.0 + fade {
            let c = (0.5 * PI * (x - 1.0) / fade).cos();
            c * c
        } else {
            0.0
        }
    }

    /// Process one channel in place.
    pub fn process(&mut self, channel: usize, buffer: &mut [f32]) {
        let Some(c) = self.channels.get_mut(channel) else { return };
        let m = self.taps;
        let w = f64::from(self.window);
        let unity = (self.ratio - 1.0).abs() < 1e-9;
        let mid = f64::from(D_MIN) + 0.5 * w;
        // Two taps: the delay of the active part spans `w`, the cycle spans `w / s`.
        let fade = self.crossfade;
        let (nominal, norm) = if m == 2 { (w * 2.0 / (1.0 + fade), 1.0) } else { (w, 2.0 / m as f64) };
        // Distance between taps; with a known period, a whole number of periods.
        let spacing = nominal / m as f64;
        let target = match self.period {
            Some(t) => (spacing / t).round().max(1.0) * t * m as f64,
            None => nominal,
        };
        if c.cycle <= 0.0 || self.period.is_none() {
            c.cycle = target;
        }
        for s in buffer.iter_mut() {
            c.cycle += (target - c.cycle) * 2e-4;
            let cycle = c.cycle;
            let step = (1.0 - self.ratio) / cycle;
            c.line.write(*s);
            let y = if unity {
                c.line.read(mid as f32)
            } else {
                let mut acc = 0.0f64;
                for i in 0..m {
                    let ph = (c.phase + i as f64 / m as f64).fract();
                    let (g, d) = if m == 2 {
                        (Self::weight2(ph * 2.0, fade), f64::from(D_MIN) + ph * cycle)
                    } else {
                        let sn = (PI * ph).sin();
                        (sn * sn, f64::from(D_MIN) + ph * cycle)
                    };
                    if g > 0.0 {
                        acc += g * f64::from(c.line.read(d as f32));
                    }
                }
                (acc * norm) as f32
            };
            c.phase = (c.phase + step).rem_euclid(1.0);
            *s = if y.is_finite() { y } else { 0.0 };
        }
    }
}
