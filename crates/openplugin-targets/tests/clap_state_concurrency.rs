//! CLAP `clap.state` beside `process`.
//!
//! A host may call `save` and `load` on the main thread while the audio thread is
//! inside `process`. The adapter keeps the plugin object single-threaded with a
//! gate: a state call announces itself, waits a bounded time for the block already
//! running, and works between blocks; a block that starts meanwhile is skipped and
//! passes its input through. A plugin whose state is only its parameters is never
//! touched by a state call at all: the parameters go through the plane.
//!
//! The plugins here make a race visible. `Stateful` keeps its state in two plain
//! fields that every operation reads or writes with a pause in between, so a state
//! call running inside a block, or a block running inside a state call, shows as two
//! different numbers. `Pair` has two parameters that must always be seen together.
//!
//! A counting global allocator, armed per thread, proves that the audio thread
//! allocates nothing while the main thread saves and loads.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_targets::clap_abi::*;
use openplugin_targets::ClapAdapter;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}
static AUDIO_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

fn note_allocation() {
    // `try_with`: the slot may already be gone while a thread exits.
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        AUDIO_ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

static LEVEL: [ParamInfo; 1] = [ParamInfo::new("level", "Level", 0.0, 1.0, 0.0, Unit::None)];
static PAIR: [ParamInfo; 2] = [
    ParamInfo::new("p0", "P0", 0.0, 100_000.0, 0.0, Unit::None),
    ParamInfo::new("p1", "P1", 0.0, 100_000.0, 0.0, Unit::None),
];

const fn info(id: &'static str, params: &'static [ParamInfo]) -> PluginInfo {
    PluginInfo {
        id,
        name: "State",
        vendor: "OpenGrid",
        url: "https://github.com/MistaMin/OpenPluginSDK",
        email: "contact@mistamin.com",
        version: "0.3.0-beta",
        category: Category::Utility,
        params,
        is_instrument: false,
        has_editor: false,
    }
}
static STATEFUL_INFO: PluginInfo = info("com.opengrid.test.clap-state-chunk", &LEVEL);
static PAIR_INFO: PluginInfo = info("com.opengrid.test.clap-state-pair", &PAIR);

fn pause() {
    for _ in 0..300 {
        std::hint::spin_loop();
    }
}

/// Own chunk: two copies of one number that are equal whenever no operation is
/// half done. The output carries `a` in the first half of a block and `b` in the second.
struct Stateful {
    a: u32,
    b: u32,
    level: f32,
}

impl Plugin for Stateful {
    fn info(&self) -> &'static PluginInfo {
        &STATEFUL_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id == "level" {
            self.level = value;
            true
        } else {
            false
        }
    }
    fn param(&self, id: &str) -> Option<f32> {
        (id == "level").then_some(self.level)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let half = frames / 2;
        for channel in io.iter_mut() {
            for sample in &mut channel[..half] {
                *sample = self.a as f32;
            }
            pause();
            for sample in &mut channel[half..frames] {
                *sample = self.b as f32;
            }
        }
    }
    fn save_state(&mut self) -> Option<Vec<u8>> {
        let a = self.a;
        pause();
        let b = self.b;
        let mut bytes = a.to_le_bytes().to_vec();
        bytes.extend_from_slice(&b.to_le_bytes());
        bytes.extend_from_slice(&self.level.to_le_bytes());
        Some(bytes)
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        if data.len() != 12 {
            return false;
        }
        let a = u32::from_le_bytes(data[0..4].try_into().unwrap());
        let b = u32::from_le_bytes(data[4..8].try_into().unwrap());
        let level = f32::from_le_bytes(data[8..12].try_into().unwrap());
        if a != b || !level.is_finite() {
            return false;
        }
        self.a = a;
        pause();
        self.b = b;
        self.level = level;
        true
    }
}

static PAIR_STATE_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Parameters only. `save_state` and `load_state` count their calls.
struct Pair {
    p0: f32,
    p1: f32,
}

impl Plugin for Pair {
    fn info(&self) -> &'static PluginInfo {
        &PAIR_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "p0" => self.p0 = value,
            "p1" => self.p1 = value,
            _ => return false,
        }
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "p0" => Some(self.p0),
            "p1" => Some(self.p1),
            _ => None,
        }
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let half = frames / 2;
        for channel in io.iter_mut() {
            for sample in &mut channel[..half] {
                *sample = self.p0;
            }
            for sample in &mut channel[half..frames] {
                *sample = self.p1;
            }
        }
    }
    // Declares its state is its parameters, so state calls must never reach it.
    fn state_is_parameters_only(&self) -> bool {
        true
    }
    fn save_state(&mut self) -> Option<Vec<u8>> {
        PAIR_STATE_CALLS.fetch_add(1, Ordering::SeqCst);
        None
    }
    fn load_state(&mut self, _: &[u8]) -> bool {
        PAIR_STATE_CALLS.fetch_add(1, Ordering::SeqCst);
        false
    }
}

const FRAMES: usize = 64;
/// What the host feeds in. A block the state call made the adapter skip returns it.
const DRY: f32 = -1.0;

#[derive(Clone, Copy)]
struct PluginPtr(*const clap_plugin);
unsafe impl Send for PluginPtr {}

struct Ostream(Vec<u8>);
struct Istream {
    data: Vec<u8>,
    read: usize,
}

unsafe extern "C" fn write_bytes(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
    let sink = unsafe { &mut *((*stream).ctx as *mut Ostream) };
    sink.0.extend_from_slice(unsafe { std::slice::from_raw_parts(buffer as *const u8, size as usize) });
    size as i64
}

unsafe extern "C" fn read_bytes(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
    let source = unsafe { &mut *((*stream).ctx as *mut Istream) };
    let n = (source.data.len() - source.read).min(size as usize);
    unsafe { std::ptr::copy_nonoverlapping(source.data[source.read..].as_ptr(), buffer as *mut u8, n) };
    source.read += n;
    n as i64
}

fn state_ext(plugin: *const clap_plugin) -> *const clap_plugin_state {
    unsafe { ((*plugin).get_extension.unwrap())(plugin, EXT_STATE.as_ptr() as *const c_char) as *const clap_plugin_state }
}

fn save(plugin: *const clap_plugin) -> Option<Vec<u8>> {
    let mut sink = Ostream(Vec::new());
    let stream = clap_ostream { ctx: &mut sink as *mut Ostream as *mut c_void, write: Some(write_bytes) };
    unsafe { (*state_ext(plugin)).save.unwrap()(plugin, &stream) }.then_some(sink.0)
}

fn load(plugin: *const clap_plugin, bytes: &[u8]) -> bool {
    let mut source = Istream { data: bytes.to_vec(), read: 0 };
    let stream = clap_istream { ctx: &mut source as *mut Istream as *mut c_void, read: Some(read_bytes) };
    unsafe { (*state_ext(plugin)).load.unwrap()(plugin, &stream) }
}

/// One stereo block. Returns the first channel's output.
fn run_block(plugin: *const clap_plugin, out: &mut [Vec<f32>; 2], input: &mut [Vec<f32>; 2]) -> i32 {
    let mut in_ptrs = [input[0].as_mut_ptr(), input[1].as_mut_ptr()];
    let mut out_ptrs = [out[0].as_mut_ptr(), out[1].as_mut_ptr()];
    let in_buf = clap_audio_buffer { data32: in_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 };
    let mut out_buf = clap_audio_buffer { data32: out_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 };
    let process = clap_process {
        steady_time: -1,
        frames_count: FRAMES as u32,
        transport: std::ptr::null(),
        audio_inputs: &in_buf,
        audio_outputs: &mut out_buf,
        audio_inputs_count: 1,
        audio_outputs_count: 1,
        in_events: std::ptr::null(),
        out_events: std::ptr::null(),
    };
    unsafe { ((*plugin).process.unwrap())(plugin, &process) }
}

struct Audio {
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<AudioReport>,
}

#[derive(Default)]
struct AudioReport {
    blocks: usize,
    dry: usize,
    torn: usize,
    allocations: usize,
}

/// Runs blocks on another thread until stopped. A block is torn when its samples
/// are not all one value.
fn start_audio(plugin: PluginPtr) -> Audio {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let handle = std::thread::spawn(move || {
        let plugin = plugin;
        let mut out = [vec![0.0f32; FRAMES], vec![0.0f32; FRAMES]];
        let mut input = [vec![DRY; FRAMES], vec![DRY; FRAMES]];
        let mut report = AudioReport::default();
        // Everything is allocated; only the blocks are counted.
        let before = AUDIO_ALLOCATIONS.load(Ordering::SeqCst);
        COUNTING.with(|c| c.set(true));
        while !flag.load(Ordering::Relaxed) {
            let status = run_block(plugin.0, &mut out, &mut input);
            assert_eq!(status, 1, "process keeps going");
            report.blocks += 1;
            let first = out[0][0];
            let uniform = out.iter().all(|channel| channel.iter().all(|s| *s == first));
            if !uniform {
                report.torn += 1;
            } else if first == DRY {
                report.dry += 1;
            }
        }
        COUNTING.with(|c| c.set(false));
        report.allocations = AUDIO_ALLOCATIONS.load(Ordering::SeqCst) - before;
        report
    });
    Audio { stop, handle }
}

impl Audio {
    fn finish(self) -> AudioReport {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.join().unwrap()
    }
}

fn chunk(value: u32, level: f32) -> Vec<u8> {
    let mut bytes = value.to_le_bytes().to_vec();
    bytes.extend_from_slice(&value.to_le_bytes());
    bytes.extend_from_slice(&level.to_le_bytes());
    bytes
}

fn activated<P: Plugin>(plugin: P) -> ClapAdapter<P> {
    let mut adapter = ClapAdapter::new(plugin);
    assert!(adapter.activate(48_000.0, 1, FRAMES as u32));
    adapter
}

#[test]
fn chunk_state_beside_process_is_never_torn() {
    let adapter = activated(Stateful { a: 1, b: 1, level: 0.0 });
    let plugin = adapter.as_plugin();
    let audio = start_audio(PluginPtr(plugin));
    let mut saves = 0;
    for round in 0..400u32 {
        assert!(load(plugin, &chunk(round + 2, 0.5)), "load {round} is accepted");
        let saved = save(plugin).expect("save succeeds beside process");
        assert_eq!(saved.len(), 12);
        let a = u32::from_le_bytes(saved[0..4].try_into().unwrap());
        let b = u32::from_le_bytes(saved[4..8].try_into().unwrap());
        assert_eq!(a, b, "a saved chunk is never half a load");
        assert!(a >= 2, "and is a state that was loaded");
        saves += 1;
    }
    let report = audio.finish();
    assert_eq!(saves, 400);
    assert!(report.blocks > 0);
    assert_eq!(report.torn, 0, "no block saw half a state ({} of {} blocks)", report.torn, report.blocks);
    assert!(report.dry < report.blocks, "most blocks still run the plugin ({} dry of {})", report.dry, report.blocks);
}

#[test]
fn parameter_state_beside_process_is_never_torn_and_never_touches_the_plugin() {
    let adapter = activated(Pair { p0: 0.0, p1: 0.0 });
    let plugin = adapter.as_plugin();
    let plane = adapter.plane().unwrap();
    // States in which both parameters carry the same value, made before the audio runs.
    let states: Vec<Vec<u8>> = (1..=40u32)
        .map(|value| {
            plane.store(0, value as f32);
            plane.store(1, value as f32);
            save(plugin).expect("save")
        })
        .collect();
    let baseline = PAIR_STATE_CALLS.load(Ordering::SeqCst);
    let audio = start_audio(PluginPtr(plugin));
    for round in 0..1500usize {
        let state = &states[round % states.len()];
        assert!(load(plugin, state), "load {round}");
        assert_eq!(&save(plugin).expect("save").len(), &state.len());
    }
    let report = audio.finish();
    assert_eq!(report.torn, 0, "no block saw one new and one old parameter ({} of {})", report.torn, report.blocks);
    assert_eq!(report.dry, 0, "a parameter-only state never costs the audio thread a block");
    assert_eq!(PAIR_STATE_CALLS.load(Ordering::SeqCst), baseline, "save and load never entered the plugin");
}

#[test]
fn parameter_load_is_applied_whole_or_not_at_all() {
    let adapter = activated(Pair { p0: 0.0, p1: 0.0 });
    let plugin = adapter.as_plugin();
    let plane = adapter.plane().unwrap();
    plane.store(0, 10.0);
    plane.store(1, 20.0);
    let good = save(plugin).unwrap();
    // A non-finite value anywhere refuses the whole state.
    let mut bad = good.clone();
    let len = bad.len();
    bad[len - 4..].copy_from_slice(&f32::NAN.to_le_bytes());
    plane.store(0, 1.0);
    plane.store(1, 2.0);
    assert!(!load(plugin, &bad));
    assert_eq!((plane.load(0), plane.load(1)), (Some(1.0), Some(2.0)));
    assert!(!load(plugin, &good[..good.len() - 1]));
    assert_eq!((plane.load(0), plane.load(1)), (Some(1.0), Some(2.0)));
    assert!(load(plugin, &good));
    assert_eq!((plane.load(0), plane.load(1)), (Some(10.0), Some(20.0)));
}

#[test]
fn audio_thread_does_not_allocate_during_saves_and_loads() {
    for chunked in [true, false] {
        let (plugin, _keep): (PluginPtr, Box<dyn std::any::Any>) = if chunked {
            let adapter = activated(Stateful { a: 1, b: 1, level: 0.0 });
            (PluginPtr(adapter.as_plugin()), Box::new(adapter))
        } else {
            let adapter = activated(Pair { p0: 0.0, p1: 0.0 });
            (PluginPtr(adapter.as_plugin()), Box::new(adapter))
        };
        let audio = start_audio(plugin);
        for round in 0..300u32 {
            if chunked {
                assert!(load(plugin.0, &chunk(round + 2, 0.25)));
            }
            let state = save(plugin.0).expect("save");
            assert!(load(plugin.0, &state));
        }
        let report = audio.finish();
        assert!(report.blocks > 0);
        assert_eq!(report.allocations, 0, "{} audio thread allocated beside a state call", if chunked { "chunk" } else { "parameter" });
    }
}

/// Negative control: the counter really sees an allocation on a counted thread.
#[test]
fn the_counter_sees_audio_thread_allocations() {
    let seen = std::thread::spawn(|| {
        let before = AUDIO_ALLOCATIONS.load(Ordering::SeqCst);
        COUNTING.with(|c| c.set(true));
        let boxed = std::hint::black_box(Box::new([0u8; 128]));
        COUNTING.with(|c| c.set(false));
        drop(boxed);
        AUDIO_ALLOCATIONS.load(Ordering::SeqCst) - before
    })
    .join()
    .unwrap();
    assert!(seen >= 1);
}

#[test]
fn a_plugin_with_its_own_chunk_round_trips() {
    let first = activated(Stateful { a: 7, b: 7, level: 0.0 });
    first.plane().unwrap().store(0, 0.75);
    let saved = save(first.as_plugin()).expect("save");
    assert_eq!(saved, chunk(7, 0.0), "the chunk is the plugin's own, not the parameter snapshot");

    let second = activated(Stateful { a: 0, b: 0, level: 0.0 });
    assert!(load(second.as_plugin(), &chunk(42, 0.75)));
    assert_eq!(second.plane().unwrap().load(0), Some(0.75), "the plane shows the loaded parameter");
    let mut out = [vec![0.0f32; FRAMES], vec![0.0f32; FRAMES]];
    let mut input = [vec![DRY; FRAMES], vec![DRY; FRAMES]];
    assert_eq!(run_block(second.as_plugin(), &mut out, &mut input), 1);
    assert!(out.iter().all(|c| c.iter().all(|s| *s == 42.0)), "the next block runs the loaded state");
    assert_eq!(save(second.as_plugin()).unwrap(), chunk(42, 0.75));

    // A chunk the plugin refuses changes nothing.
    assert!(!load(second.as_plugin(), &[1, 2, 3]));
    assert!(!load(second.as_plugin(), &chunk(9, f32::NAN)));
    assert_eq!(save(second.as_plugin()).unwrap(), chunk(42, 0.75));
    assert_eq!(second.plane().unwrap().load(0), Some(0.75));
}
