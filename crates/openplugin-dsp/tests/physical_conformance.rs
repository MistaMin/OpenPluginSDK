//! A tiny plucked-string instrument (notes in, audio out) built from
//! `PluckedString`, run through `openplugin-testing`'s conformance kit.

use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Taper, Unit};
use openplugin_dsp::{hz_from_midi, PluckedString};
use openplugin_testing::{conformance, Options};

openplugin_testing::install_allocator!();

static PARAMS: [ParamInfo; 4] = [
    ParamInfo::new("gain", "Gain", -60.0, 6.0, -9.0, Unit::Db).with_taper(Taper::Linear),
    ParamInfo::new("decay", "Decay", 0.1, 10.0, 2.5, Unit::Seconds),
    ParamInfo::new("bright", "Brightness", 0.0, 0.5, 0.1, Unit::None),
    ParamInfo::new("pick", "Pick position", 0.0, 0.5, 0.15, Unit::None),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.test.pluck",
    name: "Test Pluck",
    vendor: "OpenGrid",
    url: "https://github.com/MistaMin/OpenPluginSDK",
    email: "contact@mistamin.com",
    version: "0.4.0-beta",
    category: Category::Instrument,
    params: &PARAMS,
    is_instrument: true,
    has_editor: false,
};

const VOICES: usize = 6;
const QUEUE: usize = 128;
const RELEASE_S: f32 = 0.15;
/// A released string is dropped after this many seconds (about 400 dB down).
const DROP_S: f32 = 1.0;

#[derive(Clone, Copy)]
enum Ev {
    On(u8, u8),
    Off(u8),
}

struct Voice {
    string: PluckedString,
    note: u8,
    held: bool,
    sounding: bool,
    released_for: u32,
    age: u64,
}

struct TestPluck {
    values: [f32; 4],
    sample_rate: f32,
    voices: Vec<Voice>,
    clock: u64,
    queue: [(u32, Ev); QUEUE],
    queued: usize,
}

impl Default for TestPluck {
    fn default() -> Self {
        let mut p = Self {
            values: [0.0; 4],
            sample_rate: 48_000.0,
            voices: Vec::new(),
            clock: 0,
            queue: [(0, Ev::Off(0)); QUEUE],
            queued: 0,
        };
        for (v, info) in p.values.iter_mut().zip(PARAMS.iter()) {
            *v = info.default;
        }
        p.prepare(48_000.0, 512, 2);
        p
    }
}

impl TestPluck {
    fn configure(&mut self, i: usize) {
        let (decay, bright, pick) = (self.values[1], self.values[2], self.values[3]);
        let v = &mut self.voices[i];
        v.string.set_stretch(bright);
        v.string.set_pick_position(pick);
        v.string.set_decay(if v.held || !v.sounding { decay } else { RELEASE_S });
    }

    fn apply(&mut self, ev: Ev) {
        match ev {
            Ev::On(note, vel) => {
                self.clock += 1;
                // A free voice, else the oldest.
                let i = self
                    .voices
                    .iter()
                    .position(|v| !v.sounding)
                    .or_else(|| self.voices.iter().enumerate().min_by_key(|(_, v)| v.age).map(|(i, _)| i))
                    .unwrap_or(0);
                let age = self.clock;
                let v = &mut self.voices[i];
                v.note = note;
                v.held = true;
                v.sounding = true;
                v.released_for = 0;
                v.age = age;
                v.string.set_frequency(hz_from_midi(f32::from(note)));
                self.configure(i);
                self.voices[i].string.pluck(f32::from(vel) / 127.0);
            }
            Ev::Off(note) => {
                for i in 0..self.voices.len() {
                    if self.voices[i].sounding && self.voices[i].held && self.voices[i].note == note {
                        self.voices[i].held = false;
                        self.configure(i);
                    }
                }
            }
        }
    }

    fn push(&mut self, offset: usize, ev: Ev) {
        if self.queued == QUEUE {
            self.apply(ev);
            return;
        }
        let off = offset.min(u32::MAX as usize) as u32;
        let mut at = self.queued;
        while at > 0 && self.queue[at - 1].0 > off {
            self.queue[at] = self.queue[at - 1];
            at -= 1;
        }
        self.queue[at] = (off, ev);
        self.queued += 1;
    }
}

impl Plugin for TestPluck {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn state_is_parameters_only(&self) -> bool {
        true
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.sample_rate = sample_rate;
        self.queued = 0;
        self.voices.clear();
        for _ in 0..VOICES {
            self.voices.push(Voice {
                string: PluckedString::new(sample_rate, 25.0),
                note: 0,
                held: false,
                sounding: false,
                released_for: 0,
                age: 0,
            });
        }
        for i in 0..VOICES {
            self.configure(i);
        }
    }

    fn reset(&mut self) {
        self.queued = 0;
        for v in &mut self.voices {
            v.string.reset();
            v.sounding = false;
            v.held = false;
        }
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(index) = PARAMS.iter().position(|p| p.id == id) else {
            return false;
        };
        self.values[index] = PARAMS[index].clamp(value);
        for i in 0..self.voices.len() {
            self.configure(i);
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        PARAMS.iter().position(|p| p.id == id).map(|i| self.values[i])
    }

    fn tail_samples(&self) -> usize {
        (DROP_S * self.sample_rate) as usize + 64
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        if velocity == 0 {
            self.push(offset, Ev::Off(note));
        } else {
            self.push(offset, Ev::On(note, velocity));
        }
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.push(offset, Ev::Off(note));
    }

    fn all_notes_off(&mut self) {
        for note in 0..128u8 {
            self.apply(Ev::Off(note));
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let gain = 10f32.powf(self.values[0] / 20.0);
        let drop_after = (DROP_S * self.sample_rate) as u32;
        let mut next_event = 0;
        for i in 0..frames {
            while next_event < self.queued && self.queue[next_event].0 as usize <= i {
                let ev = self.queue[next_event].1;
                next_event += 1;
                self.apply(ev);
            }
            let mut sum = 0.0f32;
            for v in &mut self.voices {
                if !v.sounding {
                    continue;
                }
                sum += v.string.next();
                if !v.held {
                    v.released_for += 1;
                    if v.released_for >= drop_after {
                        v.sounding = false;
                        v.string.reset();
                    }
                }
            }
            let out = sum * gain;
            for ch in io.iter_mut() {
                if let Some(s) = ch.get_mut(i) {
                    *s = out;
                }
            }
        }
        while next_event < self.queued {
            let ev = self.queue[next_event].1;
            next_event += 1;
            self.apply(ev);
        }
        self.queued = 0;
    }
}

fn options() -> Options {
    if cfg!(debug_assertions) { Options::quick() } else { Options::default() }
}

#[test]
fn test_pluck_conforms() {
    let report = conformance(TestPluck::default, options());
    println!("{report}");
    report.assert_pass();
}

#[test]
fn test_pluck_sounds_at_the_right_pitch_and_stops() {
    let mut p = TestPluck::default();
    p.set_param("decay", 4.0);
    p.set_param("bright", 0.02);
    p.note_on(0, 57, 110); // A3, 220 Hz
    let mut io = vec![vec![0.0f32; 4800], vec![0.0f32; 4800]];
    p.process(&mut io, 4800);
    let peak = io[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak > 0.01, "{peak}");
    assert_eq!(io[0], io[1]);
    // The autocorrelation peak of a plucked tone sits at the period (218.2 samples).
    let x = &io[0][1000..4000];
    let best = (150..300usize)
        .max_by(|&a, &b| {
            let ca: f32 = (0..2000).map(|i| x[i] * x[i + a]).sum();
            let cb: f32 = (0..2000).map(|i| x[i] * x[i + b]).sum();
            ca.total_cmp(&cb)
        })
        .unwrap();
    assert!((best as f32 - 48_000.0 / 220.0).abs() < 1.5, "lag {best}");
    p.note_off(0, 57);
    for _ in 0..12 {
        p.process(&mut io, 4800);
    }
    assert!(io[0].iter().all(|v| *v == 0.0), "silent after release");
}
