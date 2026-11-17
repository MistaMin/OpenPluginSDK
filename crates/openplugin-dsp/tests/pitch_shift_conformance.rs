//! A tiny pitch-shift plugin (the phase-vocoder `PitchShifter` with a dry/wet
//! mix would hide the latency, so it is fully wet) run through
//! `openplugin-testing`'s conformance kit: every sample rate, block size,
//! hostile input, parameter fuzz, state round-trip and layout a host can
//! throw at it, the allocation check on the audio path, and the latency check
//! by impulse against `Plugin::latency`.

use openplugin_core::{Category, NotePorts, ParamInfo, Plugin, PluginInfo, Tail, Unit, MAX_AUDIO_CHANNELS};
use openplugin_dsp::{PitchShifter, VocoderConfig};
use openplugin_testing::{conformance, Check, Options, Status};

openplugin_testing::install_allocator!();

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("shift", "Shift", -12.0, 12.0, 0.0, Unit::Semitones),
    ParamInfo::new("transients", "Transients", 0.0, 1.0, 0.0, Unit::Toggle),
];

static INFO: PluginInfo = PluginInfo::new(
    "test.pitchshift",
    "Test Pitch Shift",
    "test",
    "0",
    Category::Modulation,
    &PARAMS,
);

struct ShiftPlugin {
    shifter: PitchShifter,
    values: [f32; 2],
    channels: usize,
    sample_rate: f32,
    config: VocoderConfig,
}

impl Default for ShiftPlugin {
    fn default() -> Self {
        let config = VocoderConfig::for_rate(48_000.0);
        Self {
            shifter: PitchShifter::new(2, 48_000.0, config),
            values: [0.0, 0.0],
            channels: 2,
            sample_rate: 48_000.0,
            config,
        }
    }
}

impl ShiftPlugin {
    fn apply(&mut self) {
        self.shifter.set_semitones(self.values[0]);
    }
}

impl Plugin for ShiftPlugin {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.channels = channels.clamp(1, MAX_AUDIO_CHANNELS);
        self.sample_rate = sample_rate;
        self.config = VocoderConfig::for_rate(sample_rate);
        self.config.transients = self.values[1] >= 0.5;
        // Allocates: prepare is not the audio path.
        self.shifter = PitchShifter::new(self.channels, sample_rate, self.config);
        self.apply();
    }
    fn reset(&mut self) {
        self.shifter.reset();
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(i) = PARAMS.iter().position(|p| p.id == id) else { return false };
        self.values[i] = PARAMS[i].clamp(value);
        // The transient option is read at prepare; the shift applies at once.
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
        self.shifter.latency()
    }
    fn tail(&self) -> Tail {
        // The input is spread over one frame in the stretched stream, read at
        // 0.5x at the slowest: twice the frame, plus the interpolator.
        Tail::Finite((2 * self.shifter.frame() + 256) as u32)
    }
    fn state_is_parameters_only(&self) -> bool {
        true
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        for (ch, buf) in io.iter_mut().enumerate().take(self.channels) {
            let n = frames.min(buf.len());
            self.shifter.process(ch, &mut buf[..n]);
        }
    }
}

fn options() -> Options {
    if cfg!(debug_assertions) { Options::quick() } else { Options::default() }
}

#[test]
fn pitch_shift_plugin_passes_the_conformance_kit() {
    let report = conformance(ShiftPlugin::default, options());
    println!("{report}");
    report.assert_pass();
    assert_eq!(report.status(Check::NoAlloc), Status::Pass);
    assert_eq!(report.status(Check::Latency), Status::Pass);
}
