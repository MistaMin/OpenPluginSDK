//! A tiny plugin built from `Chorus`, run through `openplugin-testing`'s
//! conformance kit: every sample rate, block size, hostile input, parameter
//! fuzz, state round-trip, layout and transport change a host can throw at it,
//! and the allocation check on the audio path.

use openplugin_core::{
    Category, NotePorts, ParamInfo, Plugin, PluginInfo, Tail, Transport, Unit, MAX_AUDIO_CHANNELS,
};
use openplugin_dsp::{Chorus, Feel, LfoRate, NoteDivision};
use openplugin_testing::{conformance, Check, Options, Status};

openplugin_testing::install_allocator!();

static PARAMS: [ParamInfo; 6] = [
    ParamInfo::new("rate", "Rate", 0.05, 10.0, 0.8, Unit::Hz),
    ParamInfo::new("depth", "Depth", 0.0, 10.0, 3.0, Unit::Ms),
    ParamInfo::new("delay", "Delay", 1.0, 40.0, 18.0, Unit::Ms),
    ParamInfo::new("voices", "Voices", 1.0, 8.0, 3.0, Unit::None),
    ParamInfo::new("mix", "Mix", 0.0, 1.0, 0.5, Unit::None),
    ParamInfo::new("sync", "Sync", 0.0, 1.0, 0.0, Unit::Toggle),
];

static INFO: PluginInfo = PluginInfo {
    id: "test.chorus",
    name: "Test Chorus",
    vendor: "test",
    url: "",
    email: "",
    version: "0",
    category: Category::Modulation,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

struct ChorusPlugin {
    chorus: Chorus,
    values: [f32; 6],
    channels: usize,
    sample_rate: f32,
}

impl Default for ChorusPlugin {
    fn default() -> Self {
        let values = [0.8, 3.0, 18.0, 3.0, 0.5, 0.0];
        let mut p = Self { chorus: Chorus::new(2, 48_000.0), values, channels: 2, sample_rate: 48_000.0 };
        p.apply();
        p
    }
}

impl ChorusPlugin {
    fn apply(&mut self) {
        let v = self.values;
        self.chorus.set_rate(if v[5] >= 0.5 {
            LfoRate::Sync(NoteDivision::new(4, Feel::Dotted))
        } else {
            LfoRate::Hz(v[0])
        });
        self.chorus.set_depth_ms(v[1]);
        self.chorus.set_delay_ms(v[2]);
        self.chorus.set_voices(v[3].round() as usize);
        self.chorus.set_mix(v[4]);
    }
}

impl Plugin for ChorusPlugin {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.channels = channels.clamp(1, MAX_AUDIO_CHANNELS);
        self.sample_rate = sample_rate;
        self.chorus.prepare(self.channels, sample_rate);
        self.apply();
    }
    fn reset(&mut self) {
        self.chorus.reset();
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(i) = PARAMS.iter().position(|p| p.id == id) else { return false };
        self.values[i] = PARAMS[i].clamp(value);
        self.apply();
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        PARAMS.iter().position(|p| p.id == id).map(|i| self.values[i])
    }
    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }
    fn set_transport(&mut self, transport: &Transport) {
        self.chorus.set_transport(transport);
    }
    fn tail(&self) -> Tail {
        // The wet path holds the input for at most delay + depth.
        let ms = f64::from(self.values[1] + self.values[2]) + 1.0;
        Tail::Finite((ms * f64::from(self.sample_rate) / 1000.0).ceil() as u32)
    }
    fn state_is_parameters_only(&self) -> bool {
        true
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for (ch, buf) in io.iter_mut().enumerate().take(self.channels) {
            let n = frames.min(buf.len());
            self.chorus.process(ch, &mut buf[..n]);
        }
    }
}

fn options() -> Options {
    if cfg!(debug_assertions) { Options::quick() } else { Options::default() }
}

#[test]
fn chorus_plugin_passes_the_conformance_kit() {
    let report = conformance(ChorusPlugin::default, options());
    println!("{report}");
    report.assert_pass();
    // The allocation check ran (it was not skipped) and passed.
    assert_eq!(report.status(Check::NoAlloc), Status::Pass);
}
