//! Hostile-input fuzzing for the CLAP bridge.
//!
//! A deterministic xorshift64* generator (seed [`SEED`]) drives 3,000 stereo
//! and 500 mono blocks of junk at an in-process [`ClapAdapter`]: malformed
//! parameter events, note/MIDI/transport events, lying headers and event
//! counts, null and empty event lists, null and mismatched audio pointers,
//! block sizes from zero to four times the activated maximum, and in-place and
//! separate buffers of finite, huge and non-finite samples.
//!
//! Every call must return [`PROCESS_CONTINUE`] or [`PROCESS_ERROR`], must not
//! write past `frames_count` (a guard region after every channel is checked),
//! must leave every parameter finite and inside its range, and must leave the
//! plugin able to render a clean block at the expected gain afterwards. The
//! test process crashing or aborting is itself a failure: a panic in one of the
//! `extern "C"` callbacks aborts the binary.
//!
//! Deliberately not asserted: output finiteness when the input is NaN or
//! infinite (the gain path propagates non-finite samples by design), the exact
//! value a plugin sees for a junk note key or MIDI byte after the bridge has
//! narrowed it to `u8`/`[u8; 3]`, and what happens when a host genuinely hands
//! over a buffer shorter than the event struct named by `header.type_` (that is
//! out of the CLAP contract; this test keeps a full struct's bytes in every
//! slot and fuzzes the `size` field as data so the test itself stays in
//! bounds).
//!
//! The generator, iteration counts and seed are fixed, so a failure reproduces
//! exactly. Keep the test deterministic and fast.

use std::ffi::c_void;

use openplugin_core::{
    Category, IoConfig, ParamInfo, Plugin, PluginInfo, SpeakerLayout, Unit, STEREO_LAYOUTS,
};
use openplugin_targets::clap_abi::*;
use openplugin_targets::ClapAdapter;

/// Fixed seed for the deterministic xorshift64* stream.
const SEED: u64 = 0x5EED_0BAD_F00D_CAFE;
const STEREO_ITERATIONS: usize = 3_000;
const MONO_ITERATIONS: usize = 500;
/// Frames requested from `activate`; larger fuzzed blocks are four times this.
const MAX_BLOCK: u32 = 64;
/// Room for the largest event struct the bridge reads (`clap_event_param_value`).
const SLOT_BYTES: usize = 64;
/// Guard floats after every channel so an out-of-bounds write is visible.
const GUARD: usize = 32;
const INPUT_GUARD: f32 = -1111.5;
const OUTPUT_GUARD: f32 = -2222.5;

static PARAMS: [ParamInfo; 3] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
    ParamInfo::new("pan", "Pan", -1.0, 1.0, 0.0, Unit::None),
    ParamInfo::new("enabled", "Enabled", 0.0, 1.0, 0.0, Unit::Toggle),
];

static INFO: PluginInfo = PluginInfo::new(
    "com.opengrid.test.hostile",
    "Hostile Input",
    "OpenGrid",
    "0.0.1",
    Category::Utility,
    &PARAMS,
)
.with_url("https://github.com/MistaMin/OpenPluginSDK")
.with_email("contact@mistamin.com");

static MONO_LAYOUTS: &[IoConfig] = &[IoConfig {
    input: SpeakerLayout::Mono,
    output: SpeakerLayout::Mono,
    sidechain: None,
}];

/// Small plugin whose writes go through [`ParamInfo::clamp`].
struct Hostile {
    db: f32,
    linear: f32,
    pan: f32,
    enabled: f32,
    layouts: &'static [IoConfig],
    /// The last junk MIDI message, handed back to the host once.
    midi_mirror: Option<[u8; 3]>,
    notes: u32,
    midi_messages: u32,
    transports: u32,
}

impl Hostile {
    fn new(layouts: &'static [IoConfig]) -> Self {
        Self {
            db: 0.0,
            linear: 1.0,
            pan: 0.0,
            enabled: 0.0,
            layouts,
            midi_mirror: None,
            notes: 0,
            midi_messages: 0,
            transports: 0,
        }
    }

    fn stereo() -> Self {
        Self::new(STEREO_LAYOUTS)
    }

    fn mono() -> Self {
        Self::new(MONO_LAYOUTS)
    }
}

impl Plugin for Hostile {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {
        self.linear = 10f32.powf(self.db / 20.0);
    }

    fn reset(&mut self) {}

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "gain" => {
                self.db = PARAMS[0].clamp(value);
                self.linear = 10f32.powf(self.db / 20.0);
                true
            }
            "pan" => {
                self.pan = PARAMS[1].clamp(value);
                true
            }
            "enabled" => {
                self.enabled = PARAMS[2].clamp(value);
                true
            }
            _ => false,
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "gain" => Some(self.db),
            "pan" => Some(self.pan),
            "enabled" => Some(self.enabled),
            _ => None,
        }
    }

    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        // No allocation and no panic: every channel is at least `frames` long
        // because the bridge sizes it from activate's max_frames.
        for channel in io.iter_mut() {
            for sample in channel.iter_mut().take(frames) {
                *sample *= self.linear;
            }
        }
    }

    fn supported_layouts(&self) -> &'static [IoConfig] {
        self.layouts
    }

    fn note_on_channel(&mut self, _offset: usize, _channel: u8, _note: u8, _velocity: u8) {
        self.notes = self.notes.wrapping_add(1);
    }

    fn note_off(&mut self, _offset: usize, _note: u8) {
        self.notes = self.notes.wrapping_add(1);
    }

    fn midi(&mut self, _offset: usize, message: [u8; 3]) {
        self.midi_messages = self.midi_messages.wrapping_add(1);
        self.midi_mirror = Some(message);
    }

    fn pull_midi(&mut self) -> Option<(u32, [u8; 3])> {
        self.midi_mirror.take().map(|message| (0, message))
    }

    fn set_transport(&mut self, _transport: &openplugin_core::Transport) {
        self.transports = self.transports.wrapping_add(1);
    }
}

/// xorshift64*. Small, dependency-free, and identical on every run.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        assert!(seed != 0, "xorshift64* needs a non-zero seed");
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.0 = state;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, upper: u32) -> u32 {
        assert!(upper > 0);
        (self.next_u64() % u64::from(upper)) as u32
    }

    fn pick<T: Copy>(&mut self, values: &[T]) -> T {
        values[self.below(values.len() as u32) as usize]
    }
}

/// A slot in the host-provided event arena. Every slot is a full-size event
/// struct, even when its header claims a different size, so the `size` field
/// is fuzzed as data without making this test read out of bounds itself.
#[repr(C, align(8))]
struct Slot {
    bytes: [u8; SLOT_BYTES],
}

impl Slot {
    fn from_event<T>(event: &T) -> Self {
        let size = std::mem::size_of::<T>();
        assert!(
            size <= SLOT_BYTES,
            "event struct does not fit the fuzz slot"
        );
        let mut slot = Slot {
            bytes: [0; SLOT_BYTES],
        };
        unsafe {
            std::ptr::copy_nonoverlapping(
                event as *const T as *const u8,
                slot.bytes.as_mut_ptr(),
                size,
            );
        }
        slot
    }
}

struct EventList {
    slots: Vec<Slot>,
    /// The count the host reports. May be larger than `slots`, like a host that
    /// lies; `get` then returns null for the missing tail.
    count: u32,
    /// One index for which `get` returns null even though a slot exists.
    null_at: Option<u32>,
}

unsafe extern "C" fn list_size(list: *const clap_input_events) -> u32 {
    unsafe { &*((*list).ctx as *const EventList) }.count
}

unsafe extern "C" fn list_get(
    list: *const clap_input_events,
    index: u32,
) -> *const clap_event_header {
    let list = unsafe { &*((*list).ctx as *const EventList) };
    if list.null_at == Some(index) {
        return std::ptr::null();
    }
    match list.slots.get(index as usize) {
        Some(slot) => slot.bytes.as_ptr() as *const clap_event_header,
        None => std::ptr::null(),
    }
}

struct Sink {
    accept: bool,
    pushed: u32,
}

unsafe extern "C" fn sink_push(
    list: *const clap_output_events,
    _event: *const clap_event_header,
) -> bool {
    let sink = unsafe { &mut *((*list).ctx as *mut Sink) };
    sink.pushed += 1;
    sink.accept
}

#[derive(Clone, Copy)]
enum Signal {
    Finite,
    Loud,
    NonFinite,
    Zero,
}

fn sample(signal: Signal, index: usize) -> f32 {
    match signal {
        Signal::Finite => 0.5,
        Signal::Loud => {
            if index % 2 == 0 {
                1.0e30
            } else {
                -1.0e30
            }
        }
        Signal::NonFinite => [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 3.0e38][index % 4],
        Signal::Zero => 0.0,
    }
}

/// One host audio bus. Every real channel is `frames + GUARD` long; the guard
/// tail must be untouched after `process`.
struct Bus {
    frames: usize,
    channels: Vec<Vec<f32>>,
    ptrs: Vec<*mut f32>,
    declared_channels: u32,
    null_data: bool,
    guard: f32,
}

impl Bus {
    fn new(
        frames: usize,
        real_channels: usize,
        declared_channels: u32,
        signal: Signal,
        guard: f32,
        null_data: bool,
        null_channel: Option<usize>,
    ) -> Self {
        let mut channels = Vec::with_capacity(real_channels);
        for channel in 0..real_channels {
            let mut data = Vec::with_capacity(frames + GUARD);
            data.extend((0..frames).map(|index| sample(signal, index + channel)));
            data.extend(std::iter::repeat_n(guard, GUARD));
            channels.push(data);
        }
        // A hostile channel_count can exceed the buffers the host allocated;
        // the bridge must only look at the pointer slots it could have been
        // given, so cap the array and leave the rest null.
        let pointers = (declared_channels as usize).clamp(real_channels, 8);
        let mut ptrs = Vec::with_capacity(pointers);
        for channel in 0..pointers {
            let pointer = match channels.get_mut(channel) {
                Some(data) if null_channel != Some(channel) => data.as_mut_ptr(),
                _ => std::ptr::null_mut(),
            };
            ptrs.push(pointer);
        }
        Self {
            frames,
            channels,
            ptrs,
            declared_channels,
            null_data,
            guard,
        }
    }

    fn buffer(&mut self) -> clap_audio_buffer {
        clap_audio_buffer {
            data32: if self.null_data {
                std::ptr::null_mut()
            } else {
                self.ptrs.as_mut_ptr()
            },
            data64: std::ptr::null_mut(),
            channel_count: self.declared_channels,
            latency: 0,
            constant_mask: 0,
        }
    }

    fn guards_intact(&self) -> bool {
        self.channels
            .iter()
            .all(|data| data[self.frames..].iter().all(|value| *value == self.guard))
    }

    fn output(&self) -> Vec<Vec<f32>> {
        self.channels
            .iter()
            .map(|channel| channel[..self.frames].to_vec())
            .collect()
    }
}

fn event_header(size: u32, time: u32, space_id: u16, type_: u16, flags: u32) -> clap_event_header {
    clap_event_header {
        size,
        time,
        space_id,
        type_,
        flags,
    }
}

fn param_event(
    size: u32,
    time: u32,
    space_id: u16,
    type_: u16,
    param_id: u32,
    value: f64,
    flags: u32,
) -> clap_event_param_value {
    clap_event_param_value {
        header: event_header(size, time, space_id, type_, flags),
        param_id,
        cookie: std::ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        value,
    }
}

// One argument per CLAP note-event field keeps the fuzz constructors direct.
#[allow(clippy::too_many_arguments)]
fn note_event(
    type_: u16,
    size: u32,
    time: u32,
    note_id: i32,
    port_index: i16,
    channel: i16,
    key: i16,
    velocity: f64,
    flags: u32,
) -> clap_event_note {
    clap_event_note {
        header: event_header(size, time, 0, type_, flags),
        note_id,
        port_index,
        channel,
        key,
        velocity,
    }
}

fn midi_event(size: u32, time: u32, port_index: u16, data: [u8; 3], flags: u32) -> clap_event_midi {
    clap_event_midi {
        header: event_header(size, time, 0, EVENT_MIDI, flags),
        port_index,
        data,
    }
}

fn gesture_event(
    type_: u16,
    size: u32,
    time: u32,
    param_id: u32,
    flags: u32,
) -> clap_event_param_gesture {
    clap_event_param_gesture {
        header: event_header(size, time, 0, type_, flags),
        param_id,
    }
}

const PARAM_IDS: &[u32] = &[0, 1, 2, 3, 7, 255, 65_535, u32::MAX];
const NOTE_IDS: &[i32] = &[0, 1, -1, i32::MIN, i32::MAX, 1 << 20];
const PORTS: &[i16] = &[0, -1, 15, i16::MIN, i16::MAX, 1];
const JUNK_TYPES: &[u16] = &[0, 2, 3, 4, 8, 9, 11, 12, 99, 1_000, u16::MAX];
const NUMBERS: &[f64] = &[
    f64::NAN,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::MAX,
    -f64::MAX,
    1.0e300,
    -1.0e300,
    3.0e38,
    -3.0e38,
    1.0e-320,
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    6.0,
    24.0,
    -24.0,
    25.0,
    -25.0,
    99.0,
];

fn hostile_value(rng: &mut Rng) -> f64 {
    match rng.below(6) {
        0 => f64::from_bits(rng.next_u64()),
        1 => rng.next_u64() as f64,
        2 => -(rng.next_u64() as f64),
        _ => rng.pick(NUMBERS),
    }
}

fn hostile_param_id(rng: &mut Rng) -> u32 {
    match rng.below(6) {
        // Duplicate ids, including several writes to the same slot in one block.
        0..=2 => 0,
        3 => rng.pick(PARAM_IDS),
        _ => rng.next_u64() as u32,
    }
}

fn hostile_i16(rng: &mut Rng) -> i16 {
    rng.pick(&[0, 1, 42, 127, 255, -1, -32768, 32767])
}

fn hostile_time(rng: &mut Rng, frames: u32) -> u32 {
    match rng.below(8) {
        0 => 0,
        1 => frames,
        2 => frames.saturating_sub(1),
        3 => frames.saturating_add(1),
        4 => u32::MAX,
        5 => rng.below(1_000_000),
        _ => rng.below(frames.max(1)),
    }
}

fn hostile_size(rng: &mut Rng) -> u32 {
    match rng.below(8) {
        0 => 0,
        1 => 4,
        2 => 8,
        3 => 16,
        4 => std::mem::size_of::<clap_event_param_value>() as u32,
        5 => std::mem::size_of::<clap_event_param_value>() as u32 - 1,
        6 => u32::MAX,
        _ => rng.below(512),
    }
}

fn hostile_transport(rng: &mut Rng) -> clap_event_transport {
    clap_event_transport {
        header: event_header(0, rng.next_u64() as u32, rng.next_u64() as u16, 11, 0),
        flags: rng.next_u64() as u32,
        song_pos_beats: rng.next_u64() as i64,
        song_pos_seconds: rng.next_u64() as i64,
        tempo: hostile_value(rng),
        tempo_inc: hostile_value(rng),
        loop_start_beats: rng.next_u64() as i64,
        loop_end_beats: rng.next_u64() as i64,
        loop_start_seconds: rng.next_u64() as i64,
        loop_end_seconds: rng.next_u64() as i64,
        bar_start: rng.next_u64() as i64,
        bar_number: rng.next_u64() as i32,
        tsig_num: rng.next_u64() as u16,
        tsig_denom: rng.next_u64() as u16,
    }
}

fn push_random_event(rng: &mut Rng, slots: &mut Vec<Slot>, frames: u32) {
    let time = hostile_time(rng, frames);
    let size = hostile_size(rng);
    let flags = if rng.below(4) == 0 {
        rng.next_u64() as u32
    } else {
        0
    };
    let space_id = if rng.below(16) == 0 {
        rng.next_u64() as u16
    } else {
        0
    };
    match rng.below(9) {
        0 | 1 => {
            let id = hostile_param_id(rng);
            let value = hostile_value(rng);
            slots.push(Slot::from_event(&param_event(
                size,
                time,
                space_id,
                EVENT_PARAM_VALUE,
                id,
                value,
                flags,
            )));
        }
        2 => {
            let note = note_event(
                EVENT_NOTE_ON,
                size,
                time,
                rng.pick(NOTE_IDS),
                rng.pick(PORTS),
                hostile_i16(rng),
                hostile_i16(rng),
                hostile_value(rng),
                flags,
            );
            slots.push(Slot::from_event(&note));
        }
        3 => {
            let note = note_event(
                EVENT_NOTE_OFF,
                size,
                time,
                rng.pick(NOTE_IDS),
                rng.pick(PORTS),
                hostile_i16(rng),
                hostile_i16(rng),
                hostile_value(rng),
                flags,
            );
            slots.push(Slot::from_event(&note));
        }
        4 => {
            let data = [
                rng.next_u64() as u8,
                rng.next_u64() as u8,
                rng.next_u64() as u8,
            ];
            let midi = midi_event(size, time, rng.next_u64() as u16, data, flags);
            slots.push(Slot::from_event(&midi));
        }
        5 => {
            let gesture = gesture_event(
                EVENT_PARAM_GESTURE_BEGIN,
                size,
                time,
                hostile_param_id(rng),
                flags,
            );
            slots.push(Slot::from_event(&gesture));
        }
        6 => {
            let gesture = gesture_event(
                EVENT_PARAM_GESTURE_END,
                size,
                time,
                hostile_param_id(rng),
                flags,
            );
            slots.push(Slot::from_event(&gesture));
        }
        7 => {
            // A known layout under an unknown type id: it must be ignored.
            let type_ = rng.pick(JUNK_TYPES);
            let id = hostile_param_id(rng);
            let value = hostile_value(rng);
            slots.push(Slot::from_event(&param_event(
                size, time, 0, type_, id, value, flags,
            )));
        }
        _ => {
            // Parameter events outside namespace 0 must never reach the plane.
            let space_id = rng.pick(&[1, 2, 0x1234, u16::MAX]);
            let id = hostile_param_id(rng);
            let value = hostile_value(rng);
            slots.push(Slot::from_event(&param_event(
                size,
                time,
                space_id,
                EVENT_PARAM_VALUE,
                id,
                value,
                flags,
            )));
        }
    }
}

struct ProcessSpec {
    frames: u32,
    steady_time: i64,
    transport: *const clap_event_transport,
    in_events: *const clap_input_events,
    out_events: *const clap_output_events,
    in_count: u32,
    out_count: u32,
    null_inputs: bool,
    null_outputs: bool,
}

fn call_process(
    plugin: *const clap_plugin,
    spec: &ProcessSpec,
    inputs: &mut [clap_audio_buffer],
    outputs: &mut [clap_audio_buffer],
) -> i32 {
    let block = clap_process {
        steady_time: spec.steady_time,
        frames_count: spec.frames,
        transport: spec.transport,
        audio_inputs: if spec.null_inputs {
            std::ptr::null()
        } else {
            inputs.as_ptr()
        },
        audio_outputs: if spec.null_outputs {
            std::ptr::null_mut()
        } else {
            outputs.as_mut_ptr()
        },
        audio_inputs_count: spec.in_count,
        audio_outputs_count: spec.out_count,
        in_events: spec.in_events,
        out_events: spec.out_events,
    };
    let process = unsafe { (*plugin).process }.expect("clap_plugin.process");
    unsafe { process(plugin, &block) }
}

fn activate(
    plugin: *const clap_plugin,
    sample_rate: f64,
    min_frames: u32,
    max_frames: u32,
) -> bool {
    let activate = unsafe { (*plugin).activate }.expect("clap_plugin.activate");
    unsafe { activate(plugin, sample_rate, min_frames, max_frames) }
}

fn params_extension(plugin: *const clap_plugin) -> *const clap_plugin_params {
    let get_extension = unsafe { (*plugin).get_extension }.expect("clap_plugin.get_extension");
    let raw = unsafe { get_extension(plugin, c"clap.params".as_ptr()) };
    assert!(!raw.is_null(), "the CLAP bridge always offers clap.params");
    raw as *const clap_plugin_params
}

/// One clean block with `list` as the input events, returning the output.
fn play_events(
    plugin: *const clap_plugin,
    channels: usize,
    frames: u32,
    list: &EventList,
) -> (i32, Vec<Vec<f32>>) {
    let in_events = clap_input_events {
        ctx: list as *const EventList as *mut c_void,
        size: Some(list_size),
        get: Some(list_get),
    };
    let mut input = Bus::new(
        frames as usize,
        channels,
        channels as u32,
        Signal::Finite,
        INPUT_GUARD,
        false,
        None,
    );
    let mut output = Bus::new(
        frames as usize,
        channels,
        channels as u32,
        Signal::Zero,
        OUTPUT_GUARD,
        false,
        None,
    );
    let mut inputs = vec![input.buffer()];
    let mut outputs = vec![output.buffer()];
    let spec = ProcessSpec {
        frames,
        steady_time: 0,
        transport: std::ptr::null(),
        in_events: &in_events,
        out_events: std::ptr::null(),
        in_count: 1,
        out_count: 1,
        null_inputs: false,
        null_outputs: false,
    };
    let rc = call_process(plugin, &spec, &mut inputs, &mut outputs);
    assert!(
        input.guards_intact(),
        "wrote past frames_count into the input"
    );
    assert!(
        output.guards_intact(),
        "wrote past frames_count into the output"
    );
    (rc, output.output())
}

/// Send `db` as a clean parameter event, then render an event-free block and
/// check every sample is `0.5 * gain(db)`. This is the "the junk left the
/// parameter plane and the engine uncorrupted" assertion.
fn assert_clean_gain(
    plugin: *const clap_plugin,
    params: *const clap_plugin_params,
    channels: usize,
    db: f64,
) {
    let set = EventList {
        slots: vec![Slot::from_event(&param_event(
            std::mem::size_of::<clap_event_param_value>() as u32,
            0,
            0,
            EVENT_PARAM_VALUE,
            0,
            db,
            0,
        ))],
        count: 1,
        null_at: None,
    };
    let (rc, _) = play_events(plugin, channels, MAX_BLOCK, &set);
    assert_eq!(rc, PROCESS_CONTINUE, "setting the gain after fuzzing");

    let quiet = EventList {
        slots: Vec::new(),
        count: 0,
        null_at: None,
    };
    let (rc, output) = play_events(plugin, channels, MAX_BLOCK, &quiet);
    assert_eq!(rc, PROCESS_CONTINUE, "the clean block after fuzzing");
    assert_eq!(output.len(), channels);
    let expected = 0.5f32 * 10f32.powf(db as f32 / 20.0);
    for (channel, samples) in output.iter().enumerate() {
        for (index, value) in samples.iter().enumerate() {
            assert!(
                (value - expected).abs() < 1.0e-4,
                "channel {channel} sample {index}: {value} != {expected}"
            );
        }
    }

    let mut stored = 0.0f64;
    let get_value = unsafe { (*params).get_value }.expect("clap_plugin_params.get_value");
    assert!(unsafe { get_value(plugin, 0, &mut stored) });
    assert!(
        (stored - db).abs() < 1.0e-6,
        "host reads back {stored}, expected {db}"
    );
}

fn fuzz_blocks(adapter: &ClapAdapter<Hostile>, iterations: usize, rng: &mut Rng, channels: usize) {
    let plugin = adapter.as_plugin();
    let plane = adapter
        .plane()
        .expect("the adapter always has a parameter plane");
    let params = params_extension(plugin);

    // Coverage counters; they also make the fixed seed describe what it hit.
    let mut calls = 0usize;
    let mut errors = 0usize;
    let mut non_finite_blocks = 0usize;
    let mut null_event_blocks = 0usize;
    let mut oversized_blocks = 0usize;

    for iteration in 0..iterations {
        let frames = rng.pick(&[0u32, 1, 2, 7, 17, 64, 65, 129, 255]);
        let event_count = match rng.below(12) {
            0 => 300, // more than the bridge's 256-event schedule
            1 => 257,
            2 => 64,
            3 => 17,
            _ => rng.below(8) as usize,
        };
        let mut slots = Vec::with_capacity(event_count);
        for _ in 0..event_count {
            push_random_event(rng, &mut slots, frames);
        }
        let reported = match rng.below(12) {
            0 => slots.len() as u32 + 1 + rng.below(4),
            _ => slots.len() as u32,
        };
        let null_at = if !slots.is_empty() && rng.below(8) == 0 {
            Some(rng.below(slots.len() as u32))
        } else {
            None
        };
        let mut list = EventList {
            slots,
            count: reported,
            null_at,
        };

        let mut in_events = clap_input_events {
            ctx: &mut list as *mut EventList as *mut c_void,
            size: Some(list_size),
            get: Some(list_get),
        };
        let in_events_ptr: *const clap_input_events = match rng.below(14) {
            0 => std::ptr::null(),
            1 => {
                in_events.size = None;
                in_events.get = None;
                &in_events
            }
            2 => {
                in_events.get = None;
                &in_events
            }
            3 => {
                in_events.size = None;
                &in_events
            }
            _ => &in_events,
        };

        let mut sink = Sink {
            accept: rng.below(2) == 0,
            pushed: 0,
        };
        let mut out_events = clap_output_events {
            ctx: &mut sink as *mut Sink as *mut c_void,
            try_push: Some(sink_push),
        };
        let out_events_ptr: *const clap_output_events = match rng.below(8) {
            0 | 1 => std::ptr::null(),
            2 => {
                out_events.try_push = None;
                &out_events
            }
            _ => &out_events,
        };

        let transport = hostile_transport(rng);
        let transport_ptr = if rng.below(3) == 0 {
            std::ptr::null()
        } else {
            &transport as *const clap_event_transport
        };
        let random_steady = rng.next_u64() as i64;
        let steady_time = rng.pick(&[0i64, -1, i64::MIN, i64::MAX, 48_000, random_steady]);

        let in_place = rng.below(2) == 0;
        let signal = rng.pick(&[
            Signal::Finite,
            Signal::Finite,
            Signal::Loud,
            Signal::NonFinite,
            Signal::Zero,
        ]);
        let declared_in = rng.pick(&[channels as u32, channels as u32, 0, 1, 3]);
        let declared_out = rng.pick(&[channels as u32, channels as u32, 0, 1, 3]);
        let in_count = rng.pick(&[1u32, 1, 1, 0, 2, u32::MAX]);
        let out_count = rng.pick(&[1u32, 1, 1, 1, 0, 2, u32::MAX]);
        let null_inputs = rng.below(30) == 0;
        let null_outputs = rng.below(30) == 0;
        let null_in_data = rng.below(25) == 0;
        let null_out_data = rng.below(25) == 0;
        let null_in_channel = if channels == 2 && rng.below(20) == 0 {
            Some(1)
        } else {
            None
        };
        let null_out_channel = if channels == 2 && rng.below(20) == 0 {
            Some(1)
        } else {
            None
        };

        let mut input = Bus::new(
            frames as usize,
            channels,
            declared_in,
            signal,
            INPUT_GUARD,
            null_in_data,
            null_in_channel,
        );
        let mut separate_output = if in_place {
            None
        } else {
            Some(Bus::new(
                frames as usize,
                channels,
                declared_out,
                Signal::Zero,
                OUTPUT_GUARD,
                null_out_data,
                null_out_channel,
            ))
        };

        let mut inputs: Vec<clap_audio_buffer> = if in_count == 0 {
            Vec::new()
        } else if in_count == 2 {
            vec![input.buffer(), input.buffer()]
        } else {
            vec![input.buffer()]
        };
        let mut outputs: Vec<clap_audio_buffer> = if out_count == 0 {
            Vec::new()
        } else if let Some(output) = separate_output.as_mut() {
            vec![output.buffer()]
        } else {
            // In place: the same channel pointers for input and output.
            vec![input.buffer()]
        };

        let spec = ProcessSpec {
            frames,
            steady_time,
            transport: transport_ptr,
            in_events: in_events_ptr,
            out_events: out_events_ptr,
            in_count,
            out_count,
            null_inputs: null_inputs || in_count == 0,
            null_outputs: null_outputs || out_count == 0,
        };

        let input_visible = spec.in_count != 0 && !spec.null_inputs;
        let output_data_missing = match separate_output.as_ref() {
            Some(output) => output.null_data,
            // In place, the output shares the input's data pointer.
            None => input.null_data,
        };
        let expects_error = frames > 0
            && out_count != 0
            && (spec.null_outputs || output_data_missing || (input_visible && input.null_data));

        let rc = call_process(plugin, &spec, &mut inputs, &mut outputs);
        assert!(
            rc == PROCESS_CONTINUE || rc == PROCESS_ERROR,
            "iteration {iteration}: process returned {rc}"
        );
        assert_eq!(
            rc,
            if expects_error {
                PROCESS_ERROR
            } else {
                PROCESS_CONTINUE
            },
            "iteration {iteration}: frames {frames}, in {in_count}, out {out_count}, \
             in-place {in_place}, signal-non-finite {}",
            matches!(signal, Signal::NonFinite | Signal::Loud)
        );

        assert!(
            input.guards_intact(),
            "iteration {iteration}: the bridge wrote past frames_count into the input bus"
        );
        if let Some(output) = separate_output.as_ref() {
            assert!(
                output.guards_intact(),
                "iteration {iteration}: the bridge wrote past frames_count into the output bus"
            );
        }
        calls += 1;
        if rc == PROCESS_ERROR {
            errors += 1;
        }
        if matches!(signal, Signal::NonFinite | Signal::Loud) {
            non_finite_blocks += 1;
        }
        if in_events_ptr.is_null() {
            null_event_blocks += 1;
        }
        if frames > MAX_BLOCK {
            oversized_blocks += 1;
        }

        // The event flush path takes the same hostile lists.
        if iteration % 3 == 0 {
            let flush = unsafe { (*params).flush }.expect("clap_plugin_params.flush");
            unsafe { flush(plugin, in_events_ptr, out_events_ptr) };
        }
        assert!(
            sink.pushed <= 1,
            "iteration {iteration}: more than one MIDI event drained"
        );

        for (index, info) in PARAMS.iter().enumerate() {
            let value = plane.load(index).expect("every parameter has a plane slot");
            assert!(
                value.is_finite(),
                "iteration {iteration}: parameter {} became {value}",
                info.id
            );
            assert!(
                (info.min..=info.max).contains(&value),
                "iteration {iteration}: parameter {} is {value}, outside [{}, {}]",
                info.id,
                info.min,
                info.max
            );
        }
    }

    assert_eq!(
        calls, iterations,
        "the fuzz loop did not run its declared blocks"
    );
    assert!(
        errors > 0,
        "the fixed seed never reached the PROCESS_ERROR path"
    );
    assert!(
        non_finite_blocks > 0,
        "the fixed seed never sent non-finite audio"
    );
    assert!(
        null_event_blocks > 0,
        "the fixed seed never sent a null event list"
    );
    assert!(
        oversized_blocks > 0,
        "the fixed seed never sent an oversized block"
    );
}

#[test]
fn fuzzed_process_blocks_survive_and_leave_a_clean_gain() {
    let mut rng = Rng::new(SEED);

    let stereo = ClapAdapter::new(Hostile::stereo());
    let plugin = stereo.as_plugin();
    assert!(activate(plugin, 48_000.0, 1, MAX_BLOCK));
    let params = params_extension(plugin);
    fuzz_blocks(&stereo, STEREO_ITERATIONS, &mut rng, 2);
    assert_clean_gain(plugin, params, 2, 6.0);

    let mono = ClapAdapter::new(Hostile::mono());
    let plugin = mono.as_plugin();
    assert!(activate(plugin, 44_100.0, 1, MAX_BLOCK));
    let params = params_extension(plugin);
    fuzz_blocks(&mono, MONO_ITERATIONS, &mut rng, 1);
    assert_clean_gain(plugin, params, 1, -7.5);
}

#[test]
fn activation_rejects_hostile_rates_and_zero_is_a_one_frame_block() {
    let adapter = ClapAdapter::new(Hostile::stereo());
    let plugin = adapter.as_plugin();

    for rate in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -1.0,
        0.0,
        1.0e-300,
        999.0,
        768_001.0,
        1.0e12,
    ] {
        assert!(
            !activate(plugin, rate, 0, MAX_BLOCK),
            "activate accepted the hostile rate {rate}"
        );
    }
    assert!(
        activate(plugin, 1_000.0, 0, 0),
        "the shared range includes 1 kHz"
    );
    assert!(
        activate(plugin, 7_999.0, 0, 0),
        "the widened range includes 7.999 kHz"
    );
    assert_clean_gain(plugin, params_extension(plugin), 2, 0.0);

    // Reactivating at the normal rate with a real maximum is the state the
    // block-size matrix expects.
    assert!(activate(plugin, 48_000.0, 1, MAX_BLOCK));
}

#[test]
fn hostile_parameter_values_are_clamped_and_unknown_events_are_ignored() {
    let adapter = ClapAdapter::new(Hostile::stereo());
    let plugin = adapter.as_plugin();
    assert!(activate(plugin, 48_000.0, 1, MAX_BLOCK));
    let plane = adapter.plane().expect("parameter plane");

    // Duplicate id 0 at the same time: NaN falls to the default, the
    // infinities and huge finite values clamp to -24..24. The last one wins.
    let duplicates = EventList {
        slots: vec![
            Slot::from_event(&param_event(56, 0, 0, EVENT_PARAM_VALUE, 0, f64::NAN, 0)),
            Slot::from_event(&param_event(
                56,
                0,
                0,
                EVENT_PARAM_VALUE,
                0,
                f64::INFINITY,
                0,
            )),
            Slot::from_event(&param_event(56, 0, 0, EVENT_PARAM_VALUE, 0, -7.5, 0)),
            Slot::from_event(&param_event(56, 0, 0, EVENT_PARAM_VALUE, 0, 1.0e300, 0)),
            Slot::from_event(&param_event(56, 0, 0, EVENT_PARAM_VALUE, 0, -1.0e300, 0)),
        ],
        count: 5,
        null_at: None,
    };
    let (rc, _) = play_events(plugin, 2, MAX_BLOCK, &duplicates);
    assert_eq!(rc, PROCESS_CONTINUE);
    assert_eq!(
        plane.load(0),
        Some(-24.0),
        "the last duplicate wins, clamped"
    );

    // Out-of-range ids, including u32::MAX, must be dropped without touching
    // any real slot.
    let out_of_range = EventList {
        slots: vec![
            Slot::from_event(&param_event(56, 1, 0, EVENT_PARAM_VALUE, 3, 1.0e300, 0)),
            Slot::from_event(&param_event(
                56,
                2,
                0,
                EVENT_PARAM_VALUE,
                u32::MAX,
                f64::NAN,
                0,
            )),
            Slot::from_event(&param_event(56, 3, 0, EVENT_PARAM_VALUE, 65_535, -99.0, 0)),
        ],
        count: 3,
        null_at: None,
    };
    let (rc, _) = play_events(plugin, 2, MAX_BLOCK, &out_of_range);
    assert_eq!(rc, PROCESS_CONTINUE);
    assert_eq!(plane.load(0), Some(-24.0), "an unknown id changed gain");
    assert_eq!(plane.load(1), Some(0.0), "an unknown id changed pan");
    assert_eq!(plane.load(2), Some(0.0), "an unknown id changed the toggle");

    // The toggle rounds; a value in the middle lands on a defined stop.
    let toggle = EventList {
        slots: vec![Slot::from_event(&param_event(
            56,
            0,
            0,
            EVENT_PARAM_VALUE,
            2,
            0.4,
            0,
        ))],
        count: 1,
        null_at: None,
    };
    let (rc, _) = play_events(plugin, 2, MAX_BLOCK, &toggle);
    assert_eq!(rc, PROCESS_CONTINUE);
    assert_eq!(plane.load(2), Some(0.0), "0.4 rounds down for a toggle");

    for (index, info) in PARAMS.iter().enumerate() {
        let value = plane.load(index).expect("plane slot");
        assert!(value.is_finite());
        assert!((info.min..=info.max).contains(&value));
    }
}

#[test]
fn malformed_headers_and_unknown_types_do_not_touch_the_plane() {
    let adapter = ClapAdapter::new(Hostile::stereo());
    let plugin = adapter.as_plugin();
    assert!(activate(plugin, 48_000.0, 1, MAX_BLOCK));
    let plane = adapter.plane().expect("parameter plane");
    let before: Vec<f32> = (0..PARAMS.len())
        .map(|index| plane.load(index).expect("plane slot"))
        .collect();

    // Unknown event types, a foreign namespace, gestures, junk notes and MIDI,
    // and header sizes from zero to u32::MAX. None may reach a parameter.
    let slots = vec![
        Slot::from_event(&param_event(0, 0, 0, 999, 0, 24.0, 0)),
        Slot::from_event(&param_event(4, 0, 0, u16::MAX, 0, 24.0, 0)),
        Slot::from_event(&param_event(u32::MAX, 0, 0, 12, 1, f64::NAN, 0)),
        Slot::from_event(&param_event(56, 0, 7, EVENT_PARAM_VALUE, 0, 24.0, 0)),
        Slot::from_event(&param_event(56, 0, u16::MAX, EVENT_PARAM_VALUE, 0, 24.0, 0)),
        Slot::from_event(&gesture_event(EVENT_PARAM_GESTURE_BEGIN, 20, 0, 0, 0)),
        Slot::from_event(&gesture_event(EVENT_PARAM_GESTURE_END, 20, 0, 0, 0)),
        Slot::from_event(&note_event(
            EVENT_NOTE_ON,
            40,
            0,
            -1,
            -1,
            -1,
            -256,
            f64::NAN,
            0,
        )),
        Slot::from_event(&note_event(
            EVENT_NOTE_OFF,
            40,
            u32::MAX,
            i32::MAX,
            1,
            i16::MAX,
            i16::MAX,
            f64::INFINITY,
            0,
        )),
        Slot::from_event(&midi_event(24, 0, u16::MAX, [0xFF, 0x00, 0x7F], 0)),
        Slot::from_event(&midi_event(24, 1, 0, [0xF0, 0xF7, 0x00], u32::MAX)),
    ];
    let list = EventList {
        slots,
        count: 11,
        null_at: None,
    };
    let (rc, _) = play_events(plugin, 2, MAX_BLOCK, &list);
    assert_eq!(rc, PROCESS_CONTINUE);
    let after: Vec<f32> = (0..PARAMS.len())
        .map(|index| plane.load(index).expect("plane slot"))
        .collect();
    assert_eq!(after, before, "ignored events changed the parameter plane");

    // A known parameter event whose header lies about its size is dispatched
    // on `type_` alone. The bytes of a full struct are present in the slot, so
    // the bridge reads only what a valid event of that type would hold.
    let lying_sizes = EventList {
        slots: vec![
            Slot::from_event(&param_event(0, 0, 0, EVENT_PARAM_VALUE, 0, 1.0, 0)),
            Slot::from_event(&param_event(4, 1, 0, EVENT_PARAM_VALUE, 0, 2.0, 0)),
            Slot::from_event(&param_event(16, 2, 0, EVENT_PARAM_VALUE, 0, 3.0, 0)),
            Slot::from_event(&param_event(u32::MAX, 3, 0, EVENT_PARAM_VALUE, 0, 4.0, 0)),
        ],
        count: 4,
        null_at: None,
    };
    let (rc, _) = play_events(plugin, 2, MAX_BLOCK, &lying_sizes);
    assert_eq!(rc, PROCESS_CONTINUE);
    assert_eq!(
        plane.load(0),
        Some(4.0),
        "the last well-typed event still applies"
    );
}

#[test]
fn null_pointers_and_odd_event_lists_return_defined_codes() {
    let adapter = ClapAdapter::new(Hostile::stereo());
    let plugin = adapter.as_plugin();
    assert!(activate(plugin, 48_000.0, 1, MAX_BLOCK));
    let params = params_extension(plugin);

    let empty = EventList {
        slots: Vec::new(),
        count: 0,
        null_at: None,
    };
    let one = EventList {
        slots: vec![Slot::from_event(&param_event(
            56,
            0,
            0,
            EVENT_PARAM_VALUE,
            0,
            0.0,
            0,
        ))],
        count: 1,
        null_at: Some(0),
    };

    let play =
        |frames: u32, in_events: Option<&EventList>, null_in_events: bool, out_null: bool| {
            let mut input = Bus::new(
                frames as usize,
                2,
                2,
                Signal::Finite,
                INPUT_GUARD,
                false,
                None,
            );
            let mut output = Bus::new(
                frames as usize,
                2,
                2,
                Signal::Zero,
                OUTPUT_GUARD,
                false,
                None,
            );
            let mut inputs = vec![input.buffer()];
            let mut outputs = vec![output.buffer()];
            let input_events = in_events.map(|list| clap_input_events {
                ctx: list as *const EventList as *mut c_void,
                size: Some(list_size),
                get: Some(list_get),
            });
            let in_events_ptr = if null_in_events {
                std::ptr::null()
            } else {
                input_events.as_ref().map_or(std::ptr::null(), |events| {
                    events as *const clap_input_events
                })
            };
            let spec = ProcessSpec {
                frames,
                steady_time: 0,
                transport: std::ptr::null(),
                in_events: in_events_ptr,
                out_events: std::ptr::null(),
                in_count: 1,
                out_count: 1,
                null_inputs: false,
                null_outputs: out_null,
            };
            let rc = call_process(plugin, &spec, &mut inputs, &mut outputs);
            assert!(input.guards_intact());
            assert!(output.guards_intact());
            (rc, output.output())
        };

    // A null process pointer is a defined error, as is a null plugin.
    {
        let process = unsafe { (*plugin).process }.expect("process");
        let block: clap_process = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { process(plugin, std::ptr::null()) }, PROCESS_ERROR);
        assert_eq!(unsafe { process(std::ptr::null(), &block) }, PROCESS_ERROR);
    }

    assert_eq!(
        play(8, None, true, false).0,
        PROCESS_CONTINUE,
        "null in_events"
    );
    assert_eq!(
        play(8, Some(&empty), false, false).0,
        PROCESS_CONTINUE,
        "empty in_events"
    );
    assert_eq!(
        play(8, Some(&one), false, false).0,
        PROCESS_CONTINUE,
        "get returns null"
    );
    assert_eq!(
        play(8, Some(&empty), false, true).0,
        PROCESS_ERROR,
        "null audio_outputs"
    );
    assert_eq!(
        play(0, Some(&empty), false, true).0,
        PROCESS_CONTINUE,
        "zero frames wins first"
    );
    assert_eq!(
        play(8, None, true, false).1[0][0],
        0.5,
        "silent input is still processed"
    );

    // No output buffers at all: the block is skipped and events still apply.
    {
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let spec = ProcessSpec {
            frames: 8,
            steady_time: 0,
            transport: std::ptr::null(),
            in_events: std::ptr::null(),
            out_events: std::ptr::null(),
            in_count: 0,
            out_count: 0,
            null_inputs: true,
            null_outputs: true,
        };
        assert_eq!(
            call_process(plugin, &spec, &mut inputs, &mut outputs),
            PROCESS_CONTINUE
        );
        let spec = ProcessSpec {
            frames: 8,
            steady_time: 0,
            transport: std::ptr::null(),
            in_events: std::ptr::null(),
            out_events: std::ptr::null(),
            in_count: 0,
            out_count: 1,
            null_inputs: false,
            null_outputs: false,
        };
        let mut input = Bus::new(8, 2, 2, Signal::Finite, INPUT_GUARD, false, None);
        let mut output = Bus::new(8, 2, 2, Signal::Zero, OUTPUT_GUARD, false, None);
        let mut inputs = vec![input.buffer()];
        let mut outputs = vec![output.buffer()];
        assert_eq!(
            call_process(plugin, &spec, &mut inputs, &mut outputs),
            PROCESS_CONTINUE,
            "no audio inputs is silence, not an error"
        );
        assert_eq!(output.output()[0][0], 0.0, "silence in, silence out");
    }

    // A null channel pointer inside an otherwise valid buffer is skipped or
    // treated as silence, never dereferenced.
    {
        let mut input = Bus::new(8, 2, 2, Signal::Finite, INPUT_GUARD, false, Some(1));
        let mut output = Bus::new(8, 2, 2, Signal::Zero, OUTPUT_GUARD, false, Some(1));
        let mut inputs = vec![input.buffer()];
        let mut outputs = vec![output.buffer()];
        let spec = ProcessSpec {
            frames: 8,
            steady_time: 0,
            transport: std::ptr::null(),
            in_events: std::ptr::null(),
            out_events: std::ptr::null(),
            in_count: 1,
            out_count: 1,
            null_inputs: false,
            null_outputs: false,
        };
        assert_eq!(
            call_process(plugin, &spec, &mut inputs, &mut outputs),
            PROCESS_CONTINUE
        );
        assert_eq!(output.output()[0][0], 0.5, "the valid channel renders");
        assert_eq!(
            output.output()[1][0],
            0.0,
            "a null output channel is not written"
        );
    }

    // A null data32 on a real input buffer is an error, not a crash.
    {
        let mut input = Bus::new(8, 2, 2, Signal::Finite, INPUT_GUARD, true, None);
        let mut output = Bus::new(8, 2, 2, Signal::Zero, OUTPUT_GUARD, false, None);
        let mut inputs = vec![input.buffer()];
        let mut outputs = vec![output.buffer()];
        let spec = ProcessSpec {
            frames: 8,
            steady_time: 0,
            transport: std::ptr::null(),
            in_events: std::ptr::null(),
            out_events: std::ptr::null(),
            in_count: 1,
            out_count: 1,
            null_inputs: false,
            null_outputs: false,
        };
        assert_eq!(
            call_process(plugin, &spec, &mut inputs, &mut outputs),
            PROCESS_ERROR
        );
    }

    // Host-facing parameter queries with junk ids and pointers are refusals.
    let get_info = unsafe { (*params).get_info }.expect("get_info");
    let mut info: clap_param_info = unsafe { std::mem::zeroed() };
    assert!(!unsafe { get_info(plugin, u32::MAX, &mut info) });
    let get_value = unsafe { (*params).get_value }.expect("get_value");
    let mut value = 0.0f64;
    assert!(!unsafe { get_value(plugin, u32::MAX, &mut value) });
    assert!(!unsafe { get_value(plugin, 0, std::ptr::null_mut()) });
}

#[test]
fn block_sizes_buffer_modes_and_mono_stay_in_bounds() {
    let adapter = ClapAdapter::new(Hostile::stereo());
    let plugin = adapter.as_plugin();
    assert!(activate(plugin, 48_000.0, 1, MAX_BLOCK));
    let empty = EventList {
        slots: Vec::new(),
        count: 0,
        null_at: None,
    };
    let in_events = clap_input_events {
        ctx: &empty as *const EventList as *mut c_void,
        size: Some(list_size),
        get: Some(list_get),
    };

    let run = |frames: u32, channels: usize, in_place: bool, declared_out: u32| {
        let mut input = Bus::new(
            frames as usize,
            channels,
            channels as u32,
            Signal::Finite,
            INPUT_GUARD,
            false,
            None,
        );
        let mut output = Bus::new(
            frames as usize,
            channels,
            declared_out,
            Signal::Zero,
            OUTPUT_GUARD,
            false,
            None,
        );
        let mut inputs = vec![input.buffer()];
        let mut outputs = if in_place {
            vec![input.buffer()]
        } else {
            vec![output.buffer()]
        };
        let spec = ProcessSpec {
            frames,
            steady_time: 0,
            transport: std::ptr::null(),
            in_events: &in_events,
            out_events: std::ptr::null(),
            in_count: 1,
            out_count: 1,
            null_inputs: false,
            null_outputs: false,
        };
        let rc = call_process(plugin, &spec, &mut inputs, &mut outputs);
        assert!(input.guards_intact());
        assert!(output.guards_intact());
        let rendered = if in_place {
            input.output()
        } else {
            output.output()
        };
        (rc, rendered)
    };

    // The default gain is 0 dB, so a finite 0.5 input comes back as 0.5.
    let (rc, rendered) = run(0, 2, false, 2);
    assert_eq!(rc, PROCESS_CONTINUE);
    assert!(
        rendered.iter().all(|channel| channel.is_empty()),
        "zero frames writes nothing"
    );

    for frames in [1u32, 7, MAX_BLOCK] {
        let (rc, rendered) = run(frames, 2, false, 2);
        assert_eq!(rc, PROCESS_CONTINUE, "separate buffers, {frames} frames");
        assert!(
            rendered
                .iter()
                .all(|channel| channel.iter().all(|value| *value == 0.5)),
            "separate buffers, {frames} frames"
        );
    }

    // Four times the activated maximum: the bridge still renders every frame,
    // or would return a defined error. It must not crash or write past the end.
    let (rc, rendered) = run(255, 2, false, 2);
    assert_eq!(rc, PROCESS_CONTINUE);
    assert_eq!(rendered[0].len(), 255);
    assert!(rendered
        .iter()
        .all(|channel| channel.iter().all(|value| *value == 0.5)));

    for frames in [1u32, MAX_BLOCK, 129] {
        let (rc, rendered) = run(frames, 2, true, 2);
        assert_eq!(rc, PROCESS_CONTINUE, "in place, {frames} frames");
        assert!(rendered
            .iter()
            .all(|channel| channel.iter().all(|value| *value == 0.5)));
    }

    // Non-finite input: only survival and the guard regions are asserted.
    {
        let mut input = Bus::new(
            MAX_BLOCK as usize,
            2,
            2,
            Signal::NonFinite,
            INPUT_GUARD,
            false,
            None,
        );
        let mut output = Bus::new(
            MAX_BLOCK as usize,
            2,
            2,
            Signal::Zero,
            OUTPUT_GUARD,
            false,
            None,
        );
        let mut inputs = vec![input.buffer()];
        let mut outputs = vec![output.buffer()];
        let spec = ProcessSpec {
            frames: MAX_BLOCK,
            steady_time: 0,
            transport: std::ptr::null(),
            in_events: &in_events,
            out_events: std::ptr::null(),
            in_count: 1,
            out_count: 1,
            null_inputs: false,
            null_outputs: false,
        };
        assert_eq!(
            call_process(plugin, &spec, &mut inputs, &mut outputs),
            PROCESS_CONTINUE
        );
        assert!(input.guards_intact());
        assert!(output.guards_intact());
    }

    // Mono: one channel, separate and in place.
    let mono = ClapAdapter::new(Hostile::mono());
    let plugin = mono.as_plugin();
    assert!(activate(plugin, 48_000.0, 1, MAX_BLOCK));
    let in_events = clap_input_events {
        ctx: &empty as *const EventList as *mut c_void,
        size: Some(list_size),
        get: Some(list_get),
    };
    for in_place in [false, true] {
        let frames = 32u32;
        let mut input = Bus::new(
            frames as usize,
            1,
            1,
            Signal::Finite,
            INPUT_GUARD,
            false,
            None,
        );
        let mut output = Bus::new(
            frames as usize,
            1,
            1,
            Signal::Zero,
            OUTPUT_GUARD,
            false,
            None,
        );
        let mut inputs = vec![input.buffer()];
        let mut outputs = if in_place {
            vec![input.buffer()]
        } else {
            vec![output.buffer()]
        };
        let spec = ProcessSpec {
            frames,
            steady_time: 0,
            transport: std::ptr::null(),
            in_events: &in_events,
            out_events: std::ptr::null(),
            in_count: 1,
            out_count: 1,
            null_inputs: false,
            null_outputs: false,
        };
        assert_eq!(
            call_process(plugin, &spec, &mut inputs, &mut outputs),
            PROCESS_CONTINUE
        );
        assert!(input.guards_intact());
        assert!(output.guards_intact());
        let rendered = if in_place {
            input.output()
        } else {
            output.output()
        };
        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].iter().all(|value| *value == 0.5));
    }
}
