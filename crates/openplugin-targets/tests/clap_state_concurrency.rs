//! CLAP `clap.state` beside `process`.
//!
//! A host may call `save` and `load` on the main thread while the audio thread is
//! inside `process`. The adapter keeps the plugin object single-threaded with a
//! gate: a state call announces itself, waits a bounded time for the block already
//! running, and works between blocks; a block that starts meanwhile is skipped and
//! passes its input through, and its notes reach the next block. A plugin whose state
//! is only its parameters (it says so, or its `save_state` returns `None` when it is
//! new) is never touched by a state call at all: the parameters go through the plane.
//! What is saved is the session the shells save: the parameters, then the chunk.
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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use openplugin_core::{decode_session, encode_params, encode_session, Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_targets::clap_abi::*;
use openplugin_targets::ClapAdapter;

// Per thread, so tests running in parallel (the negative control allocates on
// purpose) never add to each other's counts.
thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static AUDIO_ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn counted() -> usize {
    AUDIO_ALLOCATIONS.with(Cell::get)
}

struct CountingAllocator;

fn note_allocation() {
    // `try_with`: the slot may already be gone while a thread exits.
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        let _ = AUDIO_ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
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
    PluginInfo::new(
        id,
        "State",
        "OpenGrid",
        "0.3.0-beta",
        Category::Utility,
        params,
    )
    .with_url("https://github.com/MistaMin/OpenPluginSDK")
    .with_email("contact@mistamin.com")
}
static STATEFUL_INFO: PluginInfo = info("com.opengrid.test.clap-state-chunk", &LEVEL);
static PAIR_INFO: PluginInfo = info("com.opengrid.test.clap-state-pair", &PAIR);
static SPLIT_INFO: PluginInfo = info("com.opengrid.test.clap-state-split", &LEVEL);

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
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
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

/// Parameters only. `save_state` and `load_state` count their calls. `declared` is
/// its `state_is_parameters_only`.
struct Pair {
    p0: f32,
    p1: f32,
    declared: bool,
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
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
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
    fn state_is_parameters_only(&self) -> bool {
        self.declared
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

/// What a test shares with its `Split`.
#[derive(Clone, Default)]
struct SplitProbe {
    /// While true, `save_state` spins (a slow save), up to a safety limit.
    hold_save: Arc<AtomicBool>,
    in_save: Arc<AtomicBool>,
    /// The notes the plugin was given, oldest first, two bits each: 1 a note-on, 2 a note-off.
    notes: Arc<AtomicU32>,
}

/// A chunk that does not hold its parameter: `program` is saved by the plugin, `level`
/// by the SDK. The output is `program + level`.
struct Split {
    level: f32,
    program: u8,
    probe: SplitProbe,
}

impl Plugin for Split {
    fn info(&self) -> &'static PluginInfo {
        &SPLIT_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id == "level" {
            self.level = value;
        }
        id == "level"
    }
    fn param(&self, id: &str) -> Option<f32> {
        (id == "level").then_some(self.level)
    }
    fn note_on_channel(&mut self, _: usize, _: u8, _: u8, _: u8) {
        self.probe.notes.store(self.probe.notes.load(Ordering::SeqCst) << 2 | 1, Ordering::SeqCst);
    }
    fn note_off(&mut self, _: usize, _: u8) {
        self.probe.notes.store(self.probe.notes.load(Ordering::SeqCst) << 2 | 2, Ordering::SeqCst);
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        for channel in io.iter_mut() {
            channel[..frames].fill(f32::from(self.program) + self.level);
        }
    }
    fn save_state(&mut self) -> Option<Vec<u8>> {
        self.probe.in_save.store(true, Ordering::SeqCst);
        let start = std::time::Instant::now();
        while self.probe.hold_save.load(Ordering::SeqCst) && start.elapsed().as_secs() < 5 {
            std::hint::spin_loop();
        }
        self.probe.in_save.store(false, Ordering::SeqCst);
        Some(vec![self.program])
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        let [program] = data else { return false };
        self.program = *program;
        true
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

/// One stereo block without events.
fn run_block(plugin: *const clap_plugin, out: &mut [Vec<f32>; 2], input: &mut [Vec<f32>; 2]) -> i32 {
    run_block_with(plugin, out, input, std::ptr::null())
}

unsafe extern "C" fn events_size(list: *const clap_input_events) -> u32 {
    unsafe { &*((*list).ctx as *const Vec<*const clap_event_header>) }.len() as u32
}

unsafe extern "C" fn events_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let headers = unsafe { &*((*list).ctx as *const Vec<*const clap_event_header>) };
    headers[index as usize]
}

fn note(kind: u16, time: u32, key: i16) -> clap_event_note {
    clap_event_note {
        header: clap_event_header { size: std::mem::size_of::<clap_event_note>() as u32, time, space_id: 0, type_: kind, flags: 0 },
        note_id: -1,
        port_index: 0,
        channel: 0,
        key,
        velocity: 0.8,
    }
}

/// One stereo block with the host's event list.
fn run_block_with(plugin: *const clap_plugin, out: &mut [Vec<f32>; 2], input: &mut [Vec<f32>; 2], in_events: *const clap_input_events) -> i32 {
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
        in_events,
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
    let running = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ran = running.clone();
    let handle = std::thread::spawn(move || {
        let plugin = plugin;
        let mut out = [vec![0.0f32; FRAMES], vec![0.0f32; FRAMES]];
        let mut input = [vec![DRY; FRAMES], vec![DRY; FRAMES]];
        let mut report = AudioReport::default();
        // Everything is allocated; only the blocks are counted.
        let before = counted();
        COUNTING.with(|c| c.set(true));
        while !flag.load(Ordering::Relaxed) {
            let status = run_block(plugin.0, &mut out, &mut input);
            assert_eq!(status, 1, "process keeps going");
            report.blocks += 1;
            ran.store(report.blocks, Ordering::Release);
            let first = out[0][0];
            let uniform = out.iter().all(|channel| channel.iter().all(|s| *s == first));
            if !uniform {
                report.torn += 1;
            } else if first == DRY {
                report.dry += 1;
            }
        }
        COUNTING.with(|c| c.set(false));
        report.allocations = counted() - before;
        report
    });
    // The state calls must overlap processing: on a loaded machine the main thread could
    // otherwise finish every round before the audio thread is first scheduled.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while running.load(Ordering::Acquire) < 8 {
        assert!(std::time::Instant::now() < deadline, "the audio thread never started");
        std::thread::yield_now();
    }
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
        let saved = decode_session(&STATEFUL_INFO, &saved).expect("a session").chunk;
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
    // A plugin that declares its state is its parameters, and one that only leaves
    // `save_state` at `None`: neither is gated.
    for declared in [true, false] {
        let adapter = activated(Pair { p0: 0.0, p1: 0.0, declared });
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
        assert_eq!(report.dry, 0, "a parameter-only state never costs the audio thread a block (declared: {declared})");
        assert_eq!(PAIR_STATE_CALLS.load(Ordering::SeqCst), baseline, "save and load never entered the plugin (declared: {declared})");
    }
}

#[test]
fn parameter_load_is_applied_whole_or_not_at_all() {
    let adapter = activated(Pair { p0: 0.0, p1: 0.0, declared: true });
    let plugin = adapter.as_plugin();
    let plane = adapter.plane().unwrap();
    plane.store(0, 10.0);
    plane.store(1, 20.0);
    let good = save(plugin).unwrap();
    // A non-finite value anywhere refuses the whole state.
    let mut bad = good.clone();
    let len = bad.len();
    // The last value: the session ends with the (empty) chunk's length.
    bad[len - 8..len - 4].copy_from_slice(&f32::NAN.to_le_bytes());
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
            let adapter = activated(Pair { p0: 0.0, p1: 0.0, declared: true });
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
        let before = counted();
        COUNTING.with(|c| c.set(true));
        let boxed = std::hint::black_box(Box::new([0u8; 128]));
        COUNTING.with(|c| c.set(false));
        drop(boxed);
        counted() - before
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
    assert_eq!(saved, encode_session(&STATEFUL_INFO, &[0.75], &chunk(7, 0.0)), "the parameters, then the plugin's own chunk");

    let second = activated(Stateful { a: 0, b: 0, level: 0.0 });
    assert!(load(second.as_plugin(), &saved));
    assert_eq!(second.plane().unwrap().load(0), Some(0.75), "the plane shows the loaded parameter");
    let mut out = [vec![0.0f32; FRAMES], vec![0.0f32; FRAMES]];
    let mut input = [vec![DRY; FRAMES], vec![DRY; FRAMES]];
    assert_eq!(run_block(second.as_plugin(), &mut out, &mut input), 1);
    assert!(out.iter().all(|c| c.iter().all(|s| *s == 7.0)), "the next block runs the loaded state");
    assert_eq!(save(second.as_plugin()).unwrap(), encode_session(&STATEFUL_INFO, &[0.75], &chunk(7, 0.75)));

    // A chunk the plugin refuses changes nothing.
    let before = save(second.as_plugin()).unwrap();
    assert!(!load(second.as_plugin(), &[1, 2, 3]));
    assert!(!load(second.as_plugin(), &chunk(9, f32::NAN)));
    assert!(!load(second.as_plugin(), &encode_session(&STATEFUL_INFO, &[0.1], &chunk(9, f32::NAN))));
    assert_eq!(save(second.as_plugin()).unwrap(), before);
    assert_eq!(second.plane().unwrap().load(0), Some(0.75));
}

#[test]
fn sessions_saved_before_clap_wrote_sessions_still_load() {
    // The plugin's raw chunk, as CLAP saved a plugin with one.
    let chunky = activated(Stateful { a: 0, b: 0, level: 0.0 });
    assert!(load(chunky.as_plugin(), &chunk(42, 0.75)));
    assert_eq!(chunky.plane().unwrap().load(0), Some(0.75), "the plane shows the chunk's parameter");
    let mut out = [vec![0.0f32; FRAMES], vec![0.0f32; FRAMES]];
    let mut input = [vec![DRY; FRAMES], vec![DRY; FRAMES]];
    assert_eq!(run_block(chunky.as_plugin(), &mut out, &mut input), 1);
    assert!(out.iter().all(|c| c.iter().all(|s| *s == 42.0)), "the next block runs the loaded chunk");

    // A bare OPST snapshot, as CLAP saved a plugin without a chunk: into both kinds.
    assert!(load(chunky.as_plugin(), &encode_params(&STATEFUL_INFO, &[0.25])));
    assert_eq!(chunky.plane().unwrap().load(0), Some(0.25));
    let plain = activated(Pair { p0: 0.0, p1: 0.0, declared: false });
    assert!(load(plain.as_plugin(), &encode_params(&PAIR_INFO, &[3.0, 4.0])));
    assert_eq!((plain.plane().unwrap().load(0), plain.plane().unwrap().load(1)), (Some(3.0), Some(4.0)));
}

#[test]
fn a_chunk_that_leaves_out_the_parameters_gets_them_back() {
    let first = activated(Split { level: 0.0, program: 5, probe: SplitProbe::default() });
    first.plane().unwrap().store(0, 0.75);
    let saved = save(first.as_plugin()).expect("save");

    let second = activated(Split { level: 0.0, program: 0, probe: SplitProbe::default() });
    assert!(load(second.as_plugin(), &saved));
    assert_eq!(second.plane().unwrap().load(0), Some(0.75), "the knob value came back");
    let mut out = [vec![0.0f32; FRAMES], vec![0.0f32; FRAMES]];
    let mut input = [vec![DRY; FRAMES], vec![DRY; FRAMES]];
    assert_eq!(run_block(second.as_plugin(), &mut out, &mut input), 1);
    assert!(out.iter().all(|c| c.iter().all(|s| *s == 5.75)), "the engine has the chunk and the knob value: {}", out[0][0]);
}

#[test]
fn the_notes_of_a_block_skipped_by_a_save_reach_the_next_block() {
    let probe = SplitProbe::default();
    let adapter = activated(Split { level: 0.0, program: 1, probe: probe.clone() });
    let plugin = PluginPtr(adapter.as_plugin());
    probe.hold_save.store(true, Ordering::SeqCst);
    let saver = std::thread::spawn(move || {
        let plugin = plugin;
        save(plugin.0)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !probe.in_save.load(Ordering::SeqCst) {
        assert!(std::time::Instant::now() < deadline, "the save never started");
        std::thread::yield_now();
    }

    // Listed out of order on purpose: the note-off is later in the block than the note-on.
    let (off, on) = (note(EVENT_NOTE_OFF, 40, 60), note(EVENT_NOTE_ON, 8, 60));
    let headers: Vec<*const clap_event_header> = vec![&off.header, &on.header];
    let list = clap_input_events { ctx: &headers as *const Vec<*const clap_event_header> as *mut c_void, size: Some(events_size), get: Some(events_get) };
    let mut out = [vec![0.0f32; FRAMES], vec![0.0f32; FRAMES]];
    let mut input = [vec![DRY; FRAMES], vec![DRY; FRAMES]];
    assert_eq!(run_block_with(plugin.0, &mut out, &mut input, &list), 1);
    assert!(out.iter().all(|c| c.iter().all(|s| *s == DRY)), "the block during the save is dry");
    assert_eq!(probe.notes.load(Ordering::SeqCst), 0, "a skipped block does not enter the plugin");

    probe.hold_save.store(false, Ordering::SeqCst);
    assert!(saver.join().unwrap().is_some());
    assert_eq!(run_block(plugin.0, &mut out, &mut input), 1);
    assert_eq!(out[0][0], 1.0, "the plugin runs again");
    assert_eq!(probe.notes.load(Ordering::SeqCst), 0b01_10, "the note-on, then the note-off");
    assert_eq!(run_block(plugin.0, &mut out, &mut input), 1);
    assert_eq!(probe.notes.load(Ordering::SeqCst), 0b01_10, "each is delivered once");
}
