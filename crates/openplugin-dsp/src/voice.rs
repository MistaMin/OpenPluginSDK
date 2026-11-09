//! Sample playback and a fixed voice list.
//!
//! `sampler` (MIT OR Apache-2.0) plays a preloaded slice with a fractional
//! playhead. `instrument` (MIT) keeps a fixed voice array and steals the
//! oldest note. Attack and release are counted in samples, not wall time.
//! The audio thread does not allocate.

/// One voice over a mono slice the caller already owns.
#[derive(Clone, Copy, Debug)]
pub struct SampleVoice {
    cursor: f32,
    increment: f32,
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
        self.increment = ratio;
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
        let frac = self.cursor - index as f32;
        let a = pcm[index];
        let b = pcm.get(index + 1).copied().unwrap_or(a);
        let sample = (a + (b - a) * frac) * self.gain;
        self.cursor += self.increment;
        if self.cursor >= pcm.len() as f32 {
            self.active = false;
        }
        sample
    }
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    note: u8,
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
        if N == 0 {
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
        assert!(voice.stop(61) == false);
        assert!(voice.stop(60));
        assert_eq!(voice.next(&pcm), 0.0);
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
