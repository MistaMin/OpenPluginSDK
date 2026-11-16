//! Sample playback and a fixed voice list.
//!
//! `sampler` (MIT OR Apache-2.0) plays a preloaded slice with a fractional
//! playhead. `instrument` (MIT) keeps a fixed voice array and steals the
//! oldest note. Attack and release are counted in samples, not wall time.
//! The audio thread does not allocate.
//!
//! Per-note expression: [`NoteExpressions`] holds one voice's expression values
//! ([`Expression`]: volume, pan, tuning, vibrato, expression, brightness,
//! pressure) and [`MOD_SLOTS`] polyphonic modulation amounts, each ramped
//! linearly to a new value over a set number of samples, the way
//! [`crate::voice_alloc`] glides pitch. [`Polyphony`] and
//! [`crate::VoiceAllocator`] remember the [`NoteRef`] (host note id, port,
//! channel, key) of the note each voice plays, and route an expression or a
//! modulation to the voices whose note it names ([`NoteRef::matches`]): by
//! note id when the host gave one, else by port, channel and key with `-1`
//! as a wildcard.

use openplugin_core::{Expression, NoteRef};

/// Polyphonic modulation slots per voice. A plugin maps each modulatable
/// parameter to a slot.
pub const MOD_SLOTS: usize = 8;

const RAMPS: usize = Expression::COUNT + MOD_SLOTS;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Ramp {
    value: f32,
    target: f32,
    step: f32,
    left: u32,
}

impl Ramp {
    const fn at(value: f32) -> Self {
        Self { value, target: value, step: 0.0, left: 0 }
    }

    fn set(&mut self, target: f32, samples: u32) {
        self.target = target;
        if samples == 0 || (target - self.value).abs() <= f32::EPSILON {
            self.value = target;
            self.left = 0;
        } else {
            self.step = (target - self.value) / samples as f32;
            self.left = samples;
        }
    }

    fn advance(&mut self, samples: u32) {
        if self.left == 0 {
            return;
        }
        if samples >= self.left {
            self.left = 0;
            self.value = self.target;
        } else {
            self.left -= samples;
            // Closed form from the target, as the glide does: no drift.
            self.value = self.target - self.step * self.left as f32;
        }
    }
}

/// One voice's per-note expressions and polyphonic modulation amounts.
///
/// A value set before the voice has advanced by a sample (the expression a
/// host sends at the note-on's own sample, or MPE's initial state) is taken
/// at once, so the note starts there (CLAP `events.h`: a note expression at
/// the same sample as the note-on applies to all generated samples). Later
/// values ramp linearly over the ramp length. No allocation; NaN and
/// infinite values are refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteExpressions {
    ramps: [Ramp; RAMPS],
    ramp_samples: u32,
    fresh: bool,
}

impl Default for NoteExpressions {
    fn default() -> Self {
        Self::new()
    }
}

impl NoteExpressions {
    /// Every expression at its neutral value ([`Expression::neutral`]), every
    /// modulation 0, no ramp.
    pub const fn new() -> Self {
        let mut ramps = [Ramp::at(0.0); RAMPS];
        let mut i = 0;
        while i < Expression::COUNT {
            ramps[i] = Ramp::at(Expression::ALL[i].neutral());
            i += 1;
        }
        Self { ramps, ramp_samples: 0, fresh: true }
    }

    /// Back to neutral for a new note; keeps the ramp length.
    pub fn start(&mut self) {
        let samples = self.ramp_samples;
        *self = Self::new();
        self.ramp_samples = samples;
    }

    /// Ramp length for later changes, in samples (0: jump).
    pub fn set_ramp(&mut self, samples: u32) {
        self.ramp_samples = samples;
    }

    pub fn ramp(&self) -> u32 {
        self.ramp_samples
    }

    fn samples(&self) -> u32 {
        if self.fresh { 0 } else { self.ramp_samples }
    }

    /// Move `expression` to `value` (clamped to its range). False, and
    /// nothing changes, for NaN or an infinity.
    pub fn set(&mut self, expression: Expression, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        let (lo, hi) = expression.range();
        let samples = self.samples();
        self.ramps[expression.id()].set(value.clamp(lo, hi), samples);
        true
    }

    /// Move modulation slot `slot` to `amount`. False for a slot past
    /// [`MOD_SLOTS`] or a non-finite amount.
    pub fn set_modulation(&mut self, slot: usize, amount: f32) -> bool {
        if slot >= MOD_SLOTS || !amount.is_finite() {
            return false;
        }
        let samples = self.samples();
        self.ramps[Expression::COUNT + slot].set(amount, samples);
        true
    }

    /// The current (ramped) value.
    pub fn get(&self, expression: Expression) -> f32 {
        self.ramps[expression.id()].value
    }

    /// The value the ramp is heading for.
    pub fn target(&self, expression: Expression) -> f32 {
        self.ramps[expression.id()].target
    }

    /// The current amount of modulation slot `slot`; 0 past [`MOD_SLOTS`].
    pub fn modulation(&self, slot: usize) -> f32 {
        if slot >= MOD_SLOTS { 0.0 } else { self.ramps[Expression::COUNT + slot].value }
    }

    /// Whether any value is still ramping.
    pub fn is_ramping(&self) -> bool {
        self.ramps.iter().any(|r| r.left > 0)
    }

    /// Move every ramp forward by `samples`.
    pub fn advance(&mut self, samples: u32) {
        if samples == 0 {
            return;
        }
        self.fresh = false;
        for ramp in &mut self.ramps {
            ramp.advance(samples);
        }
    }
}

/// One voice over a mono slice the caller already owns.
#[derive(Clone, Copy, Debug)]
pub struct SampleVoice {
    // f64: an f32 playhead drifts out of tune within seconds and stops at 2^24.
    cursor: f64,
    increment: f64,
    gain: f32,
    note: u8,
    active: bool,
}

impl Default for SampleVoice {
    fn default() -> Self {
        Self {
            cursor: 0.0,
            increment: 1.0,
            gain: 1.0,
            note: 0,
            active: false,
        }
    }
}

impl SampleVoice {
    pub const fn new() -> Self {
        Self {
            cursor: 0.0,
            increment: 1.0,
            gain: 1.0,
            note: 0,
            active: false,
        }
    }

    pub const fn is_active(self) -> bool {
        self.active
    }

    /// `ratio` is `note_hz / root_hz`. A non-finite ratio does not start playback.
    pub fn start(&mut self, note: u8, ratio: f32, gain: f32) {
        if !ratio.is_finite() || ratio < 0.0 || !gain.is_finite() {
            return;
        }
        self.cursor = 0.0;
        self.increment = f64::from(ratio);
        self.gain = gain;
        self.note = note;
        self.active = true;
    }

    /// Release this voice when it is playing `note`.
    pub fn stop(&mut self, note: u8) -> bool {
        if self.active && self.note == note {
            self.active = false;
            true
        } else {
            false
        }
    }

    /// Linear interpolation. Returns 0 once the playhead passes the last sample.
    pub fn next(&mut self, pcm: &[f32]) -> f32 {
        if !self.active || pcm.is_empty() {
            return 0.0;
        }
        let index = self.cursor as usize;
        if index >= pcm.len() {
            self.active = false;
            return 0.0;
        }
        let frac = (self.cursor - index as f64) as f32;
        let a = pcm[index];
        let b = pcm.get(index + 1).copied().unwrap_or(a);
        let sample = (a + (b - a) * frac) * self.gain;
        self.cursor += self.increment;
        if self.cursor >= pcm.len() as f64 {
            self.active = false;
        }
        sample
    }
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    note: u8,
    reference: NoteRef,
    active: bool,
    released: bool,
    age: u64,
    pos: u32,
    level: f32,
}

impl Slot {
    const fn idle() -> Self {
        Self {
            note: 0,
            reference: NoteRef::ALL,
            active: false,
            released: false,
            age: 0,
            pos: 0,
            level: 0.0,
        }
    }
}

/// Fixed polyphony. `note_on` reuses a free slot or the oldest one.
/// `N` is chosen when the bank is built, so the audio thread does not grow it.
#[derive(Clone, Debug)]
pub struct Polyphony<const N: usize> {
    slots: [Slot; N],
    attack: u32,
    release: u32,
    clock: u64,
}

impl<const N: usize> Polyphony<N> {
    pub const fn new(attack: u32, release: u32) -> Self {
        Self {
            slots: [Slot::idle(); N],
            attack,
            release,
            clock: 0,
        }
    }

    pub fn note_on(&mut self, note: u8) -> Option<usize> {
        self.note_on_ref(NoteRef::key(note))
    }

    /// [`Self::note_on`] for a note with its id, port and channel, which the
    /// slot remembers for [`Self::matching`] and [`Self::note_off_ref`]. A
    /// key outside 0..=127 starts nothing.
    pub fn note_on_ref(&mut self, reference: NoteRef) -> Option<usize> {
        let Ok(note) = u8::try_from(reference.key) else {
            return None;
        };
        if N == 0 || note > 127 {
            return None;
        }
        self.clock = self.clock.saturating_add(1);
        let index = self
            .slots
            .iter()
            .position(|slot| !slot.active)
            .unwrap_or_else(|| {
                self.slots
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, slot)| slot.age)
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            });
        self.slots[index] = Slot {
            note,
            reference,
            active: true,
            released: false,
            age: self.clock,
            pos: 0,
            level: if self.attack == 0 { 1.0 } else { 0.0 },
        };
        Some(index)
    }

    pub fn note_off(&mut self, note: u8) {
        for slot in &mut self.slots {
            if slot.active && !slot.released && slot.note == note {
                slot.released = true;
                slot.pos = 0;
            }
        }
    }

    /// Release every held slot whose note `target` names ([`NoteRef::matches`]).
    pub fn note_off_ref(&mut self, target: NoteRef) {
        for slot in &mut self.slots {
            if slot.active && !slot.released && target.matches(slot.reference) {
                slot.released = true;
                slot.pos = 0;
            }
        }
    }

    /// The note a sounding slot plays; `None` for an idle slot.
    pub fn note_ref(&self, index: usize) -> Option<NoteRef> {
        self.slots.get(index).filter(|s| s.active).map(|s| s.reference)
    }

    /// Indices of the sounding slots (held or releasing) whose note `target`
    /// names: where to send an expression or a modulation. Allocates nothing.
    pub fn matching(&self, target: NoteRef) -> impl Iterator<Item = usize> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter(move |(_, s)| s.active && target.matches(s.reference))
            .map(|(i, _)| i)
    }

    pub fn note_off_all(&mut self) {
        for slot in &mut self.slots {
            if slot.active && !slot.released {
                slot.released = true;
                slot.pos = 0;
            }
        }
    }

    /// Envelope for one slot, advanced by one sample. `0` when the slot is idle.
    pub fn gain(&mut self, index: usize) -> f32 {
        let Some(slot) = self.slots.get_mut(index) else {
            return 0.0;
        };
        if !slot.active {
            return 0.0;
        }
        if !slot.released {
            if self.attack == 0 || slot.pos >= self.attack {
                slot.level = 1.0;
            } else {
                slot.level = slot.pos as f32 / self.attack as f32;
                slot.pos = slot.pos.saturating_add(1);
            }
            return slot.level;
        }
        if self.release == 0 {
            *slot = Slot::idle();
            return 0.0;
        }
        slot.level = 1.0 - slot.pos as f32 / self.release as f32;
        slot.pos = slot.pos.saturating_add(1);
        if slot.pos >= self.release {
            *slot = Slot::idle();
            0.0
        } else {
            slot.level
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_voice_interpolates_and_stops_on_its_note() {
        let pcm = [0.0, 1.0, 0.0];
        let mut voice = SampleVoice::new();
        voice.start(60, 1.0, 1.0);
        assert!(voice.next(&pcm).abs() < 1e-6);
        let mid = voice.next(&pcm);
        assert!(mid > 0.0 && mid <= 1.0);
        assert!(!voice.stop(61));
        assert!(voice.stop(60));
        assert_eq!(voice.next(&pcm), 0.0);
    }

    #[test]
    fn sample_voice_playhead_keeps_pitch_and_ends_past_two_to_the_24() {
        // One semitone up, 60 s into the sample at 48 kHz: an f32 playhead had
        // rounded the step to exactly 1.0 there (a full semitone flat).
        let ratio = 2f32.powf(1.0 / 12.0);
        let mut voice = SampleVoice::new();
        voice.start(60, ratio, 1.0);
        let start = 60 * 48_000;
        voice.cursor = start as f64;
        let ramp: Vec<f32> = (0..start + 4).map(|n| (n - start) as f32).collect();
        let a = voice.next(&ramp);
        let b = voice.next(&ramp);
        let cents = 1200.0 * f64::from(b - a).log2();
        assert!((cents - 100.0).abs() < 0.01, "{cents} cents");

        // Ratio 1 past 2^24 samples: an f32 playhead stuck at 16_777_216 for good.
        let len = (1usize << 24) + 8;
        let pcm = vec![0.5f32; len];
        voice.start(60, 1.0, 1.0);
        voice.cursor = (1u64 << 24) as f64;
        for _ in 0..8 {
            voice.next(&pcm);
        }
        assert!(!voice.is_active(), "voice reaches the end and stops");
    }

    #[test]
    fn polyphony_routes_by_note_id_and_falls_back_to_channel_and_key() {
        let mut bank = Polyphony::<4>::new(0, 0);
        let a = bank.note_on_ref(NoteRef::new(10, 0, 1, 60)).unwrap();
        let b = bank.note_on_ref(NoteRef::new(11, 0, 2, 60)).unwrap();
        assert_ne!(a, b);
        assert_eq!(bank.matching(NoteRef::new(10, -1, -1, -1)).collect::<Vec<_>>(), vec![a]);
        assert_eq!(bank.matching(NoteRef::new(-1, 0, 2, 60)).collect::<Vec<_>>(), vec![b]);
        assert_eq!(bank.matching(NoteRef::new(99, -1, -1, -1)).count(), 0);
        assert_eq!(bank.matching(NoteRef::ALL).count(), 2);
        bank.note_off_ref(NoteRef::new(11, -1, -1, -1));
        assert_eq!(bank.gain(b), 0.0);
        assert_eq!(bank.gain(a), 1.0);
        assert!(bank.note_on_ref(NoteRef::new(1, 0, 0, -1)).is_none());
        assert_eq!(bank.note_ref(a), Some(NoteRef::new(10, 0, 1, 60)));
    }

    #[test]
    fn expressions_start_neutral_jump_while_fresh_then_ramp() {
        let mut e = NoteExpressions::new();
        e.set_ramp(4);
        assert_eq!(e.get(Expression::Volume), 1.0);
        assert_eq!(e.get(Expression::Pan), 0.5);
        assert!(e.set(Expression::Tuning, 2.0));
        assert_eq!(e.get(Expression::Tuning), 2.0, "before the first sample a value is taken at once");
        e.advance(1);
        assert!(e.set(Expression::Tuning, 6.0));
        assert_eq!(e.get(Expression::Tuning), 2.0);
        e.advance(2);
        assert_eq!(e.get(Expression::Tuning), 4.0);
        e.advance(10);
        assert_eq!(e.get(Expression::Tuning), 6.0);
        assert!(!e.is_ramping());
        assert!(!e.set(Expression::Pressure, f32::NAN));
        assert!(e.set(Expression::Pressure, 9.0));
        assert_eq!(e.target(Expression::Pressure), 1.0);
        assert!(!e.set_modulation(MOD_SLOTS, 1.0));
        assert!(e.set_modulation(0, -3.0));
        e.advance(4);
        assert_eq!(e.modulation(0), -3.0);
        e.start();
        assert_eq!((e.get(Expression::Tuning), e.modulation(0), e.ramp()), (0.0, 0.0, 4));
    }

    #[test]
    fn polyphony_steals_the_oldest_note_and_releases() {
        let mut bank = Polyphony::<2>::new(0, 2);
        let a = bank.note_on(60).unwrap();
        let _b = bank.note_on(64).unwrap();
        let stolen = bank.note_on(67).unwrap();
        assert_eq!(stolen, a);
        assert_eq!(bank.gain(stolen), 1.0);
        bank.note_off(67);
        assert!(bank.gain(stolen) > 0.0);
        let _ = bank.gain(stolen);
        assert_eq!(bank.gain(stolen), 0.0);
    }
}
