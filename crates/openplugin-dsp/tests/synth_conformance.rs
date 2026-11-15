//! A tiny test synthesizer built from the synthesis blocks, through the
//! conformance kit (rates, block sizes, hostile input, parameter fuzz, state,
//! notes, tails, and no allocation or panic on the audio path).

use openplugin_core::{
    Category, ParamInfo, Plugin, PluginInfo, Taper, Unit,
};
use openplugin_dsp::{
    Adsr, BandlimitedOsc, BlWave, GlideMode, PinkNoise, StealPolicy, VoiceAllocator, VoiceMode,
};
use openplugin_testing::{conformance, Options};

openplugin_testing::install_allocator!();

static PARAMS: [ParamInfo; 7] = [
    ParamInfo::new("gain", "Gain", -60.0, 6.0, -12.0, Unit::Db).with_taper(Taper::Linear),
    ParamInfo::new("wave", "Wave", 0.0, 3.0, 0.0, Unit::Choice).with_choices(&["Saw", "Square", "Pulse", "Triangle"]),
    ParamInfo::new("width", "Pulse width", 0.05, 0.95, 0.5, Unit::None),
    ParamInfo::new("release", "Release", 0.001, 0.5, 0.1, Unit::Seconds),
    ParamInfo::new("glide", "Glide", 0.0, 0.5, 0.0, Unit::Seconds),
    ParamInfo::new("noise", "Noise", 0.0, 1.0, 0.0, Unit::None),
    ParamInfo::new("mono", "Mono", 0.0, 1.0, 0.0, Unit::Toggle),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.test.synth",
    name: "Test Synth",
    vendor: "OpenGrid",
    url: "https://github.com/MistaMin/OpenPluginSDK",
    email: "contact@mistamin.com",
    version: "0.3.1-beta",
    category: Category::Instrument,
    params: &PARAMS,
    is_instrument: true,
    has_editor: false,
};

#[derive(Default)]
struct SynthVoice {
    osc: Option<BandlimitedOsc>,
    env: Option<Adsr>,
}

#[derive(Clone, Copy)]
enum Ev {
    On(u8, u8),
    Off(u8),
}

const QUEUE: usize = 128;
const VOICES: usize = 8;

struct TestSynth {
    values: [f32; 7],
    sample_rate: f32,
    voices: VoiceAllocator<SynthVoice, VOICES>,
    noise: PinkNoise,
    queue: [(u32, Ev); QUEUE],
    queued: usize,
}

impl Default for TestSynth {
    fn default() -> Self {
        let mut s = Self {
            values: [0.0; 7],
            sample_rate: 48_000.0,
            voices: VoiceAllocator::new(48_000.0),
            noise: PinkNoise::new(48_000.0, 7),
            queue: [(0, Ev::Off(0)); QUEUE],
            queued: 0,
        };
        for (v, p) in s.values.iter_mut().zip(PARAMS.iter()) {
            *v = p.default;
        }
        s.prepare(48_000.0, 512, 2);
        s
    }
}

impl TestSynth {
    fn wave(&self) -> BlWave {
        match self.values[1] as u32 {
            0 => BlWave::Saw,
            1 => BlWave::Square,
            2 => BlWave::Pulse,
            _ => BlWave::Triangle,
        }
    }

    fn apply_params_to_voices(&mut self) {
        let (sr, rel, wave, width) = (self.sample_rate, self.values[3], self.wave(), self.values[2]);
        for v in self.voices.voices_mut() {
            let o = v.state.osc.get_or_insert_with(|| BandlimitedOsc::new(BlWave::Saw));
            o.set_wave(wave);
            o.set_pulse_width(width);
            let e = v.state.env.get_or_insert_with(|| Adsr::new(sr));
            e.set_sample_rate(sr);
            e.set_adsr(0.005, 0.1, 0.7, rel);
        }
        self.voices.set_glide(self.values[4], GlideMode::Always);
        self.voices.set_mode(if self.values[6] >= 0.5 { VoiceMode::Legato } else { VoiceMode::Poly });
    }

    fn apply(&mut self, ev: Ev) {
        match ev {
            Ev::On(note, vel) => {
                if let Some(a) = self.voices.note_on(note, vel) {
                    if let Some(v) = self.voices.voice_mut(a.voice) {
                        if let Some(e) = v.state.env.as_mut() {
                            if a.retrigger {
                                e.gate_on();
                            }
                        }
                    }
                }
            }
            Ev::Off(note) => {
                let r = self.voices.note_off(note);
                if let Some(i) = r.released {
                    if let Some(e) = self.voices.voice_mut(i).and_then(|v| v.state.env.as_mut()) {
                        e.gate_off();
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
        // Keep the queue ordered by offset (stable for equal offsets).
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

impl Plugin for TestSynth {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn state_is_parameters_only(&self) -> bool {
        true
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.sample_rate = sample_rate;
        self.voices.set_sample_rate(sample_rate);
        self.noise.set_sample_rate(sample_rate);
        self.voices.reset();
        self.queued = 0;
        self.apply_params_to_voices();
    }

    fn reset(&mut self) {
        self.voices.reset();
        self.noise.reset();
        self.queued = 0;
        for v in self.voices.voices_mut() {
            if let Some(e) = v.state.env.as_mut() {
                e.kill();
            }
        }
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(index) = PARAMS.iter().position(|p| p.id == id) else {
            return false;
        };
        self.values[index] = PARAMS[index].clamp(value);
        self.apply_params_to_voices();
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        PARAMS.iter().position(|p| p.id == id).map(|i| self.values[i])
    }

    fn tail_samples(&self) -> usize {
        // The longest release plus the allocator's retrigger room.
        (0.5 * self.sample_rate) as usize + 64
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.push(offset, Ev::On(note, velocity));
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.push(offset, Ev::Off(note));
    }

    fn midi(&mut self, _offset: usize, msg: [u8; 3]) {
        if msg[0] & 0xF0 == 0xB0 && msg[1] == 64 {
            let mut freed = [usize::MAX; VOICES];
            let mut n = 0;
            self.voices.set_sustain_pedal(msg[2] >= 64, |i| {
                if n < VOICES {
                    freed[n] = i;
                    n += 1;
                }
            });
            for &i in &freed[..n] {
                if let Some(e) = self.voices.voice_mut(i).and_then(|v| v.state.env.as_mut()) {
                    e.gate_off();
                }
            }
        } else if msg[0] & 0xF0 == 0xB0 && msg[1] == 123 {
            self.all_notes_off();
        }
    }

    fn all_notes_off(&mut self) {
        let mut freed = [usize::MAX; VOICES];
        let mut n = 0;
        self.voices.all_notes_off(|i| {
            if n < VOICES {
                freed[n] = i;
                n += 1;
            }
        });
        for &i in &freed[..n] {
            if let Some(e) = self.voices.voice_mut(i).and_then(|v| v.state.env.as_mut()) {
                e.gate_off();
            }
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let gain = 10f32.powf(self.values[0] / 20.0) * 0.25;
        let noise_mix = self.values[5];
        let sr = self.sample_rate;
        let mut next_event = 0;
        for i in 0..frames {
            while next_event < self.queued && self.queue[next_event].0 as usize <= i {
                let ev = self.queue[next_event].1;
                next_event += 1;
                self.apply(ev);
            }
            let mut sum = 0.0f32;
            for v in self.voices.voices_mut().iter_mut() {
                if v.is_free() {
                    continue;
                }
                v.advance(1);
                let hz = v.hz();
                let vel = v.velocity();
                let (Some(o), Some(e)) = (v.state.osc.as_mut(), v.state.env.as_mut()) else {
                    continue;
                };
                let level = e.next();
                sum += o.next(hz, sr) * level * vel;
            }
            // Free voices whose envelope has ended (second pass: the borrow above ended).
            for k in 0..VOICES {
                let ended = self
                    .voices
                    .voice(k)
                    .is_some_and(|v| !v.is_free() && v.state.env.as_ref().is_some_and(|e| !e.is_active()));
                if ended {
                    self.voices.free(k);
                }
            }
            let out = (sum + self.noise.next() * noise_mix) * gain;
            for ch in io.iter_mut() {
                ch[i] = out;
            }
        }
        // Events past the block end run at its end.
        while next_event < self.queued {
            let ev = self.queue[next_event].1;
            next_event += 1;
            self.apply(ev);
        }
        self.queued = 0;
        // Quietest-first stealing needs the levels; keep them current.
        for k in 0..VOICES {
            let lvl = self
                .voices
                .voice(k)
                .and_then(|v| v.state.env.as_ref().map(|e| e.level() * v.velocity()))
                .unwrap_or(0.0);
            self.voices.set_level(k, lvl);
        }
    }
}

fn options() -> Options {
    if cfg!(debug_assertions) { Options::quick() } else { Options::default() }
}

#[test]
fn test_synth_conforms() {
    let report = conformance(TestSynth::default, options());
    println!("{report}");
    report.assert_pass();
}

#[test]
fn test_synth_makes_sound_and_stops() {
    let mut s = TestSynth::default();
    s.set_param("release", 0.01);
    s.prepare(48_000.0, 512, 2);
    s.set_param("release", 0.01);
    s.voices.set_policy(StealPolicy::Quietest);
    s.note_on(0, 69, 100);
    s.note_on(10, 76, 100);
    let mut io = vec![vec![0.0f32; 512], vec![0.0f32; 512]];
    s.process(&mut io, 512);
    assert_eq!(io[0][0], 0.0, "first sample is before any attack");
    let peak = io[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak > 0.01, "{peak}");
    assert_eq!(io[0], io[1]);
    s.note_off(0, 69);
    s.note_off(0, 76);
    for _ in 0..4 {
        s.process(&mut io, 512);
    }
    assert!(io[0].iter().all(|v| *v == 0.0), "silent after release");
    assert_eq!(s.voices.active(), 0);
}
