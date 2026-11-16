# Porting from the CLAP SDK

This guide maps a plugin written directly against the CLAP headers onto OpenPluginSDK: the processor, parameters, state, editor, buses, notes and threading, then a small effect ported in full. It is written from CLAP's public headers and documentation and uses their public names. The Rust in it is original code for this SDK, and every code block is compiled and run as a doc test (`cargo test -p example-porting-guides`). Names are the ones CLAP 1.2 publishes; check them against the version you build with.

Read [PORTING.md](../PORTING.md) first: it has the order of work and the tools (scanner, state capture, null test) that every port uses.

## The short version

- CLAP is the format this SDK is closest to. Parameter values are plain numbers in both, the plugin id is a string in both, and the SDK's surround roles are CLAP's.
- You stop writing the C ABI. The entry, the factory, the descriptor and every extension struct come from one line, `export_clap!`, and the extensions offered follow from which `Plugin` methods you implement.
- The event loop in `process` goes away. The entry walks `in_events`, cuts the block at each event and calls `set_param`, the note calls or `midi` between the pieces.
- The same `Plugin` also exports LV2, and VST3, Audio Unit and AAX through the native shells.
- CLAP is also the one format where a port can open the old plugin's sessions today: the plugin id is yours to set, and the bytes of the old `clap.state` stream reach `load_state` unchanged.

## Concept map

### Processor

| CLAP | OpenPluginSDK | Notes |
|---|---|---|
| `clap_plugin_entry`, `clap_plugin_factory` | `export_clap!(MyPlugin)` | One plugin per binary |
| `clap_plugin_descriptor`: `id`, `name`, `vendor`, `url`, `version` | `PluginInfo { id, name, vendor, url, version, .. }` | Keep the old `id`: hosts find the plugin by it |
| `clap_plugin_descriptor::features` | `PluginInfo::category`, `PluginInfo::is_instrument` | |
| `clap_plugin::init`, `destroy` | `Default::default`, `Drop` | |
| `activate(sample_rate, min_frames_count, max_frames_count)` | `prepare(sample_rate, max_block, channels)` | Allocate here. A rate the plugin refuses in `accepts_sample_rate` fails the activation |
| `deactivate`, `start_processing`, `stop_processing` | Nothing to write | |
| `reset` | `reset()` | Must not allocate |
| `process(const clap_process*)`: `audio_inputs[0].data32`, `audio_outputs[0].data32`, `frames_count` | `process(io: &mut [Vec<f32>], frames)` | In place, planar: `io[channel][..frames]` |
| `data64` | Not available | `process` is `f32`; see the list at the end |
| The return value: `CLAP_PROCESS_CONTINUE`, `CLAP_PROCESS_TAIL`, `CLAP_PROCESS_SLEEP` | Nothing to return | The entry answers from `Plugin::tail` |
| `clap_plugin_latency::get`, `clap_host_latency::changed` | `Plugin::latency` | The entry asks for the restart CLAP requires when the value changes. `HostNotifier::latency_changed` reports a change made outside `process` |
| `clap_plugin_tail::get` | `Plugin::tail_samples`, or `Plugin::tail` returning `Tail::Finite(n)` or `Tail::Infinite` | |
| `clap_plugin_render::set`, `CLAP_RENDER_OFFLINE` | `Plugin::set_offline` | |
| `clap_process::transport`: `flags`, `tempo`, `song_pos_beats`, `bar_start`, `bar_number`, `tsig_num`, `tsig_denom` | `Plugin::set_transport(&Transport)` | Called before each block; keep the fields you use |
| `get_extension(id)` | Nothing to write | The entry offers an extension when the plugin has something to say through it |

### Parameters

| CLAP | OpenPluginSDK | Notes |
|---|---|---|
| `clap_plugin_params::count`, `get_info` | `static PARAMS: [ParamInfo; N]` | |
| `clap_param_info::id` | The position in the table, or `ParamInfo::with_clap_id` | The entry numbers parameters `0..n` in table order. Hosts store automation by this id, so keep the old order; ids that were not `0..n` are kept with `ParamInfo::with_clap_id` |
| `clap_param_info::name`, `module` | `ParamInfo::name`, `.with_group("Filter/Envelope")` | The group path is the module path |
| `min_value`, `max_value`, `default_value` | `ParamInfo::new(id, name, min, max, default, unit)` | Plain values in both |
| `CLAP_PARAM_IS_AUTOMATABLE` | The default | |
| `CLAP_PARAM_IS_STEPPED`, `CLAP_PARAM_IS_ENUM` | `.with_steps(n)`, `Unit::Toggle`, `.with_choices(&[...])` | |
| `CLAP_PARAM_IS_HIDDEN`, `CLAP_PARAM_IS_READONLY` | `.flag(ParamFlags::HIDDEN)`, `.flag(ParamFlags::READ_ONLY)` | |
| `CLAP_PARAM_IS_BYPASS` | `Plugin::set_host_bypass`, or a parameter with `ParamFlags::BYPASS` | |
| `CLAP_PARAM_IS_MODULATABLE`, `CLAP_PARAM_IS_MODULATABLE_PER_NOTE_ID` | `.flag(ParamFlags::MODULATABLE)`, `.flag(ParamFlags::POLY_MODULATION)` | |
| `clap_param_info::cookie` | `set_param_index(index, value)` | Override it to skip the string match |
| `get_value` | `param(id)` | |
| `value_to_text`, `text_to_value` | `ParamInfo::format`, `ParamInfo::parse` | By unit and choice names; see the list at the end |
| `CLAP_EVENT_PARAM_VALUE` in `in_events` | `set_param(id, value)` | Called at the event's time; the block is cut there |
| `CLAP_EVENT_PARAM_MOD` | `Plugin::param_modulation(offset, index, note, amount)` | See [EXPRESSION.md](../EXPRESSION.md) |
| `clap_plugin_params::flush` | Nothing to write | The entry applies the events |
| `CLAP_EVENT_PARAM_GESTURE_BEGIN`, `CLAP_EVENT_PARAM_VALUE`, `CLAP_EVENT_PARAM_GESTURE_END` pushed to `out_events` for a GUI edit | `EditorHost::begin`, `perform`, `end` | The entry queues the events and calls `request_flush` |
| `clap_host_params::rescan` | `HostNotifier::param_info_changed` | Names and value text only; the parameter set is fixed |

### State

| CLAP | OpenPluginSDK | Notes |
|---|---|---|
| `clap_plugin_state::save` and `load` that only stream the parameter values | Nothing to write | When parameters are the whole state, return `true` from `state_is_parameters_only`: the SDK saves and restores them |
| `save(clap_ostream)`, `load(clap_istream)` with more than parameters | `save_state() -> Option<Vec<u8>>`, `load_state(&[u8]) -> bool` | In CLAP these bytes are the whole stream, so put the parameter values in them. Start with a magic and a version, and change nothing when you return `false` |
| The stream the old plugin wrote | The same bytes, in `load_state` | Only for a port that saves a chunk of its own. See [Foreign state import](../PORTING.md#foreign-state-import) |
| `clap_host_state::mark_dirty` | Not available | See the list at the end |
| `clap_plugin_preset_load`, the preset discovery factory | Not available | See the list at the end |

### Editor

| CLAP | OpenPluginSDK | Notes |
|---|---|---|
| `clap_plugin_gui`: `create`, `destroy`, `set_parent`, `show`, `hide` | `export_clap!(MyPlugin, editor = make_editor)`, with `fn make_editor(EditorHost) -> Option<Box<dyn Editor>>`; `PluginInfo::has_editor` | The entry makes the Cocoa, Win32 or X11 view |
| Drawing into the parent window with a toolkit of your own | `Editor::render(scale) -> Frame` | Software pixels. `openplugin::dri` draws knobs, faders and meters; see [EDITOR.md](../EDITOR.md) |
| `get_size`, `can_resize`, `get_resize_hints`, `adjust_size`, `set_size`, `set_scale` | `Editor::size`, `Editor::size_range`, `Editor::resize`, the `scale` argument of `render` | Points; the entry converts where the window API uses pixels |
| `clap_host_gui::request_resize` | `EditorHost::request_resize` | |
| `clap_plugin_timer_support::on_timer`, `clap_plugin_posix_fd_support` | `Editor::tick(dt)` | About 30 times a second. The entry registers what the platform needs |
| A lock-free queue of your own from the GUI to `process` | `EditorHost` gestures | The entry stores the value in the parameter plane and reports it to the host |
| `clap_host_context_menu` | `EditorHost::context_menu` | |
| `clap_plugin_track_info::changed`, `clap_host_track_info::get` | `Plugin::track_info_changed`, `Editor::track_info` | |

### Buses

| CLAP | OpenPluginSDK | Notes |
|---|---|---|
| `clap_plugin_audio_ports::count`, `get`; `CLAP_AUDIO_PORT_IS_MAIN`, `CLAP_PORT_MONO`, `CLAP_PORT_STEREO` | `supported_layouts() -> &'static [IoConfig]` | The default is stereo in, stereo out |
| `in_place_pair` | Always in place | |
| A second input port for a sidechain | `IoConfig { sidechain: Some(layout), .. }` and `process_buses(io, sidechain, frames)` | One optional sidechain |
| `clap_plugin_audio_ports_config`: `count`, `get`, `select` | The list itself, and `set_layout` | A config's id is the layout's index in the list |
| `clap_plugin_surround::get_channel_map` | `SpeakerLayout::Surround51` and the others; `SpeakerLayout::index_of(role)` | The SDK's order is the CLAP order. See [SURROUND.md](../SURROUND.md) |
| `clap_plugin_ambisonic` | `SpeakerLayout::Ambisonic(order)` | ACN, SN3D |
| More than one output port | Not available | See the list at the end |

### Notes and MIDI

| CLAP | OpenPluginSDK | Notes |
|---|---|---|
| `clap_plugin_note_ports::count`, `get`, `supported_dialects` | `note_ports() -> NotePorts { input, output }` | An effect that reads no notes returns both `false` |
| The loop that interleaves `in_events` with rendering, by `header.time` | Calls between the pieces of the block | The offset argument of each call is relative to the next `process` call; the CLAP entry cuts the block at the event |
| `CLAP_EVENT_NOTE_ON`, `CLAP_EVENT_NOTE_OFF` with `note_id`, `channel`, `key`, `velocity` | `note_on(offset, note, velocity)`, `note_off(offset, note)`; `note_on_ref`, `note_off_ref` for the note id, port and channel | Velocity is 0 to 127 here, not 0 to 1. A wildcard key arrives in `note_off` as 255 |
| `CLAP_EVENT_MIDI` | `midi(offset, [u8; 3])` | A MIDI note-on with velocity 0 arrives as a note-on |
| `CLAP_EVENT_NOTE_EXPRESSION` | `note_expressions()`, `note_expression(offset, note, expression, value)` | |
| `CLAP_NOTE_DIALECT_MIDI_MPE` | `mpe_input() -> Option<MpeZones>` | |
| Events pushed to `out_events` | `pull_midi() -> Option<(u32, [u8; 3])>` | The entry drains it after each block |
| `CLAP_EVENT_MIDI_SYSEX`, `CLAP_EVENT_MIDI2` | Not available | See the list at the end |

### Threading

| CLAP | OpenPluginSDK | Notes |
|---|---|---|
| Functions marked `[audio-thread]`: `process`, `reset`, `params.flush` while processing | `process`, `set_param`, the note calls, `set_transport` and `reset`, one at a time | |
| Functions marked `[main-thread]`: `activate`, `state.save`, `state.load`, everything in `gui` | `prepare`, `save_state`, `load_state`, every `Editor` call | The entry never runs a state call while `process` runs: it takes the plugin between blocks, and a block that starts meanwhile passes the input through |
| Atomics and queues between the two | Plain fields | The entry carries parameter values in a lock-free plane |
| `clap_host::request_callback`, `on_main_thread` | Nothing to write | |
| `clap_plugin_thread_pool`, a thread of your own | `openplugin::core::worker::loader` for loading; no audio thread pool | See the list at the end |

## Worked port: a gate opened by notes

The original is an audio effect with a note input: it passes audio while any key is held and closes when the last one is released. Its descriptor lists the feature `audio-effect`. It implements `audio-ports` (one stereo port each way), `note-ports` (one input, the CLAP and MIDI dialects) and `params` with `attack` and `release` in milliseconds and `floor` in decibels, the level of the closed gate. Its `process` walks `in_events` in time order, renders up to each event, then handles it: `CLAP_EVENT_NOTE_ON`, `CLAP_EVENT_NOTE_OFF`, `CLAP_EVENT_MIDI` and `CLAP_EVENT_PARAM_VALUE`. `state.save` writes the three values.

The port keeps the held keys in a 128-bit set, so a repeated note-on or a stray note-off cannot leave the gate stuck, and smooths the gain with the SDK's `Ballistics`.

```rust
use openplugin::dsp::envelope::db_to_amp;
use openplugin::dsp::Ballistics;
use openplugin::prelude::*;
use openplugin_testing::{conformance, Options};

// `params.get_info`, in id order: attack 0, release 1, floor 2.
static PARAMS: [ParamInfo; 3] = [
    ParamInfo::new("attack", "Attack", 1.0, 200.0, 5.0, Unit::Ms).with_taper(Taper::Log),
    ParamInfo::new("release", "Release", 5.0, 2_000.0, 120.0, Unit::Ms).with_taper(Taper::Log),
    ParamInfo::new("floor", "Floor", -80.0, 0.0, -80.0, Unit::Db),
];

// The descriptor. The id is the old one.
static INFO: PluginInfo = PluginInfo {
    id: "com.example.note-gate",
    name: "Ported Note Gate",
    vendor: "Example Audio",
    url: "https://example.com",
    email: "",
    version: "2.0.0",
    category: Category::Dynamics,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

pub struct PortedNoteGate {
    attack_ms: f32,
    release_ms: f32,
    floor_db: f32,
    /// One bit per key that is down.
    held: u128,
    /// The gain, moving toward 1 or the floor.
    gain: Ballistics,
}

impl Default for PortedNoteGate {
    fn default() -> Self {
        Self {
            attack_ms: PARAMS[0].default,
            release_ms: PARAMS[1].default,
            floor_db: PARAMS[2].default,
            held: 0,
            gain: Ballistics::new(48_000.0),
        }
    }
}

impl PortedNoteGate {
    fn floor(&self) -> f32 {
        db_to_amp(self.floor_db)
    }
}

impl Plugin for PortedNoteGate {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    // `activate`.
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.gain.set_sample_rate(sample_rate);
        self.gain.set_attack_ms(self.attack_ms);
        self.gain.set_release_ms(self.release_ms);
        self.reset();
    }

    // `reset`: no key is down and the gate is closed.
    fn reset(&mut self) {
        self.held = 0;
        self.gain.reset(self.floor());
    }

    // `CLAP_EVENT_PARAM_VALUE`, at its time.
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "attack" => {
                self.attack_ms = PARAMS[0].clamp(value);
                self.gain.set_attack_ms(self.attack_ms);
            }
            "release" => {
                self.release_ms = PARAMS[1].clamp(value);
                self.gain.set_release_ms(self.release_ms);
            }
            "floor" => self.floor_db = PARAMS[2].clamp(value),
            _ => return false,
        }
        true
    }

    // `params.get_value`.
    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "attack" => Some(self.attack_ms),
            "release" => Some(self.release_ms),
            "floor" => Some(self.floor_db),
            _ => None,
        }
    }

    // `note-ports`: one input, no output. This is also the default.
    fn note_ports(&self) -> NotePorts {
        NotePorts { input: true, output: false }
    }

    // `CLAP_EVENT_NOTE_ON`, and a note-on that came as MIDI.
    fn note_on(&mut self, _offset: usize, note: u8, velocity: u8) {
        if note > 127 {
            return;
        }
        if velocity == 0 {
            // MIDI: a note-on with velocity 0 is a note-off.
            self.held &= !(1 << note);
        } else {
            self.held |= 1 << note;
        }
    }

    // `CLAP_EVENT_NOTE_OFF`. A wildcard key arrives as 255: every key.
    fn note_off(&mut self, _offset: usize, note: u8) {
        if note > 127 {
            self.held = 0;
        } else {
            self.held &= !(1 << note);
        }
    }

    fn all_notes_off(&mut self) {
        self.held = 0;
    }

    // The part of `process` between two events.
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let target = if self.held != 0 { 1.0 } else { self.floor() };
        for i in 0..frames {
            let gain = self.gain.step(target);
            for channel in io.iter_mut().take(2) {
                channel[i] *= gain;
            }
        }
    }

    // `state.save` wrote the three parameters and nothing else.
    fn state_is_parameters_only(&self) -> bool {
        true
    }
}

openplugin::targets::export_clap!(PortedNoteGate);
openplugin::targets::export_lv2!(PortedNoteGate);
openplugin::targets::export_shell!(PortedNoteGate);

// In a plugin crate the rest is `tests/conformance.rs`.
openplugin_testing::install_allocator!();

/// The gate's gain after `seconds` of a constant input.
fn after(plugin: &mut PortedNoteGate, seconds: f32) -> f32 {
    let mut io = vec![vec![1.0f32; 480]; 2];
    for _ in 0..(seconds * 100.0) as usize {
        io[0].fill(1.0);
        io[1].fill(1.0);
        plugin.process(&mut io, 480);
    }
    io[0][479]
}

fn main() {
    let mut plugin = PortedNoteGate::default();
    plugin.prepare(48_000.0, 480, 2);
    let closed = db_to_amp(-80.0);

    // Closed until a key goes down, open while any key is held.
    assert!((after(&mut plugin, 0.1) - closed).abs() < 1e-6);
    plugin.note_on(0, 60, 100);
    assert!(after(&mut plugin, 0.1) > 0.999);
    plugin.note_on(0, 64, 90);
    plugin.note_off(0, 60);
    assert!(after(&mut plugin, 0.1) > 0.999);
    plugin.note_off(0, 64);
    assert!(after(&mut plugin, 2.0) < 2.0 * closed);

    // A second note-on for a held key does not need a second note-off.
    plugin.note_on(0, 60, 100);
    plugin.note_on(0, 60, 100);
    plugin.note_off(0, 60);
    assert!(after(&mut plugin, 2.0) < 2.0 * closed);

    // Velocity 0, a wildcard note-off and all-notes-off each close it.
    plugin.note_on(0, 60, 100);
    plugin.note_on(0, 60, 0);
    assert_eq!(plugin.held, 0);
    plugin.note_on(0, 61, 100);
    plugin.note_off(0, 255);
    assert_eq!(plugin.held, 0);
    plugin.note_on(0, 62, 100);
    plugin.all_notes_off();
    assert_eq!(plugin.held, 0);

    // The floor is a parameter: at -6 dB the closed gate passes half.
    plugin.set_param("floor", -6.0206);
    assert!((after(&mut plugin, 2.0) - 0.5).abs() < 1e-3);

    conformance(PortedNoteGate::default, Options::quick()).assert_pass();
}
```

What moved where:

- The entry, the factory, the descriptor and the four extension structs became `INFO`, `PARAMS`, `note_ports` and the three export lines. The audio ports need nothing, because stereo in and stereo out is the default layout.
- The event loop is gone. What each `case` of its `switch` did is now a method: `set_param`, `note_on`, `note_off`. The entry calls them in time order and renders the audio between them through `process`, so the gate still opens on the sample the note arrives.
- A note that arrives as raw MIDI reaches the same two methods; other MIDI messages go to `Plugin::midi`, which this plugin leaves at its default.
- `state.save` and `state.load` are gone, because the three parameters are the whole state. The parameter ids `0`, `1`, `2` are the table positions, so automation saved for the old plugin still points at the right parameters.
- The test drives the notes the way the entry does, then runs the conformance kit, which sends notes, MIDI and `all_notes_off` at random offsets.

## What has no counterpart yet

- **Nothing for parameter ids.** A plugin that used ids other than `0..n` in table order (hashes, sparse numbers) keeps them with `ParamInfo::with_clap_id`, and its automation links with them (see [Identity pinning](../PORTING.md#identity-pinning)).
- **Custom value text.** A parameter prints and parses by its `Unit` and its choice names.
- **`mark_dirty`.** A change to state that is not a parameter cannot be reported to the host.
- **Presets in the host.** `preset_count`, `preset_name` and `preset_values` reach a Rust host through `openplugin-ffi`; the CLAP entry has no `preset-load` and no preset discovery.
- **64-bit audio buffers** (`data64`).
- **More than one output port, or more than one sidechain.** An `IoConfig` is one main input, one main output and one optional sidechain.
- **SysEx and MIDI 2.** MIDI reaches the plugin as messages of up to three bytes.
- **The host's thread pool** (`clap_plugin_thread_pool`).
- **Other extensions** such as `remote-controls`, `param-indication`, `state-context` and `voice-info`.
