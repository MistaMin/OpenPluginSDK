# Porting from iPlug2

This guide maps an iPlug2 plugin onto OpenPluginSDK: the processor, parameters, state, editor, buses, MIDI and threading, then a small effect ported in full. It is written from iPlug2's public documentation and public API names. The Rust in it is original code for this SDK, and every code block is compiled and run as a doc test (`cargo test -p example-porting-guides`). API names are the ones iPlug2 publishes at the time of writing; check them against the version you build with.

Read [PORTING.md](../PORTING.md) first: it has the order of work and the tools (scanner, state capture, null test) that every port uses.

## The short version

- One `struct` implements `Plugin` and `Default`. There is no base class and no `config.h`: what `config.h` declares becomes a `static PluginInfo`, a few `Plugin` methods and build options.
- The parameter table is a `static` array of `ParamInfo`. The plugin keeps the current values in its own fields. The SDK writes them with `set_param` and never while `process` runs, so there is no parameter mutex and no atomic.
- Audio is 32-bit float, processed in place. iPlug2's `sample` is `double` unless the project asks for float, so a port converts its DSP state or keeps `f64` state inside and converts at the edges.
- A plugin whose whole state is its parameters writes no state code: the SDK saves and restores the values in every format. `save_state` and `load_state` are for a plugin that has more.
- The editor is a separate object on the main thread. It draws a frame of pixels and reports edits as gestures. It never holds a pointer to the processor.

## Concept map

### Processor

| iPlug2 | OpenPluginSDK | Notes |
|---|---|---|
| A class derived from `Plugin`, constructed with `MakeConfig(kNumParams, kNumPresets)` | A `struct` with `impl Plugin` and `impl Default` | `Default::default()` is the constructor the format entries call |
| `config.h`: `PLUG_NAME`, `PLUG_MFR`, `PLUG_VERSION_STR`, `PLUG_URL_STR`, `PLUG_EMAIL_STR` | `PluginInfo::new(..)`, `with_url`, `with_email` | Returned by `Plugin::info` |
| `PLUG_UNIQUE_ID`, `PLUG_MFR_ID`, `BUNDLE_NAME`, `BUNDLE_MFR`, `BUNDLE_DOMAIN` | `PluginInfo::id`, and `--au-subtype`, `--au-manufacturer`, `--bundle-id` of `scripts/build_plugin.sh` | See [BUILD.md](../BUILD.md) |
| `AAX_TYPE_IDS`, `AAX_PLUG_MFR_STR` | `Plugin::aax_ids` | Four-character codes |
| `PLUG_TYPE` (effect, instrument, MIDI effect) | `PluginInfo::is_instrument`, `Plugin::note_ports`, `PluginInfo::category` | |
| `OnReset()` | `prepare(sample_rate, max_block, channels)` and `reset()` | Allocate in `prepare`. `reset` clears filter and delay history and must not allocate |
| `GetSampleRate()`, `GetBlockSize()` | The arguments of `prepare` | Keep what you need in fields |
| `ProcessBlock(sample** inputs, sample** outputs, int nFrames)` | `process(io: &mut [&mut [f32]], frames)` | In place: `io[channel][..frames]` holds the input and receives the output |
| `SetLatency(n)`, `PLUG_LATENCY` | `Plugin::latency` | The entries read it after every block. `HostNotifier::latency_changed` reports a change made outside `process` |
| `SetTailSize(n)` | `Plugin::tail` | `Tail::from_samples(n)`; `Tail::Infinite` and `Tail::Unknown` exist too |
| `GetBypassed()` | `Plugin::set_host_bypass` | The host's bypass, separate from a power parameter of your own |
| `GetRenderingOffline()` | `Plugin::set_offline` | Called before the blocks it applies to |
| `GetTempo()`, `GetSamplePos()`, `GetTransportIsRunning()`, `GetTimeInfo()` | `Plugin::set_transport(&Transport)` | Called before each block; keep the fields you use |

### Parameters

| iPlug2 | OpenPluginSDK | Notes |
|---|---|---|
| `enum EParams { kGain, kNumParams }` | `static PARAMS: [ParamInfo; N]` | The position in the table is the parameter's index in every format, so keep the old order |
| `GetParam(kGain)->InitDouble(name, default, min, max, step, label)` | `ParamInfo::new(id, name, min, max, default, unit)` | The argument order differs: range first, then the default |
| `InitGain`, `InitFrequency`, `InitPercentage`, `InitSeconds`, `InitMilliseconds` | `Unit::Db`, `Unit::Hz`, `Unit::Percent`, `Unit::Seconds`, `Unit::Ms` | The unit also decides how the value is printed and parsed |
| `InitBool` | `Unit::Toggle` | Range 0 to 1 |
| `InitEnum` with a list of names | `.with_choices(&[...])` | The value is the index |
| `InitInt`, a non-zero step | `.with_steps(n)` | `n` positions including both ends |
| `IParam::ShapePowCurve`, `IParam::ShapeExp` | `.with_taper(Taper::Log)` | Only linear and logarithmic travel exist; see the list at the end |
| `IParam::kFlagCannotAutomate`, `kFlagStepped` | `.with_flag(ParamFlags::NOT_REALTIME)`, `.with_flag(ParamFlags::STEPPED)` | |
| The parameter group string | `.with_group("Filter/Envelope")` | A path, outermost name first |
| `GetParam(kGain)->Value()` inside `ProcessBlock` | A field of the plugin, written in `set_param` | Plain units, already clamped by `ParamInfo::clamp` |
| `OnParamChange(paramIdx)` | `set_param(id, value)` | Compute coefficients here. Override `set_param_index` to skip the string match |
| `LogParamSmooth` and similar smoothers | `OnePole`, `SampleRamp`, `openplugin::dsp::utility::Gain` | |
| `SetDisplayFunc` | `ParamInfo::format` and `ParamInfo::parse` by unit and choices | No custom text function; see the list at the end |

### State

| iPlug2 | OpenPluginSDK | Notes |
|---|---|---|
| `SerializeParams`, `UnserializeParams` alone | Nothing to write | When parameters are the whole state, the SDK saves and restores them in every format |
| `SerializeState(IByteChunk&)`, `UnserializeState(chunk, startPos)` | `save_state() -> Option<Vec<u8>>`, `load_state(&[u8]) -> bool` | For a plugin with more state than its parameters. Put the parameter values in the bytes too: the CLAP entry stores this chunk alone when there is one. Start with a magic and a version, and change nothing when you return `false` |
| `PLUG_DOES_STATE_CHUNKS` off | `state_is_parameters_only()` returns `true` | Then a save or load never touches the engine and costs the audio thread no block |
| `IByteChunk::Get`, `GetStr` on an old chunk | `openplugin::core::foreign_state::bytes::Reader` | Bounds-checked; see [Foreign state import](../PORTING.md#foreign-state-import) |
| `MakePreset`, `MakeDefaultPreset`, `kNumPresets` | `PresetBank`, and `preset_count`, `preset_name`, `preset_values` | Plain values in table order. Not published to the host's preset menu yet; see the list at the end |

### Editor

| iPlug2 | OpenPluginSDK | Notes |
|---|---|---|
| `PLUG_HAS_UI`, `mMakeGraphicsFunc`, `mLayoutFunc` | `PluginInfo::has_editor`, and a function `fn(EditorHost) -> Option<Box<dyn Editor>>` given to the export macros as `editor = ...` | |
| `PLUG_WIDTH`, `PLUG_HEIGHT`, `PLUG_HOST_RESIZE` | `Editor::size`, `Editor::size_range`, `Editor::resize`, `EditorHost::request_resize` | Points, not pixels |
| `IGraphics` drawing calls, `IControl::Draw` | `Editor::render(scale) -> Frame` | The editor returns pixels. `openplugin::dri` has knobs, faders, meters and a `SceneCanvas` to draw them on |
| `IVKnobControl(bounds, kGain)` and other parameter-linked controls | `openplugin::dri::ControlSet` with `Control::knob`, `Control::toggle` | Hit testing, drags, keys and accessibility; see [EDITOR.md](../EDITOR.md) |
| `BeginInformHostOfParamChangeFromUI`, `SendParameterValueFromUI`, `EndInformHostOfParamChangeFromUI` | `EditorHost::begin`, `perform`, `end` | Plain values, not normalised ones |
| `OnParamChangeUI`, `SendParameterValueFromDelegate` | `Editor::param_changed(index, plain)` | |
| `ISender`, `IPeakSender`, `TransmitData` from `OnIdle` | A shared `Arc` returned by `Plugin::editor_state`, read in `Editor::tick` | Atomics or a `Seqlock`; the editor gets it through `Editor::bind` |
| `SendArbitraryMsgFromUI`, `OnMessage` | The same shared state | There is no message channel; share atomics or a lock-free queue |
| `OnIdle()` | `Editor::tick(dt)` | About 30 times a second, main thread |

### Buses

| iPlug2 | OpenPluginSDK | Notes |
|---|---|---|
| `PLUG_CHANNEL_IO "1-1 2-2"` | `supported_layouts() -> &'static [IoConfig]` | One `IoConfig` per configuration. The default is stereo in, stereo out |
| A sidechain input bus in the channel string | `IoConfig { sidechain: Some(layout), .. }` and `process_buses(io, sidechain, frames)` | One optional sidechain |
| `NInChansConnected()`, `NOutChansConnected()`, `IsChannelConnected` | `set_layout`, `prepare_layout` | The host picks one layout while the plugin is deactivated |
| More than one output bus | Not available | See the list at the end |

### MIDI

| iPlug2 | OpenPluginSDK | Notes |
|---|---|---|
| `PLUG_DOES_MIDI_IN`, `PLUG_DOES_MIDI_OUT` | `note_ports() -> NotePorts { input, output }` | An effect that reads no MIDI returns both `false` |
| `ProcessMidiMsg(const IMidiMsg&)` with an `IMidiQueue` drained in `ProcessBlock` | `note_on(offset, note, velocity)`, `note_off(offset, note)`, `midi(offset, [u8; 3])` | The entries split the block at each event, so no queue is needed |
| `SendMidiMsg` | `pull_midi() -> Option<(u32, [u8; 3])>` | The entry drains it after each block |
| `PLUG_DOES_MPE` | `mpe_input() -> Option<MpeZones>` | See [EXPRESSION.md](../EXPRESSION.md) |
| `MidiSynth`, `VoiceAllocator` | `openplugin::dsp::voice_alloc` | |
| `ProcessSysEx`, `SendSysEx` | Not available | See the list at the end |

### Threading

| iPlug2 | OpenPluginSDK | Notes |
|---|---|---|
| `ProcessBlock` on the audio thread | `process`, `set_param`, the note calls, `set_transport` and `reset` on the audio thread | One at a time, never two at once on the same plugin |
| `OnParamChange` from whichever thread the host uses, guarded by the parameter mutex | `set_param` between blocks or at an event's offset inside one | The entries carry values across threads in a lock-free parameter plane |
| `OnIdle`, UI callbacks on the main thread | Every `Editor` call on the main thread | The editor and the engine share only what `editor_state` returns |
| Loading files in `OnReset` or on a thread of your own | `openplugin::core::worker::loader` | Builds the value on a background thread and swaps it in on the audio thread without a lock |

On the audio path the rules are the ones in [PERFORMANCE.md](../PERFORMANCE.md): no allocation, no lock, no panic. The conformance kit checks the first and the last.

## Worked port: a gain with a peak meter

The original is the kind of plugin the iPlug2 project template starts from. Its `config.h` declares the channel configurations `1-1 2-2`. Its constructor declares one parameter, `kGain`, with `InitDouble`: 0 to 100 percent, default 100. `ProcessBlock` reads the value, divides by 100, multiplies every sample and tracks the output peak, which a sender passes to a meter control from `OnIdle`. It does not override `SerializeState`.

The port keeps the parameter's range and unit, so automation written for the old plugin means the same thing. It adds a 20 ms smoother, because a stepped gain clicks.

```rust
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use openplugin::core::OnePole;
use openplugin::prelude::*;
use openplugin_testing::{conformance, Options};

// What `kGain` declared: 0 to 100 percent, default 100.
static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", 0.0, 100.0, 100.0, Unit::Percent)];

// What `config.h` declared.
static INFO: PluginInfo = PluginInfo::new(
    "com.example.ported-gain",
    "Ported Gain",
    "Example Audio",
    "2.0.0",
    Category::Utility,
    &PARAMS,
)
.with_url("https://example.com")
.with_email("support@example.com");

// `PLUG_CHANNEL_IO "1-1 2-2"`. The first layout is the default.
static LAYOUTS: [IoConfig; 2] = [
    IoConfig::STEREO,
    IoConfig { input: SpeakerLayout::Mono, output: SpeakerLayout::Mono, sidechain: None },
];

/// What the engine shows its editor: the output peak since the editor last
/// looked, linear, as `f32` bits.
pub struct Shared {
    peak: AtomicU32,
}

impl Shared {
    fn raise(&self, value: f32) {
        if value > f32::from_bits(self.peak.load(Ordering::Relaxed)) {
            self.peak.store(value.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak.swap(0, Ordering::Relaxed))
    }
}

pub struct PortedGain {
    percent: f32,
    smooth: OnePole,
    channels: usize,
    shared: Arc<Shared>,
}

impl Default for PortedGain {
    fn default() -> Self {
        Self {
            percent: PARAMS[0].default,
            smooth: OnePole::new(),
            channels: 2,
            shared: Arc::new(Shared { peak: AtomicU32::new(0) }),
        }
    }
}

impl Plugin for PortedGain {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    // `OnReset`: the rate is known, nothing is playing yet.
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.channels = channels;
        self.smooth.set_time(sample_rate, 20.0);
        self.smooth.reset(self.percent / 100.0);
    }

    fn reset(&mut self) {
        self.smooth.reset(self.percent / 100.0);
    }

    // `OnParamChange`. Never called while `process` runs.
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id != "gain" {
            return false;
        }
        self.percent = PARAMS[0].clamp(value);
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        (id == "gain").then_some(self.percent)
    }

    // `ProcessBlock`, in place.
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let target = self.percent / 100.0;
        let channels = self.channels.min(io.len());
        let mut peak = 0.0f32;
        for i in 0..frames {
            let gain = self.smooth.next_value(target);
            for channel in io.iter_mut().take(channels) {
                channel[i] *= gain;
                if channel[i].is_finite() {
                    peak = peak.max(channel[i].abs());
                }
            }
        }
        self.shared.raise(peak);
    }

    fn supported_layouts(&self) -> &'static [IoConfig] {
        &LAYOUTS
    }

    // The original had `PLUG_DOES_MIDI_IN 0`.
    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }

    // The original did not override `SerializeState`: parameters are the whole state.
    fn state_is_parameters_only(&self) -> bool {
        true
    }

    fn editor_state(&self) -> Option<EditorState> {
        Some(self.shared.clone())
    }
}

// One line per format family: CLAP, LV2, and the shell that the VST3, Audio
// Unit and AAX adapters load.
openplugin::targets::export_clap!(PortedGain);
openplugin::targets::export_lv2!(PortedGain);
openplugin::targets::export_shell!(PortedGain);

// In a plugin crate the rest is `tests/conformance.rs`.
openplugin_testing::install_allocator!();

fn main() {
    let mut plugin = PortedGain::default();
    plugin.prepare(48_000.0, 64, 2);
    assert!(plugin.set_param("gain", 50.0));
    let mut io = vec![vec![1.0f32; 64], vec![-1.0f32; 64]];
    for _ in 0..200 {
        io[0].fill(1.0);
        io[1].fill(-1.0);
        plugin.process(&mut ChannelsMut::new(&mut io), 64);
    }
    // 50 percent is half the level, once the smoother has settled.
    assert!((io[0][63] - 0.5).abs() < 1e-4 && (io[1][63] + 0.5).abs() < 1e-4);
    assert!(plugin.shared.take_peak() >= 0.5);
    assert_eq!(PARAMS[0].format(50.0), "50%");

    conformance(PortedGain::default, Options::quick()).assert_pass();
}
```

What moved where:

- `config.h` became `INFO`, `LAYOUTS` and `note_ports`. The Audio Unit codes and the bundle identifier are build options; `Plugin::aax_ids` pins the AAX codes.
- The `EParams` enum became the `PARAMS` table. With one parameter the string match in `set_param` costs nothing; a plugin with many overrides `set_param_index` and matches on the index.
- `GetParam(kGain)->Value()` became the field `percent`. Because `set_param` and `process` never overlap, the field is a plain `f32`.
- The sender and its queue became `Shared`, an atomic the audio thread raises and the editor takes.
- The last lines run the plugin through the conformance kit: every host sample rate and block size, NaN at the input, parameter fuzz, a check that the audio path does not allocate. A port is not done until it passes.

## The editor

iPlug2 builds the UI from controls attached to an `IGraphics` context. Here the editor is one object that owns its pixels. This one draws the gain as a bar and the peak as a line, and a drag sets the gain. A real editor would use the controls of `openplugin::dri`, which also give keyboard focus, typing a value and accessibility; [EDITOR.md](../EDITOR.md) and `examples/editor` show that.

```rust
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use openplugin::core::{Editor, EditorHost, EditorState, Frame, Gesture};

/// The same type the engine returns from `Plugin::editor_state`.
pub struct Shared {
    peak: AtomicU32,
}

const WIDTH: u32 = 200;
const HEIGHT: u32 = 40;
/// The gain parameter's index in the table, and its range.
const GAIN: usize = 0;
const MAX_PERCENT: f32 = 100.0;

pub struct GainEditor {
    host: EditorHost,
    percent: f32,
    peak: f32,
    dragging: bool,
    shared: Option<Arc<Shared>>,
    pixels: Vec<u32>,
}

impl GainEditor {
    pub fn new(host: EditorHost) -> Self {
        Self { host, percent: MAX_PERCENT, peak: 0.0, dragging: false, shared: None, pixels: Vec::new() }
    }

    fn drag_to(&mut self, x: f32) {
        self.percent = (x / WIDTH as f32).clamp(0.0, 1.0) * MAX_PERCENT;
        self.host.perform(GAIN, self.percent);
    }
}

impl Editor for GainEditor {
    fn size(&self) -> (u32, u32) {
        (WIDTH, HEIGHT)
    }

    // The engine this editor shows, or none. It can change while the editor is open.
    fn bind(&mut self, state: Option<EditorState>) {
        self.shared = state.and_then(|s| s.downcast::<Shared>().ok());
    }

    // `OnParamChangeUI`: automation, a preset, another surface.
    fn param_changed(&mut self, index: usize, plain: f32) {
        if index == GAIN {
            self.percent = plain;
        }
    }

    // `OnIdle`.
    fn tick(&mut self, _dt: f32) {
        if let Some(shared) = &self.shared {
            let fresh = f32::from_bits(shared.peak.swap(0, Ordering::Relaxed));
            self.peak = fresh.max(self.peak * 0.9);
        }
    }

    fn mouse_down(&mut self, x: f32, _y: f32, _clicks: u32) {
        self.dragging = true;
        self.host.begin(GAIN);
        self.drag_to(x);
    }

    fn mouse_move(&mut self, x: f32, _y: f32) {
        if self.dragging {
            self.drag_to(x);
        }
    }

    fn mouse_up(&mut self, _x: f32, _y: f32) {
        if self.dragging {
            self.dragging = false;
            self.host.end(GAIN);
        }
    }

    // Pixels are 0x00RRGGBB, rows top to bottom, `scale` device pixels per point.
    fn render(&mut self, scale: f32) -> Frame<'_> {
        let width = (WIDTH as f32 * scale).round() as usize;
        let height = (HEIGHT as f32 * scale).round() as usize;
        self.pixels.resize(width * height, 0);
        let bar = (self.percent / MAX_PERCENT * width as f32) as usize;
        let mark = ((self.peak.min(1.0) * width as f32) as usize).min(width - 1);
        for row in self.pixels.chunks_exact_mut(width) {
            for (x, pixel) in row.iter_mut().enumerate() {
                *pixel = if x == mark {
                    0x00FF_4040
                } else if x < bar {
                    0x0040_A0FF
                } else {
                    0x0020_2020
                };
            }
        }
        Frame::new(&self.pixels, width, height)
    }
}

/// Given to the export macros: `export_clap!(PortedGain, editor = make_editor)`.
pub fn make_editor(host: EditorHost) -> Option<Box<dyn Editor>> {
    Some(Box::new(GainEditor::new(host)))
}

fn main() {
    // A host that records what the editor reports.
    let seen = Rc::new(RefCell::new(Vec::new()));
    let log = seen.clone();
    let mut editor = make_editor(EditorHost::new(move |g| log.borrow_mut().push(g))).unwrap();

    editor.mouse_down(50.0, 10.0, 1);
    editor.mouse_move(150.0, 10.0);
    editor.mouse_up(150.0, 10.0);
    assert_eq!(
        *seen.borrow(),
        [Gesture::Begin(0), Gesture::Perform(0, 25.0), Gesture::Perform(0, 75.0), Gesture::End(0)]
    );

    // The engine's peak reaches the editor through the shared state.
    let shared = Arc::new(Shared { peak: AtomicU32::new(0.5f32.to_bits()) });
    editor.bind(Some(shared.clone()));
    editor.tick(1.0 / 30.0);

    let frame = editor.render(2.0);
    assert_eq!((frame.width, frame.height), (400, 80));
    assert_eq!(frame.pixels[0], 0x0040_A0FF);
    assert_eq!(frame.pixels[200], 0x00FF_4040);
    assert_eq!(frame.pixels[399], 0x0020_2020);
}
```

The differences that matter in a port:

- The editor has no pointer to the plugin. It reports a change with `begin`, `perform` and `end`, and the host brings the value back to the engine as an ordinary parameter change. That replaces the three `InformHostOfParamChange` calls and keeps automation recording correct in every format.
- Values that change elsewhere arrive through `param_changed`, in plain units.
- An editor exists before it has an engine, and the engine can be replaced while it is open. Keep what `bind` gives you as an `Option`.

## What has no counterpart yet

- **Factory presets in the host's menu.** `preset_count`, `preset_name` and `preset_values` reach a Rust host through `openplugin-host`, and the plugin's own editor can offer them, but no format entry publishes a program list to the host yet.
- **Power-curve and custom parameter shapes.** `Taper` is linear or logarithmic. A parameter shaped with `ShapePowCurve` keeps its range and plain values, but a host knob travels differently from the old plugin's.
- **Custom display text.** A parameter prints by its `Unit` and its choice names. A `SetDisplayFunc` that prints something else (note names, a ratio table) has no equivalent.
- **More than one output bus, or more than one sidechain.** An `IoConfig` is one main input, one main output and one optional sidechain.
- **SysEx.** MIDI reaches the plugin as messages of up to three bytes.
- **64-bit audio buffers.** `process` is `f32`.
- **Web view editors.** The editor contract is a frame of pixels.
- **Keeping the old plugin's saved state outside CLAP.** The identity itself carries over: the VST3 class ids, the Audio Unit codes and the parameter ids are pinned in the plugin (see [Identity pinning](../PORTING.md#identity-pinning); `openplugin-port scan` writes them). The VST3, Audio Unit and AAX shells still accept only state they wrote themselves, so a chunk saved by the old plugin reaches `load_state` only through CLAP; that part is open.
