//! A panic in the plugin never leaves the CLAP entry points.
//!
//! `Bomb` panics in `process`, `set_param`, `save_state`, `load_state`, a note callback or
//! `reset`, on demand. Each test drives the exported `clap_plugin` and the extension tables
//! as a host does and checks: the call returns its failure value (`CLAP_PROCESS_ERROR`,
//! `false`), the output is silence, the process is still alive, a later call is harmless and
//! the instance recovers when the host activates it again. A panic in a state call does not
//! stop the audio.

#[path = "common/bomb.rs"]
mod bomb;

use std::ffi::{c_char, c_void};

use bomb::*;
use openplugin_targets::clap_abi::*;
use openplugin_targets::ClapAdapter;

const FRAMES: usize = 64;

struct Events {
    headers: Vec<*const clap_event_header>,
}

unsafe extern "C" fn size(list: *const clap_input_events) -> u32 {
    unsafe { &*((*list).ctx as *const Events) }.headers.len() as u32
}

unsafe extern "C" fn get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let events = unsafe { &*((*list).ctx as *const Events) };
    events.headers.get(index as usize).copied().unwrap_or(std::ptr::null())
}

fn header(size: usize, kind: u16) -> clap_event_header {
    clap_event_header { size: size as u32, time: 0, space_id: 0, type_: kind, flags: 0 }
}

fn param_event(value: f64) -> clap_event_param_value {
    clap_event_param_value {
        header: header(std::mem::size_of::<clap_event_param_value>(), EVENT_PARAM_VALUE),
        param_id: 0,
        cookie: std::ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        value,
    }
}

fn note_event() -> clap_event_note {
    clap_event_note {
        header: header(std::mem::size_of::<clap_event_note>(), EVENT_NOTE_ON),
        note_id: 1,
        port_index: 0,
        channel: 0,
        key: 60,
        velocity: 1.0,
    }
}

/// One stereo block with the output prefilled with 9.0 so silence is a change.
/// Returns the status and the output.
fn block(plugin: *const clap_plugin, events: &[*const clap_event_header]) -> (i32, Vec<Vec<f32>>) {
    let mut input = [vec![0.5f32; FRAMES], vec![0.5f32; FRAMES]];
    let mut out = vec![vec![9.0f32; FRAMES], vec![9.0f32; FRAMES]];
    let mut in_ptrs = [input[0].as_mut_ptr(), input[1].as_mut_ptr()];
    let mut out_ptrs = [out[0].as_mut_ptr(), out[1].as_mut_ptr()];
    let in_buf = clap_audio_buffer { data32: in_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 };
    let mut out_buf = clap_audio_buffer { data32: out_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 };
    let list = Events { headers: events.to_vec() };
    let in_events = clap_input_events { ctx: &list as *const Events as *mut c_void, size: Some(size), get: Some(get) };
    let process = clap_process {
        steady_time: -1,
        frames_count: FRAMES as u32,
        transport: std::ptr::null(),
        audio_inputs: &in_buf,
        audio_outputs: &mut out_buf,
        audio_inputs_count: 1,
        audio_outputs_count: 1,
        in_events: &in_events,
        out_events: std::ptr::null(),
    };
    let status = unsafe { ((*plugin).process.unwrap())(plugin, &process) };
    (status, out)
}

fn silent(out: &[Vec<f32>]) -> bool {
    out.iter().all(|channel| channel.iter().all(|s| *s == 0.0))
}

fn sounding(out: &[Vec<f32>]) -> bool {
    out.iter().all(|channel| channel.iter().all(|s| *s == OUT))
}

fn ext<T>(plugin: *const clap_plugin, id: &[u8]) -> *const T {
    unsafe { ((*plugin).get_extension.unwrap())(plugin, id.as_ptr() as *const c_char) as *const T }
}

struct Sink(Vec<u8>);
struct Source {
    data: Vec<u8>,
    at: usize,
}

unsafe extern "C" fn write_bytes(stream: *const clap_ostream, buffer: *const c_void, n: u64) -> i64 {
    let sink = unsafe { &mut *((*stream).ctx as *mut Sink) };
    sink.0.extend_from_slice(unsafe { std::slice::from_raw_parts(buffer as *const u8, n as usize) });
    n as i64
}

unsafe extern "C" fn read_bytes(stream: *const clap_istream, buffer: *mut c_void, n: u64) -> i64 {
    let source = unsafe { &mut *((*stream).ctx as *mut Source) };
    let take = (source.data.len() - source.at).min(n as usize);
    unsafe { std::ptr::copy_nonoverlapping(source.data[source.at..].as_ptr(), buffer as *mut u8, take) };
    source.at += take;
    take as i64
}

fn save(plugin: *const clap_plugin) -> Option<Vec<u8>> {
    let mut sink = Sink(Vec::new());
    let stream = clap_ostream { ctx: &mut sink as *mut Sink as *mut c_void, write: Some(write_bytes) };
    let state = ext::<clap_plugin_state>(plugin, EXT_STATE);
    unsafe { (*state).save.unwrap()(plugin, &stream) }.then_some(sink.0)
}

fn load(plugin: *const clap_plugin, data: &[u8]) -> bool {
    let mut source = Source { data: data.to_vec(), at: 0 };
    let stream = clap_istream { ctx: &mut source as *mut Source as *mut c_void, read: Some(read_bytes) };
    let state = ext::<clap_plugin_state>(plugin, EXT_STATE);
    unsafe { (*state).load.unwrap()(plugin, &stream) }
}

fn rig() -> (ClapAdapter<Bomb>, *const clap_plugin) {
    let mut adapter = ClapAdapter::new(Bomb::default());
    let plugin = adapter.as_plugin();
    assert!(adapter.activate(48_000.0, 1, FRAMES as u32));
    (adapter, plugin)
}

fn reactivate(adapter: &mut ClapAdapter<Bomb>, plugin: *const clap_plugin) {
    unsafe { ((*plugin).deactivate.unwrap())(plugin) };
    assert!(adapter.activate(48_000.0, 1, FRAMES as u32));
}

/// Silent, error status, and still silent on the next block; activating again recovers.
fn assert_contained_then_recovers(adapter: &mut ClapAdapter<Bomb>, plugin: *const clap_plugin, first: (i32, Vec<Vec<f32>>)) {
    assert_eq!(first.0, PROCESS_ERROR);
    assert!(silent(&first.1), "a panicking block must output silence");
    arm(NONE);
    let (status, out) = block(plugin, &[]);
    assert_eq!(status, PROCESS_ERROR, "a poisoned instance keeps refusing");
    assert!(silent(&out), "a poisoned instance keeps outputting silence");
    reactivate(adapter, plugin);
    let (status, out) = block(plugin, &[]);
    assert_eq!(status, PROCESS_CONTINUE, "activation clears the poison");
    assert!(sounding(&out));
}

#[test]
fn a_healthy_block_makes_sound() {
    let _g = serial();
    arm(NONE);
    let (_adapter, plugin) = rig();
    let (status, out) = block(plugin, &[]);
    assert_eq!(status, PROCESS_CONTINUE);
    assert!(sounding(&out));
}

#[test]
fn panic_in_process() {
    let _g = serial();
    arm(PROCESS);
    let (mut adapter, plugin) = rig();
    let first = block(plugin, &[]);
    assert_contained_then_recovers(&mut adapter, plugin, first);
}

#[test]
fn panic_in_set_param_from_a_process_event() {
    let _g = serial();
    arm(NONE);
    let (mut adapter, plugin) = rig();
    arm(SET_PARAM);
    let event = param_event(0.9);
    let first = block(plugin, &[&event.header]);
    assert_contained_then_recovers(&mut adapter, plugin, first);
}

#[test]
fn panic_in_set_param_from_params_flush() {
    let _g = serial();
    arm(NONE);
    let (mut adapter, plugin) = rig();
    arm(SET_PARAM);
    let event = param_event(0.9);
    let list = Events { headers: vec![&event.header] };
    let in_events = clap_input_events { ctx: &list as *const Events as *mut c_void, size: Some(size), get: Some(get) };
    let params = ext::<clap_plugin_params>(plugin, EXT_PARAMS);
    unsafe { (*params).flush.unwrap()(plugin, &in_events, std::ptr::null()) };
    arm(NONE);
    // The flush poisoned the instance: the next block is refused and silent.
    let (status, out) = block(plugin, &[]);
    assert_eq!(status, PROCESS_ERROR);
    assert!(silent(&out));
    reactivate(&mut adapter, plugin);
    let (status, out) = block(plugin, &[]);
    assert_eq!(status, PROCESS_CONTINUE);
    assert!(sounding(&out));
}

#[test]
fn panic_in_a_note_callback() {
    let _g = serial();
    arm(NOTE);
    let (mut adapter, plugin) = rig();
    let note = note_event();
    let first = block(plugin, &[&note.header]);
    assert_contained_then_recovers(&mut adapter, plugin, first);
}

#[test]
fn panic_in_reset() {
    let _g = serial();
    arm(NONE);
    let (mut adapter, plugin) = rig();
    arm(RESET);
    unsafe { ((*plugin).reset.unwrap())(plugin) };
    arm(NONE);
    let (status, out) = block(plugin, &[]);
    assert_eq!(status, PROCESS_ERROR);
    assert!(silent(&out));
    reactivate(&mut adapter, plugin);
    assert_eq!(block(plugin, &[]).0, PROCESS_CONTINUE);
}

#[test]
fn panic_in_save_state_returns_false_and_audio_goes_on() {
    let _g = serial();
    arm(NONE);
    let (_adapter, plugin) = rig();
    assert!(save(plugin).is_some(), "control: saving works");
    arm(SAVE);
    assert_eq!(save(plugin), None);
    arm(NONE);
    let (status, out) = block(plugin, &[]);
    assert_eq!(status, PROCESS_CONTINUE, "a failed save does not stop the audio");
    assert!(sounding(&out));
    assert!(save(plugin).is_some(), "the state gate was released");
}

#[test]
fn panic_in_load_state_returns_false_and_audio_goes_on() {
    let _g = serial();
    arm(NONE);
    let (_adapter, plugin) = rig();
    let saved = save(plugin).expect("save");
    arm(LOAD);
    assert!(!load(plugin, &saved));
    arm(NONE);
    let (status, out) = block(plugin, &[]);
    assert_eq!(status, PROCESS_CONTINUE);
    assert!(sounding(&out));
    assert!(load(plugin, &saved), "the state gate was released");
}

#[test]
fn queries_still_answer_after_a_panic() {
    let _g = serial();
    arm(PROCESS);
    let (_adapter, plugin) = rig();
    assert_eq!(block(plugin, &[]).0, PROCESS_ERROR);
    arm(NONE);
    let params = ext::<clap_plugin_params>(plugin, EXT_PARAMS);
    assert_eq!(unsafe { (*params).count.unwrap()(plugin) }, 1);
    let ports = ext::<clap_plugin_audio_ports>(plugin, EXT_AUDIO_PORTS);
    assert_eq!(unsafe { (*ports).count.unwrap()(plugin, true) }, 1);
}
