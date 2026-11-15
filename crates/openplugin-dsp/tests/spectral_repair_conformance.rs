//! A tiny denoiser `Plugin` built from `SpectralRepair`, run through
//! `openplugin-testing`'s conformance kit: every sample rate and block size,
//! hostile input, parameter fuzz (including the learn toggle), a state round
//! trip that carries the learned profile, layouts, the allocation check on the
//! audio path, and the latency check by impulse against `Plugin::latency`.

use openplugin_core::{Category, NotePorts, ParamInfo, Plugin, PluginInfo, Tail, Unit, MAX_AUDIO_CHANNELS};
use openplugin_dsp::{GainRule, RepairConfig, RepairParams, SpectralRepair};
use openplugin_testing::{conformance, Check, Options, Status};

openplugin_testing::install_allocator!();

const FRAME: usize = 1024;

static PARAMS: [ParamInfo; 4] = [
    ParamInfo::new("amount", "Reduction", 0.0, 60.0, 12.0, Unit::Db),
    ParamInfo::new("over", "Over-subtraction", 0.5, 6.0, 2.0, Unit::None),
    ParamInfo::new("learn", "Learn noise", 0.0, 1.0, 0.0, Unit::Toggle),
    ParamInfo::new("listen", "Listen to removed", 0.0, 1.0, 0.0, Unit::Toggle),
];

static INFO: PluginInfo = PluginInfo {
    id: "test.denoise",
    name: "Test Denoiser",
    vendor: "test",
    url: "",
    email: "",
    version: "0",
    category: Category::Other,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

struct Denoiser {
    repair: SpectralRepair,
    values: [f32; 4],
    channels: usize,
}

impl Default for Denoiser {
    fn default() -> Self {
        let mut p = Self {
            repair: SpectralRepair::new(RepairConfig::new(48_000.0, 2, FRAME)).unwrap(),
            values: [12.0, 2.0, 0.0, 0.0],
            channels: 2,
        };
        p.apply();
        p
    }
}

impl Denoiser {
    fn apply(&mut self) {
        let v = self.values;
        self.repair.set_params(RepairParams {
            amount_db: v[0],
            over_subtraction: v[1],
            rule: GainRule::MmseStsa,
            decision_directed: true,
            listen_removed: v[3] >= 0.5,
            ..RepairParams::default()
        });
        if v[2] >= 0.5 && !self.repair.is_learning() {
            self.repair.start_learning();
        } else if v[2] < 0.5 && self.repair.is_learning() {
            self.repair.stop_learning();
        }
    }
}

impl Plugin for Denoiser {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.channels = channels.clamp(1, MAX_AUDIO_CHANNELS);
        let old = self.repair.save_profile();
        // Allocates: prepare is not the audio path.
        self.repair = SpectralRepair::new(RepairConfig::new(sample_rate, self.channels, FRAME)).unwrap();
        let _ = self.repair.load_profile(&old);
        self.apply();
    }
    fn reset(&mut self) {
        self.repair.reset();
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
        self.repair.latency()
    }
    fn tail(&self) -> Tail {
        // The input sits in the frame for one frame, and leaves over another.
        Tail::Finite((2 * FRAME + 256) as u32)
    }
    fn save_state(&mut self) -> Option<Vec<u8>> {
        Some(self.repair.save_profile())
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        self.repair.load_profile(data).is_ok()
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for (ch, buf) in io.iter_mut().enumerate().take(self.channels) {
            let n = frames.min(buf.len());
            self.repair.process_in_place(ch, &mut buf[..n]);
        }
    }
}

fn options() -> Options {
    if cfg!(debug_assertions) { Options::quick() } else { Options::default() }
}

#[test]
fn denoiser_plugin_passes_the_conformance_kit() {
    let report = conformance(Denoiser::default, options());
    println!("{report}");
    report.assert_pass();
    assert_eq!(report.status(Check::NoAlloc), Status::Pass);
    assert_eq!(report.status(Check::Latency), Status::Pass);
}
