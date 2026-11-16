# Porting from JUCE

This guide maps a JUCE plugin onto OpenPluginSDK: the processor, parameters, state, editor, buses, MIDI and threading, then a small effect ported in full, including opening the old plugin's saved state. It is written from JUCE's public documentation and names its public classes and methods so you can find the matching place in your own project. It contains no JUCE code. The Rust in it is original code for this SDK, and every code block is compiled and run as a doc test (`cargo test -p example-porting-guides`). API names are the ones JUCE publishes at the time of writing; check them against the version you build with.

Read [PORTING.md](../PORTING.md) first: it has the order of work and the tools (scanner, state capture, null test) that every port uses.

## The short version

- A port is a rewrite of the plugin layer, not a translation. Nothing from the JUCE modules comes along: no `AudioBuffer`, no `ValueTree`, no `Component`. Where the old code used a JUCE DSP class, use a block from `openplugin::dsp` or write the DSP from the paper it implements, and prove the result against the original with the null test.
- One `struct` implements `Plugin` and `Default`. The CMake `juce_add_plugin` properties become a `static PluginInfo`, a few `Plugin` methods and build options.
- The parameter tree becomes a `static` table of `ParamInfo`. The plugin keeps current values in plain fields that the SDK writes with `set_param`, never while `process` runs. There are no atomics to read and no listeners.
- Values are plain units everywhere. Nothing in the plugin or the editor handles 0-to-1 values unless it wants to.
- The editor is a separate object that draws a frame of pixels and reports edits as gestures. It holds no reference to the processor.

## Concept map

### Processor

| JUCE | OpenPluginSDK | Notes |
|---|---|---|
| A class derived from `AudioProcessor`; `createPluginFilter()` | A `struct` with `impl Plugin` and `impl Default`; the export macros | `Default::default()` is what the format entries call |
| `juce_add_plugin`: `PRODUCT_NAME`, `COMPANY_NAME`, `VERSION` | `PluginInfo::new(..)` | Returned by `Plugin::info` |
| `BUNDLE_ID`, `PLUGIN_MANUFACTURER_CODE`, `PLUGIN_CODE` | `PluginInfo::id`, and `--bundle-id`, `--au-manufacturer`, `--au-subtype` of `scripts/build_plugin.sh` | See [BUILD.md](../BUILD.md). Keep the old four-character codes for the Audio Unit |
| `AAX_IDENTIFIER` and the AAX codes | `Plugin::aax_ids` | |
| `LV2URI` | `Plugin::lv2_uri` | |
| `FORMATS` | `export_clap!`, `export_lv2!`, `export_shell!` | The shell export serves VST3, Audio Unit and AAX |
| `prepareToPlay(sampleRate, samplesPerBlock)` | `prepare(sample_rate, max_block, channels)` | Allocate here |
| `releaseResources()` | Nothing; `Drop` | |
| `reset()` | `reset()` | Must not allocate |
| `processBlock(AudioBuffer<float>&, MidiBuffer&)` | `process(io: &mut [&mut [f32]], frames)` | In place, planar: `io[channel][..frames]`. MIDI arrives separately |
| `supportsDoublePrecisionProcessing`, the `double` overload | Not available | See the list at the end |
| `processBlockBypassed`, `getBypassParameter` | `Plugin::set_host_bypass`, `ParamFlags::BYPASS`, `openplugin::dsp::SoftBypass` | The shells already delay and crossfade the dry signal by the reported latency |
| `setLatencySamples` | `Plugin::latency` | The entries read it after every block. `HostNotifier::latency_changed` reports a change made outside `process` |
| `getTailLengthSeconds` | `Plugin::tail` | `Tail::from_samples(n)`. `Tail::Infinite` exists |
| `isNonRealtime` | `Plugin::set_offline` | |
| `getPlayHead()`, `AudioPlayHead::PositionInfo` | `Plugin::set_transport(&Transport)` | Called before each block; keep the fields you use |
| `ScopedNoDenormals` | Nothing to write | Every entry flushes denormals around the audio calls (`Plugin::flush_denormals`) |
| `dsp::ProcessSpec`, `dsp::IIR`, `dsp::DelayLine`, `dsp::Oversampling`, `SmoothedValue` | `openplugin::dsp`: `Biquad`, `Svf`, `ModDelay`, `MinPhaseHalfband`, `LinearPhaseOversampler`; `OnePole`, `SampleRamp` | Not drop-in equivalents: responses differ in detail. See [DSP_COOKBOOK.md](../DSP_COOKBOOK.md) and check with the null test |

### Parameters

| JUCE | OpenPluginSDK | Notes |
|---|---|---|
| `AudioProcessorValueTreeState` with a `ParameterLayout` | `static PARAMS: [ParamInfo; N]` | The position in the table is the parameter's index in every format, so keep the old order |
| `AudioParameterFloat` with a `ParameterID`, a name, a `NormalisableRange` and a default | `ParamInfo::new(id, name, min, max, default, unit)` | Keep the id string |
| The skew of a `NormalisableRange`, `setSkewForCentre` | `.with_taper(Taper::Log)` | Only linear and logarithmic travel exist; see the list at the end |
| The interval of a `NormalisableRange`, `AudioParameterInt` | `.with_steps(n)` | `n` positions including both ends |
| `AudioParameterBool` | `Unit::Toggle` | |
| `AudioParameterChoice` | `.with_choices(&[...])` | The value is the index |
| `withLabel`, `withStringFromValueFunction`, `withValueFromStringFunction` | `Unit::Db`, `Unit::Hz`, `Unit::Ms`, ... | The unit prints and parses the value. No custom text function; see the list at the end |
| `withAutomatable(false)` | `.with_flag(ParamFlags::NOT_REALTIME)` | |
| `AudioProcessorParameterGroup` | `.with_group("Filter/Envelope")` | |
| `getRawParameterValue(id)` read in `processBlock` | A field of the plugin, written in `set_param` | Plain units, clamped by `ParamInfo::clamp` |
| A parameter listener's `parameterChanged` | `set_param(id, value)` | Compute coefficients here. Override `set_param_index` to skip the string match |
| `getValue`, `setValueNotifyingHost`, `convertTo0to1`, `convertFrom0to1` | `ParamInfo::to_normalized`, `ParamInfo::from_normalized` | Rarely needed: the SDK's calls carry plain values |
| `SmoothedValue` | `OnePole`, `SampleRamp`, `openplugin::dsp::Gain`, `openplugin::dsp::Glide` | Owned by the plugin and reset in `reset` |

### State

| JUCE | OpenPluginSDK | Notes |
|---|---|---|
| `getStateInformation` that only copies the parameter tree | Nothing to write | When parameters are the whole state, return `true` from `state_is_parameters_only`: the SDK saves and restores them in every format |
| `getStateInformation(MemoryBlock&)`, `setStateInformation(data, size)` | `save_state() -> Option<Vec<u8>>`, `load_state(&[u8]) -> bool` | For a plugin with more state than its parameters, or one that must read the old plugin's state. Put the parameter values in the bytes too: the CLAP entry stores this chunk alone when there is one. Start with a magic and a version, and change nothing when you return `false` |
| State written as XML (`createXml`, `copyXmlToBinary`) | `openplugin::core::foreign_state::xml` | Capture a state with `openplugin-port capture` and look at it; the worked port below reads one |
| State written as a binary stream | `openplugin::core::foreign_state::bytes::Reader` | Read field by field, from what the capture shows |
| `getNumPrograms`, `getProgramName`, `setCurrentProgram` | `preset_count`, `preset_name`, `preset_values`, served by a `PresetBank` | A preset is a plain value per parameter. Not published to the host's preset menu yet; see the list at the end |

### Editor

| JUCE | OpenPluginSDK | Notes |
|---|---|---|
| `hasEditor`, `createEditor`, a class derived from `AudioProcessorEditor` | `PluginInfo::has_editor`, an `impl Editor`, and a function `fn(EditorHost) -> Option<Box<dyn Editor>>` given to the export macros as `editor = ...` | The editor is made without the plugin |
| `paint(Graphics&)` | `Editor::render(scale) -> Frame` | Software pixels. `openplugin::dri` draws knobs, faders, plates and meters on a `SceneCanvas` |
| `setSize`, `setResizable`, `setResizeLimits`, a fixed aspect ratio; `resized()` | `Editor::size`, `Editor::size_range` (`SizeRange::with_aspect`), `Editor::resize` | Points. `EditorHost::request_resize` asks the host for a size |
| `Slider`, `Button`, `ComboBox` with `SliderAttachment`, `ButtonAttachment`, `ComboBoxAttachment` | `openplugin::dri::ControlSet` with `Control::knob`, `Control::toggle` | Hit testing, drags, keys, typing a value; see [EDITOR.md](../EDITOR.md) |
| `beginChangeGesture`, `setValueNotifyingHost`, `endChangeGesture` | `EditorHost::begin`, `perform`, `end` | By table index, in plain units |
| A parameter listener in the editor | `Editor::param_changed(index, plain)` | |
| `Timer::timerCallback` | `Editor::tick(dt)` | About 30 times a second |
| `mouseDown`, `mouseDrag`, `mouseUp`, `mouseWheelMove`, `keyPressed` | `mouse_down`, `mouse_move`, `mouse_up`, `wheel`, `key_down` | Keys arrive only when `wants_keys` returns `true` |
| `LookAndFeel` | The knob, plate and meter styles of `openplugin::dri`; an editor built from a design file | |
| `AccessibilityHandler` | `Editor::accessibility_nodes`, `accessibility_action` | `ControlSet` provides them |
| `FileChooser`, `FileDragAndDropTarget`, `TooltipWindow` | `take_file_dialog`, `files_dropped`, `tooltips` | |
| The host's context menu for a parameter | `EditorHost::context_menu` | |
| A reference to the processor held by the editor | The `Arc` returned by `Plugin::editor_state`, handed to `Editor::bind` | The engine can be replaced while the editor is open |
| OpenGL rendering, a web view | Not available | See the list at the end |

### Buses

| JUCE | OpenPluginSDK | Notes |
|---|---|---|
| `BusesProperties` with one input and one output | `supported_layouts() -> &'static [IoConfig]` | The default is stereo in, stereo out |
| `isBusesLayoutSupported` | The list itself, and `set_layout` | A list of accepted layouts, not a predicate. The first is the default |
| A second input bus used as a sidechain, `getBusBuffer` | `IoConfig { sidechain: Some(layout), .. }` and `process_buses(io, sidechain, frames)` | One optional sidechain |
| `AudioChannelSet::mono()`, `stereo()`, `create5point1()`, `create7point1point4()` | `SpeakerLayout::Mono`, `Stereo`, `Surround51`, `Surround714` | The plugin always sees one channel order; find a channel with `SpeakerLayout::index_of`. See [SURROUND.md](../SURROUND.md) |
| More than one output bus | Not available | See the list at the end |

### MIDI

| JUCE | OpenPluginSDK | Notes |
|---|---|---|
| `acceptsMidi`, `producesMidi`, `isMidiEffect` | `note_ports() -> NotePorts { input, output }`, `PluginInfo::is_instrument` | An effect that reads no MIDI returns both `false` |
| Iterating the `MidiBuffer` by sample position | `note_on(offset, note, velocity)`, `note_off(offset, note)`, `midi(offset, [u8; 3])` | The entries split the block at each event, so there is nothing to iterate. A note-on with velocity 0 arrives as a note-on |
| Adding events to the `MidiBuffer` | `pull_midi() -> Option<(u32, [u8; 3])>` | The entry drains it after each block |
| `Synthesiser`, `MPESynthesiser` | `openplugin::dsp::voice_alloc`, `Plugin::mpe_input` | See [EXPRESSION.md](../EXPRESSION.md) |
| SysEx messages | Not available | See the list at the end |

### Threading

| JUCE | OpenPluginSDK | Notes |
|---|---|---|
| `processBlock` on the audio thread; parameters read through atomics | `process`, `set_param`, the note calls, `set_transport` and `reset` on the audio thread, one at a time | The entries carry values across threads in a lock-free plane; the plugin's fields need no atomic |
| `setStateInformation` on any thread, possibly while audio runs | `load_state`, `save_state` on a host thread, never while `process` runs | The entries hold the plugin between blocks; a block that starts during a state call passes the input through |
| `getCallbackLock`, `suspendProcessing` | Nothing to write | No call overlaps `process` |
| `MessageManager`, `AsyncUpdater`, `Timer` for audio-to-UI updates | Atomics or a `Seqlock` in the shared editor state, read in `Editor::tick` | The audio thread never posts a message |
| `Thread`, `ThreadPool` for loading files | `openplugin::core::worker::loader` | Builds a value on a background thread and swaps it in on the audio thread without a lock |

## Worked port: an echo that opens the old plugin's sessions

The original is a stereo echo. Its parameter layout has `delayTime` in seconds (0.01 to 1, default 0.3), `feedback` in percent (0 to 90, default 35) and `mix` in percent (0 to 100, default 30). `prepareToPlay` sizes two delay lines, `processBlock` reads the three values, and `getTailLengthSeconds` reports the decay. `getStateInformation` writes the parameter tree as XML.

A capture of the old plugin's state (`openplugin-port capture`, see [PORTING.md](../PORTING.md#foreign-state-import)) shows what the port must read: a short binary header, then an XML document whose root has one `PARAM` element per parameter with `id` and `value` attributes holding the plain value, then a zero byte. Your plugin's capture will differ; read your own.

The port names the first parameter `time` and declares it in milliseconds, which reads better in a host. That is a deliberate change, so the import converts it.

```rust
use openplugin::core::foreign_state::import::{import, ImportedState, ParamMap, StateFormat};
use openplugin::core::foreign_state::{xml, ErrorKind, ForeignError};
use openplugin::dsp::{Glide, Interpolation, ModDelay};
use openplugin::prelude::*;
use openplugin_testing::{conformance, Options};

static PARAMS: [ParamInfo; 3] = [
    ParamInfo::new("time", "Time", 10.0, 1_000.0, 300.0, Unit::Ms),
    ParamInfo::new("feedback", "Feedback", 0.0, 90.0, 35.0, Unit::Percent),
    ParamInfo::new("mix", "Mix", 0.0, 100.0, 30.0, Unit::Percent),
];

static INFO: PluginInfo = PluginInfo::new(
    "com.example.ported-echo",
    "Ported Echo",
    "Example Audio",
    "2.0.0",
    Category::Delay,
    &PARAMS,
)
.with_url("https://example.com");

// ---- The old plugin's state -------------------------------------------------

/// The XML document inside an old state: from the first `<` (which must be
/// near the front) to the last `>`.
fn xml_part(data: &[u8]) -> Option<&[u8]> {
    let start = data.iter().position(|b| *b == b'<')?;
    let end = data.iter().rposition(|b| *b == b'>')?;
    (start <= 16 && end > start).then(|| &data[start..=end])
}

fn old_detect(data: &[u8]) -> Option<u32> {
    xml_part(data).map(|_| 1)
}

fn old_decode(data: &[u8], version: u32) -> Result<ImportedState, ForeignError> {
    let invalid = |what| ForeignError { kind: ErrorKind::Invalid, offset: 0, what };
    let root = xml::parse(xml_part(data).ok_or(invalid("no XML document"))?)?;
    let mut state = ImportedState::new("old-echo-xml", version);
    for param in root.children_named("PARAM") {
        let id = param.attr("id").ok_or(invalid("a PARAM without an id"))?;
        let value = param.attr("value").and_then(|v| v.parse::<f64>().ok()).ok_or(invalid("a PARAM without a number"))?;
        state.push_plain(id.into(), value);
    }
    Ok(state)
}

static OLD_FORMATS: [StateFormat; 1] = [StateFormat { name: "old-echo-xml", detect: old_detect, decode: old_decode }];

/// Old ids and units to the port's.
fn old_params() -> ParamMap {
    let mut map = ParamMap::new();
    map.map_with("delayTime", "time", |seconds| seconds * 1_000.0);
    map.map("feedback", "feedback");
    map.map("mix", "mix");
    map
}

// ---- The port's own state ---------------------------------------------------

const MAGIC: &[u8; 4] = b"ECH1";

fn encode_own(values: [f32; 3]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn decode_own(data: &[u8]) -> Option<[f32; 3]> {
    let body = data.strip_prefix(MAGIC)?;
    if body.len() != 12 {
        return None;
    }
    let mut values = [0.0f32; 3];
    for (index, bytes) in body.chunks_exact(4).enumerate() {
        let value = f32::from_le_bytes(bytes.try_into().ok()?);
        if !value.is_finite() {
            return None;
        }
        values[index] = PARAMS[index].clamp(value);
    }
    Some(values)
}

// ---- The plugin ---------------------------------------------------------------

pub struct PortedEcho {
    values: [f32; 3],
    sample_rate: f32,
    lines: [ModDelay; 2],
    /// The delay in samples, moved gradually so a change of time is a short
    /// sweep and not a click.
    delay: Glide,
    tail: usize,
}

impl Default for PortedEcho {
    fn default() -> Self {
        let line = || ModDelay::new(48_000.0, PARAMS[0].max, Interpolation::Linear);
        let mut echo = Self {
            values: [PARAMS[0].default, PARAMS[1].default, PARAMS[2].default],
            sample_rate: 48_000.0,
            lines: [line(), line()],
            delay: Glide::new(48_000.0, 50.0),
            tail: 0,
        };
        echo.update();
        echo
    }
}

impl PortedEcho {
    fn delay_samples(&self) -> f32 {
        self.values[0] * 0.001 * self.sample_rate
    }

    /// The tail: the number of repeats until the echo is 120 dB down, times the delay.
    fn update(&mut self) {
        let feedback = self.values[1] / 100.0;
        let repeats = if feedback > 0.0 { (1e-6f32.ln() / feedback.ln()).ceil() } else { 1.0 };
        self.tail = (self.delay_samples() * repeats).ceil() as usize;
    }

    fn apply(&mut self, values: [f32; 3]) {
        self.values = values;
        self.update();
    }
}

impl Plugin for PortedEcho {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    // `prepareToPlay`: the only place that allocates.
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.sample_rate = sample_rate;
        for line in &mut self.lines {
            line.prepare(sample_rate, PARAMS[0].max);
        }
        self.delay.set_time(sample_rate, 50.0);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        for line in &mut self.lines {
            line.reset();
        }
        self.delay.set_immediate(self.delay_samples());
    }

    // What the parameter listener did. Never called while `process` runs.
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(index) = PARAMS.iter().position(|p| p.id == id) else { return false };
        self.values[index] = PARAMS[index].clamp(value);
        self.update();
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        PARAMS.iter().position(|p| p.id == id).map(|index| self.values[index])
    }

    // `processBlock`, in place.
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let target = self.delay_samples();
        let feedback = self.values[1] / 100.0;
        let mix = self.values[2] / 100.0;
        for i in 0..frames {
            // Read before writing, so a delay of `d` reads the sample `d` ago.
            let delay = (self.delay.next(target) - 1.0).max(0.0);
            for (channel, line) in io.iter_mut().zip(&mut self.lines) {
                let dry = channel[i];
                let wet = line.read(delay);
                // The line stores NaN and infinity as silence and flushes denormals.
                line.write(dry + wet * feedback);
                channel[i] = dry + (wet - dry) * mix;
            }
        }
    }

    // `getTailLengthSeconds`, in samples.
    fn tail(&self) -> Tail {
        Tail::from_samples(self.tail)
    }

    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }

    // `getStateInformation`. The port has a chunk of its own so that it is
    // handed the old plugin's bytes; the chunk holds the parameters.
    fn save_state(&mut self) -> Option<Vec<u8>> {
        Some(encode_own(self.values))
    }

    // `setStateInformation`: the old format if it is one, else the port's own.
    // Everything is decoded and checked before anything changes.
    fn load_state(&mut self, data: &[u8]) -> bool {
        let values = match import(&OLD_FORMATS, data) {
            Some(Ok(state)) => {
                let applied = old_params().apply(&PARAMS, &state);
                if !applied.unmapped.is_empty() || applied.values.len() != PARAMS.len() {
                    return false;
                }
                let mut values = self.values;
                for (index, value) in applied.values {
                    values[index] = value;
                }
                values
            }
            // Recognised but damaged: refuse it, do not guess.
            Some(Err(_)) => return false,
            None => match decode_own(data) {
                Some(values) => values,
                None => return false,
            },
        };
        self.apply(values);
        true
    }
}

openplugin::targets::export_clap!(PortedEcho);
openplugin::targets::export_lv2!(PortedEcho);
openplugin::targets::export_shell!(PortedEcho);

// In a plugin crate the rest is `tests/conformance.rs` and a state test over
// the captured blobs.
openplugin_testing::install_allocator!();

fn main() {
    // An impulse comes back after the delay, at the mix level.
    let mut plugin = PortedEcho::default();
    plugin.prepare(48_000.0, 512, 2);
    plugin.set_param("time", 10.0);
    plugin.set_param("mix", 50.0);
    plugin.reset();
    let mut io = vec![vec![0.0f32; 512]; 2];
    io[0][0] = 1.0;
    plugin.process(&mut ChannelsMut::new(&mut io), 512);
    assert_eq!(io[0][0], 0.5);
    assert!(io[0][1..470].iter().all(|s| *s == 0.0));
    assert!((io[0][480] - 0.5).abs() < 1e-3, "{}", io[0][480]);

    // A state as the old plugin saved it: a header, XML, a zero byte.
    let mut old = vec![0x10, 0x32, 0x54, 0x76, 0x60, 0, 0, 0];
    old.extend_from_slice(
        br#"<?xml version="1.0" encoding="UTF-8"?> <Echo><PARAM id="delayTime" value="0.25"/><PARAM id="feedback" value="60.0"/><PARAM id="mix" value="45.0"/></Echo>"#,
    );
    old.push(0);
    let mut plugin = PortedEcho::default();
    assert!(plugin.load_state(&old));
    assert_eq!(plugin.values, [250.0, 60.0, 45.0]);

    // The port's own format round-trips.
    let saved = plugin.save_state().unwrap();
    let mut fresh = PortedEcho::default();
    assert!(fresh.load_state(&saved));
    assert_eq!(fresh.values, plugin.values);

    // A damaged or unknown state is refused and changes nothing.
    let before = fresh.values;
    assert!(!fresh.load_state(&old[..old.len() - 20]));
    assert!(!fresh.load_state(&old[..old.len() - 1].iter().map(|b| if *b == b'6' { b'x' } else { *b }).collect::<Vec<u8>>()));
    assert!(!fresh.load_state(b"something else"));
    assert_eq!(fresh.values, before);

    conformance(PortedEcho::default, Options::quick()).assert_pass();
}
```

What moved where:

- The parameter layout became the `PARAMS` table, in the old order. `set_param` replaces the listener; because it never overlaps `process`, the values are a plain array.
- The delay lines are the SDK's `ModDelay`, sized in `prepare`. It stores NaN and infinity as silence and flushes denormals, two things the conformance kit checks and a hand-written buffer has to do itself.
- `getTailLengthSeconds` became `tail`, computed when a parameter changes, because the entries read it once per block on the audio thread.
- `setStateInformation` became `load_state` with the import pattern: detect by looking at the front of the bytes, decode into the old ids and values, map them to the port's ids and units, and only then change the plugin. A state that is recognised but damaged is refused.
- The port saves a chunk of its own, with the parameter values in it. A plugin that saves no chunk has its parameters stored by the SDK and is never handed foreign bytes.

Keep the captured blobs in the port's repository and load each one in a test, comparing every parameter with the values the capture recorded. `crates/openplugin-port/tests/state_import.rs` is that test for the SDK's own example.

## What has no counterpart yet

- **Skewed ranges.** `Taper` is linear or logarithmic. A parameter with a skew factor keeps its range and plain values, but a host knob travels differently from the old plugin's, and normalised automation recorded against the old skew plays back at different plain values.
- **Custom value text.** A parameter prints and parses by its `Unit` and its choice names.
- **64-bit audio buffers.** `process` is `f32`.
- **More than one output bus, or more than one sidechain.** An `IoConfig` is one main input, one main output and one optional sidechain.
- **SysEx.** MIDI reaches the plugin as messages of up to three bytes.
- **OpenGL and web view editors.** The editor contract is a frame of pixels.
- **Factory presets in the host's menu.** `preset_count`, `preset_name` and `preset_values` reach a Rust host through `openplugin-host`, and the plugin's own editor can offer them, but no format entry publishes a program list to the host yet.
- **An undo manager.** `AbCompare` gives an A/B pair of parameter snapshots; there is no undo history.
- **Keeping the old plugin's saved state outside CLAP.** The identity itself carries over: the Audio Unit codes, the VST3 class ids and JUCE's hashed parameter ids are pinned in the plugin (`Plugin::au_ids`, `Plugin::vst3_ids`, `ParamInfo::with_host_id`; see [Identity pinning](../PORTING.md#identity-pinning), and `openplugin-port scan` writes them). The VST3, Audio Unit and AAX shells still accept only state they wrote themselves, so the old plugin's bytes reaches `load_state` only through CLAP; that part is open.
