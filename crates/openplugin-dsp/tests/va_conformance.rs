//! A tiny filter plugin built from `TransistorLadder`, run through
//! `openplugin-testing`'s conformance kit.

use openplugin_core::{
    Category, NotePorts, ParamInfo, Plugin, PluginInfo, Tail, Taper, Unit, MAX_AUDIO_CHANNELS,
};
use openplugin_dsp::TransistorLadder;
use openplugin_testing::{conformance, Check, Options, Status};

openplugin_testing::install_allocator!();

static PARAMS: [ParamInfo; 4] = [
    ParamInfo::new("cutoff", "Cutoff", 20.0, 20_000.0, 1000.0, Unit::Hz).with_taper(Taper::Log),
    ParamInfo::new("resonance", "Resonance", 0.0, 1.2, 0.3, Unit::None),
    ParamInfo::new("drive", "Drive", 0.1, 16.0, 1.0, Unit::None),
    ParamInfo::new("poles", "Poles", 1.0, 4.0, 4.0, Unit::None),
];

static INFO: PluginInfo = PluginInfo {
    id: "test.ladder",
    name: "Test Ladder",
    vendor: "test",
    url: "",
    email: "",
    version: "0",
    category: Category::Eq,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

struct LadderPlugin {
    filters: Vec<TransistorLadder>,
    values: [f32; 4],
    channels: usize,
    sample_rate: f32,
}

impl Default for LadderPlugin {
    fn default() -> Self {
        let mut p = Self {
            filters: (0..MAX_AUDIO_CHANNELS).map(|_| TransistorLadder::new(48_000.0)).collect(),
            values: [1000.0, 0.3, 1.0, 4.0],
            channels: 2,
            sample_rate: 48_000.0,
        };
        p.apply();
        p
    }
}

impl LadderPlugin {
    fn apply(&mut self) {
        let (sr, v) = (self.sample_rate, self.values);
        for f in &mut self.filters {
            f.set_sample_rate(sr);
            f.set_cutoff(v[0]);
            f.set_resonance(v[1]);
            f.set_drive(v[2]);
            f.set_poles(v[3].round() as usize);
        }
    }
}

impl Plugin for LadderPlugin {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.channels = channels.clamp(1, MAX_AUDIO_CHANNELS);
        self.sample_rate = sample_rate;
        self.apply();
        self.reset();
    }
    fn reset(&mut self) {
        for f in &mut self.filters {
            f.reset();
        }
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
    fn tail(&self) -> Tail {
        // Resonance near 1 rings for a long time.
        Tail::Infinite
    }
    fn state_is_parameters_only(&self) -> bool {
        true
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for (ch, buf) in io.iter_mut().enumerate().take(self.channels) {
            let n = frames.min(buf.len());
            self.filters[ch].process(&mut buf[..n]);
        }
    }
}

fn options() -> Options {
    if cfg!(debug_assertions) { Options::quick() } else { Options::default() }
}

#[test]
fn ladder_plugin_passes_the_conformance_kit() {
    let report = conformance(LadderPlugin::default, options());
    println!("{report}");
    report.assert_pass();
    assert_eq!(report.status(Check::NoAlloc), Status::Pass);
}
