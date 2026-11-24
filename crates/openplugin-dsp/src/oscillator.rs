//! Phase oscillator.
//!
//! The MIT `synth` crate steps a phase accumulator and reads sine, saw, and
//! square at that phase. Noise and the allocating bezier envelope stay there.
//! `next` only adds `hz / sample_rate` and wraps.
#![allow(deprecated)] // this module defines the deprecated items

/// Waveform of the deprecated [`Oscillator`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
#[deprecated(since = "1.0.0", note = "not band-limited; use `BlWave` with `BandlimitedOsc`")]
pub enum Wave {
    /// A sine.
    Sine,
    /// A rising ramp, -1 to 1, not band-limited.
    Saw,
    /// +1 for the first half cycle, -1 for the second, not band-limited.
    Square,
}

/// A naive phase-accumulator oscillator (it aliases). Plain state, no
/// allocation.
#[derive(Clone, Copy, Debug)]
#[deprecated(since = "1.0.0", note = "saw and square alias; use `BandlimitedOsc` or `WavetableOsc`")]
pub struct Oscillator {
    phase: f32,
    wave: Wave,
}

impl Oscillator {
    /// An oscillator at phase 0.
    pub const fn new(wave: Wave) -> Self {
        Self { phase: 0.0, wave }
    }

    /// Phase in cycles, 0..1.
    pub const fn phase(self) -> f32 {
        self.phase
    }

    /// One sample in -1..1. A non-finite or negative frequency does not move the phase.
    pub fn next(&mut self, hz: f32, sample_rate: f32) -> f32 {
        let sample = match self.wave {
            Wave::Sine => (self.phase * std::f32::consts::TAU).sin(),
            Wave::Saw => self.phase.mul_add(2.0, -1.0),
            Wave::Square => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        };
        if hz.is_finite() && hz >= 0.0 && sample_rate.is_finite() && sample_rate > 0.0 {
            self.phase = (self.phase + hz / sample_rate).rem_euclid(1.0);
        }
        sample
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_starts_at_zero_and_a_full_cycle_returns_home() {
        let mut osc = Oscillator::new(Wave::Sine);
        let first = osc.next(48_000.0, 48_000.0);
        assert!(first.abs() < 1e-5);
        for _ in 0..47_999 {
            osc.next(48_000.0, 48_000.0);
        }
        assert!(osc.phase().abs() < 1e-3);
        assert_eq!(Oscillator::new(Wave::Saw).next(0.0, 48_000.0), -1.0);
        assert_eq!(Oscillator::new(Wave::Square).next(0.0, 48_000.0), 1.0);
    }
}
