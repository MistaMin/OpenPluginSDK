# Porting from the Audio Unit SDK

This guide maps a plugin written directly against Apple's Audio Unit APIs onto OpenPluginSDK: the processor, parameters, state, editor, buses, MIDI and threading, then a small effect ported in full. It covers both generations: version 2 units built on the AudioUnitSDK base classes (`AUEffectBase` and its relatives), and version 3 units built on `AUAudioUnit`. It is written from Apple's public documentation and public API names. The Rust in it is original code for this SDK, and every code block is compiled and run as a doc test (`cargo test -p example-porting-guides`). API names are the ones Apple publishes at the time of writing; check them against the SDK you build with.

Read [PORTING.md](../PORTING.md) first: it has the order of work and the tools (scanner, state capture, null test) that every port uses.

## The short version

- The plugin becomes one `struct` that implements `Plugin`. The SDK's Audio Unit shell (`native/au`, [SHELL.md](../SHELL.md)) is the component a host loads. It is a version 2 unit built on the AudioUnitSDK base classes; there is no version 3 app extension.
- The same code also exports CLAP, LV2, VST3 and AAX, and runs on Windows and Linux in the formats those systems have.
- Scopes and elements go away. Parameters are global, a plugin has one main input, one main output and at most one sidechain, and audio is processed in place.
- Times are in samples, not seconds: latency and tail are sample counts.
- The Audio Unit is the shell format where the old identity carries over today: the type follows from the plugin's kind, and the subtype and manufacturer codes are build options. Keep them, and keep the parameters in the old order.

## Concept map

### Processor

| Audio Unit v2 | Audio Unit v3 | OpenPluginSDK | Notes |
|---|---|---|---|
| A class derived from `AUEffectBase`, with an `AUKernelBase` per channel; the component factory | A class derived from `AUAudioUnit` in an app extension | A `struct` with `impl Plugin` and `impl Default`; `export_shell!` | The kernels become an array of per-channel DSP state inside the struct |
| `AudioComponentDescription`: type, subtype, manufacturer, in the bundle's `AudioComponents` | The same, in the extension's `Info.plist` | `--au-subtype`, `--au-manufacturer` of `scripts/build_plugin.sh`; the type from the plugin's kind | `aufx`, `aumu` or `aumf`. Keep the old four-character codes. See [BUILD.md](../BUILD.md) |
| The component name and version | The same | `--name`, `--manufacturer`, `--version`; `PluginInfo` | |
| `Initialize`, `Cleanup` | `allocateRenderResourcesAndReturnError`, `deallocateRenderResources` | `prepare(sample_rate, max_block, channels)` | Allocate here |
| The stream format's sample rate; `kAudioUnitProperty_MaximumFramesPerSlice` | The bus `format`; `maximumFramesToRender` | The arguments of `prepare` | |
| `Reset` | `reset` | `reset()` | Must not allocate. The shell runs it on the render thread before the next block |
| `ProcessBufferLists`, or the kernel's `Process(inSource, inDest, inFrames, ...)` | `internalRenderBlock` | `process(io: &mut [Vec<f32>], frames)` | In place, planar: `io[channel][..frames]` |
| `GetLatency`, `kAudioUnitProperty_Latency` | `latency` | `Plugin::latency` | Samples, not seconds |
| `SupportsTail`, `GetTailTime` | `tailTime` | `Plugin::tail_samples`, or `Plugin::tail` | Samples. Build the shell with `--tail` |
| `kAudioUnitProperty_BypassEffect` | `shouldBypassEffect` | `Plugin::set_host_bypass` | The shell owns bypass: the input delayed by the reported latency, crossfaded over 10 ms |
| `kAudioUnitProperty_OfflineRender` | `renderingOffline` | `Plugin::set_offline` | |
| `HostCallbackInfo`: `beatAndTempoProc`, `transportStateProc2`, `musicalTimeLocationProc` | `musicalContextBlock`, `transportStateBlock` | `Plugin::set_transport(&Transport)` | Called before each block; keep the fields you use |

### Parameters

| Audio Unit v2 | Audio Unit v3 | OpenPluginSDK | Notes |
|---|---|---|---|
| `GetParameterInfo` filling an `AudioUnitParameterInfo` | An `AUParameterTree` of `AUParameter`s | `static PARAMS: [ParamInfo; N]` | |
| The `AudioUnitParameterID`, in the global scope | The `AUParameterAddress` | The position in the table, or `ParamInfo::with_au_id` | The shell numbers parameters `0..n` in table order. Hosts store automation by this id, so keep the old order |
| `minValue`, `maxValue`, `defaultValue` | `minValue`, `maxValue`, the initial `value` | `ParamInfo::new(id, name, min, max, default, unit)` | Plain values in both |
| `unit`: `kAudioUnitParameterUnit_Decibels`, `_Hertz`, `_Milliseconds`, `_Percent`, `_Boolean`, `_Generic` | The same units | `Unit::Db`, `Unit::Hz`, `Unit::Ms`, `Unit::Percent`, `Unit::Toggle`, `Unit::None` | |
| `kAudioUnitParameterUnit_Indexed` with `GetParameterValueStrings` | `valueStrings` | `.with_choices(&[...])` | |
| `kAudioUnitParameterFlag_DisplayLogarithmic` | The same flag | `.with_taper(Taper::Log)` | |
| `kAudioUnitParameterFlag_IsWritable` cleared | The same flag | `.flag(ParamFlags::READ_ONLY)` | |
| `kAudioUnitParameterFlag_NonRealTime` | The same flag | `.flag(ParamFlags::NOT_REALTIME)` | |
| `kAudioUnitParameterFlag_HasClump`, `clumpID`, the clump name | Parameter groups in the tree | `.with_group("Filter/Envelope")` | One clump per path |
| `kAudioUnitProperty_ParameterStringFromValue`, `ParameterValueFromString` | `implementorStringFromValueCallback`, `implementorValueFromStringCallback` | `ParamInfo::format`, `ParamInfo::parse` | By unit and choice names; see the list at the end |
| `GetParameter(id)` read in the kernel | The DSP's copy, written by `implementorValueObserver` | A field of the plugin, written in `set_param` | Plain units, clamped by `ParamInfo::clamp` |
| `SetParameter` | `implementorValueObserver` | `set_param(id, value)` | Compute coefficients here. Override `set_param_index` to skip the string match |
| `AudioUnitScheduleParameters`: immediate events | `AURenderEventParameter` | `set_param` at the event's offset | The shell cuts the block there |
| `AudioUnitScheduleParameters`: ramped events, `kAudioUnitParameterFlag_CanRamp` | `AURenderEventParameterRamp` | `set_param` in steps 16 frames apart | A staircase, not a slope. Use `OnePole` or `SampleRamp` where the audio needs a smooth change |

### State

| Audio Unit v2 | Audio Unit v3 | OpenPluginSDK | Notes |
|---|---|---|---|
| The base class's `SaveState` and `RestoreState`: the `kAudioUnitProperty_ClassInfo` dictionary with the parameter values under `data` | The default `fullState` | Nothing to write | When parameters are the whole state, return `true` from `state_is_parameters_only`. The shell keeps every key the base class writes |
| `SaveState`, `RestoreState` overridden to add keys | `fullState` overridden | `save_state() -> Option<Vec<u8>>`, `load_state(&[u8]) -> bool` | The shell stores the bytes under one more key, `OpenPluginState`. Put the parameter values in the bytes too: the CLAP entry stores this chunk alone when there is one. Change nothing when you return `false` |
| A dictionary saved by the old unit | The same | The base class's restore | A dictionary without the shell's key is restored by the base class, which reads the parameters from `data`. See below |
| `GetPresets`, `NewFactoryPresetSet`, `kAudioUnitProperty_FactoryPresets` | `factoryPresets`, `currentPreset` | `preset_count`, `preset_name`, `preset_values` | Not published to the host yet; see the list at the end |
| An old dictionary read outside the Audio Unit | | `openplugin::core::foreign_state::plist` | Reads XML and binary property lists. See [Foreign state import](../PORTING.md#foreign-state-import) |

### Editor

| Audio Unit v2 | Audio Unit v3 | OpenPluginSDK | Notes |
|---|---|---|---|
| `kAudioUnitProperty_CocoaUI`, a view factory class (`AUCocoaUIBase`) | `requestViewControllerWithCompletionHandler`, an `AUViewController` | `PluginInfo::has_editor`, an `impl Editor`, and a function `fn(EditorHost) -> Option<Box<dyn Editor>>` given to `export_shell!` as `editor = ...` | Build the shell with `--editor` |
| An `NSView` drawn with AppKit | A view controller's view | `Editor::render(scale) -> Frame` | Software pixels. `openplugin::dri` draws knobs, faders and meters; see [EDITOR.md](../EDITOR.md) |
| The view's frame | `preferredContentSize` | `Editor::size`, `Editor::size_range`, `Editor::resize` | Points |
| `AUParameterSet` with `kAudioUnitEvent_BeginParameterChangeGesture` and `EndParameterChangeGesture` notifications | `setValue:originator:` and the parameter's observer tokens | `EditorHost::begin`, `perform`, `end` | By table index, in plain units |
| An `AUEventListener` for parameter changes | A parameter observer | `Editor::param_changed(index, plain)` | |
| A custom property the view polls for meters | KVO or a shared object | The `Arc` returned by `Plugin::editor_state`, read in `Editor::tick` | Atomics or a `Seqlock` |
| A timer in the view | A display link or timer | `Editor::tick(dt)` | About 30 times a second |

### Buses

| Audio Unit v2 | Audio Unit v3 | OpenPluginSDK | Notes |
|---|---|---|---|
| `SupportedNumChannels` returning `AUChannelInfo` pairs | `channelCapabilities` | `supported_layouts() -> &'static [IoConfig]` | The default is stereo in, stereo out. The shell also offers mono for a plugin that lists only stereo |
| Input element 1 used as a sidechain | A second input bus in `inputBusses` | `IoConfig { sidechain: Some(layout), .. }` and `process_buses(io, sidechain, frames)` | One optional sidechain |
| `kAudioUnitProperty_SupportedChannelLayoutTags`, `AudioChannelLayout` | `supportedChannelLayoutTags` on the bus | `SpeakerLayout::Surround51` and the others | The plugin always sees the SDK's channel order; the shell reorders by role. Find a channel with `SpeakerLayout::index_of`. See [SURROUND.md](../SURROUND.md) |
| `kAudioUnitProperty_InPlaceProcessing` | `canProcessInPlace` | Always in place | |
| More than one output element | More than one output bus | Not available | See the list at the end |

### MIDI

| Audio Unit v2 | Audio Unit v3 | OpenPluginSDK | Notes |
|---|---|---|---|
| The component type `aumu` or `aumf`; `MusicDeviceBase`, `AUMIDIEffectBase` | The same types | `PluginInfo::is_instrument`, `note_ports()` | The shell picks the type from the plugin's kind; `-DOP_AU_TYPE=aumf` makes an effect a music effect |
| `HandleNoteOn`, `HandleNoteOff`, `HandleMIDIEvent` with an offset frame | `AURenderEventMIDI` in the render event list | `note_on(offset, note, velocity)`, `note_off(offset, note)`, `midi(offset, [u8; 3])` | The shell cuts the block at each event. A note-on with velocity 0 arrives as a note-off |
| `kAudioUnitProperty_MIDIOutputCallback` | `MIDIOutputEventBlock` | `pull_midi() -> Option<(u32, [u8; 3])>` | Drained after each piece of the block |
| `HandleSysEx` | SysEx in the event list | Not available | See the list at the end |

### Threading

| Audio Unit v2 | Audio Unit v3 | OpenPluginSDK | Notes |
|---|---|---|---|
| `Render` on the host's render thread | `internalRenderBlock` on the render thread, capturing no Objective-C objects | `process`, `set_param`, the note calls, `set_transport` and `reset` on the audio thread, one at a time | |
| `SetParameter` and property calls from any thread | Parameter values written from any thread | `set_param` between blocks or at an event's offset inside one | The shell carries values across threads in a lock-free plane; the plugin's fields need no atomic |
| `SaveState`, `RestoreState` on the main thread while rendering continues | `fullState` on any thread | `save_state`, `load_state` never overlap `process` | A block that starts during a state call passes the input through. A plugin with `state_is_parameters_only` loses no block |
| `AudioUnitReset` from the main thread | `reset` | `reset()` on the render thread | The shell marks the reset and the render thread performs it |
| A thread of your own for loading | The same | `openplugin::core::worker::loader` | Builds a value on a background thread and swaps it in on the audio thread without a lock |

## Worked port: one band of parametric EQ

The original is a version 2 effect built on `AUEffectBase`. Its `GetParameterInfo` describes three global parameters: `Frequency` (20 Hz to 20 kHz, hertz, displayed logarithmically, default 1 kHz), `Gain` (-18 to 18 dB, default 0) and `Q` (0.3 to 10, generic, default 1). `SupportedNumChannels` allows mono and stereo. Each channel has a kernel whose `Process` reads the three values with `GetParameter`, recomputes its coefficients when one changed, and filters; `Reset` clears the kernel. The unit reports a tail through `GetTailTime`, and its state is the base class's.

In the port the kernels are two filters in an array. The filter is the SDK's state-variable filter in its bell mode, which accepts new coefficients at any sample.

```rust
use openplugin::dsp::{Svf, SvfMode};
use openplugin::prelude::*;
use openplugin_testing::{conformance, Options};

// `GetParameterInfo`, in parameter id order: Frequency 0, Gain 1, Q 2.
static PARAMS: [ParamInfo; 3] = [
    ParamInfo::new("frequency", "Frequency", 20.0, 20_000.0, 1_000.0, Unit::Hz).with_taper(Taper::Log),
    ParamInfo::new("gain", "Gain", -18.0, 18.0, 0.0, Unit::Db),
    ParamInfo::new("q", "Q", 0.3, 10.0, 1.0, Unit::None),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.example.ported-band",
    name: "Ported Band",
    vendor: "Example Audio",
    url: "https://example.com",
    email: "",
    version: "2.0.0",
    category: Category::Eq,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

// `SupportedNumChannels`: {2, 2} and {1, 1}. The first is the default.
static LAYOUTS: [IoConfig; 2] = [
    IoConfig::STEREO,
    IoConfig { input: SpeakerLayout::Mono, output: SpeakerLayout::Mono, sidechain: None },
];

pub struct PortedBand {
    frequency: f32,
    gain_db: f32,
    q: f32,
    sample_rate: f32,
    /// One per channel: what the kernels were.
    filters: [Svf; 2],
    stale: bool,
    tail: usize,
}

impl Default for PortedBand {
    fn default() -> Self {
        let mut band = Self {
            frequency: PARAMS[0].default,
            gain_db: PARAMS[1].default,
            q: PARAMS[2].default,
            sample_rate: 48_000.0,
            filters: [Svf::new(), Svf::new()],
            stale: true,
            tail: 0,
        };
        band.update_tail();
        band
    }
}

impl PortedBand {
    /// `GetTailTime`, in samples: how long the band rings until it is 90 dB
    /// down. The bell's poles have the quality `A * Q` with `A = 10^(gain / 40)`,
    /// and a resonance of quality `Qp` at `f` decays as `exp(-pi * f * t / Qp)`.
    fn update_tail(&mut self) {
        let pole_q = self.q * 10f32.powf(self.gain_db / 40.0);
        let seconds = 90.0 / 20.0 * std::f32::consts::LN_10 * pole_q / (std::f32::consts::PI * self.frequency);
        self.tail = (seconds * self.sample_rate).ceil() as usize;
    }
}

impl Plugin for PortedBand {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    // `Initialize`.
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.sample_rate = sample_rate;
        self.stale = true;
        self.update_tail();
        self.reset();
    }

    // `Reset`.
    fn reset(&mut self) {
        for filter in &mut self.filters {
            filter.reset();
        }
    }

    // `SetParameter`, or one scheduled parameter event at its offset.
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "frequency" => self.frequency = PARAMS[0].clamp(value),
            "gain" => self.gain_db = PARAMS[1].clamp(value),
            "q" => self.q = PARAMS[2].clamp(value),
            _ => return false,
        }
        self.stale = true;
        self.update_tail();
        true
    }

    // `GetParameter`.
    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "frequency" => Some(self.frequency),
            "gain" => Some(self.gain_db),
            "q" => Some(self.q),
            _ => None,
        }
    }

    fn supported_layouts(&self) -> &'static [IoConfig] {
        &LAYOUTS
    }

    // The kernels' `Process`, in place. `io` has one channel in the mono layout.
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if self.stale {
            self.stale = false;
            for filter in &mut self.filters {
                filter.configure(SvfMode::Bell, self.sample_rate, self.frequency, self.q, self.gain_db);
            }
        }
        for (channel, filter) in io.iter_mut().zip(&mut self.filters) {
            for sample in &mut channel[..frames] {
                *sample = filter.process_sample(*sample);
            }
        }
    }

    fn tail_samples(&self) -> usize {
        self.tail
    }

    // An `aufx` effect: no MIDI.
    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }

    // The state was the base class's: the parameters.
    fn state_is_parameters_only(&self) -> bool {
        true
    }
}

// The shell is what the Audio Unit, VST3 and AAX adapters load.
openplugin::targets::export_shell!(PortedBand);
openplugin::targets::export_clap!(PortedBand);
openplugin::targets::export_lv2!(PortedBand);

// In a plugin crate the rest is `tests/conformance.rs`.
openplugin_testing::install_allocator!();

/// The level in dB of a sine of `hz` after the band, on `channels` channels.
fn gain_db_at(plugin: &mut PortedBand, hz: f32, channels: usize) -> f32 {
    plugin.reset();
    let mut peak = 0.0f32;
    let mut io = vec![vec![0.0f32; 64]; channels];
    for block in 0..400 {
        for i in 0..64 {
            let t = (block * 64 + i) as f32 / 48_000.0;
            let x = (std::f32::consts::TAU * hz * t).sin();
            for channel in io.iter_mut() {
                channel[i] = x;
            }
        }
        plugin.process(&mut io, 64);
        if block >= 200 {
            peak = io[channels - 1].iter().fold(peak, |p, s| p.max(s.abs()));
        }
    }
    20.0 * peak.log10()
}

fn main() {
    let mut plugin = PortedBand::default();
    plugin.prepare(48_000.0, 64, 2);

    // Flat at 0 dB; +12 dB at the centre and nothing four octaves below it.
    assert!(gain_db_at(&mut plugin, 1_000.0, 2).abs() < 0.01);
    plugin.set_param("gain", 12.0);
    assert!((gain_db_at(&mut plugin, 1_000.0, 2) - 12.0).abs() < 0.05);
    assert!(gain_db_at(&mut plugin, 60.0, 2).abs() < 0.2);

    // The mono layout, selected and prepared the way a host does it.
    assert!(plugin.set_layout(LAYOUTS[1]));
    plugin.prepare_layout(48_000.0, 64, LAYOUTS[1]);
    assert!((gain_db_at(&mut plugin, 1_000.0, 1) - 12.0).abs() < 0.05);

    // A narrow boost low down rings for longer than a broad one high up.
    plugin.set_param("frequency", 50.0);
    plugin.set_param("q", 10.0);
    let narrow = plugin.tail_samples();
    plugin.set_param("frequency", 5_000.0);
    plugin.set_param("q", 0.5);
    assert!(narrow > 100 * plugin.tail_samples());

    // The unit showed "1.00 kHz"; so does the port, and it reads it back.
    assert_eq!(PARAMS[0].format(1_000.0), "1.00 kHz");
    assert_eq!(PARAMS[0].parse("1 kHz"), Some(1_000.0));

    conformance(PortedBand::default, Options::quick()).assert_pass();
}
```

What moved where:

- The class and its kernels became one `struct` with a filter per channel. `SupportedNumChannels` became `LAYOUTS`, and `process` filters as many channels as it is given.
- `GetParameterInfo` became the `PARAMS` table. The order is the old parameter id order, so a host's automation still points at the right parameter. `kAudioUnitParameterFlag_DisplayLogarithmic` is `Taper::Log`.
- The kernels read the parameters on every call and compared them with the last values. Here `set_param` marks the coefficients stale and `process` recomputes them once. The shell cuts a block at every scheduled parameter event, so a change still lands on its sample.
- `GetTailTime` became `tail_samples`, kept up to date in `set_param` because the shell reads it once per block on the render thread.
- There is no state code, because the parameters are the whole state.

### The old unit's sessions and presets

Build the shell with the old subtype and manufacturer codes and a host sees the same Audio Unit. A session or `.aupreset` saved by the old unit holds the dictionary its base class wrote, without the shell's `OpenPluginState` key. [SHELL.md](../SHELL.md#audio-unit-state) describes that case: the dictionary is restored by the base class, which sets the parameters from `data` by parameter id. With the parameters in the old order and the old ranges, the values land where they belong.

That holds for a unit whose state was only its parameters. If the old unit added its own keys to the dictionary, the shell does not hand them to the plugin: `load_state` receives only what the shell itself saved. Test this early, with a session saved by the old unit, in each host you support.

## What has no counterpart yet

- **A version 3 app extension.** The shell is a version 2 component. An iOS or sandboxed extension target is not built.
- **Parameters outside the global scope.** Parameter ids that are not `0..n` in table order are kept with `ParamInfo::with_au_id` (see [Identity pinning](../PORTING.md#identity-pinning)).
- **Custom keys in the class info dictionary.** The old unit's own keys do not reach `load_state` through the Audio Unit shell. Same task.
- **Factory presets in the host.** `preset_count`, `preset_name` and `preset_values` reach a Rust host through `openplugin-ffi`; the shell does not publish `kAudioUnitProperty_FactoryPresets`.
- **True parameter ramps.** A ramped event arrives as steps 16 frames apart.
- **Custom value text.** A parameter prints and parses by its `Unit` and its choice names.
- **More than one output element, or more than one sidechain.**
- **SysEx and MIDI 2 event lists.** MIDI reaches the plugin as messages of up to three bytes.
- **Custom properties.** A host or a view cannot read a property the plugin defines; the editor shares state through `Plugin::editor_state`.
