//! The CLAP `process` call must not allocate.
//!
//! This is the CLAP half of the 0.3.x roadmap item "Test that the audio path
//! does not allocate in every format, not only in the FFI session". A counting
//! `#[global_allocator]` forwards to `System` and only increments while
//! `ARMED` is set, so the setup that necessarily allocates — the adapter,
//! activation (which allocates the internal buffers), the event lists, the
//! output sink and the audio storage — is invisible to the counters.
//!
//! Everything is built before the measured section. The measured section then
//! arms the counters and calls `clap_plugin.process` for representative block
//! shapes: mono and stereo, 1 and 512 frames, separate and in-place buffers,
//! and one block carrying a host parameter-value event plus editor gesture
//! begin/end traffic. Both the allocation and deallocation counts must be
//! exactly zero.
//!
//! The probe in [`assert_allocator_counts`] runs before the measured section
//! and proves that the allocator really sees an allocation and a deallocation
//! in this binary. Without it, a broken hook or an `ARMED` flag that never
//! reaches the allocator would let the test pass vacuously.

use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_targets::clap_abi::*;
use openplugin_targets::ClapAdapter;

/// Longest measured block. The adapter is activated for this maximum.
const MAX_BLOCK: usize = 512;
/// Widest measured block; the plugin exports one stereo layout.
const MAX_CHANNELS: usize = 2;

// The armed flag is per thread: only allocations made on the thread that armed
// it are counted, so the test harness's other threads cannot add to the count.
// `const` initialisation: reading it inside the allocator never allocates.
thread_local! {
    static ARMED_HERE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct ThreadArm;

impl ThreadArm {
    fn load(&self, _: Ordering) -> bool {
        ARMED_HERE.try_with(|a| a.get()).unwrap_or(false)
    }

    fn store(&self, value: bool, _: Ordering) {
        ARMED_HERE.with(|a| a.set(value));
    }
}

static ARMED: ThreadArm = ThreadArm;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.load(Ordering::SeqCst) {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if ARMED.load(Ordering::SeqCst) {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.load(Ordering::SeqCst) {
            DEALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ARMED.load(Ordering::SeqCst) {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

/// Prove the instrument is live, then clear it. This allocation and
/// deallocation are deliberately not part of the measured `process` section.
fn assert_allocator_counts() {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    DEALLOCATIONS.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
    let probe = std::hint::black_box(Box::new([0u8; 64]));
    drop(std::hint::black_box(probe));
    ARMED.store(false, Ordering::SeqCst);
    let allocations = ALLOCATIONS.load(Ordering::SeqCst);
    let deallocations = DEALLOCATIONS.load(Ordering::SeqCst);
    assert!(
        allocations >= 1,
        "the counting allocator did not see the probe allocation"
    );
    assert!(
        deallocations >= 1,
        "the counting allocator did not see the probe deallocation"
    );
    ALLOCATIONS.store(0, Ordering::SeqCst);
    DEALLOCATIONS.store(0, Ordering::SeqCst);
}

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
    ParamInfo::new("mute", "Mute", 0.0, 1.0, 0.0, Unit::Toggle),
];

static INFO: PluginInfo = PluginInfo::new(
    "com.opengrid.test.clap-no-alloc",
    "CLAP No Alloc",
    "OpenGrid",
    "0.3.0-beta",
    Category::Utility,
    &PARAMS,
)
.with_url("https://github.com/MistaMin/OpenPluginSDK")
.with_email("contact@mistamin.com");

/// A gain and a toggle. Both setter paths clamp, so the measured blocks also
/// cover the clamped parameter plane feeding `set_param_index`.
struct GainToggle {
    db: f32,
    linear: f32,
    muted: bool,
}

impl Default for GainToggle {
    fn default() -> Self {
        Self {
            db: 0.0,
            linear: 1.0,
            muted: false,
        }
    }
}

impl Plugin for GainToggle {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {
        self.linear = 10f32.powf(self.db / 20.0);
    }

    fn reset(&mut self) {}

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "gain" => {
                self.db = PARAMS[0].clamp(value);
                self.linear = 10f32.powf(self.db / 20.0);
                true
            }
            "mute" => {
                self.muted = PARAMS[1].clamp(value) >= 0.5;
                true
            }
            _ => false,
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "gain" => Some(self.db),
            "mute" => Some(if self.muted { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let gain = if self.muted { 0.0 } else { self.linear };
        for channel in io {
            for sample in &mut channel[..frames] {
                *sample *= gain;
            }
        }
    }
}

/// A block shape to measure. The event/gesture block is first because
/// `report_gestures` drains the editor queue on the first `process` call.
#[derive(Clone, Copy)]
struct BlockSpec {
    label: &'static str,
    channels: u32,
    frames: u32,
    in_place: bool,
    param_events: bool,
    editor_gestures: usize,
}

const BLOCKS: [BlockSpec; 7] = [
    BlockSpec {
        label: "stereo/512/separate/param-event+gestures",
        channels: 2,
        frames: 512,
        in_place: false,
        param_events: true,
        editor_gestures: 3,
    },
    BlockSpec {
        label: "mono/1/separate",
        channels: 1,
        frames: 1,
        in_place: false,
        param_events: false,
        editor_gestures: 0,
    },
    BlockSpec {
        label: "mono/512/separate",
        channels: 1,
        frames: 512,
        in_place: false,
        param_events: false,
        editor_gestures: 0,
    },
    BlockSpec {
        label: "stereo/1/separate",
        channels: 2,
        frames: 1,
        in_place: false,
        param_events: false,
        editor_gestures: 0,
    },
    BlockSpec {
        label: "stereo/512/separate",
        channels: 2,
        frames: 512,
        in_place: false,
        param_events: false,
        editor_gestures: 0,
    },
    BlockSpec {
        label: "mono/512/in-place",
        channels: 1,
        frames: 512,
        in_place: true,
        param_events: false,
        editor_gestures: 0,
    },
    BlockSpec {
        label: "stereo/512/in-place",
        channels: 2,
        frames: 512,
        in_place: true,
        param_events: false,
        editor_gestures: 0,
    },
];

/// Host audio storage. Boxed so the channel pointers stay valid when the
/// [`Block`] that owns this moves into the block vector.
struct BlockStorage {
    input: [[f32; MAX_BLOCK]; MAX_CHANNELS],
    output: [[f32; MAX_BLOCK]; MAX_CHANNELS],
    in_place: [[f32; MAX_BLOCK]; MAX_CHANNELS],
    input_ptrs: [*mut f32; MAX_CHANNELS],
    output_ptrs: [*mut f32; MAX_CHANNELS],
    in_place_ptrs: [*mut f32; MAX_CHANNELS],
}

impl BlockStorage {
    fn new() -> Box<Self> {
        let mut storage = Box::new(Self {
            input: [[0.5; MAX_BLOCK]; MAX_CHANNELS],
            output: [[0.0; MAX_BLOCK]; MAX_CHANNELS],
            in_place: [[0.25; MAX_BLOCK]; MAX_CHANNELS],
            input_ptrs: [std::ptr::null_mut(); MAX_CHANNELS],
            output_ptrs: [std::ptr::null_mut(); MAX_CHANNELS],
            in_place_ptrs: [std::ptr::null_mut(); MAX_CHANNELS],
        });
        for channel in 0..MAX_CHANNELS {
            let input = storage.input[channel].as_mut_ptr();
            storage.input_ptrs[channel] = input;
            let output = storage.output[channel].as_mut_ptr();
            storage.output_ptrs[channel] = output;
            let in_place = storage.in_place[channel].as_mut_ptr();
            storage.in_place_ptrs[channel] = in_place;
        }
        storage
    }
}

/// Host-to-plugin events for the measured block: a parameter value halfway
/// through the block, framed by gesture begin and end events as a host sends
/// them for a control change. All three live in this box before arming.
struct InputEvents {
    value: clap_event_param_value,
    begin: clap_event_param_gesture,
    end: clap_event_param_gesture,
    headers: [*const clap_event_header; 3],
}

impl InputEvents {
    fn new() -> Box<Self> {
        let mut events = Box::new(Self {
            value: clap_event_param_value {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_value>() as u32,
                    time: 256,
                    space_id: 0,
                    type_: EVENT_PARAM_VALUE,
                    flags: 0,
                },
                param_id: 0,
                cookie: std::ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value: -6.0,
            },
            begin: clap_event_param_gesture {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_gesture>() as u32,
                    time: 0,
                    space_id: 0,
                    type_: EVENT_PARAM_GESTURE_BEGIN,
                    flags: 0,
                },
                param_id: 0,
            },
            end: clap_event_param_gesture {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_gesture>() as u32,
                    time: 511,
                    space_id: 0,
                    type_: EVENT_PARAM_GESTURE_END,
                    flags: 0,
                },
                param_id: 0,
            },
            headers: [std::ptr::null(); 3],
        });
        let value = &events.value.header as *const clap_event_header;
        let begin = &events.begin.header as *const clap_event_header;
        let end = &events.end.header as *const clap_event_header;
        events.headers = [value, begin, end];
        events
    }
}

unsafe extern "C" fn input_events_size(list: *const clap_input_events) -> u32 {
    let events = unsafe { &*((*list).ctx as *const InputEvents) };
    events.headers.len() as u32
}

unsafe extern "C" fn input_events_get(
    list: *const clap_input_events,
    index: u32,
) -> *const clap_event_header {
    let events = unsafe { &*((*list).ctx as *const InputEvents) };
    events
        .headers
        .get(index as usize)
        .copied()
        .unwrap_or(std::ptr::null())
}

/// Counts the events the adapter pushes to the host without allocating.
struct OutputSink {
    count: usize,
    kinds: [u16; 8],
}

impl OutputSink {
    fn new() -> Box<Self> {
        Box::new(Self {
            count: 0,
            kinds: [0; 8],
        })
    }
}

unsafe extern "C" fn push_output_event(
    list: *const clap_output_events,
    event: *const clap_event_header,
) -> bool {
    let sink = unsafe { &mut *((*list).ctx as *mut OutputSink) };
    if sink.count < sink.kinds.len() {
        sink.kinds[sink.count] = unsafe { (*event).type_ };
    }
    sink.count += 1;
    true
}

struct Block {
    label: &'static str,
    channels: u32,
    frames: u32,
    in_place: bool,
    storage: Box<BlockStorage>,
    events: Option<Box<InputEvents>>,
    sink: Box<OutputSink>,
    expected_gesture_events: usize,
}

impl Block {
    fn new(spec: &BlockSpec) -> Self {
        Self {
            label: spec.label,
            channels: spec.channels,
            frames: spec.frames,
            in_place: spec.in_place,
            storage: BlockStorage::new(),
            events: if spec.param_events {
                Some(InputEvents::new())
            } else {
                None
            },
            sink: OutputSink::new(),
            expected_gesture_events: spec.editor_gestures,
        }
    }
}

/// Everything here happens after arming but allocates nothing: it only builds
/// stack-local CLAP structs around buffers and event lists prepared earlier.
unsafe fn process_block(plugin: *const clap_plugin, block: &mut Block, steady_time: i64) -> i32 {
    let storage = &mut *block.storage;
    let (input_data, output_data) = if block.in_place {
        (
            storage.in_place_ptrs.as_mut_ptr(),
            storage.in_place_ptrs.as_mut_ptr(),
        )
    } else {
        (
            storage.input_ptrs.as_mut_ptr(),
            storage.output_ptrs.as_mut_ptr(),
        )
    };
    let input = clap_audio_buffer {
        data32: input_data,
        data64: std::ptr::null_mut(),
        channel_count: block.channels,
        latency: 0,
        constant_mask: 0,
    };
    let mut output = clap_audio_buffer {
        data32: output_data,
        data64: std::ptr::null_mut(),
        channel_count: block.channels,
        latency: 0,
        constant_mask: 0,
    };

    let input_events = block.events.as_mut().map(|events| clap_input_events {
        ctx: &mut **events as *mut InputEvents as *mut c_void,
        size: Some(input_events_size),
        get: Some(input_events_get),
    });
    let in_events = input_events
        .as_ref()
        .map_or(std::ptr::null(), |list| list as *const clap_input_events);

    let output_events = clap_output_events {
        ctx: &mut *block.sink as *mut OutputSink as *mut c_void,
        try_push: Some(push_output_event),
    };
    let process = clap_process {
        steady_time,
        frames_count: block.frames,
        transport: std::ptr::null(),
        audio_inputs: &input,
        audio_outputs: &mut output,
        audio_inputs_count: 1,
        audio_outputs_count: 1,
        in_events,
        out_events: &output_events,
    };

    let run = unsafe { (*plugin).process }.expect("clap process is installed");
    unsafe { run(plugin, &process) }
}

#[test]
fn clap_process_does_not_allocate() {
    // Instrument check first: setup below is allowed to allocate, but the
    // counters must demonstrably work in this binary.
    assert_allocator_counts();

    // ---- setup: every allocation happens before the measured section ----
    let mut adapter = ClapAdapter::new(GainToggle::default());
    let host = adapter.editor_host().expect("CLAP editor host");
    let plugin = adapter.as_plugin();
    assert!(
        adapter.activate(48_000.0, 1, MAX_BLOCK as u32),
        "activate refused 48 kHz / 512 frames"
    );

    let mut blocks: Vec<Block> = BLOCKS.iter().map(Block::new).collect();

    // The first measured block carries the editor's gesture: begin, a new
    // value, end. `report_gestures` drains the queue on the first `process`.
    host.begin(0);
    host.perform(0, 6.0);
    host.end(0);

    // ---- measured section: only `clap_plugin.process` runs while armed ----
    let mut return_codes = [PROCESS_ERROR; BLOCKS.len()];
    let mut deltas = [(0usize, 0usize); BLOCKS.len()];
    let mut steady_time = 0i64;

    ALLOCATIONS.store(0, Ordering::SeqCst);
    DEALLOCATIONS.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
    for (index, block) in blocks.iter_mut().enumerate() {
        let allocations_before = ALLOCATIONS.load(Ordering::SeqCst);
        let deallocations_before = DEALLOCATIONS.load(Ordering::SeqCst);
        return_codes[index] = unsafe { process_block(plugin, block, steady_time) };
        std::hint::black_box(return_codes[index]);
        deltas[index] = (
            ALLOCATIONS.load(Ordering::SeqCst) - allocations_before,
            DEALLOCATIONS.load(Ordering::SeqCst) - deallocations_before,
        );
        steady_time += i64::from(block.frames);
    }
    ARMED.store(false, Ordering::SeqCst);
    let total_allocations = ALLOCATIONS.load(Ordering::SeqCst);
    let total_deallocations = DEALLOCATIONS.load(Ordering::SeqCst);

    // ---- assertions (outside the armed section, so a failure can format) ----
    for (index, block) in blocks.iter().enumerate() {
        assert_eq!(
            return_codes[index], PROCESS_CONTINUE,
            "process returned {} for block {}",
            return_codes[index], block.label
        );
        assert_eq!(
            deltas[index], (0, 0),
            "block {} allocated {} and freed {} (channels {}, frames {}, in_place {}, input_events {}); \
             CLAP process must not allocate",
            block.label, deltas[index].0, deltas[index].1, block.channels, block.frames, block.in_place,
            block.events.is_some()
        );
    }
    assert_eq!(
        total_allocations,
        0,
        "CLAP process allocated {} times over {} blocks",
        total_allocations,
        blocks.len()
    );
    assert_eq!(
        total_deallocations,
        0,
        "CLAP process freed {} times over {} blocks",
        total_deallocations,
        blocks.len()
    );

    // The event block really reached the plane/cursor and the gesture path.
    assert_eq!(
        blocks[0].sink.count, 3,
        "the host should receive begin, value and end for the editor gesture"
    );
    assert_eq!(
        blocks[0].sink.kinds[..3],
        [
            EVENT_PARAM_GESTURE_BEGIN,
            EVENT_PARAM_VALUE,
            EVENT_PARAM_GESTURE_END
        ]
    );
    assert_eq!(blocks[0].sink.count, blocks[0].expected_gesture_events);
    for block in &blocks[1..] {
        assert_eq!(
            block.sink.count, block.expected_gesture_events,
            "block {} pushed {} events to the host",
            block.label, block.sink.count
        );
    }

    // The gesture's +6 dB is applied at sample 0 and the host's -6 dB event
    // is applied at sample 256, so the audio proves both paths ran.
    let hot = 0.5 * 10f32.powf(6.0 / 20.0);
    let cold = 0.5 * 10f32.powf(-6.0 / 20.0);
    assert!(
        (blocks[0].storage.output[0][0] - hot).abs() < 1e-4,
        "block {} did not apply the +6 dB gesture at sample 0: {} != {}",
        blocks[0].label,
        blocks[0].storage.output[0][0],
        hot
    );
    assert!(
        (blocks[0].storage.output[0][256] - cold).abs() < 1e-4,
        "block {} did not apply the -6 dB input event at sample 256: {} != {}",
        blocks[0].label,
        blocks[0].storage.output[0][256],
        cold
    );
}
