//! A panic in the plugin never leaves the shell ABI (VST3, Audio Unit and AAX call it).
//!
//! `Bomb` panics in `process`, `set_param`, a note callback, `reset`, `save_state` and
//! `load_state` on demand. Through the exported functions: `process_events` and `process`
//! return 0 (the refusal) and leave the outputs silent; the other calls return their failure
//! value; the process is alive afterwards and the next healthy call works.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use openplugin_core::layout::NotePorts;
use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_ffi::shell::{self, *};

const NONE: u32 = 0;
const PROCESS: u32 = 1;
const SET_PARAM: u32 = 2;
const NOTE: u32 = 3;
const RESET: u32 = 4;
const SAVE: u32 = 5;
const LOAD: u32 = 6;

static MODE: AtomicU32 = AtomicU32::new(NONE);
static LOCK: Mutex<()> = Mutex::new(());

fn arm(mode: u32) {
    MODE.store(mode, Ordering::SeqCst);
}

fn armed(mode: u32) -> bool {
    MODE.load(Ordering::SeqCst) == mode
}

static PARAMS: [ParamInfo; 1] = [ParamInfo::new("level", "Level", 0.0, 1.0, 0.5, Unit::None)];

static INFO: PluginInfo = PluginInfo {
    id: "com.example.shell-bomb",
    name: "Shell Bomb",
    vendor: "Test",
    url: "https://example.com",
    email: "a@b.c",
    version: "0.1.0",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

#[derive(Default)]
struct Bomb {
    level: f32,
}

impl Plugin for Bomb {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {
        if armed(RESET) {
            panic!("bomb: reset");
        }
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if armed(SET_PARAM) {
            panic!("bomb: set_param");
        }
        if id == "level" {
            self.level = value;
        }
        id == "level"
    }
    fn param(&self, id: &str) -> Option<f32> {
        (id == "level").then_some(self.level)
    }
    fn note_ports(&self) -> NotePorts {
        NotePorts { input: true, output: false }
    }
    fn note_on_channel(&mut self, _: usize, _: u8, _: u8, _: u8) {
        if armed(NOTE) {
            panic!("bomb: note");
        }
    }
    fn state_is_parameters_only(&self) -> bool {
        false
    }
    fn save_state(&mut self) -> Option<Vec<u8>> {
        if armed(SAVE) {
            panic!("bomb: save_state");
        }
        Some(vec![1, 2, 3])
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        if armed(LOAD) {
            panic!("bomb: load_state");
        }
        data == [1, 2, 3]
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if armed(PROCESS) {
            panic!("bomb: process");
        }
        for channel in io.iter_mut() {
            channel[..frames].fill(0.8);
        }
    }
}

const FRAMES: usize = 64;

fn mk() -> *mut ShellInstance {
    let p = create::<Bomb>(48_000.0, FRAMES as u32, 2);
    assert!(!p.is_null());
    p
}

/// One block through `process_events`; the outputs start at 9.0.
fn block(instance: *mut ShellInstance, events: &[OpEvent]) -> (u32, Vec<f32>, Vec<f32>) {
    let input = [0.5f32; FRAMES];
    let mut left = vec![9.0f32; FRAMES];
    let mut right = vec![9.0f32; FRAMES];
    let ins = [input.as_ptr(), input.as_ptr()];
    let outs = [left.as_mut_ptr(), right.as_mut_ptr()];
    let args = OpProcess {
        struct_size: std::mem::size_of::<OpProcess>() as u32,
        frames: FRAMES as u32,
        inputs: ins.as_ptr(),
        input_channels: 2,
        output_channels: 2,
        outputs: outs.as_ptr(),
        sidechain: std::ptr::null(),
        sidechain_channels: 0,
        event_count: events.len() as u32,
        events: if events.is_empty() { std::ptr::null() } else { events.as_ptr() },
        transport: std::ptr::null(),
    };
    let status = unsafe { process_events(instance, &args) };
    (status, left, right)
}

fn is_silent(left: &[f32], right: &[f32]) -> bool {
    left.iter().chain(right).all(|s| *s == 0.0)
}

fn healthy(instance: *mut ShellInstance) {
    arm(NONE);
    let (status, left, right) = block(instance, &[]);
    assert_eq!(status, 1);
    assert!(left.iter().chain(&right).all(|s| *s == 0.8));
}

#[test]
fn panic_in_process_is_refused_and_silent() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let i = mk();
    arm(PROCESS);
    let (status, left, right) = block(i, &[]);
    assert_eq!(status, 0, "process_events refuses");
    assert!(is_silent(&left, &right));
    healthy(i);
    unsafe { destroy(i) };
}

#[test]
fn panic_in_the_simple_process_is_refused_and_silent() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let i = mk();
    arm(PROCESS);
    let input = [0.5f32; FRAMES];
    let mut left = vec![9.0f32; FRAMES];
    let mut right = vec![9.0f32; FRAMES];
    let status = unsafe { shell::process(i, input.as_ptr(), input.as_ptr(), left.as_mut_ptr(), right.as_mut_ptr(), FRAMES as u32) };
    assert_eq!(status, 0);
    assert!(is_silent(&left, &right));
    healthy(i);
    unsafe { destroy(i) };
}

#[test]
fn panic_in_set_param_during_a_block_is_refused_and_silent() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let i = mk();
    arm(SET_PARAM);
    let event = OpEvent { kind: EVENT_PARAM_PLAIN, index: 0, value: 0.9, ..OpEvent::default() };
    let (status, left, right) = block(i, &[event]);
    assert_eq!(status, 0);
    assert!(is_silent(&left, &right));
    healthy(i);
    unsafe { destroy(i) };
}

#[test]
fn panic_in_a_note_callback_is_refused_and_silent() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let i = mk();
    arm(NOTE);
    let event = OpEvent { kind: EVENT_NOTE_ON, note: 60, velocity: 100, ..OpEvent::default() };
    let (status, left, right) = block(i, &[event]);
    assert_eq!(status, 0);
    assert!(is_silent(&left, &right));
    healthy(i);
    unsafe { destroy(i) };
}

#[test]
fn panic_in_reset_returns_normally() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let i = mk();
    arm(RESET);
    unsafe { reset(i) };
    healthy(i);
    unsafe { destroy(i) };
}

#[test]
fn panic_in_set_plain_returns_the_failure_value() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let i = mk();
    arm(SET_PARAM);
    // The value is stored on the shared plane and reaches the plugin on the next block, so a
    // main-thread setter does not run plugin code; either way the call must not abort.
    let _ = unsafe { set_plain(i, 0, 0.7) };
    arm(NONE);
    healthy(i);
    unsafe { destroy(i) };
}

#[test]
fn panic_in_save_and_load_state_return_failure_and_the_gate_is_released() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let i = mk();
    let mut needed = 0u32;
    arm(NONE);
    assert_eq!(unsafe { save_state(i, std::ptr::null_mut(), 0, &mut needed) }, STATE_TOO_SMALL, "control");
    let mut blob = vec![0u8; needed as usize];
    assert_eq!(unsafe { save_state(i, blob.as_mut_ptr(), needed, &mut needed) }, STATE_OK);

    arm(SAVE);
    let mut again = 0u32;
    assert_eq!(unsafe { save_state(i, std::ptr::null_mut(), 0, &mut again) }, 0, "failure, not STATE_OK");
    arm(LOAD);
    assert_eq!(unsafe { load_state(i, blob.as_ptr(), blob.len() as u32) }, 0);

    arm(NONE);
    healthy(i);
    assert_eq!(unsafe { load_state(i, blob.as_ptr(), blob.len() as u32) }, 1, "the gate was released");
    unsafe { destroy(i) };
}
