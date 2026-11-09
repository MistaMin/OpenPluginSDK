//! CLAP target bridge.
//!
//! The lifecycle matches OpenGridder's from-scratch CLAP shell
//! (`OpenGridder-Plugins/native/clap/OpenGridderCLAP.mm`): `clap_entry`, one factory,
//! `init` / `activate` / `process`, then the audio-ports, note-ports, latency, params,
//! and state extensions. Parameter values are the plugin's real units. OpenGridder's
//! shell talks to the offload client and draws a Cocoa editor; this one calls
//! [`openplugin_core::Plugin`] and does not open a window.
//!
//! A host loads the `clap_entry` symbol from a cdylib that called [`export_clap!`].
//! [`ClapAdapter`] builds the same instance in-process so a test can drive it.

use std::ffi::{CStr, CString, c_char, c_void};
use std::marker::PhantomData;
use std::sync::Mutex;

use openplugin_core::{MeterFrame, ParamCursor, ParamPlane, Plugin, Seqlock};

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
        let kind = if info.is_instrument { "instrument" } else { "audio-effect" };
        let features_owned = vec![cstring(kind), cstring("stereo")];
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
    plane: ParamPlane,
    cursor: ParamCursor,
    meters: Seqlock<MeterFrame>,
    sched: [Sched; MAX_SCHED],
    sched_count: usize,
    buffers: Vec<Vec<f32>>,
    sample_rate: f32,
    max_block: usize,
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
                push_sched(instance, Sched {
                    time: header.time,
                    kind: KIND_NOTE_ON,
                    note: event.key as u8,
                    velocity,
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
            KIND_NOTE_ON => instance.inner.note_on(0, event.note, event.velocity),
            KIND_NOTE_OFF => instance.inner.note_off(0, event.note),
            KIND_MIDI => instance.inner.midi(0, event.midi),
            _ => {}
        }
    }
    if dirty {
        instance.cursor.pull(&instance.plane, instance.inner.as_mut());
    }
}

fn process_span(
    instance: &mut Instance,
    input: &clap_audio_buffer,
    output: &mut clap_audio_buffer,
    in_channels: usize,
    out_channels: usize,
    start: usize,
    end: usize,
) {
    let n = end.saturating_sub(start);
    if n == 0 || instance.buffers.len() < 2 || instance.buffers[0].len() < n {
        return;
    }
    for channel in 0..2 {
        let src_channel = if channel < in_channels { channel } else { 0 };
        let src = unsafe { *input.data32.add(src_channel) };
        let dst = &mut instance.buffers[channel][..n];
        if src.is_null() {
            dst.fill(0.0);
        } else {
            let samples = unsafe { std::slice::from_raw_parts(src.add(start), n) };
            dst.copy_from_slice(samples);
        }
    }
    instance.inner.process(&mut instance.buffers, n);
    for channel in 0..out_channels {
        let dst = unsafe { *output.data32.add(channel) };
        if dst.is_null() {
            continue;
        }
        let out = unsafe { std::slice::from_raw_parts_mut(dst.add(start), n) };
        out.copy_from_slice(&instance.buffers[channel][..n]);
    }
}

fn render_block(
    instance: &mut Instance,
    input: &clap_audio_buffer,
    output: &mut clap_audio_buffer,
    in_channels: usize,
    out_channels: usize,
    frames: usize,
) {
    let max_block = instance.max_block.max(1);
    let count = instance.sched_count;
    let mut done = 0;
    let mut event = 0;
    while done < frames {
        let chunk_end = (done + max_block).min(frames);
        instance.cursor.pull(&instance.plane, instance.inner.as_mut());
        let mut cursor = done;
        while event < count {
            let raw = instance.sched[event].time as usize;
            if raw > chunk_end {
                break;
            }
            let at = raw.clamp(done, chunk_end);
            if at > cursor {
                process_span(instance, input, output, in_channels, out_channels, cursor, at);
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
            process_span(instance, input, output, in_channels, out_channels, cursor, chunk_end);
        }
        done = chunk_end;
    }
    instance.meters.publish(instance.inner.meters());
}

unsafe extern "C" fn plugin_init(plugin: *const clap_plugin) -> bool {
    instance(plugin).is_some()
}

unsafe extern "C" fn plugin_destroy(plugin: *const clap_plugin) {
    if plugin.is_null() {
        return;
    }
    let data = unsafe { (*plugin).plugin_data };
    if !data.is_null() {
        drop(unsafe { Box::from_raw(data as *mut Instance) });
    }
}

unsafe extern "C" fn plugin_activate(plugin: *const clap_plugin, sample_rate: f64, _min_frames: u32, max_frames: u32) -> bool {
    let Some(instance) = instance(plugin) else { return false };
    let rate = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate as f32 } else { 48_000.0 };
    let max_block = max_frames.max(1) as usize;
    instance.sample_rate = rate;
    instance.max_block = max_block;
    instance.buffers = vec![vec![0.0; max_block]; 2];
    instance.inner.prepare(rate, max_block, 2);
    instance.cursor.invalidate();
    instance.cursor.pull(&instance.plane, instance.inner.as_mut());
    true
}

unsafe extern "C" fn plugin_deactivate(_plugin: *const clap_plugin) {}

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
    if process.frames_count == 0 || process.audio_inputs_count == 0 || process.audio_outputs_count == 0 {
        if instance.sched_count > 0 {
            let count = instance.sched_count;
            apply_group(instance, 0, count);
        }
        return PROCESS_CONTINUE;
    }
    if process.audio_inputs.is_null() || process.audio_outputs.is_null() {
        return PROCESS_ERROR;
    }
    let input = unsafe { &*process.audio_inputs };
    let output = unsafe { &mut *process.audio_outputs };
    if input.data32.is_null() || output.data32.is_null() {
        return PROCESS_ERROR;
    }
    let frames = process.frames_count as usize;
    let in_channels = input.channel_count.min(2) as usize;
    let out_channels = output.channel_count.min(2) as usize;
    if in_channels == 0 || out_channels == 0 {
        return PROCESS_CONTINUE;
    }
    if instance.buffers.len() < 2 {
        return PROCESS_ERROR;
    }
    render_block(instance, input, output, in_channels, out_channels, frames);
    PROCESS_CONTINUE
}

unsafe extern "C" fn audio_ports_count(_plugin: *const clap_plugin, _is_input: bool) -> u32 {
    1
}

unsafe extern "C" fn audio_ports_get(_plugin: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_audio_port_info) -> bool {
    if index != 0 || info.is_null() {
        return false;
    }
    unsafe {
        std::ptr::write_bytes(info, 0, 1);
        let info = &mut *info;
        info.id = 0;
        copy_cstr(&mut info.name, if is_input { "Audio In" } else { "Audio Out" });
        info.flags = AUDIO_PORT_IS_MAIN;
        info.channel_count = 2;
        info.port_type = c"stereo".as_ptr();
        info.in_place_pair = 0;
    }
    true
}

static AUDIO_PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(audio_ports_count),
    get: Some(audio_ports_get),
};

unsafe extern "C" fn note_ports_count(_plugin: *const clap_plugin, is_input: bool) -> u32 {
    if is_input { 1 } else { 0 }
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
        info.flags = PARAM_IS_AUTOMATABLE;
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

unsafe extern "C" fn params_text_to_value(_plugin: *const clap_plugin, _param_id: u32, text: *const c_char, out_value: *mut f64) -> bool {
    if text.is_null() || out_value.is_null() {
        return false;
    }
    let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    let Ok(value) = text.trim().parse::<f64>() else { return false };
    unsafe { *out_value = value };
    true
}

unsafe extern "C" fn params_flush(plugin: *const clap_plugin, input: *const clap_input_events, _output: *const clap_output_events) {
    if let Some(instance) = instance(plugin) {
        collect_events(instance, input);
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
    let Some(bytes) = instance.inner.save_state() else { return false };
    let wrote = unsafe { write(stream, bytes.as_ptr() as *const c_void, bytes.len() as u64) };
    wrote == bytes.len() as i64
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
    !bytes.is_empty() && instance.inner.load_state(&bytes)
}

static STATE: clap_plugin_state = clap_plugin_state {
    save: Some(state_save),
    load: Some(state_load),
};

unsafe extern "C" fn plugin_extension(_plugin: *const clap_plugin, id: *const c_char) -> *const c_void {
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
    std::ptr::null()
}

unsafe extern "C" fn plugin_main_thread(_plugin: *const clap_plugin) {}

fn spawn(inner: Box<dyn Plugin>) -> *const clap_plugin {
    let info = inner.info();
    let kind = if info.is_instrument { "instrument" } else { "audio-effect" };
    let strings = vec![
        cstring(info.id),
        cstring(info.name),
        cstring(info.vendor),
        cstring(info.url),
        cstring(info.url),
        cstring(info.email),
        cstring(info.version),
        cstring(info.name),
        cstring(kind),
        cstring("stereo"),
    ];
    let plane = ParamPlane::new(info);
    plane.seed_from(inner.as_ref());
    let cursor = ParamCursor::new(plane.len());
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
        meters: Seqlock::new(MeterFrame::SILENT),
        sched: [Sched::empty(); MAX_SCHED],
        sched_count: 0,
        buffers: Vec::new(),
        sample_rate: 48_000.0,
        max_block: 0,
    });
    let raw = Box::into_raw(instance);
    unsafe {
        let inst = &mut *raw;
        inst.plugin.plugin_data = raw as *mut c_void;
        inst.feature_ptrs = vec![inst.strings[8].as_ptr(), inst.strings[9].as_ptr(), std::ptr::null()];
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
        let raw = spawn(Box::new(inner));
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

unsafe extern "C" fn factory_create(_factory: *const clap_plugin_factory, _host: *const clap_host, plugin_id: *const c_char) -> *const clap_plugin {
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
    spawn(create())
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

/// Export `clap_entry` from a cdylib. `$plugin` must implement [`Plugin`] and [`Default`].
///
/// The host calls `clap_entry.init`, which registers the plugin, then `get_factory`.
#[macro_export]
macro_rules! export_clap {
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
        id: "com.mistamin.test.gain",
        name: "Test Gain",
        vendor: "MistaMin",
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
            assert_eq!(CStr::from_ptr((*desc).id), c"com.mistamin.test.gain");
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

            (*plugin).destroy.unwrap()(plugin);
        }
    }
}
