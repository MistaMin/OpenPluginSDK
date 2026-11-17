# Porting from DPF (DISTRHO Plugin Framework)

This guide maps a DPF plugin onto OpenPluginSDK: the processor, parameters, state, editor, buses, MIDI and threading, then a small effect ported in full. It is written from DPF's public documentation and public API names. The Rust in it is original code for this SDK, and every code block is compiled and run as a doc test (`cargo test -p example-porting-guides`). API names are the ones DPF publishes at the time of writing; check them against the version you build with.

Read [PORTING.md](../PORTING.md) first: it has the order of work and the tools (scanner, state capture, null test) that every port uses.

## The short version

- DPF and this SDK have the same shape: one plugin class, a parameter list by index, separate UI class, one source for every format. Most of a port is a change of language.
- `DistrhoPluginInfo.h` goes away. Its macros become fields of a `static PluginInfo`, a `static` parameter table and a few `Plugin` methods.
- `run` has separate input and output buffers. `process` works in place: the buffer you read is the buffer you write.
- DPF state is strings by key. Here state is one byte blob, and a plugin whose whole state is its parameters writes no state code at all.
- Programs become factory presets: lists of plain values, not code that runs. The format entries do not show them in the host's preset menu yet.

## Concept map

### Processor

| DPF | OpenPluginSDK | Notes |
|---|---|---|
| A class derived from `Plugin`, constructed with the parameter, program and state counts; `createPlugin()` | A `struct` with `impl Plugin` and `impl Default`; the export macros | `Default::default()` is what the format entries call |
| `DISTRHO_PLUGIN_NAME`, `DISTRHO_PLUGIN_BRAND`, `getLabel`, `getMaker`, `getHomePage`, `getVersion` | `PluginInfo::new(..)`, `with_url` | There is no licence field |
| `DISTRHO_PLUGIN_CLAP_ID` | `PluginInfo::id` | The CLAP id is the plugin id, so keep the old string |
| `DISTRHO_PLUGIN_URI` | `Plugin::lv2_uri` | LV2 sessions find the plugin by it, so keep the old URI |
| `getUniqueId`, `DISTRHO_PLUGIN_BRAND_ID`, `DISTRHO_PLUGIN_UNIQUE_ID` | `--au-subtype`, `--au-manufacturer` of `scripts/build_plugin.sh`; `Plugin::aax_ids` | See [BUILD.md](../BUILD.md) |
| `DISTRHO_PLUGIN_IS_SYNTH` | `PluginInfo::is_instrument` | |
| `DISTRHO_PLUGIN_IS_RT_SAFE` | Always required | The conformance kit checks that the audio path does not allocate |
| `activate()` | `reset()` | Clear filter and delay history; must not allocate |
| `sampleRateChanged`, `bufferSizeChanged`, `getSampleRate()`, `getBufferSize()` | `prepare(sample_rate, max_block, channels)` | Called again when either changes. Allocate here |
| `run(const float** inputs, float** outputs, uint32_t frames)` | `process(io: &mut [&mut [f32]], frames)` | In place: `io[channel][..frames]` holds the input and receives the output |
| `setLatency`, `DISTRHO_PLUGIN_WANT_LATENCY` | `Plugin::latency` | The entries read it after every block |
| `getTimePosition()`, `DISTRHO_PLUGIN_WANT_TIMEPOS` | `Plugin::set_transport(&Transport)` | Called before each block. `playing`, `sample_pos`, `tempo`, `bar_number`, `quarter_notes` |
| (none) | `Plugin::tail` | DPF has no tail report; a reverb or delay port should add one |

### Parameters

| DPF | OpenPluginSDK | Notes |
|---|---|---|
| `initParameter(index, Parameter&)` | `static PARAMS: [ParamInfo; N]` | The position in the table is the index in every format, so keep the old order |
| `parameter.symbol`, `parameter.name` | `ParamInfo::id`, `ParamInfo::name` | The symbol is a good id: it is already what LV2 sessions store |
| `parameter.ranges.def`, `.min`, `.max` | `ParamInfo::new(id, name, min, max, default, unit)` | |
| `parameter.unit` | `Unit::Db`, `Unit::Hz`, `Unit::Ms`, `Unit::Percent`, ... | The unit also decides how the value is printed and parsed |
| `kParameterIsAutomatable` | The default | `ParamInfo::new` sets `ParamFlags::AUTOMATABLE` |
| `kParameterIsBoolean` | `Unit::Toggle` | |
| `kParameterIsInteger` | `.with_steps(n)` | `n` positions including both ends |
| `kParameterIsLogarithmic` | `.with_taper(Taper::Log)` | Needs a minimum above zero |
| `kParameterIsOutput` | `.with_flag(ParamFlags::READ_ONLY)` | For meters prefer `Plugin::meters` or shared editor state |
| `kParameterIsTrigger` | `.with_flag(ParamFlags::MOMENTARY)` | |
| `parameter.enumValues` | `.with_choices(&[...])` | The value is the index |
| `parameter.designation = kParameterDesignationBypass` | `Plugin::set_host_bypass`, or a parameter with `ParamFlags::BYPASS` | |
| `parameter.groupId`, `initPortGroup` | `.with_group("Filter/Envelope")` | |
| `getParameterValue(index)` | `param(id)` | |
| `setParameterValue(index, value)` | `set_param(id, value)` | Override `set_param_index` to skip the string match |
| `parameter.midiCC`, `requestParameterValueChange` | Not available | See the list at the end |

### State

| DPF | OpenPluginSDK | Notes |
|---|---|---|
| Parameter values in the host's session, no state keys | Nothing to write | When parameters are the whole state, the SDK saves and restores them in every format |
| `DISTRHO_PLUGIN_WANT_STATE`, `initState`, `setState(key, value)` | `load_state(&[u8]) -> bool` | One blob, not a call per key. Change nothing when you return `false` |
| `DISTRHO_PLUGIN_WANT_FULL_STATE`, `getState(key)` | `save_state() -> Option<Vec<u8>>` | Put the parameter values in the bytes too: the CLAP entry stores this chunk alone when there is one. Start with a magic and a version |
| No state keys | `state_is_parameters_only()` returns `true` | Then a save or load never touches the engine |
| Key and value strings in an old session | `openplugin::core::foreign_state::kv`, `bytes::Reader` | See [Foreign state import](../PORTING.md#foreign-state-import) |
| `DISTRHO_PLUGIN_WANT_PROGRAMS`, `initProgramName`, `loadProgram` | `preset_count`, `preset_name`, `preset_values`, served by a `PresetBank` | A preset is a plain value per parameter, applied as parameter changes. Not published to the host's preset menu yet; see the list at the end |

### Editor

| DPF | OpenPluginSDK | Notes |
|---|---|---|
| `DISTRHO_PLUGIN_HAS_UI`, a class derived from `UI`, `createUI()` | `PluginInfo::has_editor`, an `impl Editor`, and a function `fn(EditorHost) -> Option<Box<dyn Editor>>` given to the export macros as `editor = ...` | |
| `DISTRHO_UI_DEFAULT_WIDTH`, `DISTRHO_UI_DEFAULT_HEIGHT`, `DISTRHO_UI_USER_RESIZABLE`, `setGeometryConstraints` | `Editor::size`, `Editor::size_range`, `Editor::resize` | Points. `SizeRange::with_aspect` keeps a shape |
| `onDisplay()`, `onNanoDisplay()` on OpenGL, Cairo or NanoVG | `Editor::render(scale) -> Frame` | Software pixels. `openplugin::dri` draws knobs, faders and meters on a `SceneCanvas`. There is no GL context |
| `getScaleFactor()` | The `scale` argument of `render` | |
| `parameterChanged(index, value)` | `Editor::param_changed(index, plain)` | |
| `editParameter(index, true)`, `setParameterValue(index, value)`, `editParameter(index, false)` | `EditorHost::begin`, `perform`, `end` | |
| `onMouse`, `onMotion`, `onScroll`, `onKeyboard` | `mouse_down`, `mouse_move`, `mouse_up`, `wheel`, `key_down` | Keys arrive only when `wants_keys` returns `true` |
| `uiIdle()` | `Editor::tick(dt)` | About 30 times a second |
| `stateChanged(key, value)`, `setState` from the UI | A shared `Arc` returned by `Plugin::editor_state` | The editor gets it through `Editor::bind` |
| `DISTRHO_PLUGIN_WANT_DIRECT_ACCESS`, `getPluginInstancePointer()` | The same shared `Arc` | The editor never holds the plugin itself |
| `sendNote` | Not available | See the list at the end |

### Buses

| DPF | OpenPluginSDK | Notes |
|---|---|---|
| `DISTRHO_PLUGIN_NUM_INPUTS`, `DISTRHO_PLUGIN_NUM_OUTPUTS` | `supported_layouts() -> &'static [IoConfig]` | The default is stereo in, stereo out. List more to let the host choose |
| `initAudioPort` with `kAudioPortIsSidechain` | `IoConfig { sidechain: Some(layout), .. }` and `process_buses(io, sidechain, frames)` | One optional sidechain |
| `port.groupId = kPortGroupMono`, `kPortGroupStereo` | `SpeakerLayout::Mono`, `SpeakerLayout::Stereo` | Surround layouts are in [SURROUND.md](../SURROUND.md) |
| `kAudioPortIsCV` | Not available | See the list at the end |

### MIDI

| DPF | OpenPluginSDK | Notes |
|---|---|---|
| `DISTRHO_PLUGIN_WANT_MIDI_INPUT`, `DISTRHO_PLUGIN_WANT_MIDI_OUTPUT` | `note_ports() -> NotePorts { input, output }` | An effect that reads no MIDI returns both `false` |
| The `MidiEvent` array passed to `run`, each with a `frame` | `note_on(offset, note, velocity)`, `note_off(offset, note)`, `midi(offset, [u8; 3])` | The entries split the block at each event, so there is no array to walk. A note-on with velocity 0 arrives as a note-on |
| `writeMidiEvent` | `pull_midi() -> Option<(u32, [u8; 3])>` | The entry drains it after each block |
| `MidiEvent::dataExt` for long messages | Not available | Messages of up to three bytes |

### Threading

| DPF | OpenPluginSDK | Notes |
|---|---|---|
| `run` on the audio thread | `process`, `set_param`, the note calls, `set_transport` and `reset` on the audio thread | One at a time, never two at once on the same plugin |
| `setParameterValue` from whichever thread the format uses | `set_param` between blocks or at an event's offset inside one | So the plugin's fields need no atomic and no lock |
| `setState`, `loadProgram` off the audio thread | `load_state`, `save_state` on a host thread, never while `process` runs | A block that starts during a state call passes the input through |
| UI callbacks on the main thread | Every `Editor` call on the main thread | |
| A thread of your own for loading | `openplugin::core::worker::loader` | In LV2 its jobs run on the host's worker |

## Worked port: a low-pass filter with programs

The original is a stereo low-pass filter. `DistrhoPluginInfo.h` declares two inputs, two outputs and `DISTRHO_PLUGIN_WANT_PROGRAMS`. `initParameter` declares `cutoff` (20 Hz to 20 kHz, logarithmic, default 1 kHz) and `resonance` (0.5 to 10, default 0.707). `initProgramName` and `loadProgram` give three programs. `activate` clears the filter state, `setParameterValue` stores the value and recomputes the coefficients, and `run` filters each channel.

The port uses the SDK's state-variable filter, which accepts new coefficients at any sample, ignores NaN at its input and flushes denormals in its state. The three programs become three preset texts.

```rust
use std::sync::OnceLock;

use openplugin::core::{restore_params, PresetBank};
use openplugin::dsp::{Svf, SvfMode};
use openplugin::prelude::*;
use openplugin_testing::{conformance, Options};

// `initParameter`. The ids are the old symbols.
static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("cutoff", "Cutoff", 20.0, 20_000.0, 1_000.0, Unit::Hz).with_taper(Taper::Log),
    ParamInfo::new("resonance", "Resonance", 0.5, 10.0, 0.707, Unit::None),
];

static INFO: PluginInfo = PluginInfo::new(
    "com.example.ported-lowpass",
    "Ported Low-pass",
    "Example Audio",
    "2.0.0",
    Category::Eq,
    &PARAMS,
)
.with_url("https://example.com");

// `initProgramName` and `loadProgram`. A parameter a preset leaves out keeps
// its default. In a plugin crate these are files embedded by
// `openplugin::core::build::embed_files`.
fn presets() -> &'static PresetBank {
    static BANK: OnceLock<PresetBank> = OnceLock::new();
    BANK.get_or_init(|| {
        let files = [
            ("open.preset", "name = Open\ncutoff = 18 kHz\n"),
            ("warm.preset", "name = Warm\ncutoff = 2.5 kHz\nresonance = 0.6\n"),
            ("telephone.preset", "name = Telephone\ncutoff = 900 Hz\nresonance = 4\n"),
        ];
        let (bank, errors) = PresetBank::parse_all(&INFO, &files);
        assert!(errors.is_empty(), "{errors:?}");
        bank
    })
}

pub struct PortedLowpass {
    cutoff: f32,
    resonance: f32,
    sample_rate: f32,
    filters: [Svf; 2],
    stale: bool,
}

impl Default for PortedLowpass {
    fn default() -> Self {
        Self {
            cutoff: PARAMS[0].default,
            resonance: PARAMS[1].default,
            sample_rate: 48_000.0,
            filters: [Svf::new(), Svf::new()],
            stale: true,
        }
    }
}

impl Plugin for PortedLowpass {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    // `sampleRateChanged`.
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.sample_rate = sample_rate;
        self.stale = true;
        self.reset();
    }

    // `activate`.
    fn reset(&mut self) {
        for filter in &mut self.filters {
            filter.reset();
        }
    }

    // `setParameterValue`. Never called while `process` runs.
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "cutoff" => self.cutoff = PARAMS[0].clamp(value),
            "resonance" => self.resonance = PARAMS[1].clamp(value),
            _ => return false,
        }
        self.stale = true;
        true
    }

    // `getParameterValue`.
    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "cutoff" => Some(self.cutoff),
            "resonance" => Some(self.resonance),
            _ => None,
        }
    }

    // `run`, in place.
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        if self.stale {
            self.stale = false;
            for filter in &mut self.filters {
                filter.configure(SvfMode::LowPass, self.sample_rate, self.cutoff, self.resonance, 0.0);
            }
        }
        for (channel, filter) in io.iter_mut().zip(&mut self.filters) {
            for sample in &mut channel[..frames] {
                *sample = filter.process_sample(*sample);
            }
        }
    }

    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }

    // No `initState` keys in the original: parameters are the whole state.
    fn state_is_parameters_only(&self) -> bool {
        true
    }

    fn preset_count(&self) -> usize {
        presets().len()
    }

    fn preset_name(&self, index: usize) -> &'static str {
        presets().name(index)
    }

    fn preset_values(&self, index: usize) -> Option<&'static [f32]> {
        presets().values(index)
    }

    // `DISTRHO_PLUGIN_URI`: saved LV2 sessions find the plugin by it.
    fn lv2_uri(&self) -> Option<&'static str> {
        Some("https://example.com/plugins/lowpass")
    }
}

openplugin::targets::export_clap!(PortedLowpass);
openplugin::targets::export_lv2!(PortedLowpass);
openplugin::targets::export_shell!(PortedLowpass);

// In a plugin crate the rest is `tests/conformance.rs`.
openplugin_testing::install_allocator!();

/// The level of a sine of `hz` after the filter, relative to its input.
fn gain_at(plugin: &mut PortedLowpass, hz: f32) -> f32 {
    plugin.reset();
    let mut peak = 0.0f32;
    let mut io = vec![vec![0.0f32; 64]; 2];
    for block in 0..200 {
        for i in 0..64 {
            let t = (block * 64 + i) as f32 / 48_000.0;
            io[0][i] = (std::f32::consts::TAU * hz * t).sin();
            io[1][i] = io[0][i];
        }
        plugin.process(&mut ChannelsMut::new(&mut io), 64);
        if block >= 100 {
            peak = io[0].iter().fold(peak, |p, s| p.max(s.abs()));
        }
    }
    peak
}

fn main() {
    let mut plugin = PortedLowpass::default();
    plugin.prepare(48_000.0, 64, 2);

    // Unity in the pass band, 12 dB per octave above the cutoff.
    assert!((gain_at(&mut plugin, 100.0) - 1.0).abs() < 0.02);
    let two_octaves_up = 20.0 * gain_at(&mut plugin, 4_000.0).log10();
    assert!((-26.0..=-22.0).contains(&two_octaves_up), "{two_octaves_up} dB");

    // The third program, applied the way a host applies a preset.
    assert_eq!(plugin.preset_count(), 3);
    assert_eq!(plugin.preset_name(2), "Telephone");
    let values = plugin.preset_values(2).unwrap();
    assert_eq!(values, [900.0, 4.0]);
    assert!(restore_params(&mut plugin, values));
    assert_eq!(plugin.param("cutoff"), Some(900.0));

    conformance(PortedLowpass::default, Options::quick()).assert_pass();
}
```

What moved where:

- `DistrhoPluginInfo.h` became `INFO`, `note_ports` and `lv2_uri`. The channel counts need nothing, because stereo in and stereo out is the default layout.
- `initParameter` became the `PARAMS` table. The hint `kParameterIsLogarithmic` is `Taper::Log`; the unit string is `Unit::Hz`, which also makes hosts show `2.50 kHz` and read `900 Hz`.
- `setParameterValue` marks the coefficients stale and `process` recomputes them once. The entries split a block at every parameter event, so a change still lands on its sample.
- `loadProgram` became data. A preset cannot run code, so a program that also cleared a buffer or loaded a file needs that behaviour moved into `set_param` or a parameter of its own. Today the bank serves a Rust host and the plugin's own editor; the host's program menu stays empty until the format entries publish it.
- The separate output buffer is gone. A `run` that reads `inputs[0][i]` after it wrote `outputs[0][i]` must copy the input sample first.

## What has no counterpart yet

- **Programs in the host's menu.** `preset_count`, `preset_name` and `preset_values` reach a Rust host through `openplugin-host`, and the plugin's own editor can offer them, but no format entry publishes a program list to the host yet.
- **CV ports.** Audio buses only.
- **A parameter bound to a MIDI CC in the descriptor** (`midiCC`). Read the CC in `Plugin::midi` and map it yourself.
- **A parameter change requested by the plugin** (`requestParameterValueChange`). Only the host and the editor change parameters.
- **Notes sent from the UI** (`sendNote`).
- **Long MIDI messages.** MIDI reaches the plugin as messages of up to three bytes.
- **OpenGL, Cairo or NanoVG drawing.** The editor contract is a frame of pixels; drawing code is rewritten against `openplugin::dri` or your own rasteriser.
- **More than one output bus, or more than one sidechain.**
- **Keeping the old plugin's saved state outside CLAP and LV2.** The identity itself carries over: the CLAP id, the LV2 URI, the VST3 class ids and the parameter ids are pinned in the plugin (see [Identity pinning](../PORTING.md#identity-pinning)). The VST3, Audio Unit and AAX shells still accept only state they wrote themselves, so state saved by the old plugin reaches `load_state` only through CLAP; that part is open.
