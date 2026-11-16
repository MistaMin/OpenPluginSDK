//! A tiny three-band compressor `Plugin` built from `MultibandCompressor`, run
//! through `openplugin-testing`'s conformance kit: every sample rate and block
//! size, hostile input, parameter fuzz, state round trip, layouts, and the
//! allocation check on the audio path.

use openplugin_core::{Category, NotePorts, ParamInfo, Plugin, PluginInfo, Tail, Unit};
use openplugin_dsp::{DynamicsParams, LrOrder, MultibandCompressor};
use openplugin_testing::{conformance, Check, Options, Status};

openplugin_testing::install_allocator!();

const MAX_CH: usize = 8;

static PARAMS: [ParamInfo; 6] = [
    ParamInfo::new("freq1", "Low / Mid", 50.0, 500.0, 200.0, Unit::Hz),
    ParamInfo::new("freq2", "Mid / High", 1000.0, 8000.0, 3000.0, Unit::Hz),
    ParamInfo::new("threshold", "Threshold", -60.0, 0.0, -20.0, Unit::Db),
    ParamInfo::new("ratio", "Ratio", 1.0, 20.0, 4.0, Unit::None),
    ParamInfo::new("solo_mid", "Solo mid", 0.0, 1.0, 0.0, Unit::Toggle),
    ParamInfo::new("bypass_high", "Bypass high", 0.0, 1.0, 0.0, Unit::Toggle),
];

static INFO: PluginInfo = PluginInfo::new(
    "test.multiband",
    "Test Multiband",
    "test",
    "0",
    Category::Dynamics,
    &PARAMS,
);

struct Multiband {
    mb: MultibandCompressor,
    values: [f32; 6],
    sample_rate: f32,
}

impl Default for Multiband {
    fn default() -> Self {
        let mut p = Self {
            mb: MultibandCompressor::new(48_000.0, 2, 3, LrOrder::Lr4, &[200.0, 3000.0]),
            values: [200.0, 3000.0, -20.0, 4.0, 0.0, 0.0],
            sample_rate: 48_000.0,
        };
        p.apply();
        p
    }
}

impl Multiband {
    fn apply(&mut self) {
        let v = self.values;
        self.mb.set_crossovers(&[v[0], v[1]]);
        for b in 0..3 {
            self.mb.set_band_params(
                b,
                DynamicsParams { threshold_db: v[2], ratio: v[3], knee_db: 6.0, ..DynamicsParams::default() },
            );
            self.mb.set_band_solo(b, v[4] >= 0.5 && b == 1);
        }
        self.mb.set_band_bypass(2, v[5] >= 0.5);
    }
}

impl Plugin for Multiband {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sample_rate = sample_rate;
        self.mb = MultibandCompressor::new(sample_rate, channels.clamp(1, MAX_CH), 3, LrOrder::Lr4, &[200.0, 3000.0]);
        self.apply();
    }
    fn reset(&mut self) {
        self.mb.reset();
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
    fn latency(&self) -> usize {
        0
    }
    fn tail(&self) -> Tail {
        // Crossover ringing at the 50 Hz minimum, with margin.
        Tail::Finite((0.6 * f64::from(self.sample_rate)) as u32)
    }
    fn state_is_parameters_only(&self) -> bool {
        true
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let mut chans: [&mut [f32]; MAX_CH] = Default::default();
        let n = io.len().min(MAX_CH);
        for (slot, buf) in chans.iter_mut().zip(io.iter_mut()).take(n) {
            let len = frames.min(buf.len());
            *slot = &mut buf[..len];
        }
        self.mb.process(&mut chans[..n]);
    }
}

fn options() -> Options {
    if cfg!(debug_assertions) { Options::quick() } else { Options::default() }
}

#[test]
fn multiband_plugin_passes_the_conformance_kit() {
    let report = conformance(Multiband::default, options());
    println!("{report}");
    report.assert_pass();
    assert_eq!(report.status(Check::NoAlloc), Status::Pass);
}
