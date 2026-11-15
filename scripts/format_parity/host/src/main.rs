//! Rust side of the format parity test (see docs/FORMAT_PARITY.md).
//!
//! Subcommands:
//!   gen <scenario>                      write the shared scenario
//!   reference <scenario> <out> [late]   render with the Rust engine directly; `late`
//!                                       applies every event that many frames late
//!                                       (the timing negative control)
//!   clap <scenario> <out> <binary>      render through a built .clap
//!   lv2 <scenario> <out> <binary>       render through a built .lv2
//!   compare <ref> <out> <tol> <label>   exit 0 match, 1 mismatch, 2 error
//!
//! Scenario file (little endian): `OPFP`, version 1, frames u32, block u32,
//! rate f64, event count u32, then events {frame u32, param u32, value f32}
//! (param 0 is Gain in dB, 1 is Warm), then the left and right input, frames
//! f32 each. An output file is the left then the right channel, f32.

use std::ffi::{c_char, c_void, CStr, CString};
use std::fs;
use std::process::exit;

use example_editor::EditorGain;
use openplugin::prelude::Plugin;
use openplugin::targets::clap_abi::*;
use openplugin::targets::lv2::Lv2Descriptor;

const RATE: f64 = 48_000.0;
const FRAMES: usize = 4096;
/// The host block size of every format.
const BLOCK: usize = 512;
const GAIN: u32 = 0;
const WARM: u32 = 1;
const PARAM_NAMES: [&str; 2] = ["Gain", "Warm"];

#[derive(Clone, Copy)]
struct Event {
    frame: u32,
    param: u32,
    value: f32,
}

struct Scenario {
    rate: f64,
    block: usize,
    events: Vec<Event>,
    input: [Vec<f32>; 2],
}

fn fail(message: &str) -> ! {
    eprintln!("parity_host: {message}");
    exit(2);
}

/// The automation: a mid-block gain change (frame 700 is offset 188 of block
/// 1), a mid-block Warm switch, a change on a block edge (2048), a late one.
fn events() -> Vec<Event> {
    let e = |frame, param, value| Event { frame, param, value };
    vec![
        e(0, GAIN, -3.0),
        e(0, WARM, 0.0),
        e(700, GAIN, 6.0),
        e(1800, WARM, 1.0),
        e(2048, GAIN, 2.0),
        e(2600, GAIN, -9.5),
        e(3300, GAIN, 12.0),
        e(3300, WARM, 0.0),
    ]
}

/// A tone per channel plus deterministic noise, so no sample repeats and a
/// misplaced change cannot hide. Peak 0.55 before gain.
fn input() -> [Vec<f32>; 2] {
    let mut state = 0x1234_5678u32;
    let mut noise = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    };
    let tau = std::f64::consts::TAU;
    let left = (0..FRAMES)
        .map(|n| (0.45 * (tau * 997.0 * n as f64 / RATE).sin()) as f32 + 0.1 * noise())
        .collect();
    let right = (0..FRAMES)
        .map(|n| (0.3 * (tau * 1523.0 * n as f64 / RATE + 0.5).sin()) as f32 + 0.1 * noise())
        .collect();
    [left, right]
}

fn write_scenario(path: &str) {
    let events = events();
    let [left, right] = input();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"OPFP");
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&(FRAMES as u32).to_le_bytes());
    bytes.extend_from_slice(&(BLOCK as u32).to_le_bytes());
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(events.len() as u32).to_le_bytes());
    for e in &events {
        bytes.extend_from_slice(&e.frame.to_le_bytes());
        bytes.extend_from_slice(&e.param.to_le_bytes());
        bytes.extend_from_slice(&e.value.to_le_bytes());
    }
    for v in left.iter().chain(&right) {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(path, bytes).unwrap_or_else(|e| fail(&format!("write {path}: {e}")));
}

fn read_scenario(path: &str) -> Scenario {
    let b = fs::read(path).unwrap_or_else(|e| fail(&format!("read {path}: {e}")));
    let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    if &b[0..4] != b"OPFP" || u32_at(4) != 1 {
        fail("not a scenario file");
    }
    let frames = u32_at(8) as usize;
    let block = u32_at(12) as usize;
    let rate = f64::from_le_bytes(b[16..24].try_into().unwrap());
    let count = u32_at(24) as usize;
    let mut o = 28;
    let mut events = Vec::new();
    for _ in 0..count {
        events.push(Event { frame: u32_at(o), param: u32_at(o + 4), value: f32::from_bits(u32_at(o + 8)) });
        o += 12;
    }
    let channel = |o: &mut usize| -> Vec<f32> {
        let v = (0..frames).map(|i| f32::from_bits(u32_at(*o + 4 * i))).collect();
        *o += 4 * frames;
        v
    };
    let left = channel(&mut o);
    let right = channel(&mut o);
    Scenario { rate, block, events, input: [left, right] }
}

fn write_output(path: &str, out: &[Vec<f32>; 2]) {
    let mut bytes = Vec::new();
    for v in out[0].iter().chain(&out[1]) {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(path, bytes).unwrap_or_else(|e| fail(&format!("write {path}: {e}")));
}

/// Frames at which a format that applies changes only between blocks must
/// start a new sub-block: every event frame, and every host block edge.
fn cut_points(s: &Scenario) -> Vec<usize> {
    let frames = s.input[0].len();
    let mut cuts: Vec<usize> = (0..frames).step_by(s.block).collect();
    cuts.extend(s.events.iter().map(|e| e.frame as usize).filter(|&f| f < frames));
    cuts.push(frames);
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

// ---- reference: the plugin itself, no adapter ------------------------------

fn reference(s: &Scenario, late: u32) -> [Vec<f32>; 2] {
    let frames = s.input[0].len();
    let mut s = Scenario { rate: s.rate, block: s.block, events: s.events.clone(), input: s.input.clone() };
    for e in &mut s.events {
        e.frame = e.frame.saturating_add(late);
    }
    let s = &s;
    let mut plugin = EditorGain::default();
    plugin.prepare(s.rate as f32, s.block, 2);
    let mut out = [vec![0.0; frames], vec![0.0; frames]];
    let cuts = cut_points(s);
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        for e in s.events.iter().filter(|e| e.frame as usize == a) {
            assert!(plugin.set_param(["gain", "warm"][e.param as usize], e.value));
        }
        let mut io = vec![s.input[0][a..b].to_vec(), s.input[1][a..b].to_vec()];
        plugin.process(&mut io, b - a);
        out[0][a..b].copy_from_slice(&io[0]);
        out[1][a..b].copy_from_slice(&io[1]);
    }
    out
}

// ---- dlopen ---------------------------------------------------------------

unsafe extern "C" {
    fn dlopen(path: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

fn open_library(path: &str) -> *mut c_void {
    let c = CString::new(path).unwrap();
    // RTLD_NOW | RTLD_LOCAL
    let h = unsafe { dlopen(c.as_ptr(), 2 | 4) };
    if h.is_null() {
        let err = unsafe { CStr::from_ptr(dlerror()) }.to_string_lossy().into_owned();
        fail(&format!("dlopen {path}: {err}"));
    }
    h
}

fn symbol(handle: *mut c_void, name: &str) -> *mut c_void {
    let c = CString::new(name).unwrap();
    let p = unsafe { dlsym(handle, c.as_ptr()) };
    if p.is_null() {
        fail(&format!("symbol {name} not found"));
    }
    p
}

// ---- CLAP ------------------------------------------------------------------

#[repr(C, align(8))]
struct Slot([u8; 64]);

struct Events(Vec<Slot>);

unsafe extern "C" fn list_size(list: *const clap_input_events) -> u32 {
    unsafe { &*((*list).ctx as *const Events) }.0.len() as u32
}

unsafe extern "C" fn list_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let events = unsafe { &*((*list).ctx as *const Events) };
    events.0.get(index as usize).map_or(std::ptr::null(), |s| s.0.as_ptr() as *const clap_event_header)
}

fn param_event(time: u32, id: u32, value: f64) -> Slot {
    let event = clap_event_param_value {
        header: clap_event_header {
            size: std::mem::size_of::<clap_event_param_value>() as u32,
            time,
            space_id: 0,
            type_: EVENT_PARAM_VALUE,
            flags: 0,
        },
        param_id: id,
        cookie: std::ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        value,
    };
    let mut slot = Slot([0; 64]);
    unsafe {
        std::ptr::copy_nonoverlapping(&event as *const _ as *const u8, slot.0.as_mut_ptr(), std::mem::size_of::<clap_event_param_value>());
    }
    slot
}

fn cstr(chars: &[c_char]) -> String {
    unsafe { CStr::from_ptr(chars.as_ptr()) }.to_string_lossy().into_owned()
}

unsafe extern "C" fn no_extension(_: *const clap_host, _: *const c_char) -> *const c_void {
    std::ptr::null()
}
unsafe extern "C" fn no_request(_: *const clap_host) {}

fn clap(s: &Scenario, binary: &str) -> [Vec<f32>; 2] {
    let handle = open_library(binary);
    let entry = symbol(handle, "clap_entry") as *const clap_plugin_entry;
    let host = clap_host {
        clap_version: CLAP_VERSION,
        host_data: std::ptr::null_mut(),
        name: c"parity".as_ptr(),
        vendor: c"parity".as_ptr(),
        url: c"".as_ptr(),
        version: c"1".as_ptr(),
        get_extension: Some(no_extension),
        request_restart: Some(no_request),
        request_process: Some(no_request),
        request_callback: Some(no_request),
    };
    unsafe {
        let path = CString::new(binary).unwrap();
        assert!(((*entry).init.unwrap())(path.as_ptr()), "clap_entry.init");
        let factory = ((*entry).get_factory.unwrap())(FACTORY_ID.as_ptr() as *const c_char) as *const clap_plugin_factory;
        assert!(!factory.is_null(), "no plugin factory");
        assert_eq!(((*factory).get_plugin_count.unwrap())(factory), 1);
        let desc = ((*factory).get_plugin_descriptor.unwrap())(factory, 0);
        let plugin = ((*factory).create_plugin.unwrap())(factory, &host, (*desc).id);
        assert!(!plugin.is_null(), "create_plugin");
        assert!(((*plugin).init.unwrap())(plugin));
        // Parameter ids by name, not by assumed position.
        let params = ((*plugin).get_extension.unwrap())(plugin, EXT_PARAMS.as_ptr() as *const c_char) as *const clap_plugin_params;
        assert!(!params.is_null());
        let mut ids = [u32::MAX; 2];
        for i in 0..((*params).count.unwrap())(plugin) {
            let mut info: clap_param_info = std::mem::zeroed();
            assert!(((*params).get_info.unwrap())(plugin, i, &mut info));
            let name = cstr(&info.name);
            if let Some(k) = PARAM_NAMES.iter().position(|n| *n == name) {
                ids[k] = info.id;
            }
        }
        assert!(ids.iter().all(|&i| i != u32::MAX), "Gain and Warm not found");
        assert!(((*plugin).activate.unwrap())(plugin, s.rate, 1, s.block as u32));
        assert!(((*plugin).start_processing.unwrap())(plugin));

        let frames = s.input[0].len();
        let mut out = [vec![0.0f32; frames], vec![0.0f32; frames]];
        for start in (0..frames).step_by(s.block) {
            let end = (start + s.block).min(frames);
            let n = end - start;
            let mut input = [s.input[0][start..end].to_vec(), s.input[1][start..end].to_vec()];
            let mut output = [vec![0.0f32; n], vec![0.0f32; n]];
            let mut in_ptrs = [input[0].as_mut_ptr(), input[1].as_mut_ptr()];
            let mut out_ptrs = [output[0].as_mut_ptr(), output[1].as_mut_ptr()];
            let inputs = [clap_audio_buffer { data32: in_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 }];
            let mut outputs = [clap_audio_buffer { data32: out_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 }];
            // Sample-accurate: every event lands at its offset in the block.
            let list = Events(
                s.events
                    .iter()
                    .filter(|e| (e.frame as usize) >= start && (e.frame as usize) < end)
                    .map(|e| param_event(e.frame - start as u32, ids[e.param as usize], e.value as f64))
                    .collect(),
            );
            let in_events = clap_input_events { ctx: &list as *const Events as *mut c_void, size: Some(list_size), get: Some(list_get) };
            let process = clap_process {
                steady_time: start as i64,
                frames_count: n as u32,
                transport: std::ptr::null(),
                audio_inputs: inputs.as_ptr(),
                audio_outputs: outputs.as_mut_ptr(),
                audio_inputs_count: 1,
                audio_outputs_count: 1,
                in_events: &in_events,
                out_events: std::ptr::null(),
            };
            let status = ((*plugin).process.unwrap())(plugin, &process);
            assert_eq!(status, PROCESS_CONTINUE, "clap process status");
            out[0][start..end].copy_from_slice(&output[0]);
            out[1][start..end].copy_from_slice(&output[1]);
        }
        ((*plugin).stop_processing.unwrap())(plugin);
        ((*plugin).deactivate.unwrap())(plugin);
        ((*plugin).destroy.unwrap())(plugin);
        ((*entry).deinit.unwrap())();
        out
    }
}

// ---- LV2 -------------------------------------------------------------------

/// LV2 control ports are read once at the start of `run`. The contract is a
/// value per run call, so the host cuts a run at every event frame.
fn lv2(s: &Scenario, binary: &str) -> [Vec<f32>; 2] {
    let handle = open_library(binary);
    let get: extern "C" fn(u32) -> *const Lv2Descriptor = unsafe { std::mem::transmute(symbol(handle, "lv2_descriptor")) };
    let desc = get(0);
    assert!(!desc.is_null(), "lv2_descriptor(0)");
    let desc = unsafe { &*desc };
    let frames = s.input[0].len();
    let mut in_l = s.input[0].clone();
    let mut in_r = s.input[1].clone();
    let mut out_l = vec![0.0f32; frames];
    let mut out_r = vec![0.0f32; frames];
    let mut controls = [0.0f32; 2];
    let no_features: [*const c_void; 1] = [std::ptr::null()];
    let instance = (desc.instantiate)(desc, s.rate, c"".as_ptr(), no_features.as_ptr() as *const _);
    assert!(!instance.is_null(), "lv2 instantiate");
    // Ports: 0,1 audio in, 2,3 audio out, then one control per parameter in order.
    (desc.connect_port)(instance, 4, &mut controls[0] as *mut f32 as *mut c_void);
    (desc.connect_port)(instance, 5, &mut controls[1] as *mut f32 as *mut c_void);
    for e in s.events.iter().filter(|e| e.frame == 0) {
        controls[e.param as usize] = e.value;
    }
    (desc.activate)(instance);
    let cuts = cut_points(s);
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        for e in s.events.iter().filter(|e| e.frame as usize == a) {
            controls[e.param as usize] = e.value;
        }
        // Audio ports are moved to the first frame of the sub-block, as a host does.
        unsafe {
            (desc.connect_port)(instance, 0, in_l.as_mut_ptr().add(a) as *mut c_void);
            (desc.connect_port)(instance, 1, in_r.as_mut_ptr().add(a) as *mut c_void);
            (desc.connect_port)(instance, 2, out_l.as_mut_ptr().add(a) as *mut c_void);
            (desc.connect_port)(instance, 3, out_r.as_mut_ptr().add(a) as *mut c_void);
        }
        (desc.run)(instance, (b - a) as u32);
    }
    (desc.deactivate)(instance);
    (desc.cleanup)(instance);
    [out_l, out_r]
}

// ---- compare ----------------------------------------------------------------

fn read_output(path: &str) -> Vec<f32> {
    let b = fs::read(path).unwrap_or_else(|e| fail(&format!("read {path}: {e}")));
    if b.len() % 4 != 0 {
        fail(&format!("{path}: not whole floats"));
    }
    b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
}

fn compare(reference: &str, output: &str, tol: f64, label: &str) -> i32 {
    let (r, o) = (read_output(reference), read_output(output));
    if r.len() != o.len() || r.is_empty() {
        eprintln!("{label}: length {} against reference {}", o.len(), r.len());
        return 2;
    }
    let half = r.len() / 2;
    let (mut worst, mut at) = (0.0f64, 0usize);
    let mut nonfinite = false;
    for i in 0..r.len() {
        nonfinite |= !o[i].is_finite();
        let d = (r[i] as f64 - o[i] as f64).abs();
        if d.is_nan() || d > worst {
            worst = d;
            at = i;
        }
    }
    let peak = r.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let ok = !nonfinite && worst <= tol;
    println!(
        "{label:<8} max abs difference {worst:.3e} (channel {}, frame {}), reference peak {peak:.3}, tolerance {tol:.0e}: {}",
        at / half,
        at % half,
        if ok { "match" } else { "MISMATCH" }
    );
    if ok { 0 } else { 1 }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let arg = |i: usize| a.get(i).map(String::as_str).unwrap_or_else(|| fail("missing argument"));
    let code = match arg(1) {
        "gen" => {
            write_scenario(arg(2));
            0
        }
        "reference" => {
            let late = a.get(4).map_or(0, |v| v.parse().unwrap_or_else(|_| fail("late frames")));
            write_output(arg(3), &reference(&read_scenario(arg(2)), late));
            0
        }
        "clap" => {
            write_output(arg(3), &clap(&read_scenario(arg(2)), arg(4)));
            0
        }
        "lv2" => {
            write_output(arg(3), &lv2(&read_scenario(arg(2)), arg(4)));
            0
        }
        "compare" => compare(arg(2), arg(3), arg(4).parse().unwrap_or_else(|_| fail("tolerance")), arg(5)),
        other => fail(&format!("unknown command {other}")),
    };
    exit(code);
}
