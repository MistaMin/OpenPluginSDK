//! Host bypass through the exported `clap_plugin`, driven like a host.
//!
//! The plugin has a `ParamFlags::BYPASS` parameter, reports a latency of
//! [`LATENCY`] samples, and keeps it when bypassed. Its processed path is
//! `0.5 * x` delayed by the latency. Its bypassed path is [`SoftBypass`]: the
//! input delayed by the same latency, after a 10 ms crossfade.
//!
//! Proven here:
//! - the bypass parameter is reported with `PARAM_IS_BYPASS` and the others
//!   are not;
//! - a parameter event on it reaches `Plugin::set_host_bypass`, at its sample
//!   offset, from `process` and from `params.flush`;
//! - bypassed output is the input delayed by the reported latency, and
//!   switching back restores the processed signal;
//! - reported latency does not change with bypass;
//! - bypass survives a state save and load into a fresh instance.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use openplugin_core::{Category, IoConfig, ParamFlags, ParamInfo, Plugin, PluginInfo, Unit, STEREO_LAYOUTS};
use openplugin_dsp::SoftBypass;
use openplugin_targets::clap_abi::*;
use openplugin_targets::ClapAdapter;

const LATENCY: usize = 64;
const RATE: f64 = 48_000.0;
const BLOCK: usize = 256;
/// The crossfade is 10 ms, 480 samples at 48 kHz. Four blocks is 1024.
const SETTLE_BLOCKS: usize = 4;
const WET_GAIN: f32 = 0.5;

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("level", "Level", 0.0, 1.0, 1.0, Unit::None),
    ParamInfo::new("bypass", "Bypass", 0.0, 1.0, 0.0, Unit::Toggle).flag(ParamFlags::BYPASS),
];
const BYPASS_ID: u32 = 1;

static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.test.bypass",
    name: "Bypass Proof",
    vendor: "OpenGrid",
    url: "https://github.com/MistaMin/OpenPluginSDK",
    email: "contact@mistamin.com",
    version: "0.0.1",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

/// What the plugin saw, shared with the test.
#[derive(Default)]
struct Log {
    /// `(bypassed, frames processed in the current block before the call)`.
    bypass_calls: Vec<(bool, usize)>,
    frames_this_block: usize,
}

struct Delayed {
    log: Arc<Mutex<Log>>,
    bypass: SoftBypass,
    bypassed: bool,
    ring: Vec<Vec<f32>>,
    write: usize,
    dry: Vec<Vec<f32>>,
    level: f32,
}

impl Delayed {
    fn new(log: Arc<Mutex<Log>>) -> Self {
        Self {
            log,
            bypass: SoftBypass::prepare(2, LATENCY, 48_000.0),
            bypassed: false,
            ring: vec![vec![0.0; LATENCY + 1]; 2],
            write: 0,
            dry: vec![Vec::new(); 2],
            level: 1.0,
        }
    }
}

impl Plugin for Delayed {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, _channels: usize) {
        self.bypass = SoftBypass::prepare(2, LATENCY, sample_rate);
        self.bypass.set_latency(LATENCY);
        self.bypass.set_bypassed(self.bypassed);
        self.bypass.snap();
        self.dry = vec![vec![0.0; max_block]; 2];
        self.reset();
    }

    fn reset(&mut self) {
        self.bypass.reset();
        for ring in &mut self.ring {
            ring.fill(0.0);
        }
        self.write = 0;
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "level" => {
                self.level = PARAMS[0].clamp(value);
                true
            }
            // The host bypass reaches `set_host_bypass`; the bridge calls it.
            "bypass" => true,
            _ => false,
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "level" => Some(self.level),
            "bypass" => Some(if self.bypassed { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let length = self.ring[0].len();
        for (channel, data) in io.iter_mut().enumerate().take(2) {
            self.dry[channel][..frames].copy_from_slice(&data[..frames]);
            let ring = &mut self.ring[channel];
            let mut write = self.write;
            for sample in data.iter_mut().take(frames) {
                ring[write] = *sample;
                let read = (write + length - LATENCY) % length;
                *sample = ring[read] * WET_GAIN * self.level;
                write = (write + 1) % length;
            }
        }
        self.write = (self.write + frames) % length;
        let dry: Vec<&[f32]> = self.dry.iter().map(|c| &c[..frames]).collect();
        let mut wet: Vec<&mut [f32]> = io.iter_mut().map(|c| &mut c[..frames]).collect();
        self.bypass.mix(&dry, &mut wet);
        self.log.lock().unwrap().frames_this_block += frames;
    }

    fn latency(&self) -> usize {
        LATENCY
    }

    fn set_host_bypass(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
        self.bypass.set_bypassed(bypassed);
        let mut log = self.log.lock().unwrap();
        let at = log.frames_this_block;
        log.bypass_calls.push((bypassed, at));
    }

    fn keeps_latency_when_bypassed(&self) -> bool {
        true
    }

    fn supported_layouts(&self) -> &'static [IoConfig] {
        STEREO_LAYOUTS
    }
}

// ---- host side -------------------------------------------------------------

#[repr(C, align(8))]
struct Slot([u8; 64]);

struct Events {
    slots: Vec<Slot>,
}

unsafe extern "C" fn list_size(list: *const clap_input_events) -> u32 {
    unsafe { &*((*list).ctx as *const Events) }.slots.len() as u32
}

unsafe extern "C" fn list_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let events = unsafe { &*((*list).ctx as *const Events) };
    events
        .slots
        .get(index as usize)
        .map_or(std::ptr::null(), |slot| slot.0.as_ptr() as *const clap_event_header)
}

fn bypass_event(time: u32, value: f64) -> Slot {
    let event = clap_event_param_value {
        header: clap_event_header {
            size: std::mem::size_of::<clap_event_param_value>() as u32,
            time,
            space_id: 0,
            type_: EVENT_PARAM_VALUE,
            flags: 0,
        },
        param_id: BYPASS_ID,
        cookie: std::ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        value,
    };
    let mut slot = Slot([0; 64]);
    unsafe {
        std::ptr::copy_nonoverlapping(
            &event as *const _ as *const u8,
            slot.0.as_mut_ptr(),
            std::mem::size_of::<clap_event_param_value>(),
        );
    }
    slot
}

/// The deterministic test signal: two incommensurate tones per channel, so a
/// wrong delay cannot match by accident.
fn signal(channel: usize, n: usize) -> f32 {
    let t = n as f32;
    let (a, b) = if channel == 0 { (0.071, 0.0173) } else { (0.043, 0.0291) };
    0.6 * (t * a).sin() + 0.3 * (t * b + 1.0).sin()
}

fn wet(channel: usize, n: usize) -> f32 {
    if n < LATENCY { 0.0 } else { signal(channel, n - LATENCY) * WET_GAIN }
}

fn dry(channel: usize, n: usize) -> f32 {
    if n < LATENCY { 0.0 } else { signal(channel, n - LATENCY) }
}

struct Host {
    adapter: ClapAdapter<Delayed>,
    log: Arc<Mutex<Log>>,
    /// Absolute sample position of the next block.
    position: usize,
    output: [Vec<f32>; 2],
}

impl Host {
    fn new() -> Self {
        let log = Arc::new(Mutex::new(Log::default()));
        let adapter = ClapAdapter::new(Delayed::new(log.clone()));
        let plugin = adapter.as_plugin();
        let activate = unsafe { (*plugin).activate }.unwrap();
        assert!(unsafe { activate(plugin, RATE, 1, BLOCK as u32) });
        Self { adapter, log, position: 0, output: [Vec::new(), Vec::new()] }
    }

    fn plugin(&self) -> *const clap_plugin {
        self.adapter.as_plugin()
    }

    fn extension<T>(&self, id: &std::ffi::CStr) -> *const T {
        let get = unsafe { (*self.plugin()).get_extension }.unwrap();
        let raw = unsafe { get(self.plugin(), id.as_ptr()) };
        assert!(!raw.is_null(), "extension {id:?} missing");
        raw as *const T
    }

    fn latency(&self) -> u32 {
        let ext = self.extension::<clap_plugin_latency>(c"clap.latency");
        unsafe { ((*ext).get.unwrap())(self.plugin()) }
    }

    fn param_value(&self, id: u32) -> f64 {
        let ext = self.extension::<clap_plugin_params>(c"clap.params");
        let mut value = f64::NAN;
        assert!(unsafe { ((*ext).get_value.unwrap())(self.plugin(), id, &mut value) });
        value
    }

    fn last_bypass_call(&self) -> Option<bool> {
        self.log.lock().unwrap().bypass_calls.last().map(|call| call.0)
    }

    /// One block with `events`.
    fn block(&mut self, events: Vec<Slot>) {
        let start = self.position;
        let mut input: Vec<Vec<f32>> = (0..2)
            .map(|ch| (0..BLOCK).map(|i| signal(ch, start + i)).collect())
            .collect();
        let mut output: Vec<Vec<f32>> = vec![vec![0.0; BLOCK]; 2];
        let mut in_ptrs: Vec<*mut f32> = input.iter_mut().map(|c| c.as_mut_ptr()).collect();
        let mut out_ptrs: Vec<*mut f32> = output.iter_mut().map(|c| c.as_mut_ptr()).collect();
        let inputs = [clap_audio_buffer {
            data32: in_ptrs.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        }];
        let mut outputs = [clap_audio_buffer {
            data32: out_ptrs.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        }];
        let events = Events { slots: events };
        let in_events = clap_input_events {
            ctx: &events as *const Events as *mut c_void,
            size: Some(list_size),
            get: Some(list_get),
        };
        let process = clap_process {
            steady_time: start as i64,
            frames_count: BLOCK as u32,
            transport: std::ptr::null(),
            audio_inputs: inputs.as_ptr(),
            audio_outputs: outputs.as_mut_ptr(),
            audio_inputs_count: 1,
            audio_outputs_count: 1,
            in_events: &in_events,
            out_events: std::ptr::null(),
        };
        self.log.lock().unwrap().frames_this_block = 0;
        let result = unsafe { ((*self.plugin()).process.unwrap())(self.plugin(), &process) };
        assert_eq!(result, PROCESS_CONTINUE);
        for (kept, block) in self.output.iter_mut().zip(&output) {
            kept.extend_from_slice(block);
        }
        self.position += BLOCK;
    }

    fn blocks(&mut self, count: usize) {
        for _ in 0..count {
            self.block(Vec::new());
        }
    }

    /// The last `blocks` blocks of output against `expected(channel, n)` for
    /// absolute sample `n`.
    fn assert_last(&self, blocks: usize, what: &str, expected: fn(usize, usize) -> f32) {
        let end = self.position;
        for ch in 0..2 {
            for n in end - blocks * BLOCK..end {
                let got = self.output[ch][n];
                let want = expected(ch, n);
                assert!(
                    (got - want).abs() < 1e-5,
                    "{what}: channel {ch} sample {n}: got {got}, want {want}"
                );
            }
        }
    }
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

// ---- tests -----------------------------------------------------------------

#[test]
fn only_the_bypass_parameter_carries_the_bypass_flag() {
    let host = Host::new();
    let ext = host.extension::<clap_plugin_params>(c"clap.params");
    let count = unsafe { ((*ext).count.unwrap())(host.plugin()) };
    assert_eq!(count, 2);
    for index in 0..count {
        let mut info: clap_param_info = unsafe { std::mem::zeroed() };
        assert!(unsafe { ((*ext).get_info.unwrap())(host.plugin(), index, &mut info) });
        let flagged = info.flags & PARAM_IS_BYPASS != 0;
        assert_eq!(flagged, index == BYPASS_ID, "parameter {index}");
        if flagged {
            assert_ne!(info.flags & PARAM_IS_STEPPED, 0, "a toggle is stepped");
            assert_eq!(info.max_value, 1.0);
        }
    }
}

#[test]
fn bypass_event_reaches_the_plugin_at_its_sample_offset() {
    let mut host = Host::new();
    let before = host.log.lock().unwrap().bypass_calls.len();
    host.block(vec![bypass_event(100, 1.0)]);
    let log = host.log.lock().unwrap();
    let calls = &log.bypass_calls[before..];
    assert!(
        calls.contains(&(true, 100)),
        "set_host_bypass(true) after exactly 100 frames, saw {calls:?}"
    );
    drop(log);
    assert_eq!(host.param_value(BYPASS_ID), 1.0);
}

#[test]
fn bypass_event_through_params_flush_reaches_the_plugin() {
    let host = Host::new();
    let ext = host.extension::<clap_plugin_params>(c"clap.params");
    let events = Events { slots: vec![bypass_event(0, 1.0)] };
    let in_events = clap_input_events {
        ctx: &events as *const Events as *mut c_void,
        size: Some(list_size),
        get: Some(list_get),
    };
    unsafe { ((*ext).flush.unwrap())(host.plugin(), &in_events, std::ptr::null()) };
    assert_eq!(host.last_bypass_call(), Some(true));
    assert_eq!(host.param_value(BYPASS_ID), 1.0);
}

#[test]
fn bypassed_output_is_the_input_delayed_by_the_reported_latency() {
    let mut host = Host::new();
    assert_eq!(host.latency() as usize, LATENCY);
    host.blocks(SETTLE_BLOCKS);
    host.assert_last(1, "processed", wet);
    host.block(vec![bypass_event(0, 1.0)]);
    host.blocks(SETTLE_BLOCKS);
    host.assert_last(2, "bypassed", dry);
}

#[test]
fn switching_back_restores_the_processed_signal() {
    let mut host = Host::new();
    host.block(vec![bypass_event(0, 1.0)]);
    host.blocks(SETTLE_BLOCKS);
    host.assert_last(1, "bypassed", dry);
    host.block(vec![bypass_event(0, 0.0)]);
    host.blocks(SETTLE_BLOCKS);
    host.assert_last(2, "restored", wet);
    assert_eq!(host.param_value(BYPASS_ID), 0.0);
}

#[test]
fn latency_does_not_change_with_bypass() {
    let mut host = Host::new();
    let before = host.latency();
    assert_eq!(before as usize, LATENCY);
    host.block(vec![bypass_event(0, 1.0)]);
    host.blocks(SETTLE_BLOCKS);
    assert_eq!(host.latency(), before, "bypassed");
    host.block(vec![bypass_event(0, 0.0)]);
    host.blocks(SETTLE_BLOCKS);
    assert_eq!(host.latency(), before, "restored");
}

#[test]
fn bypass_survives_state_save_and_load() {
    let mut first = Host::new();
    first.block(vec![bypass_event(0, 1.0)]);
    first.blocks(SETTLE_BLOCKS);

    let mut bytes = Bytes { data: Vec::new(), read: 0 };
    let state = first.extension::<clap_plugin_state>(c"clap.state");
    let out = clap_ostream { ctx: &mut bytes as *mut Bytes as *mut c_void, write: Some(write_bytes) };
    assert!(unsafe { ((*state).save.unwrap())(first.plugin(), &out) });
    assert!(!bytes.data.is_empty());

    // A fresh instance starts un-bypassed, then takes the state.
    let mut second = Host::new();
    assert_eq!(second.param_value(BYPASS_ID), 0.0);
    let state = second.extension::<clap_plugin_state>(c"clap.state");
    let input = clap_istream { ctx: &mut bytes as *mut Bytes as *mut c_void, read: Some(read_bytes) };
    assert!(unsafe { ((*state).load.unwrap())(second.plugin(), &input) });
    assert_eq!(second.param_value(BYPASS_ID), 1.0, "the host reads the loaded value");
    // No bypass event follows the load. The next blocks are bypassed anyway.
    second.blocks(SETTLE_BLOCKS + 1);
    assert_eq!(second.last_bypass_call(), Some(true));
    second.assert_last(1, "bypassed after load", dry);
}
