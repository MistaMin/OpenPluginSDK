//! Expression Synth: a small polyphonic synth that plays per-note expression
//! in every format (roadmap 0.7 task 4).
//!
//! Eight voices from [`VoiceAllocator`], each a sine with a second and a
//! third harmonic. Every voice reads its own expressions, so a host can bend,
//! press, brighten, pan or swell one note of a chord:
//!
//! | Expression | What the voice does |
//! |---|---|
//! | Tuning | pitch, in semitones ([`Voice::pitch`] includes it) |
//! | Pressure | level: `0.25 + 0.75 * pressure` |
//! | Brightness | second harmonic, `brightness` times the fundamental |
//! | Expression | third harmonic, `0.5 * expression` |
//! | Volume | gain (`0..4`, 1 is unity) |
//! | Pan | equal-power pan |
//! | Vibrato | 5.5 Hz vibrato, up to ±0.5 semitone |
//!
//! The `tone` parameter adds to the second harmonic and is modulatable per
//! note ([`ParamFlags::POLY_MODULATION`], CLAP polyphonic modulation), into
//! modulation slot 0 of the voice it names. MPE over MIDI is on, starting
//! with the Lower Zone of 15 member channels; CLAP note expressions and VST3
//! note expressions address voices by note id.
//!
//! It describes its voices to a CLAP host (`voice-info`) and tells it when
//! each one ends (`CLAP_EVENT_NOTE_END`), from the allocator's ended list.
//!
//! The editor state is a [`VoiceView`]: what each voice plays, published
//! after every block with atomics (no lock, no allocation). The format
//! proofs in `tests/` read it; an editor could draw it.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use openplugin::core::{
    Category, EditorState, Expression, ExpressionSet, IoConfig, MpeZones, NotePorts, NoteRef, ParamFlags, ParamInfo,
    Plugin, PluginInfo, SpeakerLayout, Unit, VoiceInfo,
};
use openplugin::dsp::{Voice, VoiceAllocator, VoicePhase};

/// Voices.
pub const VOICES: usize = 8;
/// Parameter indices.
pub const GAIN: usize = 0;
pub const TONE: usize = 1;

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("gain", "Gain", -60.0, 0.0, -6.0, Unit::Db),
    ParamInfo::new("tone", "Tone", 0.0, 1.0, 0.0, Unit::None)
        .with_flags(ParamFlags::AUTOMATABLE.union(ParamFlags::POLY_MODULATION)),
];

static LAYOUTS: [IoConfig; 1] = [IoConfig { input: SpeakerLayout::Absent, output: SpeakerLayout::Stereo, sidechain: None }];

pub static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.example.expression-synth",
    name: "Expression Synth",
    vendor: "OpenGrid",
    url: "https://github.com/MistaMin/OpenPluginSDK",
    email: "",
    version: "1.0.0",
    category: Category::Instrument,
    params: &PARAMS,
    is_instrument: true,
    has_editor: false,
};

/// What one voice is playing, as the last block left it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VoiceSnapshot {
    pub active: bool,
    pub id: i32,
    pub channel: i16,
    pub key: i16,
    /// Pitch in semitones (MIDI note plus tuning).
    pub pitch: f32,
    pub pressure: f32,
    pub brightness: f32,
    pub volume: f32,
    pub pan: f32,
    /// Polyphonic modulation of `tone`.
    pub tone_mod: f32,
}

const FIELDS: usize = 10;

/// Every voice's [`VoiceSnapshot`], shared with the editor. The audio thread
/// stores, any thread loads; each field is one atomic.
pub struct VoiceView {
    cells: [[AtomicU32; FIELDS]; VOICES],
    /// Blocks published so far.
    blocks: AtomicU32,
}

impl Default for VoiceView {
    fn default() -> Self {
        Self { cells: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU32::new(0))), blocks: AtomicU32::new(0) }
    }
}

impl VoiceView {
    fn store(&self, index: usize, s: &VoiceSnapshot) {
        let values = [
            u32::from(s.active),
            s.id as u32,
            s.channel as i32 as u32,
            s.key as i32 as u32,
            s.pitch.to_bits(),
            s.pressure.to_bits(),
            s.brightness.to_bits(),
            s.volume.to_bits(),
            s.pan.to_bits(),
            s.tone_mod.to_bits(),
        ];
        for (cell, value) in self.cells[index].iter().zip(values) {
            cell.store(value, Ordering::Relaxed);
        }
    }

    /// Voice `index` (below [`VOICES`]).
    pub fn voice(&self, index: usize) -> VoiceSnapshot {
        let c = |f: usize| self.cells[index][f].load(Ordering::Relaxed);
        VoiceSnapshot {
            active: c(0) != 0,
            id: c(1) as i32,
            channel: c(2) as i32 as i16,
            key: c(3) as i32 as i16,
            pitch: f32::from_bits(c(4)),
            pressure: f32::from_bits(c(5)),
            brightness: f32::from_bits(c(6)),
            volume: f32::from_bits(c(7)),
            pan: f32::from_bits(c(8)),
            tone_mod: f32::from_bits(c(9)),
        }
    }

    /// The sounding voices.
    pub fn active(&self) -> Vec<VoiceSnapshot> {
        (0..VOICES).map(|i| self.voice(i)).filter(|v| v.active).collect()
    }

    /// The sounding voice playing `key` on `channel` (any channel for -1).
    pub fn find(&self, channel: i16, key: i16) -> Option<VoiceSnapshot> {
        self.active().into_iter().find(|v| v.key == key && (channel < 0 || v.channel == channel))
    }

    pub fn blocks(&self) -> u32 {
        self.blocks.load(Ordering::Acquire)
    }
}

/// One voice's oscillator and envelope.
#[derive(Clone, Copy, Debug, Default)]
pub struct Osc {
    phase: f32,
    vibrato: f32,
    env: f32,
}

const ATTACK_S: f32 = 0.002;
const RELEASE_S: f32 = 0.05;
const VIBRATO_HZ: f32 = 5.5;

pub struct ExpressionSynth {
    voices: VoiceAllocator<Osc, VOICES>,
    rate: f32,
    gain: f32,
    tone: f32,
    /// Monophonic modulation of `tone` (CLAP `PARAM_MOD` with every field -1).
    tone_global: f32,
    view: Arc<VoiceView>,
}

impl Default for ExpressionSynth {
    fn default() -> Self {
        Self {
            voices: VoiceAllocator::new(48_000.0),
            rate: 48_000.0,
            gain: db(PARAMS[GAIN].default),
            tone: 0.0,
            tone_global: 0.0,
            view: Arc::new(VoiceView::default()),
        }
    }
}

fn db(value: f32) -> f32 {
    10f32.powf(value / 20.0)
}

impl ExpressionSynth {
    fn publish(&self) {
        for (i, v) in self.voices.voices().iter().enumerate() {
            let r = v.note_ref();
            let snapshot = if v.is_free() {
                VoiceSnapshot::default()
            } else {
                VoiceSnapshot {
                    active: true,
                    id: r.id,
                    channel: r.channel,
                    key: r.key,
                    pitch: v.pitch(),
                    pressure: v.expression(Expression::Pressure),
                    brightness: v.expression(Expression::Brightness),
                    volume: v.expression(Expression::Volume),
                    pan: v.expression(Expression::Pan),
                    tone_mod: v.modulation(0),
                }
            };
            self.view.store(i, &snapshot);
        }
        self.view.blocks.fetch_add(1, Ordering::Release);
    }

    fn sample(&self, v: &Voice<Osc>, tau: f32) -> (f32, f32) {
        let s = &v.state;
        let h2 = (self.tone + self.tone_global + v.modulation(0) + v.expression(Expression::Brightness)).clamp(0.0, 2.0);
        let h3 = 0.5 * v.expression(Expression::Expression);
        let wave = (tau * s.phase).sin() + h2 * (2.0 * tau * s.phase).sin() + h3 * (3.0 * tau * s.phase).sin();
        let level = s.env * v.velocity() * (0.25 + 0.75 * v.expression(Expression::Pressure)) * v.expression(Expression::Volume);
        let angle = v.expression(Expression::Pan) * std::f32::consts::FRAC_PI_2;
        let out = wave * level * self.gain * 0.25;
        (out * angle.cos(), out * angle.sin())
    }
}

impl Plugin for ExpressionSynth {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        if sample_rate.is_finite() && sample_rate > 0.0 {
            self.rate = sample_rate;
            self.voices.set_sample_rate(sample_rate);
        }
    }

    fn reset(&mut self) {
        self.voices.reset();
        self.tone_global = 0.0;
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "gain" => self.gain = db(PARAMS[GAIN].clamp(value)),
            "tone" => self.tone = PARAMS[TONE].clamp(value),
            _ => return false,
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "gain" => Some(20.0 * self.gain.max(1e-9).log10()),
            "tone" => Some(self.tone),
            _ => None,
        }
    }

    /// The release.
    fn tail_samples(&self) -> usize {
        (RELEASE_S * self.rate).ceil() as usize + 1
    }

    fn supported_layouts(&self) -> &'static [IoConfig] {
        &LAYOUTS
    }

    fn note_ports(&self) -> NotePorts {
        NotePorts { input: true, output: false }
    }

    fn note_expressions(&self) -> ExpressionSet {
        ExpressionSet::ALL
    }

    fn mpe_input(&self) -> Option<MpeZones> {
        Some(MpeZones::LOWER_15)
    }

    /// CLAP `voice-info`: eight voices.
    fn voice_info(&self) -> Option<VoiceInfo> {
        Some(self.voices.voice_info())
    }

    /// CLAP `NOTE_END`: each voice whose release has finished, or that was taken for another note.
    fn pull_note_end(&mut self) -> Option<NoteRef> {
        self.voices.pop_ended()
    }

    fn editor_state(&self) -> Option<EditorState> {
        Some(self.view.clone())
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.note_on_ref(offset, NoteRef::key(note), velocity);
    }

    fn note_on_channel(&mut self, offset: usize, channel: u8, note: u8, velocity: u8) {
        self.note_on_ref(offset, NoteRef::midi(channel, note), velocity);
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.note_off_ref(offset, NoteRef::key(note), 0);
    }

    fn note_on_ref(&mut self, _offset: usize, note: NoteRef, velocity: u8) {
        let Some(a) = self.voices.note_on_ref(note, velocity).filter(|a| a.retrigger) else {
            return;
        };
        if let Some(v) = self.voices.voice_mut(a.voice) {
            v.state = Osc::default();
        }
    }

    fn note_off_ref(&mut self, _offset: usize, note: NoteRef, _velocity: u8) {
        // A wildcard may name several notes: release each once.
        for _ in 0..VOICES {
            if self.voices.note_off_ref(note).released.is_none() {
                break;
            }
        }
    }

    fn note_expression(&mut self, _offset: usize, note: NoteRef, expression: Expression, value: f32) {
        self.voices.note_expression(note, expression, value);
    }

    fn param_modulation(&mut self, _offset: usize, index: usize, note: NoteRef, amount: f32) {
        if index != TONE {
            return;
        }
        if note.is_all() {
            self.tone_global = amount;
        } else {
            self.voices.poly_modulation(note, 0, amount);
        }
    }

    fn midi(&mut self, _offset: usize, msg: [u8; 3]) {
        // CC 120 (all sound off) and 123 (all notes off).
        if msg[0] & 0xF0 == 0xB0 && (msg[1] == 120 || msg[1] == 123) {
            self.all_notes_off();
        }
    }

    fn all_notes_off(&mut self) {
        self.voices.all_notes_off(|_| {});
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let tau = std::f32::consts::TAU;
        let attack = 1.0 / (ATTACK_S * self.rate);
        let release = 1.0 / (RELEASE_S * self.rate);
        for n in 0..frames {
            let (mut left, mut right) = (0.0f32, 0.0f32);
            for i in 0..VOICES {
                let Some(v) = self.voices.voice(i) else { continue };
                if v.is_free() {
                    continue;
                }
                let (l, r) = self.sample(v, tau);
                left += l;
                right += r;
                let held = v.phase() == VoicePhase::Held;
                let vibrato = v.expression(Expression::Vibrato) * 0.5 * (tau * v.state.vibrato).sin();
                let hz = openplugin::dsp::hz_from_midi(v.pitch() + vibrato);
                let step = (hz / self.rate).clamp(0.0, 0.5);
                let rate = self.rate;
                let mut done = false;
                if let Some(v) = self.voices.voice_mut(i) {
                    let s = &mut v.state;
                    s.phase = (s.phase + step).fract();
                    s.vibrato = (s.vibrato + VIBRATO_HZ / rate).fract();
                    if held {
                        s.env = (s.env + attack).min(1.0);
                    } else {
                        s.env -= release;
                        done = s.env <= 0.0;
                    }
                    v.advance(1);
                }
                if done {
                    self.voices.free(i);
                }
            }
            if let Some(out) = io.get_mut(0).and_then(|c| c.get_mut(n)) {
                *out = left;
            }
            if let Some(out) = io.get_mut(1).and_then(|c| c.get_mut(n)) {
                *out = right;
            }
        }
        self.publish();
    }
}

openplugin::targets::export_clap!(ExpressionSynth);
openplugin::targets::export_lv2!(ExpressionSynth);
openplugin::targets::export_shell!(ExpressionSynth);
