//! A fake CLAP host for the change-notification tests: it counts
//! `request_restart`, `request_callback`, `latency.changed` and
//! `params.rescan`, and notes whether each main-thread call came from the
//! thread the test named its main thread. Nothing here allocates once built,
//! so the allocation test can call into it from `process`.
#![allow(dead_code)]

use std::cell::Cell;
use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use openplugin_targets::clap_abi::*;

thread_local! {
    static MAIN: Cell<bool> = const { Cell::new(false) };
}

/// Call on the thread that plays the host's main thread.
pub fn mark_main_thread() {
    MAIN.with(|main| main.set(true));
}

fn on_main() -> bool {
    MAIN.with(|main| main.get())
}

pub struct ChangeHost {
    pub restarts: AtomicU32,
    pub callbacks: AtomicU32,
    pub latency_changed: AtomicU32,
    pub rescans: AtomicU32,
    pub last_rescan_flags: AtomicU32,
    /// Set when `latency.changed` or `params.rescan` ran off the main thread.
    pub off_main: AtomicBool,
    /// Set when `latency.changed` ran outside `activate`.
    pub changed_outside_activate: AtomicBool,
    pub in_activate: AtomicBool,
    pub offer_callback: bool,
}

impl ChangeHost {
    pub fn new(offer_callback: bool) -> Box<Self> {
        Box::new(Self {
            restarts: AtomicU32::new(0),
            callbacks: AtomicU32::new(0),
            latency_changed: AtomicU32::new(0),
            rescans: AtomicU32::new(0),
            last_rescan_flags: AtomicU32::new(0),
            off_main: AtomicBool::new(false),
            changed_outside_activate: AtomicBool::new(false),
            in_activate: AtomicBool::new(false),
            offer_callback,
        })
    }

    pub fn clap_host(&self) -> clap_host {
        clap_host {
            clap_version: CLAP_VERSION,
            host_data: self as *const ChangeHost as *mut c_void,
            name: c"change host".as_ptr(),
            vendor: c"test".as_ptr(),
            url: c"".as_ptr(),
            version: c"1".as_ptr(),
            get_extension: Some(extension),
            request_restart: Some(request_restart),
            request_process: None,
            request_callback: if self.offer_callback { Some(request_callback) } else { None },
        }
    }

    pub fn count(&self, counter: &AtomicU32) -> u32 {
        counter.load(Ordering::SeqCst)
    }
}

fn host<'a>(host: *const clap_host) -> &'a ChangeHost {
    unsafe { &*((*host).host_data as *const ChangeHost) }
}

unsafe extern "C" fn request_restart(h: *const clap_host) {
    host(h).restarts.fetch_add(1, Ordering::SeqCst);
}

unsafe extern "C" fn request_callback(h: *const clap_host) {
    host(h).callbacks.fetch_add(1, Ordering::SeqCst);
}

unsafe extern "C" fn latency_changed(h: *const clap_host) {
    let this = host(h);
    this.latency_changed.fetch_add(1, Ordering::SeqCst);
    if !on_main() {
        this.off_main.store(true, Ordering::SeqCst);
    }
    if !this.in_activate.load(Ordering::SeqCst) {
        this.changed_outside_activate.store(true, Ordering::SeqCst);
    }
}

unsafe extern "C" fn rescan(h: *const clap_host, flags: u32) {
    let this = host(h);
    this.rescans.fetch_add(1, Ordering::SeqCst);
    this.last_rescan_flags.store(flags, Ordering::SeqCst);
    if !on_main() {
        this.off_main.store(true, Ordering::SeqCst);
    }
}

static LATENCY: clap_host_latency = clap_host_latency { changed: Some(latency_changed) };
static PARAMS: clap_host_params = clap_host_params { rescan: Some(rescan), clear: None, request_flush: None };

unsafe extern "C" fn extension(_host: *const clap_host, id: *const c_char) -> *const c_void {
    let id = unsafe { CStr::from_ptr(id) }.to_bytes_with_nul();
    if id == EXT_LATENCY {
        return &LATENCY as *const clap_host_latency as *const c_void;
    }
    if id == EXT_PARAMS {
        return &PARAMS as *const clap_host_params as *const c_void;
    }
    std::ptr::null()
}

/// One parameter-value event, as a host hands it to `process`.
pub struct Events {
    events: Vec<clap_event_param_value>,
    list: clap_input_events,
}

unsafe extern "C" fn events_size(list: *const clap_input_events) -> u32 {
    unsafe { &*((*list).ctx as *const Vec<clap_event_param_value>) }.len() as u32
}

unsafe extern "C" fn events_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let events = unsafe { &*((*list).ctx as *const Vec<clap_event_param_value>) };
    events.get(index as usize).map(|event| &event.header as *const clap_event_header).unwrap_or(std::ptr::null())
}

impl Events {
    pub fn new(values: &[(u32, f64)]) -> Box<Self> {
        let events = values
            .iter()
            .map(|&(param_id, value)| clap_event_param_value {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_value>() as u32,
                    time: 0,
                    space_id: 0,
                    type_: EVENT_PARAM_VALUE,
                    flags: 0,
                },
                param_id,
                cookie: std::ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value,
            })
            .collect();
        let mut boxed = Box::new(Self {
            events,
            list: clap_input_events { ctx: std::ptr::null_mut(), size: Some(events_size), get: Some(events_get) },
        });
        boxed.list.ctx = &boxed.events as *const Vec<clap_event_param_value> as *mut c_void;
        boxed
    }

    pub fn list(&self) -> *const clap_input_events {
        &self.list
    }
}

/// Stereo audio for `process`, in place, built before any measured section.
pub struct Audio {
    left: Vec<f32>,
    right: Vec<f32>,
    pointers: [*mut f32; 2],
    input: clap_audio_buffer,
    output: clap_audio_buffer,
}

impl Audio {
    pub fn new(frames: usize) -> Box<Self> {
        let mut audio = Box::new(Self {
            left: vec![0.0; frames],
            right: vec![0.0; frames],
            pointers: [std::ptr::null_mut(); 2],
            input: clap_audio_buffer { data32: std::ptr::null_mut(), data64: std::ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 },
            output: clap_audio_buffer { data32: std::ptr::null_mut(), data64: std::ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 },
        });
        audio.pointers = [audio.left.as_mut_ptr(), audio.right.as_mut_ptr()];
        audio.input.data32 = audio.pointers.as_mut_ptr();
        audio.output.data32 = audio.pointers.as_mut_ptr();
        audio
    }

    /// One block of `process` with `events` (or none).
    ///
    /// # Safety
    /// `plugin` is an activated CLAP plugin.
    pub unsafe fn run(&mut self, plugin: *const clap_plugin, events: Option<&Events>) -> i32 {
        let block = clap_process {
            steady_time: 0,
            frames_count: self.left.len() as u32,
            transport: std::ptr::null(),
            audio_inputs: &self.input,
            audio_outputs: &mut self.output,
            audio_inputs_count: 1,
            audio_outputs_count: 1,
            in_events: events.map(|e| e.list()).unwrap_or(std::ptr::null()),
            out_events: std::ptr::null(),
        };
        unsafe { (*plugin).process.unwrap()(plugin, &block) }
    }
}

// The raw pointers address this value's own buffers.
unsafe impl Send for Audio {}
unsafe impl Send for Events {}
unsafe impl Sync for Events {}
