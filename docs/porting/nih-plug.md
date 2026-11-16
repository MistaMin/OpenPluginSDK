# Porting from nih-plug

This guide maps a nih-plug plugin onto OpenPluginSDK: the processor, parameters, state, editor, buses, MIDI and threading, then a small effect ported in full. It is written from nih-plug's public documentation and public API names. The Rust in it is original code for this SDK, and every code block is compiled and run as a doc test (`cargo test -p example-porting-guides`). API names are the ones nih-plug publishes at the time of writing; check them against the version you build with.

Read [PORTING.md](../PORTING.md) first: it has the order of work and the tools (scanner, state capture, null test) that every port uses.

## The short version

- Both are Rust and both have a `Plugin` trait, so the DSP usually moves over unchanged. What changes is who owns the parameters and how the block reaches you.
- nih-plug parameters are objects in a `Params` struct shared through an `Arc`; the plugin reads them. Here the parameter table is `static` data and the plugin owns the current values as plain fields, written by `set_param`.
- There is no `Buffer` type and no process context. `process` gets `&mut [Vec<f32>]` and a frame count. Transport, notes and parameter changes arrive as calls before or between the pieces of a block.
- Smoothing is not part of a parameter. The plugin owns a smoother where it wants one.
- The formats are CLAP and LV2 directly, and VST3, Audio Unit and AAX through the native shells of [SHELL.md](../SHELL.md).

## Concept map

### Processor

| nih-plug | OpenPluginSDK | Notes |
|---|---|---|
| `impl Plugin` with the constants `NAME`, `VENDOR`, `URL`, `EMAIL`, `VERSION` | `Plugin::info` returning a `static PluginInfo` | |
| `impl ClapPlugin`: `CLAP_ID`, `CLAP_FEATURES` | `PluginInfo::id`, `PluginInfo::category`, `PluginInfo::is_instrument` | The CLAP id is the plugin id, so keep the old string |
| `impl Vst3Plugin`: `VST3_CLASS_ID`, `VST3_SUBCATEGORIES` | Derived from `PluginInfo::id` and the category | The class id cannot be set yet; see the list at the end |
| `nih_export_clap!`, `nih_export_vst3!` | `export_clap!`, `export_lv2!`, `export_shell!` | The shell export serves VST3, Audio Unit and AAX |
| `impl Default` | `impl Default` | The format entries call it |
| `initialize(audio_io_layout, buffer_config, context) -> bool` | `prepare(sample_rate, max_block, channels)`, or `prepare_layout(sample_rate, max_block, layout)` | It cannot fail. Refuse a rate with `accepts_sample_rate` and a layout with `set_layout`. Allocate here |
| `BufferConfig::process_mode`, `ProcessMode::Offline` | `Plugin::set_offline` | |
| `reset()` | `reset()` | Must not allocate |
| `process(buffer, aux, context) -> ProcessStatus` | `process(io: &mut [Vec<f32>], frames)`, or `process_buses(io, sidechain, frames)` | In place, planar: `io[channel][..frames]` |
| `buffer.iter_samples()`, `buffer.as_slice()` | Plain loops over `io` | A channel `Vec` can be longer than `frames`; use only `..frames` |
| `ProcessStatus::Tail(n)`, `ProcessStatus::KeepAlive` | `Plugin::tail_samples`, or `Plugin::tail` returning `Tail::Finite(n)` or `Tail::Infinite` | A property the host reads, not a return value |
| `ProcessStatus::Error` | Not available | `process` cannot fail; a plugin handles its own faults and stays silent |
| `context.set_latency_samples(n)` | `Plugin::latency` | The entries read it after every block. `HostNotifier::latency_changed` reports a change made outside `process` |
| `context.transport()` | `Plugin::set_transport(&Transport)` | Called before each block; keep the fields you use |
| `SAMPLE_ACCURATE_AUTOMATION` | Always on | The entries split a block at every parameter event |
| `context.execute_background`, `task_executor`, `BackgroundTask` | `openplugin::core::worker::loader` | Builds a value on a background thread and swaps it in on the audio thread without a lock |

### Parameters

| nih-plug | OpenPluginSDK | Notes |
|---|---|---|
| `#[derive(Params)]` on a struct, `fn params() -> Arc<dyn Params>` | `static PARAMS: [ParamInfo; N]`, and fields in the plugin for the current values | The position in the table is the parameter's index in every format |
| `#[id = "gain"]` | `ParamInfo::id` | Keep the old strings |
| `FloatParam::new(name, default, FloatRange::Linear { min, max })` | `ParamInfo::new(id, name, min, max, default, unit)` | The argument order differs: range first, then the default |
| `FloatRange::Skewed`, `FloatRange::SymmetricalSkewed` | `.with_taper(Taper::Log)` | Only linear and logarithmic travel exist; see the list at the end |
| `IntParam` | `.with_steps(n)` | `n` positions including both ends |
| `BoolParam` | `Unit::Toggle` | |
| `EnumParam<T>` with `#[derive(Enum)]` | `.with_choices(&[...])` | The value is the index |
| `.with_unit(" dB")`, `.with_value_to_string`, `.with_string_to_value`, the `formatters` module | `Unit::Db`, `Unit::Hz`, ... | The unit prints and parses the value. No custom formatter; see the list at the end |
| `.with_smoother(SmoothingStyle::...)`, `param.smoothed.next()` | `OnePole`, `SampleRamp`, `openplugin::dsp::Gain`, `openplugin::dsp::Ballistics` | Owned by the plugin and reset in `reset` |
| `param.value()` inside `process` | A field of the plugin, written in `set_param` | Plain units, clamped by `ParamInfo::clamp` |
| `.with_callback(...)` | `set_param(id, value)` | Compute coefficients here. Override `set_param_index` to skip the string match |
| `.non_automatable()`, `.hide()`, `.make_bypass()` | `.flag(ParamFlags::NOT_REALTIME)`, `.flag(ParamFlags::HIDDEN)`, `.flag(ParamFlags::BYPASS)` | The host's own bypass also arrives through `Plugin::set_host_bypass` |
| `#[nested(group = "Filter")]` | `.with_group("Filter")` | A path: `"Filter/Envelope"` nests |
| `.with_poly_modulation_id(..)`, `NoteEvent::PolyModulation` | `.flag(ParamFlags::POLY_MODULATION)`, `Plugin::param_modulation` | See [EXPRESSION.md](../EXPRESSION.md) |

A gain parameter in nih-plug is often stored as linear gain with a skewed range and printed in decibels by a formatter. A port has two choices. Keep the plain value linear, and old automation and CLAP state keep their meaning, but hosts show a gain factor. Or declare it in decibels with `Unit::Db`, which reads better and is what the worked port below does for a new parameter; old values then need converting in `load_state`.

### State

| nih-plug | OpenPluginSDK | Notes |
|---|---|---|
| Parameter values in the plugin state, no persistent fields | Nothing to write | When parameters are the whole state, the SDK saves and restores them in every format |
| `#[persist = "key"]` fields | `save_state() -> Option<Vec<u8>>`, `load_state(&[u8]) -> bool` | For a plugin with more state than its parameters. Put the parameter values in the bytes too: the CLAP entry stores this chunk alone when there is one. Start with a magic and a version, and change nothing when you return `false` |
| No persistent fields | `state_is_parameters_only()` returns `true` | Then a save or load never touches the engine |
| `filter_state` for migrating old states | Version handling inside `load_state` | |
| State saved by the nih-plug build | `load_state`, through CLAP | The old state is JSON, which `openplugin::core::foreign_state` does not read; see the list at the end |

### Editor

| nih-plug | OpenPluginSDK | Notes |
|---|---|---|
| `fn editor(&mut self, async_executor) -> Option<Box<dyn Editor>>` | A free function `fn(EditorHost) -> Option<Box<dyn Editor>>` given to the export macros as `editor = ...`; `PluginInfo::has_editor` | The editor is made without the plugin |
| `nih_plug_vizia`, `nih_plug_egui`, `nih_plug_iced` | `impl Editor` returning a frame of pixels from `render(scale)` | No adapter for those toolkits. `openplugin::dri` has knobs, faders, meters and a control registry; see [EDITOR.md](../EDITOR.md) |
| `Editor::size`, `Editor::set_scale_factor` | `Editor::size`, the `scale` argument of `render`, `Editor::size_range`, `Editor::resize` | Points |
| `Editor::param_value_changed` | `Editor::param_changed(index, plain)` | |
| `ParamSetter::begin_set_parameter`, `set_parameter`, `end_set_parameter` | `EditorHost::begin`, `perform`, `end` | By table index, in plain units |
| `GuiContext::request_resize` | `EditorHost::request_resize` | |
| An `Arc<AtomicF32>` meter shared with the editor | The `Arc` returned by `Plugin::editor_state`, handed to `Editor::bind` | `Plugin::meters` carries standard levels |

### Buses

| nih-plug | OpenPluginSDK | Notes |
|---|---|---|
| `AUDIO_IO_LAYOUTS: &[AudioIOLayout { main_input_channels, main_output_channels, .. }]` | `supported_layouts() -> &'static [IoConfig]` | The first is the default. The plain default is stereo in, stereo out |
| `aux_input_ports: &[new_nonzero_u32(2)]`, `aux.inputs[0]` | `IoConfig { sidechain: Some(SpeakerLayout::Stereo), .. }`, the `sidechain` argument of `process_buses` | One optional sidechain |
| The layout passed to `initialize` | `set_layout`, `prepare_layout` | |
| `aux_output_ports`, more than one aux input | Not available | See the list at the end |

### MIDI

| nih-plug | OpenPluginSDK | Notes |
|---|---|---|
| `MIDI_INPUT`, `MIDI_OUTPUT` (`MidiConfig`) | `note_ports() -> NotePorts { input, output }` | An effect that reads no MIDI returns both `false` |
| The `while let Some(event) = context.next_event()` loop with `event.timing()` | `note_on(offset, note, velocity)`, `note_off(offset, note)`, `midi(offset, [u8; 3])` | The entries split the block at each event |
| `NoteEvent::NoteOn { voice_id, channel, note, velocity, .. }` | `note_on_ref(offset, NoteRef, velocity)` | Velocity is 0 to 127 here, not 0 to 1 |
| `NoteEvent::MidiCC`, `MidiPitchBend`, `MidiChannelPressure` | `midi(offset, [u8; 3])` | Raw MIDI 1.0 bytes |
| `NoteEvent::PolyPressure`, `PolyTuning`, `PolyBrightness`, `PolyPan`, `PolyVolume` | `note_expressions()`, `note_expression(offset, note, expression, value)` | See [EXPRESSION.md](../EXPRESSION.md) |
| `context.send_event(..)` | `pull_midi() -> Option<(u32, [u8; 3])>` | The entry drains it after each block |
| `type SysExMessage` | Not available | See the list at the end |

### Threading

| nih-plug | OpenPluginSDK | Notes |
|---|---|---|
| `process` on the audio thread, parameters read through atomics from any thread | `process`, `set_param`, the note calls, `set_transport` and `reset` on the audio thread, one at a time | The entries carry values across threads; the plugin's fields need no atomic |
| `initialize` and `reset` may allocate or not as documented | `prepare` may allocate; `reset` and everything on the audio path must not | |
| The `assert_process_allocs` feature | The conformance kit's `no_alloc` check | It also covers `set_param`, the note calls and `reset` |
| Flush-to-zero during `process` | The same, in every entry (`Plugin::flush_denormals`) | The kit runs the plugin without it and checks the output for subnormals |
| The editor on the GUI thread with a `GuiContext` | Every `Editor` call on the main thread with an `EditorHost` | |

## Worked port: a sidechain ducker

The original turns the main signal down while the sidechain is loud. Its `AUDIO_IO_LAYOUTS` lists stereo with a stereo aux input, then plain stereo. Its `Params` struct has three `FloatParam`s: `threshold` and `amount` in decibels and `release` in milliseconds with a skewed range. `process` reads `aux.inputs[0]`, follows its peak, and multiplies the main buffer by a gain that a smoother brings toward the target.

The port keeps the three ids and ranges. The gain reduction runs through the SDK's `Ballistics` (a 5 ms attack and the release parameter), which is the smoother.

```rust
use openplugin::dsp::envelope::{amp_to_db, db_to_amp};
use openplugin::dsp::Ballistics;
use openplugin::prelude::*;
use openplugin_testing::{conformance, Options};

// The `Params` struct, as data. Ids and ranges are the old ones.
static PARAMS: [ParamInfo; 3] = [
    ParamInfo::new("threshold", "Threshold", -60.0, 0.0, -24.0, Unit::Db),
    ParamInfo::new("amount", "Amount", 0.0, 36.0, 12.0, Unit::Db),
    ParamInfo::new("release", "Release", 20.0, 2_000.0, 250.0, Unit::Ms).with_taper(Taper::Log),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.example.ported-ducker",
    name: "Ported Ducker",
    vendor: "Example Audio",
    url: "https://example.com",
    email: "",
    version: "2.0.0",
    category: Category::Dynamics,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

// `AUDIO_IO_LAYOUTS`: with the sidechain first, so it is the default.
static LAYOUTS: [IoConfig; 2] = [
    IoConfig { input: SpeakerLayout::Stereo, output: SpeakerLayout::Stereo, sidechain: Some(SpeakerLayout::Stereo) },
    IoConfig::STEREO,
];

pub struct PortedDucker {
    threshold_db: f32,
    amount_db: f32,
    release_ms: f32,
    /// The gain reduction in dB, smoothed. Positive is attenuation.
    reduction: Ballistics,
}

impl Default for PortedDucker {
    fn default() -> Self {
        Self {
            threshold_db: PARAMS[0].default,
            amount_db: PARAMS[1].default,
            release_ms: PARAMS[2].default,
            reduction: Ballistics::new(48_000.0),
        }
    }
}

impl Plugin for PortedDucker {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    // `initialize`.
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.reduction.set_sample_rate(sample_rate);
        self.reduction.set_attack_ms(5.0);
        self.reduction.set_release_ms(self.release_ms);
        self.reduction.reset(0.0);
    }

    fn reset(&mut self) {
        self.reduction.reset(0.0);
    }

    // What `param.value()` read is now a field. Never called while `process` runs.
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "threshold" => self.threshold_db = PARAMS[0].clamp(value),
            "amount" => self.amount_db = PARAMS[1].clamp(value),
            "release" => {
                self.release_ms = PARAMS[2].clamp(value);
                self.reduction.set_release_ms(self.release_ms);
            }
            _ => return false,
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "threshold" => Some(self.threshold_db),
            "amount" => Some(self.amount_db),
            "release" => Some(self.release_ms),
            _ => None,
        }
    }

    fn supported_layouts(&self) -> &'static [IoConfig] {
        &LAYOUTS
    }

    // A host that gives no sidechain calls this: nothing to duck to.
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.process_buses(io, None, frames);
    }

    // `process` with `aux.inputs[0]`.
    fn process_buses(&mut self, io: &mut [Vec<f32>], sidechain: Option<&[Vec<f32>]>, frames: usize) {
        for i in 0..frames {
            let mut key = 0.0f32;
            for channel in sidechain.unwrap_or(&[]) {
                key = key.max(channel[i].abs());
            }
            // NaN or infinity in the key reads as silence.
            let over = amp_to_db(key) - self.threshold_db;
            let reduction = self.reduction.step(over.clamp(0.0, self.amount_db));
            let gain = db_to_amp(-reduction);
            for channel in io.iter_mut().take(2) {
                channel[i] *= gain;
            }
        }
    }

    fn gain_reduction_db(&self) -> f32 {
        self.reduction.value()
    }

    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }

    // No `#[persist]` fields in the original.
    fn state_is_parameters_only(&self) -> bool {
        true
    }
}

openplugin::targets::export_clap!(PortedDucker);
openplugin::targets::export_lv2!(PortedDucker);
openplugin::targets::export_shell!(PortedDucker);

// In a plugin crate the rest is `tests/conformance.rs`.
openplugin_testing::install_allocator!();

/// One second of a constant main signal, with the key at `key_level`.
fn run(plugin: &mut PortedDucker, key_level: Option<f32>) -> f32 {
    let mut io = vec![vec![0.0f32; 480]; 2];
    for _ in 0..100 {
        io[0].fill(0.5);
        io[1].fill(0.5);
        match key_level {
            Some(level) => {
                let key = vec![vec![level; 480]; 2];
                plugin.process_buses(&mut io, Some(&key), 480);
            }
            None => plugin.process(&mut io, 480),
        }
    }
    io[0][479]
}

fn main() {
    let mut plugin = PortedDucker::default();
    plugin.prepare(48_000.0, 480, 2);

    // A full-scale key is 24 dB over the threshold; the amount caps it at 12 dB.
    let ducked = run(&mut plugin, Some(1.0));
    assert!((ducked - 0.5 * db_to_amp(-12.0)).abs() < 1e-3, "{ducked}");
    assert!((plugin.gain_reduction_db() - 12.0).abs() < 0.01);

    // A quiet key, or no sidechain at all, lets the signal back up.
    plugin.set_param("release", 20.0);
    assert!((run(&mut plugin, Some(0.01)) - 0.5).abs() < 1e-3);
    plugin.reset();
    assert_eq!(run(&mut plugin, None), 0.5);

    conformance(PortedDucker::default, Options::quick()).assert_pass();
}
```

What moved where:

- The trait constants and the `ClapPlugin` impl became `INFO`. `AUDIO_IO_LAYOUTS` became `LAYOUTS`, with the same order.
- The `Params` struct became the `PARAMS` table plus three fields. `set_param` is where a `with_callback` closure's work goes.
- `aux.inputs[0]` became the `sidechain` argument. A host that connects no sidechain, or a format that has none, calls `process`, so that path must work too.
- `param.smoothed.next()` became a smoother the plugin owns. Here the smoothed quantity is the gain reduction itself, so one `Ballistics` does the attack, the release and the smoothing.
- The test at the end is the port's proof: the measured gain, then the conformance kit, which also runs every layout, with a sidechain where the layout has one.

## What has no counterpart yet

- **Skewed ranges.** `Taper` is linear or logarithmic. A parameter with `FloatRange::Skewed` keeps its range and plain values, but a host knob travels differently from the old plugin's.
- **Custom value formatters.** A parameter prints and parses by its `Unit` and its choice names.
- **A JSON reader for old state.** `openplugin::core::foreign_state` reads binary records, tagged chunks, XML and property lists. A port that must open nih-plug state adds its own JSON parsing inside `load_state`, with the same rule: validate everything, then change the plugin.
- **Auxiliary outputs, or more than one sidechain.** An `IoConfig` is one main input, one main output and one optional sidechain.
- **SysEx.** MIDI reaches the plugin as messages of up to three bytes.
- **A failing `process`.** There is no error status to return.
- **GUI toolkit adapters** for vizia, egui or iced.
- **Keeping the old plugin's state in VST3.** The identity itself carries over: `VST3_CLASS_ID` goes into `Plugin::vst3_ids`, and nih-plug's hashed parameter ids into `ParamInfo::with_host_id` (see [Identity pinning](../PORTING.md#identity-pinning); `openplugin-port scan` writes both). The VST3, Audio Unit and AAX shells still accept only state they wrote themselves, so state saved by the old plugin reaches `load_state` only through CLAP; that part is open.
