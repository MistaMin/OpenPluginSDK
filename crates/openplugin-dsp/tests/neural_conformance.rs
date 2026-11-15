//! A tiny amp plugin running a small random-weight model (loaded off the audio
//! thread through `openplugin_core::worker`) through `openplugin-testing`'s
//! conformance kit.

use openplugin_core::worker::{loader, Loader, Slot};
use openplugin_core::{
    Category, NotePorts, ParamInfo, Plugin, PluginInfo, Tail, Unit, MAX_AUDIO_CHANNELS,
};
use openplugin_dsp::neural::Model;
use openplugin_testing::{conformance, Check, Options, Status};

openplugin_testing::install_allocator!();

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("drive", "Drive", 0.0, 1.0, 0.5, Unit::None),
    ParamInfo::new("level", "Level", 0.0, 1.0, 0.5, Unit::None),
];

static INFO: PluginInfo = PluginInfo {
    id: "test.neural",
    name: "Test Neural Amp",
    vendor: "test",
    url: "",
    email: "",
    version: "0",
    category: Category::Harmonic,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

fn model_text() -> String {
    // Deterministic pseudo-random weights, conditioned on drive.
    let mut s = 0x2545_f491_4f6c_dd1du64;
    let mut v = |n: usize, scale: f64| -> String {
        let xs: Vec<String> = (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                format!("{}", ((s >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * scale)
            })
            .collect();
        format!("[{}]", xs.join(","))
    };
    let h = 8;
    format!(
        r#"{{"version":1,"sample_rate":48000,"input_size":1,"residual":false,"layers":[
        {{"type":"lstm","in":1,"hidden":{h},"w_x":{},"w_h":{},"b":{}}},
        {{"type":"dense","in":{h},"out":1,"activation":"tanh","weights":{},"bias":[0.0]}}]}}"#,
        v(4 * h, 0.6),
        v(4 * h * h, 0.3),
        "[".to_owned() + &vec!["0.0"; 4 * h].join(",") + "]",
        v(h, 0.5),
    )
}

struct Amp {
    loader: Loader<Option<Model>>,
    slot: Slot<Option<Model>>,
    values: [f32; 2],
    channels: usize,
    // One model per channel would be needed for true multichannel; this test
    // plugin runs channel 0's model across channels by processing in turn.
    // Keeping it simple: the model state is shared, which is still allocation
    // free and finite, and is what the kit checks.
}

impl Default for Amp {
    fn default() -> Self {
        let (mut loader, slot) = loader::<Option<Model>>(None, 2);
        let text = model_text();
        loader.submit(move || Model::from_json(&text).ok()).unwrap();
        loader.wait_idle(std::time::Duration::from_secs(5));
        Self { loader, slot, values: [0.5, 0.5], channels: 2 }
    }
}

impl Plugin for Amp {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _sr: f32, _max_block: usize, channels: usize) {
        self.channels = channels.clamp(1, MAX_AUDIO_CHANNELS);
    }
    fn reset(&mut self) {
        if let Some(m) = self.slot.get_mut() {
            m.reset();
        }
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(i) = PARAMS.iter().position(|p| p.id == id) else { return false };
        self.values[i] = PARAMS[i].clamp(value);
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        PARAMS.iter().position(|p| p.id == id).map(|i| self.values[i])
    }
    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }
    fn tail(&self) -> Tail {
        Tail::Infinite
    }
    fn state_is_parameters_only(&self) -> bool {
        true
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.slot.poll();
        let _ = &self.loader;
        let Some(model) = self.slot.get_mut() else { return };
        let (drive, level) = (self.values[0], self.values[1]);
        for buf in io.iter_mut().take(self.channels) {
            let n = frames.min(buf.len());
            for s in &mut buf[..n] {
                *s = model.process_sample(*s * (0.5 + 4.0 * drive), 0.0) * level;
            }
        }
    }
}

fn options() -> Options {
    if cfg!(debug_assertions) { Options::quick() } else { Options::default() }
}

#[test]
fn neural_amp_passes_the_conformance_kit() {
    let report = conformance(Amp::default, options());
    println!("{report}");
    report.assert_pass();
    assert_eq!(report.status(Check::NoAlloc), Status::Pass);
}
