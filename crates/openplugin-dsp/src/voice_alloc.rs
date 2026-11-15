//! Voice allocator: `N` voices, note stealing, release tails, glide, mono and legato.
//!
//! The allocator decides *which voice plays which note*; it does not make
//! sound. Each voice carries a user state `S` (oscillators, envelope, filter,
//! whatever the instrument needs) and the allocator hands back an
//! [`Allocation`] saying which voice to start and whether its envelope should
//! restart. The structure follows common synthesiser practice (for the
//! mono/legato last-note-priority note stack, see the MIDI 1.0 specification's
//! description of mono mode, and Pirkle, *Designing Software Synthesizer Plug-Ins
//! in C++*, Focal Press 2015, ch. 7, on voice management); the code is written
//! from that behaviour.
//!
//! # Rules
//!
//! 1. A note-on for a note that is already *held* retriggers the voice that
//!    holds it. At most one held voice per note.
//! 2. With [`StealPolicy::SameNote`], a *released* voice still playing the same
//!    note (its tail) is reused as well.
//! 3. Otherwise a free voice is used (the lowest index).
//! 4. Otherwise a voice is stolen. Voices in their **release tail** go first,
//!    since they are on the way out; among them, and among held voices if there
//!    is no tail, the policy picks:
//!    * [`StealPolicy::Oldest`]: the voice that started first;
//!    * [`StealPolicy::Quietest`]: the lowest level set with
//!      [`VoiceAllocator::set_level`] (call it from the voice's envelope each
//!      block; ties go to the oldest);
//!    * [`StealPolicy::SameNote`]: reuse as in 1-2, else the oldest.
//!
//! The caller frees a voice with [`VoiceAllocator::free`] when its release has
//! ended (envelope idle).
//!
//! # Mono and legato
//!
//! [`VoiceMode::Mono`] and [`VoiceMode::Legato`] use voice 0 only and a stack of
//! held notes with last-note priority: releasing the sounding key while an older
//! one is still down returns to the older note. Mono retriggers the envelope on
//! each note change; Legato does not while a key is held (the pitch just glides).
//!
//! # Glide
//!
//! With glide on, a voice's pitch moves in a straight line in semitones from
//! the previous pitch to the new note, taking exactly `glide_time * sample_rate`
//! samples (rounded), whatever the interval. Read it each sample with
//! [`Voice::pitch`] after [`Voice::advance`], or advance by a block.
//!
//! # Real time
//!
//! No allocation after construction, no panics for any byte value, and
//! velocity or time arguments that are NaN are sanitised.

use crate::pitch::hz_from_midi;

/// Which voice to take when all are busy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StealPolicy {
    Oldest,
    Quietest,
    SameNote,
}

/// Polyphonic, or monophonic with or without envelope retrigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceMode {
    Poly,
    Mono,
    Legato,
}

/// When glide applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlideMode {
    Off,
    /// Every new note glides from the previous pitch.
    Always,
    /// Only a note played while another key is still down glides.
    LegatoOnly,
}

/// Life cycle of a voice slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoicePhase {
    Free,
    Held,
    /// Key up (or stolen into release); the tail is still sounding.
    Released,
}

/// One voice slot with the user's state `S`.
#[derive(Clone, Debug)]
pub struct Voice<S> {
    /// The instrument's per-voice state. The allocator never touches it after construction.
    pub state: S,
    phase: VoicePhase,
    note: u8,
    velocity: f32,
    level: f32,
    age: u64,
    sustained: bool,
    pitch: f32,
    target: f32,
    glide_step: f32,
    glide_left: u32,
}

impl<S> Voice<S> {
    fn new(state: S) -> Self {
        Self {
            state,
            phase: VoicePhase::Free,
            note: 0,
            velocity: 0.0,
            level: 0.0,
            age: 0,
            sustained: false,
            pitch: 0.0,
            target: 0.0,
            glide_step: 0.0,
            glide_left: 0,
        }
    }

    pub fn phase(&self) -> VoicePhase {
        self.phase
    }

    pub fn is_free(&self) -> bool {
        self.phase == VoicePhase::Free
    }

    /// MIDI note that started this voice (the key, not the glided pitch).
    pub fn note(&self) -> u8 {
        self.note
    }

    /// Velocity 0..1.
    pub fn velocity(&self) -> f32 {
        self.velocity
    }

    /// Current pitch as a (fractional) MIDI note, after glide.
    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    /// Current pitch in Hz.
    pub fn hz(&self) -> f32 {
        hz_from_midi(self.pitch)
    }

    /// Whether the pitch is still gliding.
    pub fn is_gliding(&self) -> bool {
        self.glide_left > 0
    }

    /// Move the glide forward by `samples`; lands exactly on the note.
    pub fn advance(&mut self, samples: u32) {
        if self.glide_left == 0 {
            return;
        }
        if samples >= self.glide_left {
            self.glide_left = 0;
            self.pitch = self.target;
        } else {
            self.glide_left -= samples;
            // Closed form from the target: no drift from repeated addition.
            self.pitch = self.target - self.glide_step * self.glide_left as f32;
        }
    }

    fn start(&mut self, note: u8, velocity: f32, age: u64, from: Option<f32>, glide_samples: u32) {
        self.phase = VoicePhase::Held;
        self.note = note;
        self.velocity = velocity;
        self.level = velocity;
        self.age = age;
        self.sustained = false;
        self.retarget(note, from, glide_samples);
    }

    fn retarget(&mut self, note: u8, from: Option<f32>, glide_samples: u32) {
        let target = f32::from(note);
        self.target = target;
        match from {
            Some(f) if glide_samples > 0 && (f - target).abs() > 1e-6 => {
                self.pitch = f;
                self.glide_left = glide_samples;
                self.glide_step = (target - f) / glide_samples as f32;
            }
            _ => {
                self.pitch = target;
                self.glide_left = 0;
                self.glide_step = 0.0;
            }
        }
    }
}

/// What to do after a `note_on` or a mono return-to-previous-note.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Allocation {
    /// The voice to play the note.
    pub voice: usize,
    /// Restart the envelope (and, usually, the oscillator phase). False for a legato change.
    pub retrigger: bool,
    /// The note that was cut off to make room, if a sounding voice was stolen.
    pub stolen: Option<u8>,
}

/// Result of a `note_off`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct NoteOff {
    /// The voice to send into its release stage now.
    pub released: Option<usize>,
    /// Mono modes: an older held key takes over (the voice stays held).
    pub resumed: Option<usize>,
}

const MAX_HELD: usize = 32;

/// `N` voices with state `S`. See the module documentation.
#[derive(Clone, Debug)]
pub struct VoiceAllocator<S, const N: usize> {
    voices: [Voice<S>; N],
    mode: VoiceMode,
    policy: StealPolicy,
    glide_mode: GlideMode,
    glide_seconds: f32,
    sample_rate: f32,
    limit: usize,
    clock: u64,
    last_pitch: Option<f32>,
    held: [u8; MAX_HELD],
    held_len: usize,
    pedal: bool,
}

impl<S: Default, const N: usize> Default for VoiceAllocator<S, N> {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

impl<S: Default, const N: usize> VoiceAllocator<S, N> {
    /// All voices free, `S::default()` in each, polyphonic, oldest-first stealing.
    pub fn new(sample_rate: f32) -> Self {
        Self::with_state(sample_rate, |_| S::default())
    }
}

impl<S, const N: usize> VoiceAllocator<S, N> {
    /// Like [`VoiceAllocator::new`] with a state built per voice index.
    pub fn with_state(sample_rate: f32, mut make: impl FnMut(usize) -> S) -> Self {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 };
        Self {
            voices: std::array::from_fn(|i| Voice::new(make(i))),
            mode: VoiceMode::Poly,
            policy: StealPolicy::Oldest,
            glide_mode: GlideMode::Off,
            glide_seconds: 0.0,
            sample_rate: sr,
            limit: N,
            clock: 0,
            last_pitch: None,
            held: [0; MAX_HELD],
            held_len: 0,
            pedal: false,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if sample_rate.is_finite() && sample_rate > 0.0 {
            self.sample_rate = sample_rate;
        }
    }

    pub fn set_policy(&mut self, policy: StealPolicy) {
        self.policy = policy;
    }

    pub fn policy(&self) -> StealPolicy {
        self.policy
    }

    /// Changing mode silences the held-note stack; call `all_notes_off` first if voices sound.
    pub fn set_mode(&mut self, mode: VoiceMode) {
        if mode != self.mode {
            self.mode = mode;
            self.held_len = 0;
        }
    }

    pub fn mode(&self) -> VoiceMode {
        self.mode
    }

    /// Glide time in seconds (0 to 60) and when it applies.
    pub fn set_glide(&mut self, seconds: f32, mode: GlideMode) {
        self.glide_seconds = if seconds.is_finite() { seconds.clamp(0.0, 60.0) } else { 0.0 };
        self.glide_mode = mode;
    }

    /// Use only the first `n` voices for new notes (1..=N).
    pub fn set_max_voices(&mut self, n: usize) {
        self.limit = n.clamp(1, N.max(1));
    }

    pub fn voice(&self, i: usize) -> Option<&Voice<S>> {
        self.voices.get(i)
    }

    pub fn voice_mut(&mut self, i: usize) -> Option<&mut Voice<S>> {
        self.voices.get_mut(i)
    }

    pub fn voices(&self) -> &[Voice<S>; N] {
        &self.voices
    }

    pub fn voices_mut(&mut self) -> &mut [Voice<S>; N] {
        &mut self.voices
    }

    /// Number of voices that are not free.
    pub fn active(&self) -> usize {
        self.voices.iter().filter(|v| !v.is_free()).count()
    }

    /// Record the voice's current loudness (envelope level times velocity) for [`StealPolicy::Quietest`].
    pub fn set_level(&mut self, voice: usize, level: f32) {
        if let Some(v) = self.voices.get_mut(voice) {
            v.level = if level.is_finite() { level.max(0.0) } else { 0.0 };
        }
    }

    /// Mark a voice free (its release has ended).
    pub fn free(&mut self, voice: usize) {
        if let Some(v) = self.voices.get_mut(voice) {
            v.phase = VoicePhase::Free;
            v.sustained = false;
            v.level = 0.0;
        }
    }

    fn glide_samples(&self) -> u32 {
        (f64::from(self.glide_seconds) * f64::from(self.sample_rate)).round() as u32
    }

    fn top_held(&self) -> Option<u8> {
        self.held_len.checked_sub(1).map(|i| self.held[i])
    }

    fn push_held(&mut self, note: u8) {
        self.remove_held(note);
        if self.held_len == MAX_HELD {
            // Drop the oldest entry.
            self.held.copy_within(1.., 0);
            self.held_len -= 1;
        }
        self.held[self.held_len] = note;
        self.held_len += 1;
    }

    fn remove_held(&mut self, note: u8) {
        if let Some(at) = self.held[..self.held_len].iter().position(|n| *n == note) {
            self.held.copy_within(at + 1..self.held_len, at);
            self.held_len -= 1;
        }
    }

    /// Start `note` at MIDI `velocity` (0..=127; 0 counts as a note-off and returns `None`).
    pub fn note_on(&mut self, note: u8, velocity: u8) -> Option<Allocation> {
        if N == 0 {
            return None;
        }
        if velocity == 0 {
            self.note_off(note);
            return None;
        }
        let vel = f32::from(velocity.min(127)) / 127.0;
        self.clock = self.clock.wrapping_add(1);
        let age = self.clock;
        let glide = self.glide_samples();

        if self.mode != VoiceMode::Poly {
            return Some(self.mono_on(note, vel, age, glide));
        }

        let limit = self.limit.min(N);
        let overlapped = self.voices[..limit].iter().any(|v| v.phase == VoicePhase::Held);
        let from_glide = |this: &Self, own: Option<f32>| -> Option<f32> {
            match this.glide_mode {
                GlideMode::Off => None,
                GlideMode::Always => own.or(this.last_pitch),
                GlideMode::LegatoOnly => {
                    if overlapped {
                        own.or(this.last_pitch)
                    } else {
                        None
                    }
                }
            }
        };

        // Rule 1: the same note already held.
        let mut index = self.voices.iter().position(|v| v.phase == VoicePhase::Held && v.note == note);
        let mut stolen = None;
        // Rule 2: same-note tail.
        if index.is_none() && self.policy == StealPolicy::SameNote {
            index = self.voices.iter().position(|v| v.phase == VoicePhase::Released && v.note == note);
        }
        // Rule 3: a free voice.
        if index.is_none() {
            index = self.voices[..limit].iter().position(|v| v.is_free());
        }
        // Rule 4: steal.
        if index.is_none() {
            let i = self.pick_victim(limit);
            stolen = (self.voices[i].phase == VoicePhase::Held).then_some(self.voices[i].note);
            index = Some(i);
        }
        let i = index.unwrap_or(0);
        let from = from_glide(self, None);
        self.voices[i].start(note, vel, age, from, glide);
        self.last_pitch = Some(f32::from(note));
        Some(Allocation { voice: i, retrigger: true, stolen })
    }

    fn pick_victim(&self, limit: usize) -> usize {
        let pool = &self.voices[..limit];
        let any_released = pool.iter().any(|v| v.phase == VoicePhase::Released);
        let eligible = |v: &Voice<S>| -> bool {
            if any_released { v.phase == VoicePhase::Released } else { v.phase == VoicePhase::Held }
        };
        let mut best: Option<usize> = None;
        for (i, v) in pool.iter().enumerate() {
            if !eligible(v) {
                continue;
            }
            best = match best {
                None => Some(i),
                Some(b) => {
                    let w = &pool[b];
                    let better = match self.policy {
                        StealPolicy::Quietest => v.level < w.level || (v.level == w.level && v.age < w.age),
                        _ => v.age < w.age,
                    };
                    if better { Some(i) } else { Some(b) }
                }
            };
        }
        best.unwrap_or(0)
    }

    fn mono_on(&mut self, note: u8, vel: f32, age: u64, glide: u32) -> Allocation {
        let was_held = self.held_len > 0;
        self.push_held(note);
        let v = &self.voices[0];
        let sounding = v.phase != VoicePhase::Free;
        let own = if sounding { Some(v.pitch) } else { None };
        let from = match self.glide_mode {
            GlideMode::Off => None,
            GlideMode::Always => own.or(self.last_pitch),
            GlideMode::LegatoOnly => {
                if was_held { own.or(self.last_pitch) } else { None }
            }
        };
        let legato_change = self.mode == VoiceMode::Legato && v.phase == VoicePhase::Held;
        let stolen = (v.phase == VoicePhase::Held && !legato_change).then_some(v.note);
        let voice = &mut self.voices[0];
        if legato_change {
            voice.note = note;
            voice.velocity = vel;
            voice.age = age;
            voice.retarget(note, from, glide);
        } else {
            voice.start(note, vel, age, from, glide);
        }
        self.last_pitch = Some(f32::from(note));
        Allocation { voice: 0, retrigger: !legato_change, stolen: if stolen == Some(note) { None } else { stolen } }
    }

    /// Key up. Poly: the held voice for `note` is released (or flagged, with the
    /// sustain pedal down). Mono modes: see [`NoteOff`].
    pub fn note_off(&mut self, note: u8) -> NoteOff {
        if self.mode != VoiceMode::Poly {
            return self.mono_off(note);
        }
        let pedal = self.pedal;
        for (i, v) in self.voices.iter_mut().enumerate() {
            if v.phase == VoicePhase::Held && v.note == note && !v.sustained {
                if pedal {
                    v.sustained = true;
                    return NoteOff::default();
                }
                v.phase = VoicePhase::Released;
                return NoteOff { released: Some(i), resumed: None };
            }
        }
        NoteOff::default()
    }

    fn mono_off(&mut self, note: u8) -> NoteOff {
        let was_top = self.top_held() == Some(note);
        self.remove_held(note);
        if !was_top {
            return NoteOff::default();
        }
        if let Some(prev) = self.top_held() {
            // Return to the previous key.
            let glide = self.glide_samples();
            let v = &mut self.voices[0];
            let from = match self.glide_mode {
                GlideMode::Off => None,
                _ => Some(v.pitch),
            };
            if self.mode == VoiceMode::Mono {
                self.clock = self.clock.wrapping_add(1);
                let age = self.clock;
                let vel = v.velocity;
                v.start(prev, vel, age, from, glide);
            } else {
                v.note = prev;
                v.retarget(prev, from, glide);
            }
            self.last_pitch = Some(f32::from(prev));
            return NoteOff { released: None, resumed: Some(0) };
        }
        let v = &mut self.voices[0];
        if v.phase != VoicePhase::Held {
            return NoteOff::default();
        }
        if self.pedal {
            v.sustained = true;
            return NoteOff::default();
        }
        v.phase = VoicePhase::Released;
        NoteOff { released: Some(0), resumed: None }
    }

    /// Sustain pedal (CC 64). Lifting it releases every voice whose key is up;
    /// `on_release` is called with each voice index.
    pub fn set_sustain_pedal(&mut self, down: bool, mut on_release: impl FnMut(usize)) {
        self.pedal = down;
        if down {
            return;
        }
        for (i, v) in self.voices.iter_mut().enumerate() {
            if v.sustained && v.phase == VoicePhase::Held {
                v.sustained = false;
                v.phase = VoicePhase::Released;
                on_release(i);
            }
        }
    }

    /// Release every held voice (MIDI "all notes off"); `on_release` gets each index.
    pub fn all_notes_off(&mut self, mut on_release: impl FnMut(usize)) {
        self.held_len = 0;
        self.pedal = false;
        for (i, v) in self.voices.iter_mut().enumerate() {
            if v.phase == VoicePhase::Held {
                v.sustained = false;
                v.phase = VoicePhase::Released;
                on_release(i);
            }
        }
    }

    /// Free every voice at once and forget held keys (transport reset).
    pub fn reset(&mut self) {
        for v in &mut self.voices {
            v.phase = VoicePhase::Free;
            v.sustained = false;
            v.level = 0.0;
            v.glide_left = 0;
        }
        self.held_len = 0;
        self.pedal = false;
        self.last_pitch = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Alloc4 = VoiceAllocator<u32, 4>;

    fn fill(a: &mut Alloc4) {
        for n in [60, 62, 64, 65] {
            a.note_on(n, 100).unwrap();
        }
    }

    #[test]
    fn free_voices_go_first_in_index_order_and_note_off_releases() {
        let mut a = Alloc4::new(48_000.0);
        for (k, n) in [60u8, 62, 64].into_iter().enumerate() {
            let r = a.note_on(n, 100).unwrap();
            assert_eq!((r.voice, r.stolen, r.retrigger), (k, None, true));
        }
        assert_eq!(a.active(), 3);
        let off = a.note_off(62);
        assert_eq!(off.released, Some(1));
        assert_eq!(a.voice(1).unwrap().phase(), VoicePhase::Released);
        a.free(1);
        assert_eq!(a.note_on(70, 90).unwrap().voice, 1);
        assert_eq!(a.note_off(99), NoteOff::default());
    }

    #[test]
    fn oldest_policy_steals_the_oldest_held_voice() {
        let mut a = Alloc4::new(48_000.0);
        fill(&mut a);
        let r = a.note_on(72, 100).unwrap();
        assert_eq!((r.voice, r.stolen), (0, Some(60)));
        let r = a.note_on(73, 100).unwrap();
        assert_eq!((r.voice, r.stolen), (1, Some(62)));
    }

    #[test]
    fn quietest_policy_steals_the_lowest_level() {
        let mut a = Alloc4::new(48_000.0);
        a.set_policy(StealPolicy::Quietest);
        fill(&mut a);
        for (i, l) in [0.8, 0.6, 0.1, 0.5].into_iter().enumerate() {
            a.set_level(i, l);
        }
        let r = a.note_on(72, 100).unwrap();
        assert_eq!((r.voice, r.stolen), (2, Some(64)));
        // A tie on level goes to the older voice.
        for i in 0..4 {
            a.set_level(i, 0.3);
        }
        let r = a.note_on(73, 100).unwrap();
        assert_eq!(r.voice, 0);
    }

    #[test]
    fn same_note_policy_reuses_the_voice_playing_that_note() {
        let mut a = Alloc4::new(48_000.0);
        a.set_policy(StealPolicy::SameNote);
        fill(&mut a);
        // Repeated held note: same voice, no steal.
        let r = a.note_on(64, 80).unwrap();
        assert_eq!((r.voice, r.stolen, r.retrigger), (2, None, true));
        // A released tail of the same note is reused over a free/steal choice.
        a.note_off(65);
        let r = a.note_on(65, 80).unwrap();
        assert_eq!((r.voice, r.stolen), (3, None));
        // No same note: falls back to oldest.
        let r = a.note_on(90, 80).unwrap();
        assert_eq!((r.voice, r.stolen), (0, Some(60)));
    }

    #[test]
    fn same_held_note_retriggers_under_every_policy() {
        for p in [StealPolicy::Oldest, StealPolicy::Quietest, StealPolicy::SameNote] {
            let mut a = Alloc4::new(48_000.0);
            a.set_policy(p);
            fill(&mut a);
            let r = a.note_on(62, 100).unwrap();
            assert_eq!(r.voice, 1);
            assert_eq!(a.active(), 4);
        }
    }

    #[test]
    fn release_tails_are_stolen_before_held_voices() {
        for p in [StealPolicy::Oldest, StealPolicy::Quietest, StealPolicy::SameNote] {
            let mut a = Alloc4::new(48_000.0);
            a.set_policy(p);
            fill(&mut a);
            a.note_off(64); // voice 2 now a tail, younger than voice 0
            let r = a.note_on(80, 100).unwrap();
            assert_eq!((r.voice, r.stolen), (2, None), "{p:?}");
        }
    }

    #[test]
    fn sustain_pedal_defers_release() {
        let mut a = Alloc4::new(48_000.0);
        a.note_on(60, 100);
        a.set_sustain_pedal(true, |_| panic!());
        assert_eq!(a.note_off(60), NoteOff::default());
        assert_eq!(a.voice(0).unwrap().phase(), VoicePhase::Held);
        let mut got = Vec::new();
        a.set_sustain_pedal(false, |i| got.push(i));
        assert_eq!(got, vec![0]);
        assert_eq!(a.voice(0).unwrap().phase(), VoicePhase::Released);
    }

    #[test]
    fn mono_uses_last_note_priority_and_returns_to_the_older_key() {
        let mut a = Alloc4::new(48_000.0);
        a.set_mode(VoiceMode::Mono);
        let r = a.note_on(60, 100).unwrap();
        assert_eq!((r.voice, r.retrigger), (0, true));
        let r = a.note_on(64, 100).unwrap();
        assert_eq!((r.voice, r.retrigger, r.stolen), (0, true, Some(60)));
        assert_eq!(a.voice(0).unwrap().note(), 64);
        let off = a.note_off(64);
        assert_eq!((off.released, off.resumed), (None, Some(0)));
        assert_eq!(a.voice(0).unwrap().note(), 60);
        assert_eq!(a.note_off(60).released, Some(0));
        assert_eq!(a.active(), 1);
        assert_eq!(a.voice(1).unwrap().phase(), VoicePhase::Free);
    }

    #[test]
    fn legato_changes_pitch_without_retrigger_while_a_key_is_held() {
        let mut a = Alloc4::new(48_000.0);
        a.set_mode(VoiceMode::Legato);
        assert!(a.note_on(60, 100).unwrap().retrigger);
        let r = a.note_on(67, 100).unwrap();
        assert!(!r.retrigger);
        assert_eq!(a.voice(0).unwrap().pitch(), 67.0);
        a.note_off(67);
        assert_eq!(a.voice(0).unwrap().note(), 60);
        a.note_off(60);
        assert_eq!(a.voice(0).unwrap().phase(), VoicePhase::Released);
        assert!(a.note_on(62, 100).unwrap().retrigger, "new phrase after release retriggers");
    }

    #[test]
    fn glide_takes_exactly_the_set_time_and_lands_on_the_note() {
        let mut a = Alloc4::new(48_000.0);
        a.set_mode(VoiceMode::Legato);
        a.set_glide(0.1, GlideMode::Always);
        a.note_on(60, 100);
        assert_eq!(a.voice(0).unwrap().pitch(), 60.0);
        a.note_on(72, 100);
        let n = 4800;
        let v = a.voice_mut(0).unwrap();
        assert!(v.is_gliding());
        let mut last = v.pitch();
        for k in 1..=n {
            v.advance(1);
            assert!(v.pitch() >= last);
            last = v.pitch();
            if k < n {
                assert!(v.is_gliding() && v.pitch() < 72.0, "ended early at {k}");
            }
        }
        assert!(!v.is_gliding());
        assert_eq!(v.pitch(), 72.0);
        // Block advance matches.
        a.note_on(60, 100);
        let v = a.voice_mut(0).unwrap();
        v.advance(2400);
        assert!((v.pitch() - 66.0).abs() < 1e-3);
        v.advance(10_000);
        assert_eq!(v.pitch(), 60.0);
    }

    #[test]
    fn legato_only_glide_skips_the_first_note_of_a_phrase() {
        let mut a = Alloc4::new(48_000.0);
        a.set_mode(VoiceMode::Legato);
        a.set_glide(0.05, GlideMode::LegatoOnly);
        a.note_on(60, 100);
        a.note_off(60);
        a.free(0);
        a.note_on(72, 100);
        assert!(!a.voice(0).unwrap().is_gliding());
        a.note_on(79, 100);
        assert!(a.voice(0).unwrap().is_gliding());
    }

    #[test]
    fn velocity_zero_is_note_off_and_hostile_input_is_safe() {
        let mut a = Alloc4::new(f32::NAN);
        a.set_glide(f32::NAN, GlideMode::Always);
        a.note_on(60, 100);
        assert!(a.note_on(60, 0).is_none());
        assert_eq!(a.voice(0).unwrap().phase(), VoicePhase::Released);
        a.set_level(0, f32::NAN);
        a.set_level(99, 1.0);
        a.free(99);
        for n in 0..=255u8 {
            a.note_on(n, 255);
            a.note_off(n.wrapping_mul(3));
        }
        a.all_notes_off(|_| {});
        a.reset();
        assert_eq!(a.active(), 0);
        let mut z = VoiceAllocator::<(), 0>::new(48_000.0);
        assert!(z.note_on(60, 100).is_none());
        z.all_notes_off(|_| {});
    }

    #[test]
    fn per_voice_state_is_generic_and_untouched() {
        let mut a = VoiceAllocator::<Vec<f32>, 2>::with_state(48_000.0, |i| vec![i as f32; 3]);
        a.note_on(60, 100);
        a.voice_mut(0).unwrap().state.push(9.0);
        assert_eq!(a.voice(0).unwrap().state.len(), 4);
        assert_eq!(a.voice(1).unwrap().state, vec![1.0; 3]);
    }
}
