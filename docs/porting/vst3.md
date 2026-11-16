# Porting from the VST3 SDK

This guide maps a plugin written directly against Steinberg's VST3 SDK onto OpenPluginSDK: the processor, parameters, state, editor, buses, MIDI and threading, then a small effect ported in full. It is written from the VST3 SDK's public documentation and public interface names. The Rust in it is original code for this SDK, and every code block is compiled and run as a doc test (`cargo test -p example-porting-guides`). Interface names are the ones the VST3 SDK publishes at the time of writing; check them against the version you build with.

Read [PORTING.md](../PORTING.md) first: it has the order of work and the tools (scanner, state capture, null test) that every port uses.

## The short version

- The processor and the edit controller become one `struct` that implements `Plugin`. The SDK's VST3 shell (`native/vst3`, [SHELL.md](../SHELL.md)) is the component and the controller a host loads, and it also gives you CLAP, LV2, Audio Unit and AAX from the same code.
- Values are plain units. The shell converts the host's normalised 0-to-1 values with the parameter's range and taper before they reach you, and prints and parses them with the parameter's unit.
- There are no parameter queues to walk. The shell cuts the block at every change point and calls `set_param` between the pieces, so `process` always sees settled values.
- There is no message channel between a controller and a processor, because there is no second object. The editor shares an `Arc` with the engine for what it needs to see.

## Concept map

### Processor

| VST3 SDK | OpenPluginSDK | Notes |
|---|---|---|
| `AudioEffect` (`IComponent`, `IAudioProcessor`) and `EditController` (`IEditController`), or `SingleComponentEffect` | One `struct` with `impl Plugin` and `impl Default` | The shell provides both VST3 classes |
| The factory: `DEF_CLASS2`, `kVstAudioEffectClass`, `kVstComponentControllerClass`, the two class `FUID`s | `export_shell!`, and the shell built by `scripts/build_plugin.sh` | The class ids are derived from `PluginInfo::id` and cannot be set yet; see the list at the end |
| The factory's vendor, URL, e-mail and version; the class name | `PluginInfo { vendor, url, email, version, name, .. }` | |
| Subcategories such as `Fx`, `Fx\|Delay`, `Instrument\|Synth` | `PluginInfo::category`, `PluginInfo::is_instrument` | |
| `initialize`, `terminate` | `Default::default`, `Drop` | |
| `setupProcessing(ProcessSetup&)`: `sampleRate`, `maxSamplesPerBlock` | `prepare(sample_rate, max_block, channels)` | Allocate here |
| `ProcessSetup::processMode`, `kOffline` | `Plugin::set_offline` | |
| `canProcessSampleSize(kSample64)` | Not available | `process` is `f32`; see the list at the end |
| `setActive`, `setProcessing` | `prepare` before the first block; `reset` to clear history | `reset` must not allocate |
| `process(ProcessData&)`: `inputs[0].channelBuffers32`, `outputs[0].channelBuffers32`, `numSamples` | `process(io: &mut [Vec<f32>], frames)` | In place, planar: `io[channel][..frames]` |
| A `process` call with `numSamples == 0` to flush parameters | Nothing to write | The shell applies the changes |
| `getLatencySamples`, `restartComponent(kLatencyChanged)` | `Plugin::latency`, `HostNotifier::latency_changed` | The shell also compares `latency` after every block |
| `getTailSamples`, `kNoTail`, `kInfiniteTail` | `Plugin::tail`: `Tail::None`, `Tail::Finite(n)`, `Tail::Infinite` | |
| `ProcessData::processContext`: `state & kPlaying`, `tempo`, `projectTimeSamples`, `projectTimeMusic`, `barPositionMusic`, `timeSigNumerator` | `Plugin::set_transport(&Transport)`: `playing`, `tempo`, `sample_pos`, `quarter_notes`, `bar_start`, `numerator` | Called before each block. A field the host marks invalid keeps its default |
| A bypass parameter with `kIsBypass` | `Plugin::set_host_bypass` | The shell adds the parameter and outputs the input delayed by the reported latency, crossfaded |

### Parameters

| VST3 SDK | OpenPluginSDK | Notes |
|---|---|---|
| `parameters.addParameter(...)`, `RangeParameter` | `static PARAMS: [ParamInfo; N]` with `ParamInfo::new(id, name, min, max, default, unit)` | |
| The `ParamID` tag | The position in the table, or `ParamInfo::with_vst3_id` | The shell numbers parameters `0..n` in table order. Keep the old order; ids that were not `0..n` are kept with `ParamInfo::with_vst3_id` |
| `StringListParameter`, `ParameterInfo::kIsList` | `.with_choices(&[...])` | The value is the index |
| `ParameterInfo::stepCount` | `.with_steps(step_count + 1)` | Positions including both ends |
| `kCanAutomate` | The default | |
| `kIsReadOnly`, `kIsHidden` | `.flag(ParamFlags::READ_ONLY)`, `.flag(ParamFlags::HIDDEN)` | |
| `ParameterInfo::unitId`, `addUnit`, `IUnitInfo` | `.with_group("Filter/Envelope")` | The shell builds the units |
| `ParameterInfo::units` | `Unit::Db`, `Unit::Hz`, `Unit::Ms`, ... | |
| `normalizedParamToPlain`, `plainParamToNormalized`, `Parameter::toPlain` | `ParamInfo::from_normalized`, `ParamInfo::to_normalized` | Done by the shell. Linear or logarithmic only; see the list at the end |
| `getParamStringByValue`, `getParamValueByString`, `Parameter::toString`, `fromString` | `ParamInfo::format`, `ParamInfo::parse` | By unit and choice names |
| `inputParameterChanges`, `IParamValueQueue::getPoint(index, sampleOffset, value)` | `set_param(id, value)` | Called at each point's offset, in plain units. Values step at the point; use `OnePole` or `SampleRamp` where a ramp is wanted |
| The processor's copy and the controller's copy of each value | One field in the plugin | |
| `outputParameterChanges` for meters | `Plugin::meters`, or the shared editor state | |
| `restartComponent(kParamTitlesChanged)` | `HostNotifier::param_info_changed` | |

### State

| VST3 SDK | OpenPluginSDK | Notes |
|---|---|---|
| `IComponent::getState` and `setState` that only stream the parameter values; `setComponentState` in the controller | Nothing to write | When parameters are the whole state, return `true` from `state_is_parameters_only`: the SDK saves and restores them in every format |
| `getState`, `setState` with more than parameters | `save_state() -> Option<Vec<u8>>`, `load_state(&[u8]) -> bool` | Put the parameter values in the bytes too: the CLAP entry stores this chunk alone when there is one. Start with a magic and a version, and change nothing when you return `false` |
| `IEditController::getState`, `setState` (controller-only state) | The same chunk | There is one state |
| `IBStreamer` reads of an old state | `openplugin::core::foreign_state::bytes::Reader` | Mind the byte order the old code chose. See [Foreign state import](../PORTING.md#foreign-state-import) |
| Program lists (`IUnitInfo::getProgramListInfo`, `IProgramListData`) | `preset_count`, `preset_name`, `preset_values`, served by a `PresetBank` | Not published to the host yet: the VST3 shell has no program lists; see the list at the end |

### Editor

| VST3 SDK | OpenPluginSDK | Notes |
|---|---|---|
| `IEditController::createView`, `IPlugView` | `PluginInfo::has_editor`, an `impl Editor`, and a function `fn(EditorHost) -> Option<Box<dyn Editor>>` given to the export macros as `editor = ...` | Build the shells with the editor on (`--editor`) |
| VSTGUI: `VST3Editor`, a `.uidesc` file, `CKnob`, `CSlider` | `Editor::render(scale) -> Frame`, with `openplugin::dri` for knobs, faders and meters | Software pixels. See [EDITOR.md](../EDITOR.md), also for editors built from a design file |
| `IPlugView::getSize`, `canResize`, `checkSizeConstraint`, `onSize`; `IPlugFrame::resizeView` | `Editor::size`, `Editor::size_range`, `Editor::resize`; `EditorHost::request_resize` | Points |
| `IPlugViewContentScaleSupport::setContentScaleFactor` | The `scale` argument of `render` | |
| `IComponentHandler::beginEdit`, `performEdit`, `endEdit` with normalised values | `EditorHost::begin`, `perform`, `end` | By table index, in plain units |
| `EditController::setParamNormalized` reaching the view | `Editor::param_changed(index, plain)` | |
| `IConnectionPoint`, `IMessage`, `sendMessage`, `notify` | The `Arc` returned by `Plugin::editor_state`, handed to `Editor::bind` | Atomics, a `Seqlock` or a lock-free queue. There are no messages |
| `IComponentHandler3::createContextMenu` | `EditorHost::context_menu` | |
| `IInfoListener::setChannelContextInfos` | `Plugin::track_info_changed`, `Editor::track_info` | Track name and colour |
| A timer for meters | `Editor::tick(dt)` | About 30 times a second |

### Buses

| VST3 SDK | OpenPluginSDK | Notes |
|---|---|---|
| `addAudioInput`, `addAudioOutput` with a `SpeakerArrangement` | `supported_layouts() -> &'static [IoConfig]` | The default is stereo in, stereo out |
| `setBusArrangements`, `getBusArrangement` | The list itself, and `set_layout` | The shell accepts a set of arrangements that equals one listed layout |
| `addAudioInput(..., kAux)`, `data.inputs[1]` | `IoConfig { sidechain: Some(layout), .. }` and `process_buses(io, sidechain, frames)` | One optional sidechain |
| `SpeakerArr::kMono`, `kStereo`, `k51`, `k71Music`, `k71_4` | `SpeakerLayout::Mono`, `Stereo`, `Surround51`, `Surround71`, `Surround714` | The plugin always sees the SDK's channel order; the shell reorders. Find a channel with `SpeakerLayout::index_of`. See [SURROUND.md](../SURROUND.md) |
| `addEventInput`, `addEventOutput` | `note_ports() -> NotePorts { input, output }` | |
| More than one output bus | Not available | See the list at the end |

### MIDI

| VST3 SDK | OpenPluginSDK | Notes |
|---|---|---|
| `data.inputEvents`, `IEventList::getEvent`, `Event::sampleOffset` | Calls at the event's offset | The block is cut at each event, so there is no list to walk |
| `Event::kNoteOnEvent`, `kNoteOffEvent` | `note_on(offset, note, velocity)`, `note_off(offset, note)`; `note_on_ref` for the note id and channel | Velocity is 1 to 127 here, not a float |
| `kPolyPressureEvent` | `midi(offset, [0xA0 \| channel, key, pressure])` | |
| `IMidiMapping::getMidiControllerAssignment` and the parameters behind it | `midi(offset, [u8; 3])` | The shell implements the mapping with hidden parameters and hands you CC, channel pressure and pitch bend as MIDI bytes |
| `data.outputEvents` | `pull_midi() -> Option<(u32, [u8; 3])>` | Drained after each piece of the block |
| `INoteExpressionController`, `kNoteExpressionValueEvent` | `note_expressions()`, `note_expression(offset, note, expression, value)` | See [EXPRESSION.md](../EXPRESSION.md) |
| `kDataEvent` with SysEx | Not available | See the list at the end |

### Threading

| VST3 SDK | OpenPluginSDK | Notes |
|---|---|---|
| `process` on the audio thread; everything in the controller on the UI thread | `process`, `set_param`, the note calls, `set_transport` and `reset` on the audio thread, one at a time; every `Editor` call on the main thread | |
| Parameter changes reach the processor only through `inputParameterChanges` | The same rule, enforced: the editor reports gestures and the host brings values back | The shell carries values across threads in a lock-free parameter plane |
| `setState` on the UI thread while `process` may run | `load_state`, `save_state` never overlap `process` | A block that starts during a state call passes the input through. A plugin with `state_is_parameters_only` loses no block |
| A thread of your own for loading | `openplugin::core::worker::loader` | Builds a value on a background thread and swaps it in on the audio thread without a lock |

## Worked port: a tremolo that follows the tempo

The original is a stereo tremolo. Its controller declares a `RangeParameter` `Rate` (0.1 to 20 Hz), a `RangeParameter` `Depth` (0 to 100 percent) and a `StringListParameter` `Sync` with the entries Free, 1/4, 1/8 and 1/16. Its processor walks `inputParameterChanges` at the top of `process` and keeps the last point of each queue, converts the normalised values to plain ones, reads `tempo` and `projectTimeMusic` from the `ProcessContext` when the flags say they are valid, and multiplies the block by a sine. `getState` and `setState` stream the three values.

The port uses the SDK's `Lfo`, which locks to the host's musical position: after a loop or a locate it is where the position says, with no drift.

```rust
use openplugin::dsp::{Feel, Lfo, LfoRate, NoteDivision};
use openplugin::prelude::*;
use openplugin_testing::{conformance, Options};

static SYNC: [&str; 4] = ["Free", "1/4", "1/8", "1/16"];

// The controller's parameters, in tag order: Rate 0, Depth 1, Sync 2.
static PARAMS: [ParamInfo; 3] = [
    ParamInfo::new("rate", "Rate", 0.1, 20.0, 4.0, Unit::Hz).with_taper(Taper::Log),
    ParamInfo::new("depth", "Depth", 0.0, 100.0, 50.0, Unit::Percent),
    ParamInfo::new("sync", "Sync", 0.0, 3.0, 0.0, Unit::None).with_choices(&SYNC),
];

// The factory's and the class's strings.
static INFO: PluginInfo = PluginInfo {
    id: "com.example.ported-tremolo",
    name: "Ported Tremolo",
    vendor: "Example Audio",
    url: "https://example.com",
    email: "support@example.com",
    version: "2.0.0",
    category: Category::Modulation,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

pub struct PortedTremolo {
    rate_hz: f32,
    depth: f32,
    sync: f32,
    lfo: Lfo,
}

impl Default for PortedTremolo {
    fn default() -> Self {
        let mut tremolo = Self {
            rate_hz: PARAMS[0].default,
            depth: PARAMS[1].default,
            sync: PARAMS[2].default,
            lfo: Lfo::new(48_000.0),
        };
        tremolo.update_rate();
        tremolo
    }
}

impl PortedTremolo {
    fn update_rate(&mut self) {
        let division = |denominator| LfoRate::Sync(NoteDivision::new(denominator, Feel::Straight));
        self.lfo.set_rate(match self.sync as usize {
            1 => division(4),
            2 => division(8),
            3 => division(16),
            _ => LfoRate::Hz(self.rate_hz),
        });
    }
}

impl Plugin for PortedTremolo {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    // `setupProcessing`.
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.lfo.set_sample_rate(sample_rate);
        self.lfo.reset();
    }

    fn reset(&mut self) {
        self.lfo.reset();
    }

    // One point of an `IParamValueQueue`, already plain, at its sample offset.
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "rate" => self.rate_hz = PARAMS[0].clamp(value),
            "depth" => self.depth = PARAMS[1].clamp(value),
            "sync" => self.sync = PARAMS[2].clamp(value),
            _ => return false,
        }
        self.update_rate();
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "rate" => Some(self.rate_hz),
            "depth" => Some(self.depth),
            "sync" => Some(self.sync),
            _ => None,
        }
    }

    // The `ProcessContext`, before each block. The LFO ignores a tempo or a
    // position that is not a usable number.
    fn set_transport(&mut self, transport: &Transport) {
        self.lfo.set_transport(transport);
    }

    // `process`, in place.
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let depth = self.depth / 100.0;
        for i in 0..frames {
            let gain = 1.0 - depth * self.lfo.next_unipolar();
            for channel in io.iter_mut().take(2) {
                channel[i] *= gain;
            }
        }
    }

    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }

    // `getState` streamed the three parameters and nothing else.
    fn state_is_parameters_only(&self) -> bool {
        true
    }
}

// The shell is what the VST3, Audio Unit and AAX adapters load.
openplugin::targets::export_shell!(PortedTremolo);
openplugin::targets::export_clap!(PortedTremolo);
openplugin::targets::export_lv2!(PortedTremolo);

// In a plugin crate the rest is `tests/conformance.rs`.
openplugin_testing::install_allocator!();

fn block_of_ones(plugin: &mut PortedTremolo, frames: usize) -> Vec<f32> {
    let mut io = vec![vec![1.0f32; frames]; 2];
    plugin.process(&mut io, frames);
    io.swap_remove(0)
}

fn main() {
    let mut plugin = PortedTremolo::default();
    plugin.prepare(48_000.0, 24_000, 2);
    plugin.set_param("depth", 100.0);

    // The host sends normalised values; the shell converts them like this.
    assert_eq!(PARAMS[2].from_normalized(1.0 / 3.0), 1.0);
    assert_eq!(PARAMS[2].format(1.0), "1/4");
    plugin.set_param("sync", 1.0);

    // 120 BPM: a quarter note is 24000 samples at 48 kHz.
    let at = |quarter_notes| Transport { playing: true, tempo: 120.0, quarter_notes, ..Transport::default() };
    plugin.set_transport(&at(0.0));
    let out = block_of_ones(&mut plugin, 24_000);
    assert!((out[0] - 0.5).abs() < 1e-3);
    assert!(out[6_000] < 1e-3, "the dip is a quarter of the cycle in");
    assert!(out[18_000] > 0.999);

    // After a locate the LFO is where the host's position says.
    plugin.set_transport(&at(8.75));
    assert!(block_of_ones(&mut plugin, 16)[0] > 0.999);

    // Free running at 4 Hz: a cycle is 12000 samples.
    plugin.set_param("sync", 0.0);
    plugin.reset();
    let out = block_of_ones(&mut plugin, 12_000);
    assert!(out[3_000] < 1e-3 && out[9_000] > 0.999);

    conformance(PortedTremolo::default, Options::quick()).assert_pass();
}
```

What moved where:

- The two classes became one `struct`. The controller's parameter objects are the `PARAMS` table; the processor's copies of the values are the three fields.
- The queue walk at the top of `process` is gone. The shell turns every point of every queue into a `set_param` call at its offset and cuts the block there, so a change lands on its sample without code in the plugin.
- `normalizedParamToPlain` is `ParamInfo::from_normalized`, done by the shell. The list parameter became `with_choices`, and its text comes from the choice names.
- The `ProcessContext` became `set_transport`. The validity flags are handled before the call: a field the host did not mark valid has its default value.
- `getState` and `setState` are gone, because the three parameters are the whole state.
- The last lines check the shape and the sync, then run the conformance kit.

## What has no counterpart yet

- **A single-component plugin stays two classes.** The class ids and the parameter tags carry over (`Plugin::vst3_ids`, `ParamInfo::with_vst3_id`; see [Identity pinning](../PORTING.md#identity-pinning)), so a host finds the port in old projects and old automation attaches to it. The SDK always writes a processor and a controller class; a port of a single-component plugin pins the processor id (`Vst3Ids::processor_from_hex`).
- **Reading the old component state in VST3.** The shell accepts only state it wrote itself, so the old plugin's bytes reach `load_state` only through CLAP. Same task.
- **Custom curves between normalised and plain values.** `Taper` is linear or logarithmic.
- **Custom value text.** A parameter prints and parses by its `Unit` and its choice names.
- **64-bit processing** (`kSample64`).
- **Program lists.** `preset_count`, `preset_name` and `preset_values` reach a Rust host through `openplugin-ffi`, and the plugin's own editor can offer them, but no format entry publishes a program list to the host yet.
- **More than one output bus, or more than one aux input.**
- **SysEx and other data events.** MIDI reaches the plugin as messages of up to three bytes.
- **Messages between controller and processor.** Use the shared editor state.
