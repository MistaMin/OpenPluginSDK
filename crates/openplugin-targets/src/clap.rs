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
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{fence, AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant as Clock};

use openplugin_core::DenormalGuard;
use openplugin_core::{
    decode_params, encode_params, mpe, release_momentary, sample_rate_supported, BusMeters, EditorFactory, EditorState, Expression,
    ExpressionSet, Gesture, GestureQueue, HostNotifier, IoConfig, MpeReceiver, NoteRef, MeterFrame, ParamCursor, ParamFlags, ParamInfo, ParamPlane, Plugin, PluginInfo, PortKind,
    Seqlock, SpeakerLayout, Tail, TrackInfo, Transport, MAX_AUDIO_CHANNELS,
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

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
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
const KIND_EXPRESSION: u8 = 4;
const KIND_MOD: u8 = 5;

#[derive(Clone, Copy)]
struct Sched {
    time: u32,
    kind: u8,
    param: u32,
    value: f32,
    velocity: u8,
    /// Expression id (`KIND_EXPRESSION`).
    expression: u8,
    midi: [u8; 3],
    /// The note a note, expression or modulation event is for.
    target: NoteRef,
}

impl Sched {
    const fn empty() -> Self {
        Self {
            time: 0,
            kind: 0,
            param: 0,
            value: 0.0,
            velocity: 0,
            expression: 0,
            midi: [0; 3],
            target: NoteRef::ALL,
        }
    }
}

/// A CLAP note id: 0 or more, else -1 (none).
fn note_id(id: i32) -> i32 {
    if id < 0 { -1 } else { id }
}

/// Whether the event's header claims at least the size of its struct, so a
/// short event is never read past its end.
fn sized<T>(header: &clap_event_header) -> bool {
    header.size as usize >= std::mem::size_of::<T>()
}

struct Instance {
    plugin: clap_plugin,
    desc: clap_plugin_descriptor,
    strings: Vec<CString>,
    feature_ptrs: Vec<*const c_char>,
    inner: Box<dyn Plugin>,
    plane: Arc<ParamPlane>,
    /// The audio thread's consistent copy of `plane`; the cursor pulls from it.
    snapshot: ParamPlane,
    /// Odd while a state load is writing the plane (see `pull_params`).
    plane_epoch: AtomicU32,
    /// Who is inside the plugin object: `GATE_AUDIO`, `GATE_STATE` (see `enter_state`).
    gate: AtomicU32,
    /// Serialises state calls against each other (the host calls them on the main thread).
    state_lock: Mutex<()>,
    /// The plugin may have a chunk: `!Plugin::state_is_parameters_only()`, read once.
    /// When false the state is the parameters alone and state calls never touch `inner`.
    has_chunk: bool,
    info: &'static PluginInfo,
    cursor: ParamCursor,
    /// Editor gestures waiting to be reported to the host.
    gestures: Arc<GestureQueue>,
    /// What the engine shares with its editor, taken once at creation.
    editor_state: Option<EditorState>,
    host_params: *const clap_host_params,
    /// What the host last said about the track (main thread), kept for an editor made later.
    track_info: Option<TrackInfo>,
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    gui: Option<gui::Gui>,
    meters: Seqlock<MeterFrame>,
    bus_meters: Seqlock<BusMeters>,
    sched: [Sched; MAX_SCHED],
    sched_count: usize,
    /// [`Plugin::note_expressions`], read once at creation.
    expressions: ExpressionSet,
    /// The MPE receiver for a plugin that asked for MPE ([`Plugin::mpe_input`]).
    mpe: Option<MpeReceiver>,
    buffers: Vec<Vec<f32>>,
    sidechain: Vec<Vec<f32>>,
    sample_rate: f32,
    max_block: usize,
    host: *const clap_host,
    /// The latency the host was last told (`latency.changed` in `activate`).
    reported_latency: u32,
    /// The plugin's [`HostNotifier`] (a clone of the one it was given).
    notifier: HostNotifier,
    /// `request_restart` was called for a latency change; cleared by `activate`.
    restart_requested: AtomicBool,
    /// `request_callback` was called for pending changes; cleared by `on_main_thread`.
    callback_requested: AtomicBool,
    reported_tail: Option<u32>,
    layout_index: u32,
    activated: bool,
    /// Set when a callback into the plugin panicked; see [`guarded`]. Cleared by `activate`.
    poisoned: AtomicBool,
    /// [`Plugin::flush_denormals`], read once at creation: `process`, `params.flush` and
    /// `reset` hold a [`DenormalGuard`] while they are inside the plugin.
    flush_denormals: bool,
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

/// Runs one `extern "C"` entry so a panic never unwinds into the host.
///
/// `catch_unwind` costs nothing and allocates nothing unless a panic happens. On a panic the
/// call returns `fail` (the format's failure value). When `plugin` is not null (only
/// `process`, `params.flush`, `reset` and `activate` pass it) the instance is also marked
/// poisoned: `process` then outputs silence and reports an error until the host activates the
/// plugin again. State, port and query calls fail without poisoning. The default
/// panic hook still prints the message to stderr; install your own hook if that matters on the
/// audio thread.
fn guarded<R>(plugin: *const clap_plugin, fail: R, body: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(_) => {
            poison(plugin);
            fail
        }
    }
}

fn poison(plugin: *const clap_plugin) {
    if plugin.is_null() {
        return;
    }
    let data = unsafe { (*plugin).plugin_data } as *mut Instance;
    if !data.is_null() {
        unsafe { &*std::ptr::addr_of!((*data).poisoned) }.store(true, Ordering::Release);
    }
}

fn is_poisoned(plugin: *const clap_plugin) -> bool {
    let data = unsafe { (*plugin).plugin_data } as *mut Instance;
    !data.is_null() && unsafe { &*std::ptr::addr_of!((*data).poisoned) }.load(Ordering::Acquire)
}

/// Writes silence to every output buffer the host handed to `process`.
fn silence_outputs(process: &clap_process) {
    if process.audio_outputs.is_null() {
        return;
    }
    let frames = process.frames_count as usize;
    for port in 0..process.audio_outputs_count as usize {
        let buffer = unsafe { &*process.audio_outputs.add(port) };
        for channel in 0..buffer.channel_count as usize {
            if !buffer.data32.is_null() {
                let row = unsafe { *buffer.data32.add(channel) };
                if !row.is_null() {
                    unsafe { std::ptr::write_bytes(row, 0, frames) };
                }
            }
            if !buffer.data64.is_null() {
                let row = unsafe { *buffer.data64.add(channel) };
                if !row.is_null() {
                    unsafe { std::ptr::write_bytes(row, 0, frames) };
                }
            }
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
                let channel = if event.channel < 0 { 0 } else { event.channel.min(15) };
                // The key goes on as given: Plugin::note_on_ref's default hands
                // it to note_on_channel exactly as before note ids existed.
                push_sched(instance, Sched {
                    time: header.time,
                    kind: KIND_NOTE_ON,
                    velocity,
                    target: NoteRef::new(note_id(event.note_id), event.port_index.max(0), channel, event.key),
                    ..Sched::empty()
                });
            }
            EVENT_NOTE_OFF => {
                let event = unsafe { &*(header as *const clap_event_header as *const clap_event_note) };
                let velocity = if event.velocity.is_finite() { (event.velocity.clamp(0.0, 1.0) * 127.0).round() as u8 } else { 0 };
                push_sched(instance, Sched {
                    time: header.time,
                    kind: KIND_NOTE_OFF,
                    velocity,
                    target: NoteRef::new(note_id(event.note_id), event.port_index, event.channel, event.key),
                    ..Sched::empty()
                });
            }
            EVENT_NOTE_EXPRESSION => {
                if instance.expressions.is_empty() || !sized::<clap_event_note_expression>(header) {
                    continue;
                }
                let event = unsafe { &*(header as *const clap_event_header as *const clap_event_note_expression) };
                let Some(expression) = Expression::from_id(i64::from(event.expression_id)) else { continue };
                if !instance.expressions.contains(expression) {
                    continue;
                }
                let (Some(target), Some(value)) = (
                    NoteRef::checked(event.note_id, event.port_index, event.channel, event.key),
                    expression.sanitize(event.value),
                ) else {
                    continue;
                };
                push_sched(instance, Sched {
                    time: header.time,
                    kind: KIND_EXPRESSION,
                    expression: expression as u8,
                    value,
                    target,
                    ..Sched::empty()
                });
            }
            EVENT_PARAM_MOD => {
                if header.space_id != 0 || !sized::<clap_event_param_mod>(header) {
                    continue;
                }
                let event = unsafe { &*(header as *const clap_event_header as *const clap_event_param_mod) };
                let Some(param) = instance.info.params.get(event.param_id as usize) else { continue };
                let Some(target) = NoteRef::checked(event.note_id, event.port_index, event.channel, event.key) else {
                    continue;
                };
                // A per-note amount only for a parameter that offers it; the
                // whole-plugin form for any modulatable one.
                let allowed = if target.is_all() {
                    param.flags.contains(ParamFlags::MODULATABLE)
                } else {
                    param.flags.contains(ParamFlags::POLY_MODULATION)
                };
                if !allowed || !event.amount.is_finite() {
                    continue;
                }
                let span = (param.max - param.min).abs();
                push_sched(instance, Sched {
                    time: header.time,
                    kind: KIND_MOD,
                    param: event.param_id,
                    value: (event.amount as f32).clamp(-span, span),
                    target,
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
            KIND_NOTE_ON => instance.inner.note_on_ref(0, event.target, event.velocity),
            KIND_NOTE_OFF => instance.inner.note_off_ref(0, event.target, event.velocity),
            KIND_MIDI => match instance.mpe.as_mut() {
                Some(receiver) => mpe::receive_into(receiver, instance.inner.as_mut(), 0, event.midi),
                None => instance.inner.midi(0, event.midi),
            },
            KIND_EXPRESSION => {
                if let Some(expression) = Expression::from_id(i64::from(event.expression)) {
                    instance.inner.note_expression(0, event.target, expression, event.value);
                }
            }
            KIND_MOD => instance.inner.param_modulation(0, event.param as usize, event.target, event.value),
            _ => {}
        }
    }
    if dirty {
        pull_params(instance);
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
    for (channel, buffer) in buffers.iter().enumerate().take(out_channels) {
        let dst = unsafe { *output.data32.add(channel) };
        if dst.is_null() {
            continue;
        }
        let out = unsafe { std::slice::from_raw_parts_mut(dst.add(start), n) };
        out.copy_from_slice(&buffer[..n]);
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
        pull_params(instance);
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
    guarded(std::ptr::null(), false, || {
        let Some(instance) = instance(plugin) else { return false };
        instance.host_params = host_call(instance.host, EXT_PARAMS) as *const clap_host_params;
        // The host knows its track by now; a plugin that asks early starts with the right name.
        track_info_changed(plugin);
        true
    })
}

unsafe extern "C" fn plugin_destroy(plugin: *const clap_plugin) {
    guarded(std::ptr::null(), (), || {
        if plugin.is_null() {
            return;
        }
        let data = unsafe { (*plugin).plugin_data };
        if !data.is_null() {
            #[cfg(any(target_os = "macos", target_os = "linux", windows))]
            let mut instance = unsafe { Box::from_raw(data as *mut Instance) };
            #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
            let instance = unsafe { Box::from_raw(data as *mut Instance) };
            // The view goes before the engine whose state it shows.
            #[cfg(any(target_os = "macos", target_os = "linux", windows))]
            {
                instance.gui = None;
            }
            drop(instance);
        }
    })
}

unsafe extern "C" fn plugin_activate(plugin: *const clap_plugin, sample_rate: f64, _min_frames: u32, max_frames: u32) -> bool {
    guarded(plugin, false, || {
        let Some(instance) = instance(plugin) else { return false };
        if !sample_rate_supported(sample_rate) {
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
        // `latency.changed` is allowed here only; it reports whatever moved, whether
        // a block saw it or the plugin said so.
        instance.notifier.take_some(HostNotifier::LATENCY);
        notify_latency(instance);
        instance.restart_requested.store(false, Ordering::Release);
        instance.poisoned.store(false, Ordering::Release);
        instance.activated = true;
        unsafe { main_thread_changes(instance as *mut Instance) };
        true
    })
}

unsafe extern "C" fn plugin_deactivate(plugin: *const clap_plugin) {
    guarded(std::ptr::null(), (), || {
        if let Some(instance) = instance(plugin) {
            instance.activated = false;
            unsafe { main_thread_changes(instance as *mut Instance) };
        }
    })
}

unsafe extern "C" fn plugin_start(_plugin: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn plugin_stop(_plugin: *const clap_plugin) {}

unsafe extern "C" fn plugin_reset(plugin: *const clap_plugin) {
    // `[audio-thread & active]`: the plugin's own code runs, so it runs flushed like `process`.
    let _flush = DenormalGuard::new_if(flushes(plugin));
    guarded(plugin, (), || {
        if let Some(instance) = instance(plugin) {
            instance.inner.reset();
        }
    })
}

/// [`Plugin::flush_denormals`] of the instance behind `plugin`, read through a field pointer
/// (no `&mut Instance`). False for a null plugin or instance: then nothing is called anyway.
fn flushes(plugin: *const clap_plugin) -> bool {
    if plugin.is_null() {
        return false;
    }
    let data = unsafe { (*plugin).plugin_data } as *mut Instance;
    !data.is_null() && unsafe { *std::ptr::addr_of!((*data).flush_denormals) }
}

const GATE_AUDIO: u32 = 1;
const GATE_STATE: u32 = 2;
/// How long a state call waits for a block in flight before it gives up.
const STATE_WAIT: Duration = Duration::from_millis(250);

/// Held by the audio thread for the length of one block.
struct AudioGate<'a>(&'a AtomicU32);

impl Drop for AudioGate<'_> {
    fn drop(&mut self) {
        self.0.fetch_and(!GATE_AUDIO, Ordering::Release);
    }
}

/// Held by a state call while it is inside the plugin object.
struct StateGate<'a>(&'a AtomicU32);

impl Drop for StateGate<'_> {
    fn drop(&mut self) {
        self.0.fetch_and(!GATE_STATE, Ordering::Release);
    }
}

/// Takes the plugin object for a state call. Announces itself first, so a block that
/// starts from now on is skipped (the audio thread never waits), then waits a bounded
/// time for the block already running to end. `None` after `STATE_WAIT`.
fn enter_state(gate: &AtomicU32) -> Option<StateGate<'_>> {
    gate.fetch_or(GATE_STATE, Ordering::AcqRel);
    let deadline = Clock::now() + STATE_WAIT;
    let mut spins = 0u32;
    loop {
        if gate.load(Ordering::Acquire) & GATE_AUDIO == 0 {
            return Some(StateGate(gate));
        }
        if Clock::now() >= deadline {
            gate.fetch_and(!GATE_STATE, Ordering::Release);
            return None;
        }
        spins += 1;
        if spins < 64 {
            std::hint::spin_loop();
        } else {
            std::thread::sleep(Duration::from_micros(50));
        }
    }
}

/// Brackets a write of several plane slots so `pull_params` never applies half of it.
fn plane_write<R>(epoch: &AtomicU32, write: impl FnOnce() -> R) -> R {
    epoch.fetch_add(1, Ordering::Relaxed);
    fence(Ordering::Release);
    let result = write();
    epoch.fetch_add(1, Ordering::Release);
    result
}

/// Applies the plane to the plugin from a consistent copy. While a state load is
/// writing the plane, the pull is skipped and the plugin keeps its values for this
/// chunk; the cursor applies the loaded set whole at the next one.
fn pull_params(instance: &mut Instance) {
    let before = instance.plane_epoch.load(Ordering::Acquire);
    if before & 1 == 1 {
        return;
    }
    for index in 0..instance.plane.len() {
        instance.snapshot.store(index, instance.plane.load(index).unwrap_or(0.0));
    }
    fence(Ordering::Acquire);
    if instance.plane_epoch.load(Ordering::Relaxed) != before {
        return;
    }
    instance.cursor.pull(&instance.snapshot, instance.inner.as_mut());
}

/// The block a state call made the audio thread skip: parameter events still reach
/// the plane, the input passes through (silence where it has no channel), and nothing
/// touches the plugin object.
unsafe fn skipped_block(plane: &ParamPlane, process: &clap_process) -> i32 {
    if !process.in_events.is_null() {
        let events = unsafe { &*process.in_events };
        if let (Some(size), Some(get)) = (events.size, events.get) {
            for index in 0..unsafe { size(events) } {
                let header = unsafe { get(events, index) };
                if header.is_null() {
                    continue;
                }
                let header = unsafe { &*header };
                if header.type_ == EVENT_PARAM_VALUE && header.space_id == 0 {
                    let event = unsafe { &*(header as *const clap_event_header as *const clap_event_param_value) };
                    plane.store(event.param_id as usize, event.value as f32);
                }
            }
        }
    }
    if process.frames_count == 0 || process.audio_outputs_count == 0 || process.audio_outputs.is_null() {
        return PROCESS_CONTINUE;
    }
    let output = unsafe { &*process.audio_outputs };
    if output.data32.is_null() {
        return PROCESS_ERROR;
    }
    let frames = process.frames_count as usize;
    let input = if process.audio_inputs_count == 0 || process.audio_inputs.is_null() {
        None
    } else {
        Some(unsafe { &*process.audio_inputs })
    };
    for channel in 0..output.channel_count as usize {
        let dst = unsafe { *output.data32.add(channel) };
        if dst.is_null() {
            continue;
        }
        let src = match input {
            Some(input) if channel < input.channel_count as usize && !input.data32.is_null() => unsafe { *input.data32.add(channel) },
            _ => std::ptr::null_mut(),
        };
        if src.is_null() {
            unsafe { std::ptr::write_bytes(dst, 0, frames) };
        } else if src != dst {
            unsafe { std::ptr::copy(src, dst, frames) };
        }
    }
    PROCESS_CONTINUE
}

unsafe extern "C" fn plugin_process(plugin: *const clap_plugin, process: *const clap_process) -> i32 {
    if plugin.is_null() || process.is_null() {
        return PROCESS_ERROR;
    }
    let raw = unsafe { (*plugin).plugin_data } as *mut Instance;
    if raw.is_null() {
        return PROCESS_ERROR;
    }
    // A poisoned instance never calls into the plugin again until it is activated anew.
    if is_poisoned(plugin) {
        silence_outputs(unsafe { &*process });
        return PROCESS_ERROR;
    }
    // Only the gate is touched before the block owns the instance: a state call may
    // be inside the plugin object, and it never touches the rest.
    let gate = unsafe { &*std::ptr::addr_of!((*raw).gate) };
    if gate.compare_exchange(0, GATE_AUDIO, Ordering::Acquire, Ordering::Relaxed).is_err() {
        let plane = unsafe { &*std::ptr::addr_of!((*raw).plane) };
        return unsafe { skipped_block(plane, &*process) };
    }
    let _held = AudioGate(gate);
    // Flush-to-zero for the plugin, the host's mode back when this drops: after the block,
    // and after a panic, which `guarded` catches inside.
    let flush = DenormalGuard::new_if(unsafe { *std::ptr::addr_of!((*raw).flush_denormals) });
    let result = guarded(plugin, PROCESS_ERROR, || process_block(unsafe { &mut *raw }, process));
    drop(flush);
    if result == PROCESS_ERROR && is_poisoned(plugin) {
        silence_outputs(unsafe { &*process });
    }
    result
}

fn process_block(instance: &mut Instance, process: *const clap_process) -> i32 {
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
    guarded(std::ptr::null(), 0, || {
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
    })
}

unsafe extern "C" fn audio_ports_get(plugin: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_audio_port_info) -> bool {
    guarded(std::ptr::null(), false, || {
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
    })
}

static AUDIO_PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(audio_ports_count),
    get: Some(audio_ports_get),
};

unsafe extern "C" fn note_ports_count(plugin: *const clap_plugin, is_input: bool) -> u32 {
    guarded(std::ptr::null(), 0, || {
        let Some(instance) = instance(plugin) else { return 0 };
        let ports = instance.inner.note_ports();
        u32::from(if is_input { ports.input } else { ports.output })
    })
}

unsafe extern "C" fn note_ports_get(plugin: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_note_port_info) -> bool {
    guarded(std::ptr::null(), false, || {
        if !is_input || index != 0 || info.is_null() {
            return false;
        }
        // A plugin that takes per-note expressions prefers CLAP note events
        // (they carry note ids); one that asked for MPE also takes MPE MIDI.
        let (expressive, mpe) = instance(plugin).map_or((false, false), |i| (!i.expressions.is_empty(), i.mpe.is_some()));
        unsafe {
            std::ptr::write_bytes(info, 0, 1);
            let info = &mut *info;
            info.id = 0;
            info.supported_dialects = NOTE_DIALECT_CLAP | NOTE_DIALECT_MIDI | if mpe { NOTE_DIALECT_MIDI_MPE } else { 0 };
            info.preferred_dialect = if expressive { NOTE_DIALECT_CLAP } else { NOTE_DIALECT_MIDI };
            copy_cstr(&mut info.name, "MIDI In");
        }
        true
    })
}

static NOTE_PORTS: clap_plugin_note_ports = clap_plugin_note_ports {
    count: Some(note_ports_count),
    get: Some(note_ports_get),
};

unsafe extern "C" fn latency_get(plugin: *const clap_plugin) -> u32 {
    guarded(std::ptr::null(), 0, || {
        instance(plugin).map(|instance| instance.inner.latency() as u32).unwrap_or(0)
    })
}

static LATENCY: clap_plugin_latency = clap_plugin_latency { get: Some(latency_get) };

unsafe extern "C" fn params_count(plugin: *const clap_plugin) -> u32 {
    guarded(std::ptr::null(), 0, || {
        instance(plugin).map(|instance| instance.inner.info().params.len() as u32).unwrap_or(0)
    })
}

unsafe extern "C" fn params_get_info(plugin: *const clap_plugin, index: u32, info: *mut clap_param_info) -> bool {
    guarded(std::ptr::null(), false, || {
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
            // The group path is CLAP's module path ("Filter/Envelope"); empty is the top level.
            copy_cstr(&mut info.module, param.host_group());
            info.min_value = param.min as f64;
            info.max_value = param.max as f64;
            info.default_value = param.default as f64;
        }
        true
    })
}

unsafe extern "C" fn params_get_value(plugin: *const clap_plugin, param_id: u32, out_value: *mut f64) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(instance) = instance(plugin) else { return false };
        let Some(value) = instance.plane.load(param_id as usize) else { return false };
        if out_value.is_null() {
            return false;
        }
        unsafe { *out_value = value as f64 };
        true
    })
}

unsafe extern "C" fn params_value_to_text(plugin: *const clap_plugin, param_id: u32, value: f64, out_buffer: *mut c_char, capacity: u32) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(instance) = instance(plugin) else { return false };
        let Some(param) = instance.inner.info().params.get(param_id as usize) else { return false };
        if out_buffer.is_null() || capacity == 0 {
            return false;
        }
        let text = param.format(value as f32);
        let dst = unsafe { std::slice::from_raw_parts_mut(out_buffer, capacity as usize) };
        copy_cstr(dst, &text);
        true
    })
}

unsafe extern "C" fn params_text_to_value(plugin: *const clap_plugin, param_id: u32, text: *const c_char, out_value: *mut f64) -> bool {
    guarded(std::ptr::null(), false, || {
        if text.is_null() || out_value.is_null() {
            return false;
        }
        let Some(instance) = instance(plugin) else { return false };
        let Some(param) = instance.inner.info().params.get(param_id as usize) else { return false };
        let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
        let Some(value) = param.parse(&text) else { return false };
        unsafe { *out_value = value as f64 };
        true
    })
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
    // On the audio thread while active, and `set_param` runs here as it does inside
    // `process`: flushed alike, so a coefficient computed here matches one computed there.
    let _flush = DenormalGuard::new_if(flushes(plugin));
    guarded(plugin, (), || {
        if !plugin.is_null() && is_poisoned(plugin) {
            return;
        }
        if let Some(instance) = instance(plugin) {
            collect_events(instance, input);
            report_gestures(instance, output);
            if instance.sched_count > 0 {
                let count = instance.sched_count;
                apply_group(instance, 0, count);
            }
        }
    })
}

static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_get_info),
    get_value: Some(params_get_value),
    value_to_text: Some(params_value_to_text),
    text_to_value: Some(params_text_to_value),
    flush: Some(params_flush),
};

/// State calls reach the instance through field pointers and never form a
/// `&mut Instance`, which would alias the audio thread's. `gate` decides who may use
/// `inner` and `cursor`.
unsafe fn state_target(plugin: *const clap_plugin) -> Option<*mut Instance> {
    if plugin.is_null() {
        return None;
    }
    let raw = unsafe { (*plugin).plugin_data } as *mut Instance;
    (!raw.is_null()).then_some(raw)
}

fn params_snapshot(plane: &ParamPlane, info: &'static PluginInfo) -> Vec<u8> {
    let values: Vec<f32> = (0..plane.len()).map(|index| plane.load(index).unwrap_or(0.0)).collect();
    encode_params(info, &values)
}

unsafe extern "C" fn state_save(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(raw) = (unsafe { state_target(plugin) }) else { return false };
        if stream.is_null() {
            return false;
        }
        let stream = unsafe { &*stream };
        let Some(write) = stream.write else { return false };
        let (plane, info, has_chunk) = unsafe { (&*std::ptr::addr_of!((*raw).plane), (*raw).info, (*raw).has_chunk) };
        let _serial = unsafe { &*std::ptr::addr_of!((*raw).state_lock) }.lock();
        let custom = if has_chunk {
            // The plugin owns state beyond its parameters: take it between blocks.
            let gate = unsafe { &*std::ptr::addr_of!((*raw).gate) };
            let Some(_held) = enter_state(gate) else { return false };
            unsafe { &mut *std::ptr::addr_of_mut!((*raw).inner) }.save_state()
        } else {
            None
        };
        let bytes = custom.unwrap_or_else(|| params_snapshot(plane, info));
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
    })
}

unsafe extern "C" fn state_load(plugin: *const clap_plugin, stream: *const clap_istream) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(raw) = (unsafe { state_target(plugin) }) else { return false };
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
        let (plane, epoch, info, has_chunk, host) = unsafe {
            (
                &*std::ptr::addr_of!((*raw).plane),
                &*std::ptr::addr_of!((*raw).plane_epoch),
                (*raw).info,
                (*raw).has_chunk,
                (*raw).host,
            )
        };
        let _serial = unsafe { &*std::ptr::addr_of!((*raw).state_lock) }.lock();
        if has_chunk {
            let gate = unsafe { &*std::ptr::addr_of!((*raw).gate) };
            let Some(held) = enter_state(gate) else { return false };
            let inner = unsafe { &mut *std::ptr::addr_of_mut!((*raw).inner) };
            if inner.load_state(&bytes) {
                // The host reads parameters from the plane. A state chunk changes the plugin
                // directly, so republish the loaded values or the host would keep the old ones.
                plane_write(epoch, || plane.seed_from(inner.as_ref()));
                unsafe { &mut *std::ptr::addr_of_mut!((*raw).cursor) }.invalidate();
                drop(held);
                notify_params_rescan(host);
                // `load_state` may have said its latency or parameter info changed.
                unsafe { main_thread_changes(raw) };
                return true;
            }
        }
        // The parameter snapshot `save` writes for a plugin without a state chunk. It goes
        // to the plane only, and the engine pulls it before the next block. Everything is
        // checked before the first slot is written.
        let Some(values) = decode_params(info, &bytes) else { return false };
        if values.len() != plane.len() || values.iter().any(|v| !v.is_finite()) {
            return false;
        }
        plane_write(epoch, || {
            for (index, value) in values.iter().enumerate() {
                plane.store(index, *value);
            }
        });
        notify_params_rescan(host);
        unsafe { main_thread_changes(raw) };
        true
    })
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
    if param.flags.contains(ParamFlags::MODULATABLE) {
        flags |= PARAM_IS_MODULATABLE;
    }
    if param.flags.contains(ParamFlags::POLY_MODULATION) {
        flags |= PARAM_IS_MODULATABLE_PER_NOTE_ID
            | PARAM_IS_MODULATABLE_PER_KEY
            | PARAM_IS_MODULATABLE_PER_CHANNEL
            | PARAM_IS_MODULATABLE_PER_PORT;
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

fn notify_params_rescan(host: *const clap_host) {
    notify_params_rescan_flags(host, CLAP_PARAM_RESCAN_VALUES);
}

/// `clap_host_params.rescan(flags)`. Main thread.
fn notify_params_rescan_flags(host: *const clap_host, flags: u32) {
    let ext = host_call(host, EXT_PARAMS) as *const clap_host_params;
    if ext.is_null() {
        return;
    }
    unsafe {
        if let Some(rescan) = (*ext).rescan {
            rescan(host, flags);
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

/// After every block (audio thread): a latency that moved asks the host for a restart, once
/// until the next `activate` reports it; other pending changes ask for a main-thread
/// callback, once until `on_main_thread` takes them. Nothing here allocates or locks;
/// `request_restart` and `request_callback` are thread-safe in CLAP.
fn watch_latency(instance: &mut Instance) {
    let now = instance.inner.latency().min(u32::MAX as usize) as u32;
    // The comparison below is what an explicit `latency_changed` asks for.
    instance.notifier.take_some(HostNotifier::LATENCY);
    if instance.activated && now != instance.reported_latency {
        ask_restart(instance.host, &instance.restart_requested);
    }
    if instance.notifier.pending() & HostNotifier::PARAM_INFO != 0 {
        ask_callback(instance.host, &instance.callback_requested);
    }
}

fn ask_restart(host: *const clap_host, requested: &AtomicBool) {
    if !requested.swap(true, Ordering::AcqRel) {
        request_restart(host);
    }
}

fn ask_callback(host: *const clap_host, requested: &AtomicBool) {
    if host.is_null() || requested.swap(true, Ordering::AcqRel) {
        return;
    }
    match unsafe { (*host).request_callback } {
        Some(callback) => unsafe { callback(host) },
        // No callback: the next main-thread call (activate, deactivate, a state load) tells.
        None => requested.store(false, Ordering::Release),
    }
}

/// Tell the host what the plugin said changed, on the main thread. A latency change while
/// active asks for a restart (CLAP lets `latency.changed` run only inside `activate`, which
/// reports it); while inactive the next `activate` reports it. Parameter info is rescanned
/// now: names and flags (`INFO`), value text (`TEXT`) and values (`VALUES`), which CLAP
/// lets change while active. Works through field pointers, like the state calls, so it
/// never forms a reference to the instance the audio thread is using.
unsafe fn main_thread_changes(raw: *mut Instance) {
    let notifier = unsafe { &*std::ptr::addr_of!((*raw).notifier) };
    let host = unsafe { (*raw).host };
    let activated = unsafe { (*raw).activated };
    let flags = notifier.take();
    if flags & HostNotifier::LATENCY != 0 && activated {
        ask_restart(host, unsafe { &*std::ptr::addr_of!((*raw).restart_requested) });
    }
    if flags & HostNotifier::PARAM_INFO != 0 {
        notify_params_rescan_flags(host, CLAP_PARAM_RESCAN_VALUES | CLAP_PARAM_RESCAN_TEXT | CLAP_PARAM_RESCAN_INFO);
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
    let mut info = Transport {
        playing: flags & TRANSPORT_IS_PLAYING != 0,
        recording: flags & TRANSPORT_IS_RECORDING != 0,
        sample_pos: if steady_time >= 0 { steady_time } else { 0 },
        ..Transport::default()
    };
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
    guarded(std::ptr::null(), 0, || {
        instance(plugin)
            .and_then(|item| item.inner.tail().clap_samples())
            .unwrap_or(0)
    })
}

static TAIL: clap_plugin_tail = clap_plugin_tail { get: Some(tail_get) };

/// Whether CLAP can describe every bus of `layout`: a surround bus must use only the
/// `CLAP_SURROUND_*` channel ids (`SpeakerLayout::clap_expressible`). The beds with top side
/// or wide channels (5.1.2, 7.1.2, 9.1.6) cannot be, so the entry does not offer them.
fn layout_expressible(layout: IoConfig) -> bool {
    let ok = |bus: SpeakerLayout| bus.kind() != PortKind::Surround || bus.clap_expressible();
    ok(layout.input) && ok(layout.output) && layout.sidechain.is_none_or(ok)
}

/// The layouts the entry offers, with their config ids (the index in `supported_layouts`).
fn offered_layouts(plugin: &dyn Plugin) -> impl Iterator<Item = (u32, IoConfig)> + '_ {
    plugin
        .supported_layouts()
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, layout)| layout_expressible(*layout))
        .map(|(index, layout)| (index as u32, layout))
}

/// The layout with config id `config_id`, if it exists and CLAP can express it.
fn offered_layout(plugin: &dyn Plugin, config_id: u32) -> Option<IoConfig> {
    offered_layouts(plugin).find(|(id, _)| *id == config_id).map(|(_, layout)| layout)
}

unsafe extern "C" fn config_count(plugin: *const clap_plugin) -> u32 {
    guarded(std::ptr::null(), 0, || {
        instance(plugin)
            .map(|item| offered_layouts(item.inner.as_ref()).count() as u32)
            .unwrap_or(0)
    })
}

unsafe extern "C" fn config_get(plugin: *const clap_plugin, index: u32, config: *mut clap_audio_ports_config) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(instance) = instance(plugin) else { return false };
        let Some((id, layout)) = offered_layouts(instance.inner.as_ref()).nth(index as usize) else {
            return false;
        };
        if config.is_null() {
            return false;
        }
        unsafe {
            std::ptr::write_bytes(config, 0, 1);
            let config = &mut *config;
            config.id = id;
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
    })
}

unsafe extern "C" fn config_select(plugin: *const clap_plugin, config_id: u32) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(instance) = instance(plugin) else { return false };
        if instance.activated {
            return false;
        }
        let Some(layout) = offered_layout(instance.inner.as_ref(), config_id) else {
            return false;
        };
        if !instance.inner.set_layout(layout) {
            return false;
        }
        instance.layout_index = config_id;
        true
    })
}

static AUDIO_PORTS_CONFIG: clap_plugin_audio_ports_config = clap_plugin_audio_ports_config {
    count: Some(config_count),
    get: Some(config_get),
    select: Some(config_select),
};

unsafe extern "C" fn config_current(plugin: *const clap_plugin) -> u32 {
    guarded(std::ptr::null(), 0, || {
        instance(plugin).map(|item| item.layout_index).unwrap_or(CLAP_INVALID_ID)
    })
}

unsafe extern "C" fn config_info_get(
    plugin: *const clap_plugin,
    config_id: u32,
    port_index: u32,
    is_input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(instance) = instance(plugin) else { return false };
        let Some(layout) = offered_layout(instance.inner.as_ref(), config_id) else {
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
    })
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
    guarded(std::ptr::null(), false, || {
        instance(plugin).map(|item| mask_supported(item, channel_mask)).unwrap_or(false)
    })
}

unsafe extern "C" fn surround_map(
    plugin: *const clap_plugin,
    is_input: bool,
    port_index: u32,
    channel_map: *mut u8,
    channel_map_capacity: u32,
) -> u32 {
    guarded(std::ptr::null(), 0, || {
        let Some(instance) = instance(plugin) else { return 0 };
        let Some((_, _, layout, _)) = layout_port(selected_layout(instance), is_input, port_index) else {
            return 0;
        };
        let Some(map) = layout.clap_map() else { return 0 };
        if channel_map.is_null() || (channel_map_capacity as usize) < map.len() {
            return 0;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(map.as_ptr(), channel_map, map.len());
        }
        map.len() as u32
    })
}

static SURROUND: clap_plugin_surround = clap_plugin_surround {
    is_channel_mask_supported: Some(surround_supported),
    get_channel_map: Some(surround_map),
};

unsafe extern "C" fn ambisonic_supported(_plugin: *const clap_plugin, config: *const clap_ambisonic_config) -> bool {
    guarded(std::ptr::null(), false, || {
        if config.is_null() {
            return false;
        }
        let config = unsafe { &*config };
        config.ordering == AMBISONIC_ORDERING_ACN && config.normalization == AMBISONIC_NORMALIZATION_SN3D
    })
}

unsafe extern "C" fn ambisonic_get(
    plugin: *const clap_plugin,
    is_input: bool,
    port_index: u32,
    config: *mut clap_ambisonic_config,
) -> bool {
    guarded(std::ptr::null(), false, || {
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
    })
}

static AMBISONIC: clap_plugin_ambisonic = clap_plugin_ambisonic {
    is_config_supported: Some(ambisonic_supported),
    get_config: Some(ambisonic_get),
};

unsafe extern "C" fn render_hard(_plugin: *const clap_plugin) -> bool {
    false
}

unsafe extern "C" fn render_set(plugin: *const clap_plugin, mode: u32) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(instance) = instance(plugin) else { return false };
        let offline = mode == RENDER_OFFLINE;
        instance.inner.set_offline(offline);
        release_offline_momentary(instance, offline);
        true
    })
}

static RENDER: clap_plugin_render = clap_plugin_render {
    has_hard_realtime_requirement: Some(render_hard),
    set: Some(render_set),
};

unsafe extern "C" fn plugin_extension(plugin: *const clap_plugin, id: *const c_char) -> *const c_void {
    guarded(std::ptr::null(), std::ptr::null(), || {
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
        if id == EXT_TRACK_INFO || id == EXT_TRACK_INFO_COMPAT {
            return &TRACK_INFO as *const clap_plugin_track_info as *const c_void;
        }
        if id == EXT_CONTEXT_MENU || id == EXT_CONTEXT_MENU_COMPAT {
            return &CONTEXT_MENU as *const clap_plugin_context_menu as *const c_void;
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
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        if id == EXT_GUI && editor_factory().is_some() {
            return &gui::GUI as *const clap_plugin_gui as *const c_void;
        }
        // The host's timer and descriptor calls that drive the X11 view.
        #[cfg(target_os = "linux")]
        if let Some(ext) = gui::driver_extension(id) {
            return ext;
        }
        std::ptr::null()
    })
}

// ---- Track info and the context menu -----------------------------------------

/// The host's track info extension (the final id, then the draft id).
fn host_track_info(host: *const clap_host) -> *const clap_host_track_info {
    let ext = host_call(host, EXT_TRACK_INFO);
    let ext = if ext.is_null() { host_call(host, EXT_TRACK_INFO_COMPAT) } else { ext };
    ext as *const clap_host_track_info
}

/// Read the track from the host: name and colour only when the host flags them, kind bits
/// from the return, bus and master flags. `None` when the host has no track info or says no.
fn read_track_info(host: *const clap_host) -> Option<TrackInfo> {
    let ext = unsafe { host_track_info(host).as_ref() }?;
    let get = ext.get?;
    let mut raw = clap_track_info { flags: 0, name: [0; NAME_SIZE], color: clap_color::default(), audio_channel_count: 0, audio_port_type: std::ptr::null() };
    if !unsafe { get(host, &mut raw) } {
        return None;
    }
    let name = (raw.flags & TRACK_INFO_HAS_TRACK_NAME != 0)
        .then(|| {
            // The host's array is fixed-size; never read past it if it forgot the NUL.
            let bytes: Vec<u8> = raw.name.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
            String::from_utf8_lossy(&bytes).into_owned()
        })
        .filter(|name| !name.is_empty());
    let colour = (raw.flags & TRACK_INFO_HAS_TRACK_COLOR != 0)
        .then(|| u32::from_be_bytes([raw.color.red, raw.color.green, raw.color.blue, raw.color.alpha]));
    let mut kind = 0;
    if raw.flags & TRACK_INFO_IS_FOR_RETURN_TRACK != 0 {
        kind |= TrackInfo::IS_RETURN;
    }
    if raw.flags & TRACK_INFO_IS_FOR_BUS != 0 {
        kind |= TrackInfo::IS_BUS;
    }
    if raw.flags & TRACK_INFO_IS_FOR_MASTER != 0 {
        kind |= TrackInfo::IS_MASTER;
    }
    Some(TrackInfo { name, colour, kind })
}

/// Read the host's track info again and, when it differs from what the plugin last heard,
/// give it to the plugin and the open editor. Main thread. The plugin object is taken the
/// way a state call takes it: between blocks, never while `process` runs.
fn track_info_changed(plugin: *const clap_plugin) {
    let Some(raw) = (unsafe { state_target(plugin) }) else { return };
    let Some(info) = read_track_info(unsafe { (*raw).host }) else { return };
    let inst = unsafe { &mut *raw };
    if inst.track_info.as_ref() == Some(&info) {
        return;
    }
    inst.track_info = Some(info.clone());
    {
        let _serial = inst.state_lock.lock();
        let gate = unsafe { &*std::ptr::addr_of!((*raw).gate) };
        if let Some(_held) = enter_state(gate) {
            unsafe { &mut *std::ptr::addr_of_mut!((*raw).inner) }.track_info_changed(&info);
        }
    }
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    if let Some(editor) = inst.gui.as_mut().and_then(|gui| gui.view.editor()) {
        editor.track_info(&info);
    }
    unsafe { main_thread_changes(raw) };
}

unsafe extern "C" fn track_info_plugin_changed(plugin: *const clap_plugin) {
    guarded(std::ptr::null(), (), || track_info_changed(plugin))
}

static TRACK_INFO: clap_plugin_track_info = clap_plugin_track_info { changed: Some(track_info_plugin_changed) };

/// The plugin adds no items of its own to the host's menus; the extension is offered so a
/// host that asks every plugin for its items gets an answer (an empty one).
unsafe extern "C" fn context_populate(_plugin: *const clap_plugin, _target: *const clap_context_menu_target, _builder: *const clap_context_menu_builder) -> bool {
    true
}

unsafe extern "C" fn context_perform(_plugin: *const clap_plugin, _target: *const clap_context_menu_target, _action_id: u32) -> bool {
    false
}

static CONTEXT_MENU: clap_plugin_context_menu = clap_plugin_context_menu { populate: Some(context_populate), perform: Some(context_perform) };

/// Ask the host to pop up its context menu for parameter `param` at (`x`, `y`), relative to
/// the plugin's window. False when the host has no such menu, cannot show one now, or declines.
fn host_context_popup(host: *const clap_host, param: u32, x: i32, y: i32) -> bool {
    let ext = host_call(host, EXT_CONTEXT_MENU);
    let ext = if ext.is_null() { host_call(host, EXT_CONTEXT_MENU_COMPAT) } else { ext };
    let Some(ext) = (unsafe { (ext as *const clap_host_context_menu).as_ref() }) else { return false };
    let (Some(can_popup), Some(popup)) = (ext.can_popup, ext.popup) else { return false };
    if !unsafe { can_popup(host) } {
        return false;
    }
    let target = clap_context_menu_target { kind: CONTEXT_MENU_TARGET_KIND_PARAM, id: param };
    unsafe { popup(host, &target, 0, x, y) }
}

/// The host's answer to `request_callback`: tell it what changed. Main thread.
unsafe extern "C" fn plugin_main_thread(plugin: *const clap_plugin) {
    guarded(std::ptr::null(), (), || {
        let Some(raw) = (unsafe { state_target(plugin) }) else { return };
        unsafe { &*std::ptr::addr_of!((*raw).callback_requested) }.store(false, Ordering::Release);
        unsafe { main_thread_changes(raw) };
    })
}

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

fn spawn(mut inner: Box<dyn Plugin>, host: *const clap_host) -> *const clap_plugin {
    let notifier = HostNotifier::new();
    inner.set_host_notifier(notifier.clone());
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
    // The same rule as the shells: only a plugin that declares its state is its
    // parameters skips the gate. Probing `save_state` would miss a chunk that
    // appears later.
    let has_chunk = !inner.state_is_parameters_only();
    let editor_state = inner.editor_state();
    let expressions = inner.note_expressions();
    let mpe = MpeReceiver::for_plugin(inner.as_ref());
    let flush_denormals = inner.flush_denormals();
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
        snapshot: ParamPlane::new(info),
        plane_epoch: AtomicU32::new(0),
        gate: AtomicU32::new(0),
        state_lock: Mutex::new(()),
        has_chunk,
        info,
        plane,
        cursor,
        gestures: Arc::new(GestureQueue::new(GESTURE_CAPACITY)),
        editor_state,
        host_params: std::ptr::null(),
        track_info: None,
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        gui: None,
        meters: Seqlock::new(MeterFrame::SILENT),
        bus_meters: Seqlock::new(BusMeters::SILENT),
        sched: [Sched::empty(); MAX_SCHED],
        sched_count: 0,
        expressions,
        mpe,
        buffers: Vec::new(),
        sidechain: Vec::new(),
        sample_rate: 48_000.0,
        max_block: 0,
        host,
        reported_latency,
        notifier,
        restart_requested: AtomicBool::new(false),
        callback_requested: AtomicBool::new(false),
        reported_tail,
        layout_index: 0,
        activated: false,
        poisoned: AtomicBool::new(false),
        flush_denormals,
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

    /// The same, with a host the plugin calls back (extensions, requests). The host
    /// must outlive the adapter.
    ///
    /// # Safety
    /// `host` is a valid `clap_host` for as long as the adapter lives.
    pub unsafe fn with_host(inner: P, host: *const clap_host) -> Self {
        let raw = spawn(Box::new(inner), host);
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
    guarded(std::ptr::null(), std::ptr::null(), || {
        if index != 0 {
            return std::ptr::null();
        }
        publish().map(|published| &published.desc as *const clap_plugin_descriptor).unwrap_or(std::ptr::null())
    })
}

unsafe extern "C" fn factory_create(_factory: *const clap_plugin_factory, host: *const clap_host, plugin_id: *const c_char) -> *const clap_plugin {
    guarded(std::ptr::null(), std::ptr::null(), || {
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
    })
}

static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(factory_count),
    get_plugin_descriptor: Some(factory_descriptor),
    create_plugin: Some(factory_create),
};

/// `clap_entry.get_factory`. Returns the plugin factory after [`set_creator`].
///
/// # Safety
/// `factory_id` is null or a NUL-terminated string owned by the host and
/// valid for the call.
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

/// `clap_entry.deinit`. Nothing to release.
///
/// # Safety
/// Called by the CLAP host on unload; it has no preconditions.
pub unsafe extern "C" fn entry_deinit() {}

/// What an editor's gestures do in CLAP: store the value in the plane, queue
/// the gesture for the host, and ask the host to collect it.
fn editor_host_for(instance: &Instance) -> openplugin_core::EditorHost {
    #[derive(Clone, Copy)]
    struct Link(*const clap_host, *const clap_host_params);
    let link = Link(instance.host, instance.host_params);
    let plane = instance.plane.clone();
    let queue = instance.gestures.clone();
    let host = instance.host;
    // A right click on a control asks the host for its menu on that parameter.
    let menu = move |param: u32, x: i32, y: i32| host_context_popup(host, param, x, y);
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
    .with_context_menu(menu)
}

#[cfg(windows)]
#[path = "clap_gui_win32.rs"]
mod gui;

#[cfg(target_os = "macos")]
mod gui {
    //! `clap.gui` on macOS: the editor in an `NSView`, embedded. The window
    //! is as resizable as the editor says: `can_resize`, `get_resize_hints`,
    //! `adjust_size` and `set_size` follow its `SizeRange`, and the editor can
    //! ask the host for a size through `clap_host_gui::request_resize`.
    use super::*;
    use openplugin_core::{Editor, ValueMirror};
    use openplugin_core::editor::SizeRange;
    use openplugin_dri::cocoa::EditorView;

    pub(super) struct Gui {
        pub(super) view: EditorView,
    }

    fn is_cocoa(api: *const c_char, floating: bool) -> bool {
        !api.is_null() && !floating && unsafe { CStr::from_ptr(api) }.to_bytes_with_nul() == WINDOW_API_COCOA
    }

    unsafe extern "C" fn is_api_supported(_plugin: *const clap_plugin, api: *const c_char, floating: bool) -> bool {
        guarded(std::ptr::null(), false, || {
            is_cocoa(api, floating)
        })
    }

    unsafe extern "C" fn get_preferred_api(_plugin: *const clap_plugin, api: *mut *const c_char, floating: *mut bool) -> bool {
        guarded(std::ptr::null(), false, || {
            if api.is_null() || floating.is_null() {
                return false;
            }
            unsafe {
                *api = WINDOW_API_COCOA.as_ptr() as *const c_char;
                *floating = false;
            }
            true
        })
    }

    unsafe extern "C" fn create(plugin: *const clap_plugin, api: *const c_char, floating: bool) -> bool {
        guarded(std::ptr::null(), false, || {
            if !is_cocoa(api, floating) {
                return false;
            }
            let Some(instance) = instance(plugin) else { return false };
            let Some(factory) = editor_factory() else { return false };
            instance.gui = None;
            // The host's gui extension, if any: how the editor asks for a size.
            let host = instance.host;
            let host_gui = host_call(host, EXT_GUI) as *const clap_host_gui;
            let request = move |w: u32, h: u32| -> bool {
                let Some(gui) = (unsafe { host_gui.as_ref() }) else { return false };
                match gui.request_resize {
                    Some(f) => unsafe { f(host, w, h) },
                    None => false,
                }
            };
            let Some(mut editor) = factory(editor_host_for(instance).with_resize(request)) else { return false };
            editor.bind(instance.editor_state.clone());
            if let Some(info) = instance.track_info.as_ref() {
                editor.track_info(info);
            }
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
        })
    }

    unsafe extern "C" fn destroy(plugin: *const clap_plugin) {
        guarded(std::ptr::null(), (), || {
            if let Some(instance) = instance(plugin) {
                instance.gui = None;
            }
        })
    }

    unsafe extern "C" fn set_scale(_plugin: *const clap_plugin, _scale: f64) -> bool {
        guarded(std::ptr::null(), false, || {
            // Cocoa works in points; the view reads the backing scale itself.
            false
        })
    }

    fn size(plugin: *const clap_plugin) -> Option<(u32, u32)> {
        instance(plugin).and_then(|inst| inst.gui.as_ref()).map(|gui| gui.view.size())
    }

    unsafe extern "C" fn get_size(plugin: *const clap_plugin, width: *mut u32, height: *mut u32) -> bool {
        guarded(std::ptr::null(), false, || {
            let Some((w, h)) = size(plugin) else { return false };
            if width.is_null() || height.is_null() {
                return false;
            }
            unsafe {
                *width = w;
                *height = h;
            }
            true
        })
    }

    fn range(plugin: *const clap_plugin) -> Option<SizeRange> {
        instance(plugin).and_then(|inst| inst.gui.as_ref()).map(|gui| gui.view.size_range())
    }

    unsafe extern "C" fn can_resize(plugin: *const clap_plugin) -> bool {
        guarded(std::ptr::null(), false, || {
            range(plugin).is_some_and(|r| !r.is_fixed())
        })
    }

    unsafe extern "C" fn get_resize_hints(plugin: *const clap_plugin, hints: *mut clap_gui_resize_hints) -> bool {
        guarded(std::ptr::null(), false, || {
            let Some(r) = range(plugin) else { return false };
            if hints.is_null() || r.is_fixed() {
                return false;
            }
            let (aw, ah) = r.aspect_ratio().unwrap_or((0, 0));
            unsafe {
                *hints = clap_gui_resize_hints {
                    can_resize_horizontally: r.can_resize_horizontally(),
                    can_resize_vertically: r.can_resize_vertically(),
                    preserve_aspect_ratio: aw > 0,
                    aspect_ratio_width: aw,
                    aspect_ratio_height: ah,
                };
            }
            true
        })
    }

    /// The size nearest to the one proposed that the editor takes.
    unsafe extern "C" fn adjust_size(plugin: *const clap_plugin, width: *mut u32, height: *mut u32) -> bool {
        guarded(std::ptr::null(), false, || {
            let Some(r) = range(plugin) else { return false };
            if width.is_null() || height.is_null() {
                return false;
            }
            unsafe {
                let (w, h) = r.clamp(*width, *height);
                *width = w;
                *height = h;
            }
            true
        })
    }

    /// Take a size inside the editor's bounds. A fixed editor takes its own
    /// size only. The aspect is a hint (`adjust_size` gives sizes that keep
    /// it), so a size inside the bounds with another shape is taken too.
    unsafe extern "C" fn set_size(plugin: *const clap_plugin, width: u32, height: u32) -> bool {
        guarded(std::ptr::null(), false, || {
            let Some(inst) = instance(plugin) else { return false };
            let Some(gui) = inst.gui.as_ref() else { return false };
            if !gui.view.size_range().contains(width, height) {
                return false;
            }
            gui.view.resize(width, height);
            true
        })
    }

    unsafe extern "C" fn set_parent(plugin: *const clap_plugin, window: *const clap_window) -> bool {
        guarded(std::ptr::null(), false, || {
            let Some(gui) = instance(plugin).and_then(|inst| inst.gui.as_ref()) else { return false };
            if window.is_null() {
                return false;
            }
            unsafe { gui.view.attach((*window).ptr) }
        })
    }

    unsafe extern "C" fn set_transient(_plugin: *const clap_plugin, _window: *const clap_window) -> bool {
        false
    }

    unsafe extern "C" fn suggest_title(_plugin: *const clap_plugin, _title: *const c_char) {}

    unsafe extern "C" fn show(plugin: *const clap_plugin) -> bool {
        guarded(std::ptr::null(), false, || {
            match instance(plugin).and_then(|inst| inst.gui.as_ref()) {
                Some(gui) => {
                    gui.view.set_hidden(false);
                    true
                }
                None => false,
            }
        })
    }

    unsafe extern "C" fn hide(plugin: *const clap_plugin) -> bool {
        guarded(std::ptr::null(), false, || {
            match instance(plugin).and_then(|inst| inst.gui.as_ref()) {
                Some(gui) => {
                    gui.view.set_hidden(true);
                    true
                }
                None => false,
            }
        })
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

#[cfg(target_os = "linux")]
#[path = "clap_gui_x11.rs"]
mod gui;

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
            ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                $crate::clap::set_creator(__openplugin_clap_create);
                $crate::clap::set_editor($editor);
                true
            }))
            .unwrap_or(false)
        }

        #[unsafe(no_mangle)]
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
            ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                $crate::clap::set_creator(__openplugin_clap_create);
                true
            }))
            .unwrap_or(false)
        }

        #[unsafe(no_mangle)]
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

    static BED_LAYOUTS: &[openplugin_core::IoConfig] = &[
        openplugin_core::IoConfig::STEREO,
        // 5.1.2 uses top side channels CLAP has no ids for.
        openplugin_core::IoConfig {
            input: openplugin_core::SpeakerLayout::Surround512,
            output: openplugin_core::SpeakerLayout::Surround512,
            sidechain: None,
        },
        openplugin_core::IoConfig {
            input: openplugin_core::SpeakerLayout::Surround714,
            output: openplugin_core::SpeakerLayout::Surround714,
            sidechain: None,
        },
        // An expressible main bus with a 9.1.6 sidechain is not expressible either.
        openplugin_core::IoConfig {
            input: openplugin_core::SpeakerLayout::Surround51,
            output: openplugin_core::SpeakerLayout::Surround51,
            sidechain: Some(openplugin_core::SpeakerLayout::Surround916),
        },
    ];

    struct Beds;

    impl Plugin for Beds {
        fn info(&self) -> &'static PluginInfo { &INFO }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, _: &str, _: f32) -> bool { false }
        fn param(&self, _: &str) -> Option<f32> { None }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
        fn supported_layouts(&self) -> &'static [openplugin_core::IoConfig] { BED_LAYOUTS }
    }

    #[test]
    fn layouts_clap_cannot_express_are_not_offered() {
        let adapter = ClapAdapter::new(Beds);
        let plugin = adapter.as_plugin();
        unsafe {
            let ext = |id: &CStr| (*plugin).get_extension.unwrap()(plugin, id.as_ptr());
            let configs = ext(c"clap.audio-ports-config") as *const clap_plugin_audio_ports_config;
            let info = ext(c"clap.audio-ports-config-info/1") as *const clap_plugin_audio_ports_config_info;
            // Four layouts, two offered: stereo (id 0) and 7.1.4 (id 2).
            assert_eq!((*configs).count.unwrap()(plugin), 2);
            let mut ids = Vec::new();
            for i in 0..2 {
                let mut config = std::mem::zeroed::<clap_audio_ports_config>();
                assert!((*configs).get.unwrap()(plugin, i, &mut config));
                ids.push(config.id);
                if i == 1 {
                    assert_eq!(config.main_output_channel_count, 12);
                }
            }
            assert_eq!(ids, [0, 2]);
            let mut config = std::mem::zeroed::<clap_audio_ports_config>();
            assert!(!(*configs).get.unwrap()(plugin, 2, &mut config));
            // The hidden ones cannot be selected or described by id.
            assert!(!(*configs).select.unwrap()(plugin, 1));
            assert!(!(*configs).select.unwrap()(plugin, 3));
            let mut port = std::mem::zeroed::<clap_audio_port_info>();
            assert!(!(*info).get.unwrap()(plugin, 1, 0, true, &mut port));
            assert!((*info).get.unwrap()(plugin, 2, 0, true, &mut port));
            assert!((*configs).select.unwrap()(plugin, 2));
            // The channel map is the CLAP one, in the SDK order.
            let surround = ext(c"clap.surround/4") as *const clap_plugin_surround;
            let mut map = [0u8; 16];
            assert_eq!((*surround).get_channel_map.unwrap()(plugin, false, 0, map.as_mut_ptr(), 16), 12);
            assert_eq!(Some(&map[..12]), openplugin_core::SpeakerLayout::Surround714.clap_map());
        }
        let bed = openplugin_core::SpeakerLayout::Surround512;
        assert!(bed.clap_map().is_none() && !layout_expressible(BED_LAYOUTS[1]) && !layout_expressible(BED_LAYOUTS[3]));
        assert!(layout_expressible(BED_LAYOUTS[0]) && layout_expressible(BED_LAYOUTS[2]));
    }


    // ---- A fake host with context-menu and track-info ----------------------

    struct FakeTrack {
        flags: u64,
        name: &'static str,
        colour: [u8; 4],
    }

    type Popup = (u32, u32, i32, i32, i32);

    struct FakeHost {
        track: Mutex<FakeTrack>,
        track_gets: AtomicU32,
        popups: Mutex<Vec<Popup>>,
        can_popup: AtomicBool,
        offer_menu: bool,
        offer_track: bool,
        draft_ids: bool,
        popup_result: AtomicBool,
    }

    impl FakeHost {
        fn new() -> Box<Self> {
            Box::new(Self {
                track: Mutex::new(FakeTrack { flags: 0, name: "", colour: [0; 4] }),
                track_gets: AtomicU32::new(0),
                popups: Mutex::new(Vec::new()),
                can_popup: AtomicBool::new(true),
                offer_menu: true,
                offer_track: true,
                draft_ids: false,
                popup_result: AtomicBool::new(true),
            })
        }

        fn set_track(&self, flags: u64, name: &'static str, colour: [u8; 4]) {
            *self.track.lock().unwrap() = FakeTrack { flags, name, colour };
        }

        fn clap_host(&self) -> clap_host {
            clap_host {
                clap_version: CLAP_VERSION,
                host_data: self as *const FakeHost as *mut c_void,
                name: c"fake".as_ptr(),
                vendor: c"fake".as_ptr(),
                url: c"".as_ptr(),
                version: c"1".as_ptr(),
                get_extension: Some(fake_extension),
                request_restart: None,
                request_process: None,
                request_callback: None,
            }
        }
    }

    fn fake<'a>(host: *const clap_host) -> &'a FakeHost {
        unsafe { &*((*host).host_data as *const FakeHost) }
    }

    unsafe extern "C" fn fake_track_get(host: *const clap_host, info: *mut clap_track_info) -> bool {
        let this = fake(host);
        this.track_gets.fetch_add(1, Ordering::Relaxed);
        let track = this.track.lock().unwrap();
        if track.flags == u64::MAX {
            return false;
        }
        let info = unsafe { &mut *info };
        info.flags = track.flags;
        copy_cstr(&mut info.name, track.name);
        info.color = clap_color { red: track.colour[0], green: track.colour[1], blue: track.colour[2], alpha: track.colour[3] };
        true
    }

    static FAKE_TRACK: clap_host_track_info = clap_host_track_info { get: Some(fake_track_get) };

    unsafe extern "C" fn fake_can_popup(host: *const clap_host) -> bool {
        fake(host).can_popup.load(Ordering::Relaxed)
    }

    unsafe extern "C" fn fake_popup(host: *const clap_host, target: *const clap_context_menu_target, screen: i32, x: i32, y: i32) -> bool {
        let this = fake(host);
        let target = unsafe { *target };
        this.popups.lock().unwrap().push((target.kind, target.id, screen, x, y));
        this.popup_result.load(Ordering::Relaxed)
    }

    static FAKE_MENU: clap_host_context_menu = clap_host_context_menu { populate: None, perform: None, can_popup: Some(fake_can_popup), popup: Some(fake_popup) };

    unsafe extern "C" fn fake_extension(host: *const clap_host, id: *const c_char) -> *const c_void {
        let this = fake(host);
        let id = unsafe { CStr::from_ptr(id) }.to_bytes_with_nul();
        let (menu, track) = if this.draft_ids { (EXT_CONTEXT_MENU_COMPAT, EXT_TRACK_INFO_COMPAT) } else { (EXT_CONTEXT_MENU, EXT_TRACK_INFO) };
        if this.offer_menu && id == menu {
            return &FAKE_MENU as *const clap_host_context_menu as *const c_void;
        }
        if this.offer_track && id == track {
            return &FAKE_TRACK as *const clap_host_track_info as *const c_void;
        }
        std::ptr::null()
    }

    /// A plugin that records what `track_info_changed` heard, and whether it overlapped `process`.
    struct Listener {
        heard: Arc<Mutex<Vec<TrackInfo>>>,
        in_process: Arc<AtomicBool>,
        overlapped: Arc<AtomicBool>,
    }

    impl Plugin for Listener {
        fn info(&self) -> &'static PluginInfo { &INFO }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, _: &str, _: f32) -> bool { false }
        fn param(&self, _: &str) -> Option<f32> { None }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize) {
            self.in_process.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_micros(200));
            self.in_process.store(false, Ordering::SeqCst);
        }
        fn track_info_changed(&mut self, info: &TrackInfo) {
            if self.in_process.load(Ordering::SeqCst) {
                self.overlapped.store(true, Ordering::SeqCst);
            }
            self.heard.lock().unwrap().push(info.clone());
        }
    }

    fn listener() -> (Listener, Arc<Mutex<Vec<TrackInfo>>>, Arc<AtomicBool>) {
        let (heard, overlapped) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
        (Listener { heard: heard.clone(), in_process: Arc::new(AtomicBool::new(false)), overlapped: overlapped.clone() }, heard, overlapped)
    }

    unsafe fn call_changed(plugin: *const clap_plugin) {
        unsafe {
            let ext = (*plugin).get_extension.unwrap()(plugin, c"clap.track-info/1".as_ptr()) as *const clap_plugin_track_info;
            assert!(!ext.is_null());
            (*ext).changed.unwrap()(plugin);
        }
    }

    #[test]
    fn track_info_is_read_at_init_and_again_on_changed() {
        let host = FakeHost::new();
        host.set_track(TRACK_INFO_HAS_TRACK_NAME | TRACK_INFO_HAS_TRACK_COLOR | TRACK_INFO_IS_FOR_RETURN_TRACK, "Vox", [0x11, 0x22, 0x33, 0xff]);
        let ch = host.clap_host();
        let (plugin, heard, overlapped) = listener();
        let adapter = unsafe { ClapAdapter::with_host(plugin, &ch) };
        let p = adapter.as_plugin();
        // Read at init: name, colour as 0xRRGGBBAA, kind from the return flag.
        assert_eq!(*heard.lock().unwrap(), vec![TrackInfo { name: Some("Vox".into()), colour: Some(0x1122_33ff), kind: TrackInfo::IS_RETURN }]);
        // The host renames the track and says so: read again and delivered.
        host.set_track(TRACK_INFO_HAS_TRACK_NAME | TRACK_INFO_IS_FOR_BUS | TRACK_INFO_IS_FOR_MASTER, "Drums", [9, 9, 9, 9]);
        unsafe { call_changed(p) };
        let got = heard.lock().unwrap().last().cloned().unwrap();
        assert_eq!(got, TrackInfo { name: Some("Drums".into()), colour: None, kind: TrackInfo::IS_BUS | TrackInfo::IS_MASTER });
        assert!(got.is_bus() && got.is_master() && !got.is_return());
        // The same answer again is not delivered twice.
        let before = heard.lock().unwrap().len();
        unsafe { call_changed(p) };
        assert_eq!(heard.lock().unwrap().len(), before);
        assert!(host.track_gets.load(Ordering::Relaxed) >= 3, "changed re-reads the host");
        // An empty name reads as no name; a host that refuses delivers nothing.
        host.set_track(TRACK_INFO_HAS_TRACK_NAME, "", [0; 4]);
        unsafe { call_changed(p) };
        assert_eq!(heard.lock().unwrap().last().unwrap().name, None);
        let count = heard.lock().unwrap().len();
        host.set_track(u64::MAX, "", [0; 4]);
        unsafe { call_changed(p) };
        assert_eq!(heard.lock().unwrap().len(), count);
        assert!(!overlapped.load(Ordering::SeqCst));
    }

    #[test]
    fn track_info_accepts_the_draft_id_and_no_host_at_all() {
        let mut host = FakeHost::new();
        host.draft_ids = true;
        host.set_track(TRACK_INFO_HAS_TRACK_NAME, "Draft", [0; 4]);
        let ch = host.clap_host();
        let (plugin, heard, _) = listener();
        let _adapter = unsafe { ClapAdapter::with_host(plugin, &ch) };
        assert_eq!(heard.lock().unwrap().len(), 1);
        assert_eq!(heard.lock().unwrap()[0].name.as_deref(), Some("Draft"));
        // A host without the extension, or none: nothing is delivered, nothing breaks.
        let mut bare = FakeHost::new();
        bare.offer_track = false;
        let ch = bare.clap_host();
        let (plugin, heard, _) = listener();
        let adapter = unsafe { ClapAdapter::with_host(plugin, &ch) };
        unsafe { call_changed(adapter.as_plugin()) };
        assert!(heard.lock().unwrap().is_empty());
        let (plugin, heard, _) = listener();
        let adapter = ClapAdapter::new(plugin);
        unsafe { call_changed(adapter.as_plugin()) };
        assert!(heard.lock().unwrap().is_empty());
    }

    #[test]
    fn track_info_never_overlaps_process() {
        let host = FakeHost::new();
        host.set_track(TRACK_INFO_HAS_TRACK_NAME, "A", [0; 4]);
        let ch = host.clap_host();
        let (plugin, heard, overlapped) = listener();
        let adapter = unsafe { ClapAdapter::with_host(plugin, &ch) };
        let p = adapter.as_plugin() as usize;
        unsafe {
            assert!(((*adapter.as_plugin()).activate.unwrap())(adapter.as_plugin(), 48_000.0, 1, 64));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let audio = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let plugin = p as *const clap_plugin;
                let (mut l, mut r) = ([0.0f32; 16], [0.0f32; 16]);
                let mut lp = [l.as_mut_ptr(), r.as_mut_ptr()];
                let mut rp = [l.as_mut_ptr(), r.as_mut_ptr()];
                let inputs = clap_audio_buffer { data32: lp.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 };
                let mut outputs = clap_audio_buffer { data32: rp.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 };
                let block = clap_process {
                    steady_time: 0,
                    frames_count: 16,
                    transport: std::ptr::null(),
                    audio_inputs: &inputs,
                    audio_outputs: &mut outputs,
                    audio_inputs_count: 1,
                    audio_outputs_count: 1,
                    in_events: std::ptr::null(),
                    out_events: std::ptr::null(),
                };
                while !stop.load(Ordering::Relaxed) {
                    unsafe { ((*plugin).process.unwrap())(plugin, &block) };
                }
            })
        };
        let names = ["B", "C", "D", "E", "F", "G", "H", "I", "J", "K"];
        for name in names {
            host.set_track(TRACK_INFO_HAS_TRACK_NAME, name, [0; 4]);
            unsafe { call_changed(adapter.as_plugin()) };
            std::thread::sleep(Duration::from_millis(2));
        }
        stop.store(true, Ordering::Relaxed);
        audio.join().unwrap();
        assert!(!overlapped.load(Ordering::SeqCst), "track info reached the plugin while process ran");
        assert_eq!(heard.lock().unwrap().len(), 1 + names.len());
    }

    #[test]
    fn editor_right_click_opens_the_host_menu_for_the_parameter() {
        let host = FakeHost::new();
        let ch = host.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Gain { db: 0.0, linear: 1.0 }, &ch) };
        let editor = adapter.editor_host().expect("editor host");
        // The host shows a menu: true, and it was asked for parameter 0 at the click.
        assert!(editor.context_menu(0, 12.4, 30.6));
        assert_eq!(*host.popups.lock().unwrap(), vec![(CONTEXT_MENU_TARGET_KIND_PARAM, 0, 0, 12, 31)]);
        // The host cannot pop up now: not asked, and the editor falls back to its own menu.
        host.can_popup.store(false, Ordering::Relaxed);
        assert!(!editor.context_menu(0, 1.0, 1.0));
        assert_eq!(host.popups.lock().unwrap().len(), 1);
        // The host declines: false as well.
        host.can_popup.store(true, Ordering::Relaxed);
        host.popup_result.store(false, Ordering::Relaxed);
        assert!(!editor.context_menu(0, 1.0, 1.0));
        assert_eq!(host.popups.lock().unwrap().len(), 2);
        // The plugin offers its side of the extension, with no items of its own.
        unsafe {
            let p = adapter.as_plugin();
            let ext = (*p).get_extension.unwrap()(p, c"clap.context-menu/1".as_ptr()) as *const clap_plugin_context_menu;
            assert!(!ext.is_null());
            assert!((*ext).populate.unwrap()(p, std::ptr::null(), std::ptr::null()));
            assert!(!(*ext).perform.unwrap()(p, std::ptr::null(), 0));
        }
        // The draft id works, and a host without the extension or with none is false.
        let mut draft = FakeHost::new();
        draft.draft_ids = true;
        let ch = draft.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Gain { db: 0.0, linear: 1.0 }, &ch) };
        assert!(adapter.editor_host().unwrap().context_menu(0, 1.0, 2.0));
        assert_eq!(draft.popups.lock().unwrap().len(), 1);
        let mut none = FakeHost::new();
        none.offer_menu = false;
        let ch = none.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Gain { db: 0.0, linear: 1.0 }, &ch) };
        assert!(!adapter.editor_host().unwrap().context_menu(0, 1.0, 2.0));
        assert!(!ClapAdapter::new(Gain { db: 0.0, linear: 1.0 }).editor_host().unwrap().context_menu(0, 1.0, 2.0));
    }

    #[test]
    fn clap_context_menu_and_track_info_structs_match_the_host_abi() {
        let ptr = std::mem::size_of::<usize>();
        assert_eq!(std::mem::size_of::<clap_context_menu_target>(), 8);
        assert_eq!(std::mem::size_of::<clap_host_context_menu>(), 4 * ptr);
        assert_eq!(std::mem::size_of::<clap_plugin_context_menu>(), 2 * ptr);
        assert_eq!(std::mem::size_of::<clap_context_menu_builder>(), 3 * ptr);
        assert_eq!(std::mem::size_of::<clap_color>(), 4);
        assert_eq!(std::mem::offset_of!(clap_track_info, name), 8);
        assert_eq!(std::mem::offset_of!(clap_track_info, color), 8 + NAME_SIZE);
        assert_eq!(std::mem::offset_of!(clap_track_info, audio_channel_count), 8 + NAME_SIZE + 4);
        assert_eq!(std::mem::size_of::<clap_track_info>(), 8 + NAME_SIZE + 8 + ptr);
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

    struct Grouped;

    static GROUPED_PARAMS: [ParamInfo; 4] = [
        ParamInfo::new("level", "Level", 0.0, 1.0, 0.5, Unit::None),
        ParamInfo::new("cutoff", "Cutoff", 20.0, 20_000.0, 1_000.0, Unit::Hz).with_group("Filter"),
        ParamInfo::new("attack", "Attack", 0.0, 1.0, 0.01, Unit::Seconds).with_group("Filter/Envelope"),
        ParamInfo::new("broken", "Broken", 0.0, 1.0, 0.0, Unit::None).with_group("Filter//Envelope"),
    ];

    static GROUPED_INFO: PluginInfo = PluginInfo { id: "com.opengrid.test.grouped", name: "Grouped", params: &GROUPED_PARAMS, ..INFO };

    impl Plugin for Grouped {
        fn info(&self) -> &'static PluginInfo { &GROUPED_INFO }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, id: &str, _: f32) -> bool { GROUPED_INFO.param(id).is_some() }
        fn param(&self, id: &str) -> Option<f32> { GROUPED_INFO.param(id).map(|p| p.default) }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
    }

    fn modules<P: Plugin>(adapter: &ClapAdapter<P>) -> Vec<String> {
        let plugin = adapter.as_plugin();
        unsafe {
            let params = plugin_extension(plugin, EXT_PARAMS.as_ptr() as *const c_char) as *const clap_plugin_params;
            (0..(*params).count.unwrap()(plugin))
                .map(|index| {
                    let mut info = std::mem::zeroed::<clap_param_info>();
                    info.module = [b'x' as c_char; PATH_SIZE];
                    assert!((*params).get_info.unwrap()(plugin, index, &mut info));
                    CStr::from_ptr(info.module.as_ptr()).to_str().unwrap().to_string()
                })
                .collect()
        }
    }

    #[test]
    fn param_info_carries_the_group_as_the_module_path() {
        let grouped = ClapAdapter::new(Grouped);
        // The invalid path is shown as the top level.
        assert_eq!(modules(&grouped), ["", "Filter", "Filter/Envelope", ""]);
        // Without groups every module is empty, as before groups existed.
        let plain = ClapAdapter::new(Gain { db: 0.0, linear: 1.0 });
        assert_eq!(modules(&plain), [""]);
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
        assert_eq!(std::mem::size_of::<clap_host_gui>(), 5 * std::mem::size_of::<usize>());
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
