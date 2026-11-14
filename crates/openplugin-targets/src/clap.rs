//! CLAP target bridge.
//!
//! The lifecycle matches OpenGridder's from-scratch CLAP shell
//! (`OpenGridder-Plugins/native/clap/OpenGridderCLAP.mm`): `clap_entry`, one factory,
//! `init` / `activate` / `process`, then the audio-ports, note-ports, latency, params,
//! and state extensions. Parameter values are the plugin's real units. OpenGridder's
//! shell talks to the offload client and draws a Cocoa editor; this one calls
//! [`openplugin_core::Plugin`].
//!
//! With an editor (`export_clap!(P, editor = make_editor)`), the adapter also
//! offers `clap.gui` on macOS. The editor's gestures store the value in the
//! parameter plane at once (so the engine and `get_value` see it), and are
//! reported to the host as gesture-begin, param-value and gesture-end events
//! from the next `process` or `flush`, after a `request_flush`. Values that
//! the host changes reach the editor from the plane, polled by the view.
//!
//! A host loads the `clap_entry` symbol from a cdylib that called [`export_clap!`].
//! [`ClapAdapter`] builds the same instance in-process so a test can drive it.

use std::ffi::{CStr, CString, c_char, c_void};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use openplugin_core::{
    decode_params, encode_params, release_momentary, BusMeters, EditorFactory, EditorState, Gesture, GestureQueue,
    IoConfig, MeterFrame, ParamCursor, ParamFlags, ParamInfo, ParamPlane, Plugin, PortKind,
    Seqlock, SpeakerLayout, Tail, Transport, MAX_AUDIO_CHANNELS,
};

use crate::clap_abi::*;

type CreateFn = fn() -> Box<dyn Plugin>;

static CREATOR: Mutex<Option<CreateFn>> = Mutex::new(None);

struct Published {
    owned: Vec<CString>,
    features_owned: Vec<CString>,
    feature_ptrs: Vec<*const c_char>,
    desc: clap_plugin_descriptor,
}

// The raw pointers address this value's own CStrings and are not mutated after publication.
unsafe impl Send for Published {}
unsafe impl Sync for Published {}

static PUBLISHED: std::sync::OnceLock<Box<Published>> = std::sync::OnceLock::new();

/// Install the plugin a CLAP host will create. [`export_clap!`] does this from `clap_entry.init`.
pub fn set_creator(create: CreateFn) {
    *CREATOR.lock().expect("clap creator lock") = Some(create);
}

fn creator() -> Option<CreateFn> {
    *CREATOR.lock().expect("clap creator lock")
}

static EDITOR: Mutex<Option<EditorFactory>> = Mutex::new(None);

/// Install the editor `clap.gui` will open. `export_clap!(P, editor = f)`
/// does this from `clap_entry.init`.
pub fn set_editor(factory: EditorFactory) {
    if let Ok(mut slot) = EDITOR.lock() {
        *slot = Some(factory);
    }
}

fn editor_factory() -> Option<EditorFactory> {
    EDITOR.lock().ok().and_then(|slot| *slot)
}

/// Gestures an editor can have waiting before the host collects them.
const GESTURE_CAPACITY: usize = 1024;

fn cstring(text: &str) -> CString {
    CString::new(text).unwrap_or_else(|_| CString::new("openplugin").expect("static cstring"))
}

fn publish() -> Option<&'static Published> {
    let create = creator()?;
    Some(PUBLISHED.get_or_init(|| {
        let plugin = create();
        let info = plugin.info();
        let owned = vec![
            cstring(info.id),
            cstring(info.name),
            cstring(info.vendor),
            cstring(info.url),
            cstring(info.url),
            cstring(info.email),
            cstring(info.version),
            cstring(info.name),
        ];
        let features_owned = feature_labels(plugin.as_ref()).into_iter().map(cstring).collect();
        let mut published = Box::new(Published {
            owned,
            features_owned,
            feature_ptrs: Vec::new(),
            desc: clap_plugin_descriptor {
                clap_version: CLAP_VERSION,
                id: std::ptr::null(),
                name: std::ptr::null(),
                vendor: std::ptr::null(),
                url: std::ptr::null(),
                manual_url: std::ptr::null(),
                support_url: std::ptr::null(),
                version: std::ptr::null(),
                description: std::ptr::null(),
                features: std::ptr::null(),
            },
        });
        let raw = published.as_mut() as *mut Published;
        unsafe {
            let p = &mut *raw;
            p.feature_ptrs = p.features_owned.iter().map(|s| s.as_ptr()).collect();
            p.feature_ptrs.push(std::ptr::null());
            let s = |i: usize| p.owned[i].as_ptr();
            p.desc = clap_plugin_descriptor {
                clap_version: CLAP_VERSION,
                id: s(0),
                name: s(1),
                vendor: s(2),
                url: s(3),
                manual_url: s(4),
                support_url: s(5),
                version: s(6),
                description: s(7),
                features: p.feature_ptrs.as_ptr(),
            };
        }
        published
    }))
}

const MAX_SCHED: usize = 256;
const KIND_PARAM: u8 = 0;
const KIND_NOTE_ON: u8 = 1;
const KIND_NOTE_OFF: u8 = 2;
const KIND_MIDI: u8 = 3;

#[derive(Clone, Copy)]
struct Sched {
    time: u32,
    kind: u8,
    param: u32,
    value: f32,
    note: u8,
    velocity: u8,
    channel: u8,
    midi: [u8; 3],
}

impl Sched {
    const fn empty() -> Self {
        Self {
            time: 0,
            kind: 0,
            param: 0,
            value: 0.0,
            note: 0,
            velocity: 0,
            channel: 0,
            midi: [0; 3],
        }
    }
}

struct Instance {
    plugin: clap_plugin,
    desc: clap_plugin_descriptor,
    strings: Vec<CString>,
    feature_ptrs: Vec<*const c_char>,
    inner: Box<dyn Plugin>,
    plane: Arc<ParamPlane>,
    cursor: ParamCursor,
    /// Editor gestures waiting to be reported to the host.
    gestures: Arc<GestureQueue>,
    /// What the engine shares with its editor, taken once at creation.
    editor_state: Option<EditorState>,
    host_params: *const clap_host_params,
    #[cfg(target_os = "macos")]
    gui: Option<gui::Gui>,
    meters: Seqlock<MeterFrame>,
    bus_meters: Seqlock<BusMeters>,
    sched: [Sched; MAX_SCHED],
    sched_count: usize,
    buffers: Vec<Vec<f32>>,
    sidechain: Vec<Vec<f32>>,
    sample_rate: f32,
    max_block: usize,
    host: *const clap_host,
    reported_latency: u32,
    reported_tail: Option<u32>,
    layout_index: u32,
    activated: bool,
}

fn instance<'a>(plugin: *const clap_plugin) -> Option<&'a mut Instance> {
    if plugin.is_null() {
        return None;
    }
    unsafe {
        let data = (*plugin).plugin_data;
        if data.is_null() {
            None
        } else {
            Some(&mut *(data as *mut Instance))
        }
    }
}

fn copy_cstr(dst: &mut [c_char], text: &str) {
    let bytes = text.as_bytes();
    let n = bytes.len().min(dst.len().saturating_sub(1));
    for (i, b) in bytes.iter().take(n).enumerate() {
        dst[i] = *b as c_char;
    }
    if !dst.is_empty() {
        dst[n] = 0;
    }
}

fn push_sched(instance: &mut Instance, event: Sched) {
    if instance.sched_count < MAX_SCHED {
        instance.sched[instance.sched_count] = event;
        instance.sched_count += 1;
        return;
    }
    // The queue is full. Keep the value for the next pull so it is not dropped.
    if event.kind == KIND_PARAM {
        instance.plane.store(event.param as usize, event.value);
    }
}

fn collect_events(instance: &mut Instance, events: *const clap_input_events) {
    instance.sched_count = 0;
    if events.is_null() {
        return;
    }
    let events = unsafe { &*events };
    let (Some(size), Some(get)) = (events.size, events.get) else { return };
    let count = unsafe { size(events) };
    for index in 0..count {
        let header = unsafe { get(events, index) };
        if header.is_null() {
            continue;
        }
        let header = unsafe { &*header };
        match header.type_ {
            EVENT_PARAM_VALUE => {
                // CLAP namespace 0 is the only namespace defined for parameter events.
                // Events from another namespace must not reach the parameter plane.
                if header.space_id != 0 {
                    continue;
                }
                let event = unsafe { &*(header as *const clap_event_header as *const clap_event_param_value) };
                push_sched(instance, Sched {
                    time: header.time,
                    kind: KIND_PARAM,
                    param: event.param_id,
                    value: event.value as f32,
                    ..Sched::empty()
                });
            }
            EVENT_NOTE_ON => {
                let event = unsafe { &*(header as *const clap_event_header as *const clap_event_note) };
                let velocity = (event.velocity.clamp(0.0, 1.0) * 127.0).round() as u8;
                let channel = if event.channel < 0 { 0 } else { event.channel.min(15) as u8 };
                push_sched(instance, Sched {
                    time: header.time,
                    kind: KIND_NOTE_ON,
                    note: event.key as u8,
                    velocity,
                    channel,
                    ..Sched::empty()
                });
            }
            EVENT_NOTE_OFF => {
                let event = unsafe { &*(header as *const clap_event_header as *const clap_event_note) };
                push_sched(instance, Sched {
                    time: header.time,
                    kind: KIND_NOTE_OFF,
                    note: event.key as u8,
                    ..Sched::empty()
                });
            }
            EVENT_MIDI => {
                let event = unsafe { &*(header as *const clap_event_header as *const clap_event_midi) };
                push_sched(instance, Sched {
                    time: header.time,
                    kind: KIND_MIDI,
                    midi: event.data,
                    ..Sched::empty()
                });
            }
            _ => {}
        }
    }
    let count = instance.sched_count;
    for index in 1..count {
        let mut cursor = index;
        while cursor > 0 && instance.sched[cursor].time < instance.sched[cursor - 1].time {
            instance.sched.swap(cursor, cursor - 1);
            cursor -= 1;
        }
    }
}

fn apply_group(instance: &mut Instance, start: usize, end: usize) {
    let mut dirty = false;
    for index in start..end {
        let event = instance.sched[index];
        match event.kind {
            KIND_PARAM => {
                instance.plane.store(event.param as usize, event.value);
                dirty = true;
            }
            KIND_NOTE_ON => instance.inner.note_on_channel(0, event.channel, event.note, event.velocity),
            KIND_NOTE_OFF => instance.inner.note_off(0, event.note),
            KIND_MIDI => instance.inner.midi(0, event.midi),
            _ => {}
        }
    }
    if dirty {
        instance.cursor.pull(&instance.plane, instance.inner.as_mut());
        sync_bypass(instance);
    }
}

fn copy_bus(buffers: &mut [Vec<f32>], input: &clap_audio_buffer, start: usize, frames: usize) {
    let host_channels = input.channel_count as usize;
    for (channel, dst_buf) in buffers.iter_mut().enumerate() {
        let dst = &mut dst_buf[..frames];
        if channel >= host_channels || input.data32.is_null() {
            dst.fill(0.0);
            continue;
        }
        let src = unsafe { *input.data32.add(channel) };
        if src.is_null() {
            dst.fill(0.0);
        } else {
            let samples = unsafe { std::slice::from_raw_parts(src.add(start), frames) };
            dst.copy_from_slice(samples);
        }
    }
}

fn process_span(
    instance: &mut Instance,
    input: &clap_audio_buffer,
    sidechain: Option<&clap_audio_buffer>,
    output: &mut clap_audio_buffer,
    start: usize,
    end: usize,
) {
    let n = end.saturating_sub(start);
    if n == 0 || instance.buffers.is_empty() || instance.buffers[0].len() < n {
        return;
    }
    copy_bus(&mut instance.buffers, input, start, n);
    if let Some(side) = sidechain {
        if !instance.sidechain.is_empty() && instance.sidechain[0].len() >= n {
            copy_bus(&mut instance.sidechain, side, start, n);
        }
    } else {
        for channel in &mut instance.sidechain {
            channel[..n].fill(0.0);
        }
    }
    let Instance {
        inner,
        buffers,
        sidechain,
        ..
    } = instance;
    let sidechain_arg = if sidechain.is_empty() {
        None
    } else {
        Some(sidechain.as_slice())
    };
    inner.process_buses(buffers, sidechain_arg, n);
    let out_channels = (output.channel_count as usize).min(buffers.len());
    if output.data32.is_null() {
        return;
    }
    for channel in 0..out_channels {
        let dst = unsafe { *output.data32.add(channel) };
        if dst.is_null() {
            continue;
        }
        let out = unsafe { std::slice::from_raw_parts_mut(dst.add(start), n) };
        out.copy_from_slice(&buffers[channel][..n]);
    }
}

fn render_block(
    instance: &mut Instance,
    input: &clap_audio_buffer,
    sidechain: Option<&clap_audio_buffer>,
    output: &mut clap_audio_buffer,
    frames: usize,
) {
    let max_block = instance.max_block.max(1);
    let count = instance.sched_count;
    let mut done = 0;
    let mut event = 0;
    while done < frames {
        let chunk_end = (done + max_block).min(frames);
        instance.cursor.pull(&instance.plane, instance.inner.as_mut());
        sync_bypass(instance);
        let mut cursor = done;
        while event < count {
            let raw = instance.sched[event].time as usize;
            if raw > chunk_end {
                break;
            }
            let at = raw.clamp(done, chunk_end);
            if at > cursor {
                process_span(instance, input, sidechain, output, cursor, at);
                cursor = at;
            }
            let group_time = instance.sched[event].time;
            let group_start = event;
            while event < count && instance.sched[event].time == group_time {
                event += 1;
            }
            apply_group(instance, group_start, event);
        }
        if cursor < chunk_end {
            process_span(instance, input, sidechain, output, cursor, chunk_end);
        }
        done = chunk_end;
    }
    instance.meters.publish(instance.inner.meters());
    instance.bus_meters.publish(instance.inner.bus_meters());
    watch_latency(instance);
    watch_tail(instance);
}

unsafe extern "C" fn plugin_init(plugin: *const clap_plugin) -> bool {
    let Some(instance) = instance(plugin) else { return false };
    instance.host_params = host_call(instance.host, EXT_PARAMS) as *const clap_host_params;
    true
}

unsafe extern "C" fn plugin_destroy(plugin: *const clap_plugin) {
    if plugin.is_null() {
        return;
    }
    let data = unsafe { (*plugin).plugin_data };
    if !data.is_null() {
        let mut instance = unsafe { Box::from_raw(data as *mut Instance) };
        // The view goes before the engine whose state it shows.
        #[cfg(target_os = "macos")]
        {
            instance.gui = None;
        }
        drop(instance);
    }
}

unsafe extern "C" fn plugin_activate(plugin: *const clap_plugin, sample_rate: f64, _min_frames: u32, max_frames: u32) -> bool {
    let Some(instance) = instance(plugin) else { return false };
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return false;
    }
    let rate = sample_rate as f32;
    if !instance.inner.accepts_sample_rate(rate) {
        return false;
    }
    let max_block = max_frames.max(1) as usize;
    let layout = selected_layout(instance);
    let main = layout.main_channels().clamp(1, MAX_AUDIO_CHANNELS);
    let side = layout.sidechain_channels().min(MAX_AUDIO_CHANNELS);
    instance.sample_rate = rate;
    instance.max_block = max_block;
    instance.buffers = vec![vec![0.0; max_block]; main];
    instance.sidechain = vec![vec![0.0; max_block]; side];
    instance.inner.prepare_layout(rate, max_block, layout);
    instance.cursor.invalidate();
    instance.cursor.pull(&instance.plane, instance.inner.as_mut());
    sync_bypass(instance);
    notify_latency(instance);
    instance.activated = true;
    true
}

unsafe extern "C" fn plugin_deactivate(plugin: *const clap_plugin) {
    if let Some(instance) = instance(plugin) {
        instance.activated = false;
    }
}

unsafe extern "C" fn plugin_start(_plugin: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn plugin_stop(_plugin: *const clap_plugin) {}

unsafe extern "C" fn plugin_reset(plugin: *const clap_plugin) {
    if let Some(instance) = instance(plugin) {
        instance.inner.reset();
    }
}

unsafe extern "C" fn plugin_process(plugin: *const clap_plugin, process: *const clap_process) -> i32 {
    let Some(instance) = instance(plugin) else { return PROCESS_ERROR };
    if process.is_null() {
        return PROCESS_ERROR;
    }
    let process = unsafe { &*process };
    collect_events(instance, process.in_events);
    report_gestures(instance, process.out_events);
    apply_transport(instance, process.transport, process.steady_time);
    if process.frames_count == 0 || process.audio_outputs_count == 0 {
        if instance.sched_count > 0 {
            let count = instance.sched_count;
            apply_group(instance, 0, count);
        }
        watch_latency(instance);
        watch_tail(instance);
        return PROCESS_CONTINUE;
    }
    if process.audio_outputs.is_null() {
        return PROCESS_ERROR;
    }
    let output = unsafe { &mut *process.audio_outputs };
    if output.data32.is_null() {
        return PROCESS_ERROR;
    }
    let frames = process.frames_count as usize;
    let layout = selected_layout(instance);
    let (input, sidechain) = if process.audio_inputs_count == 0 || process.audio_inputs.is_null() {
        (None, None)
    } else {
        let input = unsafe { &*process.audio_inputs };
        let side = if process.audio_inputs_count > 1 && layout.sidechain_channels() > 0 {
            Some(unsafe { &*process.audio_inputs.add(1) })
        } else {
            None
        };
        (Some(input), side)
    };
    if instance.buffers.is_empty() {
        return PROCESS_ERROR;
    }
    if let Some(input) = input {
        if input.data32.is_null() && layout.input_channels() > 0 {
            return PROCESS_ERROR;
        }
        render_block(instance, input, sidechain, output, frames);
    } else {
        let silent = clap_audio_buffer {
            data32: std::ptr::null_mut(),
            data64: std::ptr::null_mut(),
            channel_count: 0,
            latency: 0,
            constant_mask: 0,
        };
        render_block(instance, &silent, None, output, frames);
    }
    if !process.out_events.is_null() {
        drain_midi(instance, process.out_events);
    }
    PROCESS_CONTINUE
}

fn selected_layout(instance: &Instance) -> IoConfig {
    instance
        .inner
        .supported_layouts()
        .get(instance.layout_index as usize)
        .copied()
        .unwrap_or(IoConfig::STEREO)
}

fn port_type_ptr(layout: SpeakerLayout) -> *const c_char {
    let bytes: &'static [u8] = match layout.kind() {
        PortKind::Mono => PORT_MONO,
        PortKind::Stereo => PORT_STEREO,
        PortKind::Surround => PORT_SURROUND,
        PortKind::Ambisonic => PORT_AMBISONIC,
        PortKind::Empty | PortKind::Discrete => return std::ptr::null(),
    };
    bytes.as_ptr() as *const c_char
}

fn write_audio_port(info: &mut clap_audio_port_info, id: u32, name: &str, layout: SpeakerLayout, main: bool) {
    info.id = id;
    copy_cstr(&mut info.name, name);
    info.flags = if main { AUDIO_PORT_IS_MAIN } else { 0 };
    info.channel_count = layout.channels().unwrap_or(0) as u32;
    info.port_type = port_type_ptr(layout);
    info.in_place_pair = if main { id } else { CLAP_INVALID_ID };
}

fn layout_port(layout: IoConfig, is_input: bool, index: u32) -> Option<(u32, &'static str, SpeakerLayout, bool)> {
    if is_input {
        if layout.input != SpeakerLayout::Absent && index == 0 {
            return Some((0, "Audio In", layout.input, true));
        }
        let side_index = if layout.input == SpeakerLayout::Absent { 0 } else { 1 };
        if index == side_index {
            if let Some(side) = layout.sidechain {
                if side.channels().unwrap_or(0) > 0 {
                    return Some((1, "Sidechain", side, false));
                }
            }
        }
        None
    } else if index == 0 && layout.output != SpeakerLayout::Absent {
        Some((0, "Audio Out", layout.output, true))
    } else {
        None
    }
}

unsafe extern "C" fn audio_ports_count(plugin: *const clap_plugin, is_input: bool) -> u32 {
    let Some(instance) = instance(plugin) else { return 0 };
    let layout = selected_layout(instance);
    if is_input {
        let main = if layout.input == SpeakerLayout::Absent { 0 } else { 1 };
        let side = if layout.sidechain_channels() > 0 { 1 } else { 0 };
        main + side
    } else if layout.output == SpeakerLayout::Absent {
        0
    } else {
        1
    }
}

unsafe extern "C" fn audio_ports_get(plugin: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_audio_port_info) -> bool {
    let Some(instance) = instance(plugin) else { return false };
    if info.is_null() {
        return false;
    }
    let Some((id, name, layout, main)) = layout_port(selected_layout(instance), is_input, index) else {
        return false;
    };
    unsafe {
        std::ptr::write_bytes(info, 0, 1);
        write_audio_port(&mut *info, id, name, layout, main);
    }
    true
}

static AUDIO_PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(audio_ports_count),
    get: Some(audio_ports_get),
};

unsafe extern "C" fn note_ports_count(plugin: *const clap_plugin, is_input: bool) -> u32 {
    let Some(instance) = instance(plugin) else { return 0 };
    let ports = instance.inner.note_ports();
    u32::from(if is_input { ports.input } else { ports.output })
}

unsafe extern "C" fn note_ports_get(_plugin: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_note_port_info) -> bool {
    if !is_input || index != 0 || info.is_null() {
        return false;
    }
    unsafe {
        std::ptr::write_bytes(info, 0, 1);
        let info = &mut *info;
        info.id = 0;
        info.supported_dialects = NOTE_DIALECT_CLAP | NOTE_DIALECT_MIDI;
        info.preferred_dialect = NOTE_DIALECT_MIDI;
        copy_cstr(&mut info.name, "MIDI In");
    }
    true
}

static NOTE_PORTS: clap_plugin_note_ports = clap_plugin_note_ports {
    count: Some(note_ports_count),
    get: Some(note_ports_get),
};

unsafe extern "C" fn latency_get(plugin: *const clap_plugin) -> u32 {
    instance(plugin).map(|instance| instance.inner.latency() as u32).unwrap_or(0)
}

static LATENCY: clap_plugin_latency = clap_plugin_latency { get: Some(latency_get) };

unsafe extern "C" fn params_count(plugin: *const clap_plugin) -> u32 {
    instance(plugin).map(|instance| instance.inner.info().params.len() as u32).unwrap_or(0)
}

unsafe extern "C" fn params_get_info(plugin: *const clap_plugin, index: u32, info: *mut clap_param_info) -> bool {
    let Some(instance) = instance(plugin) else { return false };
    let Some(param) = instance.inner.info().params.get(index as usize) else { return false };
    if info.is_null() {
        return false;
    }
    unsafe {
        std::ptr::write_bytes(info, 0, 1);
        let info = &mut *info;
        info.id = index;
        info.flags = clap_param_flags(param);
        copy_cstr(&mut info.name, param.name);
        info.min_value = param.min as f64;
        info.max_value = param.max as f64;
        info.default_value = param.default as f64;
    }
    true
}

unsafe extern "C" fn params_get_value(plugin: *const clap_plugin, param_id: u32, out_value: *mut f64) -> bool {
    let Some(instance) = instance(plugin) else { return false };
    let Some(value) = instance.plane.load(param_id as usize) else { return false };
    if out_value.is_null() {
        return false;
    }
    unsafe { *out_value = value as f64 };
    true
}

unsafe extern "C" fn params_value_to_text(plugin: *const clap_plugin, param_id: u32, value: f64, out_buffer: *mut c_char, capacity: u32) -> bool {
    let Some(instance) = instance(plugin) else { return false };
    let Some(param) = instance.inner.info().params.get(param_id as usize) else { return false };
    if out_buffer.is_null() || capacity == 0 {
        return false;
    }
    let text = param.format(value as f32);
    let dst = unsafe { std::slice::from_raw_parts_mut(out_buffer, capacity as usize) };
    copy_cstr(dst, &text);
    true
}

unsafe extern "C" fn params_text_to_value(plugin: *const clap_plugin, param_id: u32, text: *const c_char, out_value: *mut f64) -> bool {
    if text.is_null() || out_value.is_null() {
        return false;
    }
    let Some(instance) = instance(plugin) else { return false };
    let Some(param) = instance.inner.info().params.get(param_id as usize) else { return false };
    let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    let Some(value) = param.parse(&text) else { return false };
    unsafe { *out_value = value as f64 };
    true
}

fn event_header(kind: u16, size: usize) -> clap_event_header {
    clap_event_header { size: size as u32, time: 0, space_id: 0, type_: kind, flags: 0 }
}

/// Hand the editor's gestures to the host: begin, value and end events in
/// the order the editor made them. The values are already in the plane.
fn report_gestures(instance: &Instance, out: *const clap_output_events) {
    if instance.gestures.is_empty() {
        return;
    }
    let push = if out.is_null() { None } else { unsafe { (*out).try_push } };
    let infos = instance.plane.infos();
    while let Some(gesture) = instance.gestures.pop() {
        let Some(push) = push else { continue };
        match gesture {
            Gesture::Begin(id) | Gesture::End(id) => {
                if id as usize >= infos.len() {
                    continue;
                }
                let kind = if matches!(gesture, Gesture::Begin(_)) { EVENT_PARAM_GESTURE_BEGIN } else { EVENT_PARAM_GESTURE_END };
                let event = clap_event_param_gesture {
                    header: event_header(kind, std::mem::size_of::<clap_event_param_gesture>()),
                    param_id: id,
                };
                unsafe { push(out, &event.header) };
            }
            Gesture::Perform(id, value) => {
                let Some(info) = infos.get(id as usize) else { continue };
                let event = clap_event_param_value {
                    header: event_header(EVENT_PARAM_VALUE, std::mem::size_of::<clap_event_param_value>()),
                    param_id: id,
                    cookie: std::ptr::null_mut(),
                    note_id: -1,
                    port_index: -1,
                    channel: -1,
                    key: -1,
                    value: info.clamp(value) as f64,
                };
                unsafe { push(out, &event.header) };
            }
        }
    }
}

unsafe extern "C" fn params_flush(plugin: *const clap_plugin, input: *const clap_input_events, output: *const clap_output_events) {
    if let Some(instance) = instance(plugin) {
        collect_events(instance, input);
        report_gestures(instance, output);
        if instance.sched_count > 0 {
            let count = instance.sched_count;
            apply_group(instance, 0, count);
        }
    }
}

static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_get_info),
    get_value: Some(params_get_value),
    value_to_text: Some(params_value_to_text),
    text_to_value: Some(params_text_to_value),
    flush: Some(params_flush),
};

unsafe extern "C" fn state_save(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool {
    let Some(instance) = instance(plugin) else { return false };
    if stream.is_null() {
        return false;
    }
    let stream = unsafe { &*stream };
    let Some(write) = stream.write else { return false };
    let bytes = if let Some(custom) = instance.inner.save_state() {
        custom
    } else {
        let info = instance.inner.info();
        let values: Vec<f32> = (0..instance.plane.len())
            .map(|index| instance.plane.load(index).unwrap_or(0.0))
            .collect();
        encode_params(info, &values)
    };
    let mut offset = 0usize;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        let wrote = unsafe { write(stream, remaining.as_ptr() as *const c_void, remaining.len() as u64) };
        if wrote <= 0 {
            return false;
        }
        offset = offset.saturating_add(wrote as usize);
    }
    true
}

unsafe extern "C" fn state_load(plugin: *const clap_plugin, stream: *const clap_istream) -> bool {
    let Some(instance) = instance(plugin) else { return false };
    if stream.is_null() {
        return false;
    }
    let stream = unsafe { &*stream };
    let Some(read) = stream.read else { return false };
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = unsafe { read(stream, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u64) };
        if n <= 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n as usize]);
    }
    if bytes.is_empty() {
        return false;
    }
    if instance.inner.load_state(&bytes) {
        // The host reads parameters from the plane. A state chunk changes the plugin
        // directly, so republish the loaded values or the host would keep the old ones.
        instance.plane.seed_from(instance.inner.as_ref());
        instance.cursor.invalidate();
    } else {
        // `save` wrote the parameter snapshot because the plugin has no state
        // of its own. The plane takes it, and the engine pulls it before the
        // next block.
        let Some(values) = decode_params(instance.inner.info(), &bytes) else { return false };
        if values.len() != instance.plane.len() || values.iter().any(|v| !v.is_finite()) {
            return false;
        }
        for (index, value) in values.iter().enumerate() {
            instance.plane.store(index, *value);
        }
    }
    notify_params_rescan(instance);
    true
}

static STATE: clap_plugin_state = clap_plugin_state {
    save: Some(state_save),
    load: Some(state_load),
};

fn clap_param_flags(param: &ParamInfo) -> u32 {
    let stepped = param.flags.contains(ParamFlags::STEPPED)
        || param.steps > 1
        || !param.choices.is_empty()
        || matches!(param.unit, openplugin_core::Unit::Toggle | openplugin_core::Unit::Choice);
    let mut flags = 0;
    if stepped {
        flags |= PARAM_IS_STEPPED;
    }
    if param.flags.contains(ParamFlags::HIDDEN) {
        flags |= PARAM_IS_HIDDEN;
    }
    if param.flags.contains(ParamFlags::READ_ONLY) {
        flags |= PARAM_IS_READONLY;
    }
    if param.flags.contains(ParamFlags::BYPASS) {
        flags |= PARAM_IS_BYPASS;
    }
    if param.flags.contains(ParamFlags::NOT_REALTIME) {
        flags |= PARAM_REQUIRES_PROCESS;
    }
    if param.flags.contains(ParamFlags::AUTOMATABLE)
        && !param.flags.contains(ParamFlags::READ_ONLY)
        && !param.flags.contains(ParamFlags::NOT_REALTIME)
    {
        flags |= PARAM_IS_AUTOMATABLE;
    }
    flags
}

fn sync_bypass(instance: &mut Instance) {
    let params = instance.inner.info().params;
    for (index, param) in params.iter().enumerate() {
        if !param.flags.contains(ParamFlags::BYPASS) {
            continue;
        }
        let value = instance.plane.load(index).unwrap_or(param.default);
        instance.inner.set_host_bypass(value >= 0.5);
    }
}

fn host_call(host: *const clap_host, id: &[u8]) -> *const c_void {
    if host.is_null() {
        return std::ptr::null();
    }
    unsafe {
        match (*host).get_extension {
            Some(get) => get(host, id.as_ptr() as *const c_char),
            None => std::ptr::null(),
        }
    }
}

fn notify_params_rescan(instance: &Instance) {
    let ext = host_call(instance.host, EXT_PARAMS) as *const clap_host_params;
    if ext.is_null() {
        return;
    }
    unsafe {
        if let Some(rescan) = (*ext).rescan {
            rescan(instance.host, CLAP_PARAM_RESCAN_VALUES);
        }
    }
}

fn request_restart(host: *const clap_host) {
    if host.is_null() {
        return;
    }
    unsafe {
        if let Some(restart) = (*host).request_restart {
            restart(host);
        }
    }
}

fn notify_latency(instance: &mut Instance) {
    let now = instance.inner.latency().min(u32::MAX as usize) as u32;
    if now == instance.reported_latency {
        return;
    }
    let ext = host_call(instance.host, EXT_LATENCY) as *const clap_host_latency;
    if !ext.is_null() {
        unsafe {
            if let Some(changed) = (*ext).changed {
                changed(instance.host);
            }
        }
    }
    instance.reported_latency = now;
}

fn watch_latency(instance: &mut Instance) {
    if !instance.activated {
        return;
    }
    let now = instance.inner.latency().min(u32::MAX as usize) as u32;
    if now != instance.reported_latency {
        request_restart(instance.host);
    }
}

fn watch_tail(instance: &mut Instance) {
    let now = instance.inner.tail().clap_samples();
    if now == instance.reported_tail {
        return;
    }
    instance.reported_tail = now;
    let ext = host_call(instance.host, EXT_TAIL) as *const clap_host_tail;
    if !ext.is_null() {
        unsafe {
            if let Some(changed) = (*ext).changed {
                changed(instance.host);
            }
        }
    }
}

fn apply_transport(instance: &mut Instance, transport: *const clap_event_transport, steady_time: i64) {
    if transport.is_null() {
        return;
    }
    let transport = unsafe { &*transport };
    let flags = transport.flags;
    let mut info = Transport::default();
    info.playing = flags & TRANSPORT_IS_PLAYING != 0;
    info.recording = flags & TRANSPORT_IS_RECORDING != 0;
    info.sample_pos = if steady_time >= 0 { steady_time } else { 0 };
    if flags & TRANSPORT_HAS_TEMPO != 0 && transport.tempo.is_finite() {
        info.tempo = transport.tempo;
    }
    if flags & TRANSPORT_HAS_SIGNATURE != 0 {
        info.numerator = u32::from(transport.tsig_num);
        info.denominator = u32::from(transport.tsig_denom);
    }
    let beat = (1i64 << 31) as f64;
    if flags & TRANSPORT_HAS_BEATS != 0 {
        info.quarter_notes = transport.song_pos_beats as f64 / beat;
        info.bar_start = transport.bar_start as f64 / beat;
        info.bar_number = i64::from(transport.bar_number);
    }
    if flags & TRANSPORT_HAS_SECONDS != 0 {
        info.seconds = transport.song_pos_seconds as f64 / beat;
    }
    instance.inner.set_transport(&info);
}

fn drain_midi(instance: &mut Instance, out: *const clap_output_events) {
    if out.is_null() {
        return;
    }
    let out = unsafe { &*out };
    let Some(push) = out.try_push else { return };
    while let Some((time, data)) = instance.inner.pull_midi() {
        let event = clap_event_midi {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_midi>() as u32,
                time,
                space_id: 0,
                type_: EVENT_MIDI,
                flags: 0,
            },
            port_index: 0,
            data,
        };
        if !unsafe { push(out, &event.header) } {
            break;
        }
    }
}

fn release_offline_momentary(instance: &mut Instance, offline: bool) {
    if !offline {
        return;
    }
    let params = instance.inner.info().params;
    for (index, param) in params.iter().enumerate() {
        if param.flags.contains(ParamFlags::MOMENTARY) {
            instance.plane.store(index, param.default);
        }
    }
    release_momentary(instance.inner.as_mut());
}

unsafe extern "C" fn tail_get(plugin: *const clap_plugin) -> u32 {
    instance(plugin)
        .and_then(|item| item.inner.tail().clap_samples())
        .unwrap_or(0)
}

static TAIL: clap_plugin_tail = clap_plugin_tail { get: Some(tail_get) };

unsafe extern "C" fn config_count(plugin: *const clap_plugin) -> u32 {
    instance(plugin)
        .map(|item| item.inner.supported_layouts().len() as u32)
        .unwrap_or(0)
}

unsafe extern "C" fn config_get(plugin: *const clap_plugin, index: u32, config: *mut clap_audio_ports_config) -> bool {
    let Some(instance) = instance(plugin) else { return false };
    let Some(layout) = instance.inner.supported_layouts().get(index as usize).copied() else {
        return false;
    };
    if config.is_null() {
        return false;
    }
    unsafe {
        std::ptr::write_bytes(config, 0, 1);
        let config = &mut *config;
        config.id = index;
        copy_cstr(&mut config.name, &layout.name());
        config.input_port_count = if layout.input == SpeakerLayout::Absent { 0 } else { 1 }
            + u32::from(layout.sidechain_channels() > 0);
        config.output_port_count = u32::from(layout.output != SpeakerLayout::Absent);
        config.has_main_input = layout.input != SpeakerLayout::Absent;
        config.main_input_channel_count = layout.input_channels() as u32;
        config.main_input_port_type = port_type_ptr(layout.input);
        config.has_main_output = layout.output != SpeakerLayout::Absent;
        config.main_output_channel_count = layout.output_channels() as u32;
        config.main_output_port_type = port_type_ptr(layout.output);
    }
    true
}

unsafe extern "C" fn config_select(plugin: *const clap_plugin, config_id: u32) -> bool {
    let Some(instance) = instance(plugin) else { return false };
    if instance.activated {
        return false;
    }
    let Some(layout) = instance.inner.supported_layouts().get(config_id as usize).copied() else {
        return false;
    };
    if !instance.inner.set_layout(layout) {
        return false;
    }
    instance.layout_index = config_id;
    true
}

static AUDIO_PORTS_CONFIG: clap_plugin_audio_ports_config = clap_plugin_audio_ports_config {
    count: Some(config_count),
    get: Some(config_get),
    select: Some(config_select),
};

unsafe extern "C" fn config_current(plugin: *const clap_plugin) -> u32 {
    instance(plugin).map(|item| item.layout_index).unwrap_or(CLAP_INVALID_ID)
}

unsafe extern "C" fn config_info_get(
    plugin: *const clap_plugin,
    config_id: u32,
    port_index: u32,
    is_input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    let Some(instance) = instance(plugin) else { return false };
    let Some(layout) = instance.inner.supported_layouts().get(config_id as usize).copied() else {
        return false;
    };
    if info.is_null() {
        return false;
    }
    let Some((id, name, speakers, main)) = layout_port(layout, is_input, port_index) else {
        return false;
    };
    unsafe {
        std::ptr::write_bytes(info, 0, 1);
        write_audio_port(&mut *info, id, name, speakers, main);
    }
    true
}

static AUDIO_PORTS_CONFIG_INFO: clap_plugin_audio_ports_config_info = clap_plugin_audio_ports_config_info {
    current_config: Some(config_current),
    get: Some(config_info_get),
};

fn mask_supported(instance: &Instance, channel_mask: u64) -> bool {
    instance.inner.supported_layouts().iter().any(|layout| {
        layout.input.surround_mask() == Some(channel_mask) || layout.output.surround_mask() == Some(channel_mask)
    })
}

unsafe extern "C" fn surround_supported(plugin: *const clap_plugin, channel_mask: u64) -> bool {
    instance(plugin).map(|item| mask_supported(item, channel_mask)).unwrap_or(false)
}

unsafe extern "C" fn surround_map(
    plugin: *const clap_plugin,
    is_input: bool,
    port_index: u32,
    channel_map: *mut u8,
    channel_map_capacity: u32,
) -> u32 {
    let Some(instance) = instance(plugin) else { return 0 };
    let Some((_, _, layout, _)) = layout_port(selected_layout(instance), is_input, port_index) else {
        return 0;
    };
    let Some(map) = layout.surround_map() else { return 0 };
    if channel_map.is_null() || (channel_map_capacity as usize) < map.len() {
        return 0;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(map.as_ptr(), channel_map, map.len());
    }
    map.len() as u32
}

static SURROUND: clap_plugin_surround = clap_plugin_surround {
    is_channel_mask_supported: Some(surround_supported),
    get_channel_map: Some(surround_map),
};

unsafe extern "C" fn ambisonic_supported(_plugin: *const clap_plugin, config: *const clap_ambisonic_config) -> bool {
    if config.is_null() {
        return false;
    }
    let config = unsafe { &*config };
    config.ordering == AMBISONIC_ORDERING_ACN && config.normalization == AMBISONIC_NORMALIZATION_SN3D
}

unsafe extern "C" fn ambisonic_get(
    plugin: *const clap_plugin,
    is_input: bool,
    port_index: u32,
    config: *mut clap_ambisonic_config,
) -> bool {
    let Some(instance) = instance(plugin) else { return false };
    let Some((_, _, layout, _)) = layout_port(selected_layout(instance), is_input, port_index) else {
        return false;
    };
    if layout.kind() != PortKind::Ambisonic || config.is_null() {
        return false;
    }
    unsafe {
        *config = clap_ambisonic_config {
            ordering: AMBISONIC_ORDERING_ACN,
            normalization: AMBISONIC_NORMALIZATION_SN3D,
        };
    }
    true
}

static AMBISONIC: clap_plugin_ambisonic = clap_plugin_ambisonic {
    is_config_supported: Some(ambisonic_supported),
    get_config: Some(ambisonic_get),
};

unsafe extern "C" fn render_hard(_plugin: *const clap_plugin) -> bool {
    false
}

unsafe extern "C" fn render_set(plugin: *const clap_plugin, mode: u32) -> bool {
    let Some(instance) = instance(plugin) else { return false };
    let offline = mode == RENDER_OFFLINE;
    instance.inner.set_offline(offline);
    release_offline_momentary(instance, offline);
    true
}

static RENDER: clap_plugin_render = clap_plugin_render {
    has_hard_realtime_requirement: Some(render_hard),
    set: Some(render_set),
};

unsafe extern "C" fn plugin_extension(plugin: *const clap_plugin, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    let id = unsafe { CStr::from_ptr(id) }.to_bytes_with_nul();
    if id == EXT_AUDIO_PORTS {
        return &AUDIO_PORTS as *const clap_plugin_audio_ports as *const c_void;
    }
    if id == EXT_NOTE_PORTS {
        return &NOTE_PORTS as *const clap_plugin_note_ports as *const c_void;
    }
    if id == EXT_LATENCY {
        return &LATENCY as *const clap_plugin_latency as *const c_void;
    }
    if id == EXT_PARAMS {
        return &PARAMS as *const clap_plugin_params as *const c_void;
    }
    if id == EXT_STATE {
        return &STATE as *const clap_plugin_state as *const c_void;
    }
    if id == EXT_RENDER {
        return &RENDER as *const clap_plugin_render as *const c_void;
    }
    if id == EXT_TAIL {
        let unknown = instance(plugin).map(|item| item.inner.tail() == Tail::Unknown).unwrap_or(true);
        if unknown {
            return std::ptr::null();
        }
        return &TAIL as *const clap_plugin_tail as *const c_void;
    }
    if id == EXT_AUDIO_PORTS_CONFIG {
        return &AUDIO_PORTS_CONFIG as *const clap_plugin_audio_ports_config as *const c_void;
    }
    if id == EXT_AUDIO_PORTS_CONFIG_INFO {
        return &AUDIO_PORTS_CONFIG_INFO as *const clap_plugin_audio_ports_config_info as *const c_void;
    }
    if id == EXT_SURROUND || id == EXT_SURROUND_COMPAT {
        return &SURROUND as *const clap_plugin_surround as *const c_void;
    }
    if id == EXT_AMBISONIC || id == EXT_AMBISONIC_COMPAT {
        return &AMBISONIC as *const clap_plugin_ambisonic as *const c_void;
    }
    #[cfg(target_os = "macos")]
    if id == EXT_GUI && editor_factory().is_some() {
        return &gui::GUI as *const clap_plugin_gui as *const c_void;
    }
    std::ptr::null()
}

unsafe extern "C" fn plugin_main_thread(_plugin: *const clap_plugin) {}

fn feature_labels(plugin: &dyn Plugin) -> Vec<&'static str> {
    let mut labels = Vec::with_capacity(4);
    labels.push(if plugin.info().is_instrument { "instrument" } else { "audio-effect" });
    let layouts = plugin.supported_layouts();
    let stereo = layouts.is_empty()
        || layouts.iter().any(|layout| {
            layout.input.kind() == PortKind::Stereo || layout.output.kind() == PortKind::Stereo
        });
    let surround = layouts.iter().any(|layout| {
        layout.input.kind() == PortKind::Surround || layout.output.kind() == PortKind::Surround
    });
    let ambisonic = layouts.iter().any(|layout| {
        layout.input.kind() == PortKind::Ambisonic || layout.output.kind() == PortKind::Ambisonic
    });
    if stereo || (!surround && !ambisonic) {
        labels.push("stereo");
    }
    if surround {
        labels.push("surround");
    }
    if ambisonic {
        labels.push("ambisonic");
    }
    labels
}

fn spawn(inner: Box<dyn Plugin>, host: *const clap_host) -> *const clap_plugin {
    let info = inner.info();
    let labels = feature_labels(inner.as_ref());
    let reported_latency = inner.latency().min(u32::MAX as usize) as u32;
    let reported_tail = inner.tail().clap_samples();
    let mut strings = vec![
        cstring(info.id),
        cstring(info.name),
        cstring(info.vendor),
        cstring(info.url),
        cstring(info.url),
        cstring(info.email),
        cstring(info.version),
        cstring(info.name),
    ];
    for label in labels {
        strings.push(cstring(label));
    }
    let plane = Arc::new(ParamPlane::new(info));
    plane.seed_from(inner.as_ref());
    let cursor = ParamCursor::new(plane.len());
    let editor_state = inner.editor_state();
    let instance = Box::new(Instance {
        plugin: clap_plugin {
            desc: std::ptr::null(),
            plugin_data: std::ptr::null_mut(),
            init: Some(plugin_init),
            destroy: Some(plugin_destroy),
            activate: Some(plugin_activate),
            deactivate: Some(plugin_deactivate),
            start_processing: Some(plugin_start),
            stop_processing: Some(plugin_stop),
            reset: Some(plugin_reset),
            process: Some(plugin_process),
            get_extension: Some(plugin_extension),
            on_main_thread: Some(plugin_main_thread),
        },
        desc: clap_plugin_descriptor {
            clap_version: CLAP_VERSION,
            id: std::ptr::null(),
            name: std::ptr::null(),
            vendor: std::ptr::null(),
            url: std::ptr::null(),
            manual_url: std::ptr::null(),
            support_url: std::ptr::null(),
            version: std::ptr::null(),
            description: std::ptr::null(),
            features: std::ptr::null(),
        },
        strings,
        feature_ptrs: Vec::new(),
        inner,
        plane,
        cursor,
        gestures: Arc::new(GestureQueue::new(GESTURE_CAPACITY)),
        editor_state,
        host_params: std::ptr::null(),
        #[cfg(target_os = "macos")]
        gui: None,
        meters: Seqlock::new(MeterFrame::SILENT),
        bus_meters: Seqlock::new(BusMeters::SILENT),
        sched: [Sched::empty(); MAX_SCHED],
        sched_count: 0,
        buffers: Vec::new(),
        sidechain: Vec::new(),
        sample_rate: 48_000.0,
        max_block: 0,
        host,
        reported_latency,
        reported_tail,
        layout_index: 0,
        activated: false,
    });
    let raw = Box::into_raw(instance);
    unsafe {
        let inst = &mut *raw;
        inst.plugin.plugin_data = raw as *mut c_void;
        inst.feature_ptrs = inst.strings[8..].iter().map(|text| text.as_ptr()).collect();
        inst.feature_ptrs.push(std::ptr::null());
        let s = |i: usize| inst.strings[i].as_ptr();
        inst.desc = clap_plugin_descriptor {
            clap_version: CLAP_VERSION,
            id: s(0),
            name: s(1),
            vendor: s(2),
            url: s(3),
            manual_url: s(4),
            support_url: s(5),
            version: s(6),
            description: s(7),
            features: inst.feature_ptrs.as_ptr(),
        };
        inst.plugin.desc = &inst.desc;
        &inst.plugin
    }
}

/// In-process CLAP instance. Dropping it calls the same `destroy` a host calls.
pub struct ClapAdapter<P: Plugin> {
    raw: *const clap_plugin,
    _plugin: PhantomData<P>,
}

impl<P: Plugin + 'static> ClapAdapter<P> {
    pub fn new(inner: P) -> Self {
        let raw = spawn(Box::new(inner), std::ptr::null());
        unsafe {
            if let Some(init) = (*raw).init {
                init(raw);
            }
        }
        Self { raw, _plugin: PhantomData }
    }

    pub fn as_plugin(&self) -> *const clap_plugin {
        self.raw
    }

    /// Last meter frame published by `process`.
    pub fn meters(&self) -> MeterFrame {
        instance(self.raw).map(|inst| inst.meters.read()).unwrap_or(MeterFrame::SILENT)
    }

    pub fn bus_meters(&self) -> BusMeters {
        instance(self.raw).map(|inst| inst.bus_meters.read()).unwrap_or(BusMeters::SILENT)
    }

    /// What the engine shares with its editor ([`Plugin::editor_state`]).
    pub fn editor_state(&self) -> Option<EditorState> {
        instance(self.raw).and_then(|inst| inst.editor_state.clone())
    }

    /// The plugin's parameter plane, as the host and the editor see it.
    pub fn plane(&self) -> Option<Arc<ParamPlane>> {
        instance(self.raw).map(|inst| inst.plane.clone())
    }

    /// The host side of an editor: gestures go where `clap.gui` would send
    /// them. For tests that drive an editor without a window.
    pub fn editor_host(&self) -> Option<openplugin_core::EditorHost> {
        instance(self.raw).map(|inst| editor_host_for(inst))
    }

    pub fn activate(&mut self, sample_rate: f64, min_frames: u32, max_frames: u32) -> bool {
        unsafe {
            (*self.raw).activate.map(|activate| activate(self.raw, sample_rate, min_frames, max_frames)).unwrap_or(false)
        }
    }
}

impl<P: Plugin> Drop for ClapAdapter<P> {
    fn drop(&mut self) {
        if self.raw.is_null() {
            return;
        }
        unsafe {
            if let Some(destroy) = (*self.raw).destroy {
                destroy(self.raw);
            }
        }
        self.raw = std::ptr::null();
    }
}

unsafe extern "C" fn factory_count(_factory: *const clap_plugin_factory) -> u32 {
    if creator().is_some() { 1 } else { 0 }
}

unsafe extern "C" fn factory_descriptor(_factory: *const clap_plugin_factory, index: u32) -> *const clap_plugin_descriptor {
    if index != 0 {
        return std::ptr::null();
    }
    publish().map(|published| &published.desc as *const clap_plugin_descriptor).unwrap_or(std::ptr::null())
}

unsafe extern "C" fn factory_create(_factory: *const clap_plugin_factory, host: *const clap_host, plugin_id: *const c_char) -> *const clap_plugin {
    let Some(create) = creator() else { return std::ptr::null() };
    let Some(published) = publish() else { return std::ptr::null() };
    if plugin_id.is_null() {
        return std::ptr::null();
    }
    let requested = unsafe { CStr::from_ptr(plugin_id) };
    let published_id = unsafe { CStr::from_ptr(published.desc.id) };
    if requested != published_id {
        return std::ptr::null();
    }
    spawn(create(), host)
}

static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(factory_count),
    get_plugin_descriptor: Some(factory_descriptor),
    create_plugin: Some(factory_create),
};

/// `clap_entry.get_factory`. Returns the plugin factory after [`set_creator`].
pub unsafe extern "C" fn entry_get_factory(factory_id: *const c_char) -> *const c_void {
    if factory_id.is_null() {
        return std::ptr::null();
    }
    let id = unsafe { CStr::from_ptr(factory_id) }.to_bytes_with_nul();
    if id == FACTORY_ID {
        return &FACTORY as *const clap_plugin_factory as *const c_void;
    }
    std::ptr::null()
}

pub unsafe extern "C" fn entry_deinit() {}

/// What an editor's gestures do in CLAP: store the value in the plane, queue
/// the gesture for the host, and ask the host to collect it.
fn editor_host_for(instance: &Instance) -> openplugin_core::EditorHost {
    #[derive(Clone, Copy)]
    struct Link(*const clap_host, *const clap_host_params);
    let link = Link(instance.host, instance.host_params);
    let plane = instance.plane.clone();
    let queue = instance.gestures.clone();
    openplugin_core::EditorHost::new(move |gesture| {
        let link = link;
        if let Gesture::Perform(id, value) = gesture {
            if !plane.store(id as usize, value) {
                return;
            }
        }
        queue.push(gesture);
        // `request_flush` is for any thread but the audio thread; this is the main thread.
        if let Some(params) = unsafe { link.1.as_ref() } {
            if let Some(flush) = params.request_flush {
                unsafe { flush(link.0) };
            }
        }
    })
}

#[cfg(target_os = "macos")]
mod gui {
    //! `clap.gui` on macOS: the editor in an `NSView`, embedded, fixed size.
    use super::*;
    use openplugin_core::{Editor, ValueMirror};
    use openplugin_dri::cocoa::EditorView;

    pub(super) struct Gui {
        view: EditorView,
    }

    fn is_cocoa(api: *const c_char, floating: bool) -> bool {
        !api.is_null() && !floating && unsafe { CStr::from_ptr(api) }.to_bytes_with_nul() == WINDOW_API_COCOA
    }

    unsafe extern "C" fn is_api_supported(_plugin: *const clap_plugin, api: *const c_char, floating: bool) -> bool {
        is_cocoa(api, floating)
    }

    unsafe extern "C" fn get_preferred_api(_plugin: *const clap_plugin, api: *mut *const c_char, floating: *mut bool) -> bool {
        if api.is_null() || floating.is_null() {
            return false;
        }
        unsafe {
            *api = WINDOW_API_COCOA.as_ptr() as *const c_char;
            *floating = false;
        }
        true
    }

    unsafe extern "C" fn create(plugin: *const clap_plugin, api: *const c_char, floating: bool) -> bool {
        if !is_cocoa(api, floating) {
            return false;
        }
        let Some(instance) = instance(plugin) else { return false };
        let Some(factory) = editor_factory() else { return false };
        instance.gui = None;
        let Some(mut editor) = factory(editor_host_for(instance)) else { return false };
        editor.bind(instance.editor_state.clone());
        let plane = instance.plane.clone();
        let mut mirror = ValueMirror::new(plane.len());
        // The current values before the first frame, then whatever changes.
        mirror.update(editor.as_mut(), |i| plane.load(i));
        let poll = Box::new(move |editor: &mut dyn Editor| mirror.update(editor, |i| plane.load(i)));
        match EditorView::new(editor, Some(poll)) {
            Some(view) => {
                instance.gui = Some(Gui { view });
                true
            }
            None => false,
        }
    }

    unsafe extern "C" fn destroy(plugin: *const clap_plugin) {
        if let Some(instance) = instance(plugin) {
            instance.gui = None;
        }
    }

    unsafe extern "C" fn set_scale(_plugin: *const clap_plugin, _scale: f64) -> bool {
        // Cocoa works in points; the view reads the backing scale itself.
        false
    }

    fn size(plugin: *const clap_plugin) -> Option<(u32, u32)> {
        instance(plugin).and_then(|inst| inst.gui.as_ref()).map(|gui| gui.view.size())
    }

    unsafe extern "C" fn get_size(plugin: *const clap_plugin, width: *mut u32, height: *mut u32) -> bool {
        let Some((w, h)) = size(plugin) else { return false };
        if width.is_null() || height.is_null() {
            return false;
        }
        unsafe {
            *width = w;
            *height = h;
        }
        true
    }

    unsafe extern "C" fn can_resize(_plugin: *const clap_plugin) -> bool {
        false
    }

    unsafe extern "C" fn get_resize_hints(_plugin: *const clap_plugin, _hints: *mut clap_gui_resize_hints) -> bool {
        false
    }

    unsafe extern "C" fn adjust_size(plugin: *const clap_plugin, width: *mut u32, height: *mut u32) -> bool {
        unsafe { get_size(plugin, width, height) }
    }

    unsafe extern "C" fn set_size(plugin: *const clap_plugin, width: u32, height: u32) -> bool {
        size(plugin) == Some((width, height))
    }

    unsafe extern "C" fn set_parent(plugin: *const clap_plugin, window: *const clap_window) -> bool {
        let Some(gui) = instance(plugin).and_then(|inst| inst.gui.as_ref()) else { return false };
        if window.is_null() {
            return false;
        }
        unsafe { gui.view.attach((*window).ptr) }
    }

    unsafe extern "C" fn set_transient(_plugin: *const clap_plugin, _window: *const clap_window) -> bool {
        false
    }

    unsafe extern "C" fn suggest_title(_plugin: *const clap_plugin, _title: *const c_char) {}

    unsafe extern "C" fn show(plugin: *const clap_plugin) -> bool {
        match instance(plugin).and_then(|inst| inst.gui.as_ref()) {
            Some(gui) => {
                gui.view.set_hidden(false);
                true
            }
            None => false,
        }
    }

    unsafe extern "C" fn hide(plugin: *const clap_plugin) -> bool {
        match instance(plugin).and_then(|inst| inst.gui.as_ref()) {
            Some(gui) => {
                gui.view.set_hidden(true);
                true
            }
            None => false,
        }
    }

    pub(super) static GUI: clap_plugin_gui = clap_plugin_gui {
        is_api_supported: Some(is_api_supported),
        get_preferred_api: Some(get_preferred_api),
        create: Some(create),
        destroy: Some(destroy),
        set_scale: Some(set_scale),
        get_size: Some(get_size),
        can_resize: Some(can_resize),
        get_resize_hints: Some(get_resize_hints),
        adjust_size: Some(adjust_size),
        set_size: Some(set_size),
        set_parent: Some(set_parent),
        set_transient: Some(set_transient),
        suggest_title: Some(suggest_title),
        show: Some(show),
        hide: Some(hide),
    };
}

/// Export `clap_entry` from a cdylib. `$plugin` must implement [`Plugin`] and [`Default`].
///
/// The host calls `clap_entry.init`, which registers the plugin, then `get_factory`.
/// `export_clap!(P, editor = make_editor)` also offers `clap.gui` on macOS,
/// with the editor `make_editor` ([`openplugin_core::EditorFactory`]) returns.
#[macro_export]
macro_rules! export_clap {
    ($plugin:ty, editor = $editor:path) => {
        fn __openplugin_clap_create() -> ::std::boxed::Box<dyn $crate::PluginForExport> {
            ::std::boxed::Box::new(<$plugin as ::std::default::Default>::default())
        }

        unsafe extern "C" fn __openplugin_clap_init(_path: *const ::std::ffi::c_char) -> bool {
            $crate::clap::set_creator(__openplugin_clap_create);
            $crate::clap::set_editor($editor);
            true
        }

        #[no_mangle]
        pub static clap_entry: $crate::clap_abi::clap_plugin_entry = $crate::clap_abi::clap_plugin_entry {
            clap_version: $crate::clap_abi::CLAP_VERSION,
            init: Some(__openplugin_clap_init),
            deinit: Some($crate::clap::entry_deinit),
            get_factory: Some($crate::clap::entry_get_factory),
        };
    };
    ($plugin:ty) => {
        fn __openplugin_clap_create() -> ::std::boxed::Box<dyn $crate::PluginForExport> {
            ::std::boxed::Box::new(<$plugin as ::std::default::Default>::default())
        }

        unsafe extern "C" fn __openplugin_clap_init(_path: *const ::std::ffi::c_char) -> bool {
            $crate::clap::set_creator(__openplugin_clap_create);
            true
        }

        #[no_mangle]
        pub static clap_entry: $crate::clap_abi::clap_plugin_entry = $crate::clap_abi::clap_plugin_entry {
            clap_version: $crate::clap_abi::CLAP_VERSION,
            init: Some(__openplugin_clap_init),
            deinit: Some($crate::clap::entry_deinit),
            get_factory: Some($crate::clap::entry_get_factory),
        };
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use openplugin_core::{Category, ParamInfo, PluginInfo, Taper, Unit};

    struct Gain {
        db: f32,
        linear: f32,
    }

    static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db).with_taper(Taper::Linear)];

    static INFO: PluginInfo = PluginInfo {
        id: "com.opengrid.test.gain",
        name: "Test Gain",
        vendor: "OpenGrid",
        url: "https://github.com/MistaMin/OpenPluginSDK",
        email: "contact@mistamin.com",
        version: "0.1.0-beta.1",
        category: Category::Utility,
        params: &PARAMS,
        is_instrument: false,
        has_editor: false,
    };

    impl Plugin for Gain {
        fn info(&self) -> &'static PluginInfo { &INFO }
        fn prepare(&mut self, _: f32, _: usize, _: usize) { self.linear = 10f32.powf(self.db / 20.0); }
        fn reset(&mut self) {}
        fn set_param(&mut self, id: &str, value: f32) -> bool {
            if id != "gain" { return false; }
            self.db = PARAMS[0].clamp(value);
            self.linear = 10f32.powf(self.db / 20.0);
            true
        }
        fn param(&self, id: &str) -> Option<f32> { (id == "gain").then_some(self.db) }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            for channel in io {
                for sample in &mut channel[..frames] {
                    *sample *= self.linear;
                }
            }
        }
        fn save_state(&mut self) -> Option<Vec<u8>> { Some(self.db.to_le_bytes().to_vec()) }
        fn load_state(&mut self, data: &[u8]) -> bool {
            let Ok(bytes) = <[u8; 4]>::try_from(data) else { return false };
            self.set_param("gain", f32::from_le_bytes(bytes))
        }
    }

    struct EventList {
        events: Vec<clap_event_param_value>,
    }

    unsafe extern "C" fn event_size(list: *const clap_input_events) -> u32 {
        unsafe { &*((*list).ctx as *const EventList) }.events.len() as u32
    }

    unsafe extern "C" fn event_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
        let list = unsafe { &*((*list).ctx as *const EventList) };
        list.events.get(index as usize).map(|event| &event.header as *const clap_event_header).unwrap_or(std::ptr::null())
    }

    #[test]
    fn host_creates_plugin_and_processes_audio() {
        set_creator(|| Box::new(Gain { db: 0.0, linear: 1.0 }));
        let factory = unsafe { entry_get_factory(c"clap.plugin-factory".as_ptr()) } as *const clap_plugin_factory;
        assert!(!factory.is_null());
        unsafe {
            assert_eq!((*factory).get_plugin_count.unwrap()(factory), 1);
            let desc = (*factory).get_plugin_descriptor.unwrap()(factory, 0);
            assert!(!desc.is_null());
            assert_eq!(CStr::from_ptr((*desc).id), c"com.opengrid.test.gain");
            assert!((*factory).create_plugin.unwrap()(factory, std::ptr::null(), c"nope".as_ptr()).is_null());
            let plugin = (*factory).create_plugin.unwrap()(factory, std::ptr::null(), (*desc).id);
            assert!(!plugin.is_null());
            assert!((*plugin).init.unwrap()(plugin));
            assert!((*plugin).activate.unwrap()(plugin, 48_000.0, 1, 64));

            let params = (*plugin).get_extension.unwrap()(plugin, c"clap.params".as_ptr()) as *const clap_plugin_params;
            assert!(!params.is_null());
            assert_eq!((*params).count.unwrap()(plugin), 1);
            let mut info = std::mem::zeroed::<clap_param_info>();
            assert!((*params).get_info.unwrap()(plugin, 0, &mut info));
            assert!((info.default_value - 0.0).abs() < 1e-6);
            assert_eq!(info.flags & PARAM_IS_AUTOMATABLE, PARAM_IS_AUTOMATABLE);

            let event = clap_event_param_value {
                header: clap_event_header { size: std::mem::size_of::<clap_event_param_value>() as u32, time: 0, space_id: 0, type_: EVENT_PARAM_VALUE, flags: 0 },
                param_id: 0,
                cookie: std::ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value: 6.0,
            };
            let list = EventList { events: vec![event] };
            let input_events = clap_input_events {
                ctx: &list as *const EventList as *mut c_void,
                size: Some(event_size),
                get: Some(event_get),
            };

            let mut input = [0.5f32; 8];
            let mut output = [0.0f32; 8];
            let mut input_ptr = input.as_mut_ptr();
            let mut output_ptr = output.as_mut_ptr();
            let inputs = clap_audio_buffer {
                data32: &mut input_ptr,
                data64: std::ptr::null_mut(),
                channel_count: 1,
                latency: 0,
                constant_mask: 0,
            };
            let mut outputs = clap_audio_buffer {
                data32: &mut output_ptr,
                data64: std::ptr::null_mut(),
                channel_count: 1,
                latency: 0,
                constant_mask: 0,
            };
            let block = clap_process {
                steady_time: 0,
                frames_count: 8,
                transport: std::ptr::null(),
                audio_inputs: &inputs,
                audio_outputs: &mut outputs,
                audio_inputs_count: 1,
                audio_outputs_count: 1,
                in_events: &input_events,
                out_events: std::ptr::null(),
            };
            assert_eq!((*plugin).process.unwrap()(plugin, &block), PROCESS_CONTINUE);
            let expected = 0.5 * 10f32.powf(6.0 / 20.0);
            assert!((output[0] - expected).abs() < 1e-4, "output {} expected {}", output[0], expected);
            assert!((output[7] - expected).abs() < 1e-4);

            let mut stepped = input;
            stepped.fill(0.5);
            let mut stepped_out = [0.0f32; 8];
            let early = clap_event_param_value {
                header: clap_event_header { size: std::mem::size_of::<clap_event_param_value>() as u32, time: 0, space_id: 0, type_: EVENT_PARAM_VALUE, flags: 0 },
                param_id: 0,
                cookie: std::ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value: 0.0,
            };
            let late = clap_event_param_value {
                header: clap_event_header { size: std::mem::size_of::<clap_event_param_value>() as u32, time: 4, space_id: 0, type_: EVENT_PARAM_VALUE, flags: 0 },
                param_id: 0,
                cookie: std::ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value: 6.0,
            };
            let stepped_list = EventList { events: vec![late, early] };
            let stepped_events = clap_input_events {
                ctx: &stepped_list as *const EventList as *mut c_void,
                size: Some(event_size),
                get: Some(event_get),
            };
            let mut stepped_in = stepped.as_mut_ptr();
            let mut stepped_out_ptr = stepped_out.as_mut_ptr();
            let stepped_inputs = clap_audio_buffer {
                data32: &mut stepped_in,
                data64: std::ptr::null_mut(),
                channel_count: 1,
                latency: 0,
                constant_mask: 0,
            };
            let mut stepped_outputs = clap_audio_buffer {
                data32: &mut stepped_out_ptr,
                data64: std::ptr::null_mut(),
                channel_count: 1,
                latency: 0,
                constant_mask: 0,
            };
            let stepped_block = clap_process {
                steady_time: 8,
                frames_count: 8,
                transport: std::ptr::null(),
                audio_inputs: &stepped_inputs,
                audio_outputs: &mut stepped_outputs,
                audio_inputs_count: 1,
                audio_outputs_count: 1,
                in_events: &stepped_events,
                out_events: std::ptr::null(),
            };
            assert_eq!((*plugin).process.unwrap()(plugin, &stepped_block), PROCESS_CONTINUE);
            assert!((stepped_out[0] - 0.5).abs() < 1e-4, "pre-event {}", stepped_out[0]);
            assert!((stepped_out[3] - 0.5).abs() < 1e-4, "pre-event {}", stepped_out[3]);
            assert!((stepped_out[4] - expected).abs() < 1e-4, "post-event {}", stepped_out[4]);
            let mut read_back = 0.0;
            assert!((*params).get_value.unwrap()(plugin, 0, &mut read_back));
            assert!((read_back - 6.0).abs() < 1e-6);

            let ports = (*plugin).get_extension.unwrap()(plugin, c"clap.audio-ports".as_ptr());
            assert!(!ports.is_null());
            let latency = (*plugin).get_extension.unwrap()(plugin, c"clap.latency".as_ptr()) as *const clap_plugin_latency;
            assert_eq!((*latency).get.unwrap()(plugin), 0);
            let tail = (*plugin).get_extension.unwrap()(plugin, c"clap.tail".as_ptr()) as *const clap_plugin_tail;
            assert!(!tail.is_null());
            assert_eq!((*tail).get.unwrap()(plugin), 0);

            (*plugin).destroy.unwrap()(plugin);
        }
    }

    static WIDE_LAYOUTS: &[openplugin_core::IoConfig] = &[
        openplugin_core::IoConfig::STEREO,
        openplugin_core::IoConfig {
            input: openplugin_core::SpeakerLayout::Surround51,
            output: openplugin_core::SpeakerLayout::Surround51,
            sidechain: Some(openplugin_core::SpeakerLayout::Mono),
        },
    ];

    struct Wide;

    impl Plugin for Wide {
        fn info(&self) -> &'static PluginInfo { &INFO }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, _: &str, _: f32) -> bool { false }
        fn param(&self, _: &str) -> Option<f32> { None }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
        fn supported_layouts(&self) -> &'static [openplugin_core::IoConfig] { WIDE_LAYOUTS }
        fn accepts_sample_rate(&self, sample_rate: f32) -> bool {
            sample_rate == 48_000.0 || sample_rate == 96_000.0
        }
    }

    #[test]
    fn surround_layout_adds_a_sidechain_and_a_channel_map() {
        let adapter = ClapAdapter::new(Wide);
        let plugin = adapter.as_plugin();
        unsafe {
            let ports = (*plugin).get_extension.unwrap()(plugin, c"clap.audio-ports".as_ptr()) as *const clap_plugin_audio_ports;
            assert_eq!((*ports).count.unwrap()(plugin, true), 1);
            let configs = (*plugin).get_extension.unwrap()(plugin, c"clap.audio-ports-config".as_ptr()) as *const clap_plugin_audio_ports_config;
            assert_eq!((*configs).count.unwrap()(plugin), 2);
            assert!((*configs).select.unwrap()(plugin, 1));
            assert_eq!((*ports).count.unwrap()(plugin, true), 2);
            let mut info = std::mem::zeroed::<clap_audio_port_info>();
            assert!((*ports).get.unwrap()(plugin, 0, false, &mut info));
            assert_eq!(info.channel_count, 6);
            assert_eq!(CStr::from_ptr(info.port_type), c"surround");
            let surround = (*plugin).get_extension.unwrap()(plugin, c"clap.surround/4".as_ptr()) as *const clap_plugin_surround;
            let mut map = [0u8; 8];
            let stored = (*surround).get_channel_map.unwrap()(plugin, false, 0, map.as_mut_ptr(), map.len() as u32);
            assert_eq!(stored, 6);
            assert_eq!(map[3], 3);
            assert!(!(*plugin).activate.unwrap()(plugin, 11_025.0, 1, 64));
            assert!((*plugin).activate.unwrap()(plugin, 48_000.0, 1, 32));
            assert!(!(*configs).select.unwrap()(plugin, 0));
        }
    }

    struct Captured {
        events: Vec<(u16, u32, f64)>,
    }

    unsafe extern "C" fn capture(list: *const clap_output_events, event: *const clap_event_header) -> bool {
        let captured = unsafe { &mut *((*list).ctx as *mut Captured) };
        let header = unsafe { &*event };
        let entry = match header.type_ {
            EVENT_PARAM_VALUE => {
                let e = unsafe { &*(event as *const clap_event_param_value) };
                (header.type_, e.param_id, e.value)
            }
            _ => {
                let e = unsafe { &*(event as *const clap_event_param_gesture) };
                (header.type_, e.param_id, 0.0)
            }
        };
        captured.events.push(entry);
        true
    }

    #[test]
    fn editor_gestures_reach_the_plane_and_the_host_in_order() {
        let adapter = ClapAdapter::new(Gain { db: 0.0, linear: 1.0 });
        let host = adapter.editor_host().expect("editor host");
        host.begin(0);
        host.perform(0, 6.0);
        host.perform(0, 99.0); // clamped to the range
        host.end(0);
        host.set(7, 1.0); // no such parameter: dropped
        let plane = adapter.plane().expect("plane");
        assert_eq!(plane.load(0), Some(24.0));

        let mut captured = Captured { events: Vec::new() };
        let out = clap_output_events { ctx: &mut captured as *mut Captured as *mut c_void, try_push: Some(capture) };
        let plugin = adapter.as_plugin();
        unsafe {
            let params = plugin_extension(plugin, EXT_PARAMS.as_ptr() as *const c_char) as *const clap_plugin_params;
            (*params).flush.unwrap()(plugin, std::ptr::null(), &out);
        }
        assert_eq!(
            captured.events,
            vec![
                (EVENT_PARAM_GESTURE_BEGIN, 0, 0.0),
                (EVENT_PARAM_VALUE, 0, 6.0),
                (EVENT_PARAM_VALUE, 0, 24.0),
                (EVENT_PARAM_GESTURE_END, 0, 0.0),
            ]
        );
        // Collected once only.
        captured.events.clear();
        unsafe {
            let params = plugin_extension(plugin, EXT_PARAMS.as_ptr() as *const c_char) as *const clap_plugin_params;
            (*params).flush.unwrap()(plugin, std::ptr::null(), &out);
        }
        assert!(captured.events.is_empty());
    }

    struct NoState {
        db: f32,
    }

    impl Plugin for NoState {
        fn info(&self) -> &'static PluginInfo { &INFO }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, id: &str, value: f32) -> bool {
            if id != "gain" { return false; }
            self.db = PARAMS[0].clamp(value);
            true
        }
        fn param(&self, id: &str) -> Option<f32> { (id == "gain").then_some(self.db) }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
    }

    struct Bytes {
        data: Vec<u8>,
        read: usize,
    }

    unsafe extern "C" fn write_bytes(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
        let bytes = unsafe { &mut *((*stream).ctx as *mut Bytes) };
        bytes.data.extend_from_slice(unsafe { std::slice::from_raw_parts(buffer as *const u8, size as usize) });
        size as i64
    }

    unsafe extern "C" fn read_bytes(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
        let bytes = unsafe { &mut *((*stream).ctx as *mut Bytes) };
        let n = (bytes.data.len() - bytes.read).min(size as usize);
        unsafe { std::ptr::copy_nonoverlapping(bytes.data[bytes.read..].as_ptr(), buffer as *mut u8, n) };
        bytes.read += n;
        n as i64
    }

    #[test]
    fn state_without_a_plugin_chunk_restores_the_parameters() {
        let first = ClapAdapter::new(NoState { db: 0.0 });
        first.plane().unwrap().store(0, -7.5);
        let mut bytes = Bytes { data: Vec::new(), read: 0 };
        unsafe {
            let state = plugin_extension(first.as_plugin(), EXT_STATE.as_ptr() as *const c_char) as *const clap_plugin_state;
            let out = clap_ostream { ctx: &mut bytes as *mut Bytes as *mut c_void, write: Some(write_bytes) };
            assert!((*state).save.unwrap()(first.as_plugin(), &out));
            let second = ClapAdapter::new(NoState { db: 0.0 });
            let input = clap_istream { ctx: &mut bytes as *mut Bytes as *mut c_void, read: Some(read_bytes) };
            assert!((*state).load.unwrap()(second.as_plugin(), &input));
            assert_eq!(second.plane().unwrap().load(0), Some(-7.5));
            let mut junk = Bytes { data: vec![1, 2, 3], read: 0 };
            let input = clap_istream { ctx: &mut junk as *mut Bytes as *mut c_void, read: Some(read_bytes) };
            assert!(!(*state).load.unwrap()(second.as_plugin(), &input), "junk is refused");
            assert_eq!(second.plane().unwrap().load(0), Some(-7.5), "and changes nothing");
        }
    }

    #[test]
    fn text_to_value_reads_units() {
        let adapter = ClapAdapter::new(Gain { db: 0.0, linear: 1.0 });
        let plugin = adapter.as_plugin();
        unsafe {
            let params = plugin_extension(plugin, EXT_PARAMS.as_ptr() as *const c_char) as *const clap_plugin_params;
            let mut value = 0.0f64;
            assert!((*params).text_to_value.unwrap()(plugin, 0, c"+6.0 dB".as_ptr(), &mut value));
            assert_eq!(value, 6.0);
            assert!(!(*params).text_to_value.unwrap()(plugin, 0, c"loud".as_ptr(), &mut value));
            assert!(!(*params).text_to_value.unwrap()(plugin, 3, c"1".as_ptr(), &mut value));
        }
    }

    #[test]
    fn no_gui_without_an_editor_factory() {
        // This test binary never installs an editor.
        let adapter = ClapAdapter::new(Gain { db: 0.0, linear: 1.0 });
        let ext = unsafe { plugin_extension(adapter.as_plugin(), EXT_GUI.as_ptr() as *const c_char) };
        assert!(ext.is_null());
    }

    /// Sizes measured with clang on this aarch64 macOS host against CLAP 1.2.
    #[test]
    fn clap_structs_match_the_host_abi() {
        assert_eq!(std::mem::size_of::<clap_plugin_gui>(), 15 * std::mem::size_of::<usize>());
        assert_eq!(std::mem::size_of::<clap_event_param_gesture>(), 20);
        assert_eq!(std::mem::size_of::<clap_window>(), 2 * std::mem::size_of::<usize>());
        assert_eq!(std::mem::size_of::<clap_gui_resize_hints>(), 12);
        use crate::clap_abi::{clap_audio_ports_config, clap_event_transport};
        assert_eq!(std::mem::size_of::<clap_event_transport>(), 104);
        assert_eq!(std::mem::offset_of!(clap_event_transport, bar_number), 96);
        assert_eq!(std::mem::offset_of!(clap_event_transport, tsig_num), 100);
        assert_eq!(std::mem::size_of::<clap_audio_ports_config>(), 304);
        assert_eq!(std::mem::offset_of!(clap_audio_ports_config, has_main_input), 268);
        assert_eq!(std::mem::offset_of!(clap_audio_ports_config, main_input_channel_count), 272);
        assert_eq!(std::mem::offset_of!(clap_audio_ports_config, main_input_port_type), 280);
        assert_eq!(std::mem::offset_of!(clap_audio_ports_config, has_main_output), 288);
        assert_eq!(std::mem::offset_of!(clap_audio_ports_config, main_output_channel_count), 292);
        assert_eq!(std::mem::offset_of!(clap_audio_ports_config, main_output_port_type), 296);
    }
}
