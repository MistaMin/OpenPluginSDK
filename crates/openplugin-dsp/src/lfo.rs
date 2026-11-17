//! Low-frequency oscillator that free-runs or locks to the host transport.
//!
//! Shapes: sine, triangle, rising saw, square, sample-and-hold and smoothed
//! random. All are bipolar (-1..1), start at phase 0 (sine and triangle at
//! zero rising, saw at -1, square at +1) and are the plain, not band-limited,
//! waveforms: an LFO runs far below audio rate, where aliasing is not a concern.
//!
//! **Random shapes are a function of the cycle number.** Cycle `c` draws its
//! value from the hash `splitmix64(seed, c)` (Steele, Lea and Flood, "Fast
//! splittable pseudorandom number generators", OOPSLA 2014), so the same cycle
//! gives the same value however the phase got there. That is what lets a
//! synced random LFO land on the same value after the host jumps. Sample-and-
//! hold holds the value of cycle `c`; smoothed random blends from cycle `c`'s
//! value to cycle `c + 1`'s with `3t^2 - 2t^3`, so it is continuous and has
//! zero slope at each point.
//!
//! **Sync.** With [`LfoRate::Sync`] the rate is a [`NoteDivision`], a length in
//! quarter notes. [`Lfo::set_transport`], called once per block with the
//! host's [`Transport`], sets the position to `quarter_notes / length`
//! (cycles, plus the phase offset) while the host plays. The LFO is therefore
//! a function of the host's musical position, not a counter: after a loop,
//! locate or tempo change it is exactly where the host's position says, with no
//! drift. Between blocks it advances at `tempo / 60 / length` cycles per
//! second. While the host is stopped it keeps running at the last good tempo
//! (the effect is still audible on live input) and re-locks when playback
//! starts. A tempo that is zero, negative or not finite is ignored; so is a
//! non-finite `quarter_notes`.
//!
//! Phase is `f64`; the output is `f32`. Nothing here allocates.

use crate::rate::effective_rate;
use openplugin_core::Transport;

/// Waveform of an [`Lfo`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LfoShape {
    Sine,
    Triangle,
    /// Rising ramp, -1 to 1.
    Saw,
    /// +1 for the first half cycle, -1 for the second.
    Square,
    /// A new random value each cycle, held.
    SampleHold,
    /// Random values joined smoothly.
    SmoothRandom,
}

/// Dotted, straight or triplet timing of a [`NoteDivision`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Feel {
    Straight,
    /// 1.5 times as long.
    Dotted,
    /// 2/3 as long.
    Triplet,
}

/// A musical length: `1/denominator` of a whole note (four quarter notes) in a
/// feel. `NoteDivision::new(8, Feel::Dotted)` is a dotted eighth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteDivision {
    denominator: u32,
    feel: Feel,
}

impl NoteDivision {
    /// `denominator` is clamped to 1..=128 (1 = whole note, 4 = quarter).
    pub fn new(denominator: u32, feel: Feel) -> Self {
        Self { denominator: denominator.clamp(1, 128), feel }
    }

    /// A whole number of bars is `n * quarters_per_bar` quarter notes; use
    /// [`LfoRate::Quarters`] for that.
    pub fn quarters(self) -> f64 {
        let base = 4.0 / f64::from(self.denominator);
        match self.feel {
            Feel::Straight => base,
            Feel::Dotted => base * 1.5,
            Feel::Triplet => base * 2.0 / 3.0,
        }
    }
}

/// How fast an [`Lfo`] runs.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum LfoRate {
    /// Free-running, cycles per second.
    Hz(f32),
    /// Locked to the host transport, one cycle per note division.
    Sync(NoteDivision),
    /// Locked to the host transport, one cycle per this many quarter notes
    /// (for example 8 for two 4/4 bars).
    Quarters(f32),
}

/// Low-frequency oscillator.
#[derive(Clone, Debug)]
pub struct Lfo {
    shape: LfoShape,
    rate: LfoRate,
    sample_rate: f64,
    tempo: f64,
    phase: f64,
    cycle: i64,
    offset: f64,
    seed: u64,
}

impl Lfo {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            shape: LfoShape::Sine,
            rate: LfoRate::Hz(1.0),
            sample_rate: f64::from(effective_rate(sample_rate)),
            tempo: 120.0,
            phase: 0.0,
            cycle: 0,
            offset: 0.0,
            seed: 0x4C46_4F5F_5345_4544,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = f64::from(effective_rate(sample_rate));
    }

    pub fn set_shape(&mut self, shape: LfoShape) {
        self.shape = shape;
    }

    pub fn shape(&self) -> LfoShape {
        self.shape
    }

    /// Set the rate. A non-finite or negative value leaves the rate unchanged;
    /// a free rate is limited to Nyquist.
    pub fn set_rate(&mut self, rate: LfoRate) {
        let ok = match rate {
            LfoRate::Hz(h) => h.is_finite() && h >= 0.0,
            LfoRate::Quarters(q) => q.is_finite() && q > 0.0,
            LfoRate::Sync(_) => true,
        };
        if ok {
            self.rate = rate;
        }
    }

    pub fn rate(&self) -> LfoRate {
        self.rate
    }

    /// Offset added to the phase, in cycles (0.25 is a quarter cycle ahead).
    /// Applied at read time, so it does not disturb the transport lock.
    pub fn set_phase_offset(&mut self, cycles: f32) {
        if cycles.is_finite() {
            self.offset = f64::from(cycles);
        }
    }

    /// Seed of the random shapes.
    pub fn set_seed(&mut self, seed: u64) {
        self.seed = seed ^ 0x4C46_4F5F_5345_4544;
    }

    /// Tempo in quarter notes per minute, used to run a synced LFO between
    /// transport updates. Ignored unless finite and positive.
    pub fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 {
            self.tempo = bpm;
        }
    }

    /// Restart at phase 0 and cycle 0.
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.cycle = 0;
    }

    /// Jump to a position in cycles (may be negative or large).
    pub fn set_position(&mut self, cycles: f64) {
        if cycles.is_finite() {
            let c = cycles.floor();
            self.cycle = c as i64;
            self.phase = cycles - c;
        }
    }

    /// Position in cycles since the origin: whole cycles plus phase.
    pub fn position(&self) -> f64 {
        self.cycle as f64 + self.phase
    }

    /// Phase within the current cycle, 0..1, without the offset.
    pub fn phase(&self) -> f64 {
        self.phase
    }

    /// Cycles per sample at the current rate and tempo.
    pub fn increment(&self) -> f64 {
        match self.rate {
            LfoRate::Hz(h) => (f64::from(h) / self.sample_rate).min(0.5),
            LfoRate::Sync(div) => self.sync_inc(div.quarters()),
            LfoRate::Quarters(q) => self.sync_inc(f64::from(q)),
        }
    }

    fn sync_inc(&self, quarters: f64) -> f64 {
        (self.tempo / 60.0 / quarters / self.sample_rate).min(0.5)
    }

    fn quarters_per_cycle(&self) -> Option<f64> {
        match self.rate {
            LfoRate::Hz(_) => None,
            LfoRate::Sync(div) => Some(div.quarters()),
            LfoRate::Quarters(q) => Some(f64::from(q)),
        }
    }

    /// Take the host transport for the block about to run. Call it before the
    /// block's first [`Lfo::next`]. See the module documentation for the rules.
    pub fn set_transport(&mut self, transport: &Transport) {
        self.set_tempo(transport.tempo);
        if let Some(q) = self.quarters_per_cycle() {
            if transport.playing && transport.quarter_notes.is_finite() {
                self.set_position(transport.quarter_notes / q);
            }
        }
    }

    /// The value at the current position, with no advance.
    #[inline]
    pub fn value(&self) -> f32 {
        let total = self.phase + self.offset;
        let shift = total.floor();
        let phase = total - shift;
        let cycle = self.cycle.wrapping_add(shift as i64);
        shape_value(self.shape, phase, cycle, self.seed) as f32
    }

    /// The current value, then advance one sample.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        let v = self.value();
        self.advance(1);
        v
    }

    /// Like [`Lfo::next`], scaled to 0..1.
    #[inline]
    pub fn next_unipolar(&mut self) -> f32 {
        0.5 * (self.next() + 1.0)
    }

    /// Advance `samples` samples without producing output.
    pub fn advance(&mut self, samples: usize) {
        self.phase += self.increment() * samples as f64;
        if self.phase >= 1.0 {
            let whole = self.phase.floor();
            self.cycle = self.cycle.wrapping_add(whole as i64);
            self.phase -= whole;
        }
    }

    /// Fill `out` with consecutive bipolar values.
    pub fn fill(&mut self, out: &mut [f32]) {
        for o in out {
            *o = self.next();
        }
    }
}

fn splitmix(seed: u64, cycle: i64) -> f64 {
    let mut z = seed.wrapping_add((cycle as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

fn shape_value(shape: LfoShape, p: f64, cycle: i64, seed: u64) -> f64 {
    match shape {
        LfoShape::Sine => (p * std::f64::consts::TAU).sin(),
        LfoShape::Triangle => {
            if p < 0.25 {
                4.0 * p
            } else if p < 0.75 {
                2.0 - 4.0 * p
            } else {
                4.0 * p - 4.0
            }
        }
        LfoShape::Saw => 2.0 * p - 1.0,
        LfoShape::Square => {
            if p < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        LfoShape::SampleHold => splitmix(seed, cycle),
        LfoShape::SmoothRandom => {
            let a = splitmix(seed, cycle);
            let b = splitmix(seed, cycle.wrapping_add(1));
            let s = p * p * (3.0 - 2.0 * p);
            a + (b - a) * s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_hit_their_landmarks() {
        let at = |s, p| shape_value(s, p, 0, 1);
        assert!(at(LfoShape::Sine, 0.0).abs() < 1e-12);
        assert!((at(LfoShape::Sine, 0.25) - 1.0).abs() < 1e-12);
        assert_eq!(at(LfoShape::Triangle, 0.25), 1.0);
        assert_eq!(at(LfoShape::Triangle, 0.5), 0.0);
        assert_eq!(at(LfoShape::Triangle, 0.75), -1.0);
        assert_eq!(at(LfoShape::Saw, 0.0), -1.0);
        assert_eq!(at(LfoShape::Square, 0.49), 1.0);
        assert_eq!(at(LfoShape::Square, 0.5), -1.0);
    }

    #[test]
    fn free_run_has_the_right_period_at_any_rate() {
        for fs in [8_000.0, 44_100.0, 96_000.0, 384_000.0] {
            let mut lfo = Lfo::new(fs);
            lfo.set_rate(LfoRate::Hz(2.0));
            let n = (fs / 2.0) as usize; // one cycle
            lfo.advance(n);
            assert!(lfo.phase().min(1.0 - lfo.phase()) < 1e-6, "fs {fs}");
        }
    }

    #[test]
    fn random_shapes_are_bounded_and_smooth_is_continuous() {
        let mut lfo = Lfo::new(1000.0);
        lfo.set_rate(LfoRate::Hz(10.0));
        lfo.set_shape(LfoShape::SmoothRandom);
        let mut prev = lfo.next();
        let mut max_step: f32 = 0.0;
        for _ in 0..20_000 {
            let v = lfo.next();
            assert!((-1.0..=1.0).contains(&v));
            max_step = max_step.max((v - prev).abs());
            prev = v;
        }
        assert!(max_step < 0.1, "{max_step}");
    }
}
