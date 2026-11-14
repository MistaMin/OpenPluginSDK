//! The LV2 descriptor's realtime path does not allocate.
//!
//! One `#[test]` in this binary: the global allocator counts only while armed,
//! and no other test can run beside it. The test drives the exported
//! descriptor exactly as a host does (`descriptor`, `instantiate`,
//! `connect_port`, `activate`, `run`, `deactivate`, `cleanup`), not the
//! in-process `Lv2Harness`.
//!
//! Everything that may allocate (the descriptor, the instance, the port
//! buffers, the control values) is built before arming. Each block shape is
//! measured alone, so a failure names the shape, the allocation count and the
//! free count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::c_void;
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_targets::lv2::{self, AUDIO_PORTS};

static ARMED: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
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

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ARMED.load(Ordering::SeqCst) {
            // A realloc releases the old block and returns a new one.
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
            FREES.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.load(Ordering::SeqCst) {
            FREES.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Run `body` with the allocator armed and assert it allocated and freed
/// nothing. `shape` names the block being measured.
fn measured<T>(shape: &str, body: impl FnOnce() -> T) -> T {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    FREES.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
    let value = body();
    ARMED.store(false, Ordering::SeqCst);
    let allocations = ALLOCATIONS.load(Ordering::SeqCst);
    let frees = FREES.load(Ordering::SeqCst);
    assert_eq!(
        (allocations, frees),
        (0, 0),
        "{shape}: {allocations} allocations and {frees} frees in the armed block"
    );
    value
}

/// The control experiment: the counter must see one heap block while armed.
/// Without this, a removed or broken `#[global_allocator]` would let every
/// measured assertion pass vacuously.
fn assert_allocator_counts() {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    FREES.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
    let probe = vec![0u8; 64];
    let allocated = ALLOCATIONS.load(Ordering::SeqCst);
    drop(probe);
    let freed = FREES.load(Ordering::SeqCst);
    ARMED.store(false, Ordering::SeqCst);
    assert!(
        allocated >= 1,
        "the counting allocator missed an allocation"
    );
    assert!(freed >= 1, "the counting allocator missed a free");
}

fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
    ParamInfo::new("on", "On", 0.0, 1.0, 1.0, Unit::Toggle),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.example.lv2-no-alloc",
    name: "LV2 No Alloc",
    vendor: "Test",
    url: "https://example.com",
    email: "a@b.c",
    version: "0.1.0",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

/// Gain in dB plus an on/off toggle. Neither `prepare`, `reset`, `set_param`
/// nor `process` allocates.
struct GainToggle {
    gain_db: f32,
    linear: f32,
    on: bool,
    blocks: usize,
}

impl Default for GainToggle {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            linear: 1.0,
            on: true,
            blocks: 0,
        }
    }
}

impl Plugin for GainToggle {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, _: f32, _: usize, _: usize) {
        self.linear = db_to_linear(self.gain_db);
    }

    fn reset(&mut self) {
        self.blocks = 0;
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "gain" => {
                self.gain_db = PARAMS[0].clamp(value);
                self.linear = db_to_linear(self.gain_db);
                true
            }
            "on" => {
                self.on = PARAMS[1].clamp(value) >= 0.5;
                true
            }
            _ => false,
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "gain" => Some(self.gain_db),
            "on" => Some(if self.on { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let gain = if self.on { self.linear } else { 0.0 };
        for channel in io.iter_mut() {
            for sample in channel[..frames].iter_mut() {
                *sample *= gain;
            }
        }
        self.blocks += 1;
    }
}

fn control_ptr(value: &f32) -> *mut c_void {
    (value as *const f32).cast_mut().cast()
}

#[test]
fn lv2_descriptor_run_does_not_allocate() {
    assert_allocator_counts();
    lv2::set_creator(|| Box::new(GainToggle::default()));
    let raw = lv2::descriptor(0);
    assert!(!raw.is_null(), "descriptor(0) is null after set_creator");
    // SAFETY: `raw` is the published descriptor for this binary.
    let descriptor = unsafe { &*raw };

    // Instantiate, the port buffers and the control values all exist before
    // the allocator is armed. The descriptor is live, its sample rate is
    // finite, and this host passes no bundle path or features.
    let instance =
        (descriptor.instantiate)(descriptor, 48_000.0, std::ptr::null(), std::ptr::null());
    assert!(!instance.is_null(), "instantiate returned null");

    let mut in_l = [0.25f32; 512];
    let mut in_r = [0.5f32; 512];
    let mut out_l = [-1.0f32; 512];
    let mut out_r = [-7.0f32; 512];
    let mut gain = 6.0f32;
    let mut on = 1.0f32;

    // Every pointer below is null or a live instance and each port pointer
    // addresses 512 floats; the control values live for the test.
    (descriptor.connect_port)(instance, 0, in_l.as_mut_ptr().cast());
    (descriptor.connect_port)(instance, 1, in_r.as_mut_ptr().cast());
    (descriptor.connect_port)(instance, 2, out_l.as_mut_ptr().cast());
    (descriptor.connect_port)(instance, 3, out_r.as_mut_ptr().cast());
    (descriptor.connect_port)(instance, AUDIO_PORTS, control_ptr(&gain));
    (descriptor.connect_port)(instance, AUDIO_PORTS + 1, control_ptr(&on));

    // Stereo, 512 frames, the initial control values (+6 dB, on).
    let loud = db_to_linear(6.0);
    measured("stereo 512 frames", || {
        (descriptor.activate)(instance);
        (descriptor.run)(instance, 512);
        (descriptor.deactivate)(instance);
        black_box(out_l[511]);
        black_box(out_r[511]);
    });
    assert!(
        (out_l[0] - 0.25 * loud).abs() < 1e-6,
        "stereo 512 left: {}",
        out_l[0]
    );
    assert!(
        (out_r[511] - 0.5 * loud).abs() < 1e-6,
        "stereo 512 right: {}",
        out_r[511]
    );

    // Stereo, 1 frame.
    out_l[0] = -1.0;
    out_r[0] = -7.0;
    measured("stereo 1 frame", || {
        (descriptor.activate)(instance);
        (descriptor.run)(instance, 1);
        (descriptor.deactivate)(instance);
        black_box(out_l[0]);
        black_box(out_r[0]);
    });
    assert!(
        (out_l[0] - 0.25 * loud).abs() < 1e-6,
        "stereo 1 left: {}",
        out_l[0]
    );
    assert!(
        (out_r[0] - 0.5 * loud).abs() < 1e-6,
        "stereo 1 right: {}",
        out_r[0]
    );

    // Mono: ports 1 and 3 (in right, out right) stay unconnected.
    (descriptor.connect_port)(instance, 1, std::ptr::null_mut());
    (descriptor.connect_port)(instance, 3, std::ptr::null_mut());
    out_l[511] = -1.0;
    out_r[511] = -7.0;
    measured("mono 512 frames", || {
        (descriptor.activate)(instance);
        (descriptor.run)(instance, 512);
        (descriptor.deactivate)(instance);
        black_box(out_l[511]);
    });
    assert!(
        (out_l[0] - 0.25 * loud).abs() < 1e-6,
        "mono 512 left: {}",
        out_l[0]
    );
    assert_eq!(
        out_r[511], -7.0,
        "mono 512 wrote the unconnected right output"
    );

    out_l[0] = -1.0;
    measured("mono 1 frame", || {
        (descriptor.activate)(instance);
        (descriptor.run)(instance, 1);
        (descriptor.deactivate)(instance);
        black_box(out_l[0]);
    });
    assert!(
        (out_l[0] - 0.25 * loud).abs() < 1e-6,
        "mono 1 left: {}",
        out_l[0]
    );

    // Stereo again, with the control value changed between two runs in one
    // armed section: 0 dB then -6 dB.
    (descriptor.connect_port)(instance, 1, in_r.as_mut_ptr().cast());
    (descriptor.connect_port)(instance, 3, out_r.as_mut_ptr().cast());
    gain = 0.0;
    measured("stereo 512 frames, gain changed between runs", || {
        (descriptor.activate)(instance);
        (descriptor.run)(instance, 512);
        gain = -6.0;
        (descriptor.run)(instance, 512);
        (descriptor.deactivate)(instance);
        black_box(out_l[511]);
        black_box(out_r[511]);
    });
    let expected = 0.25 * db_to_linear(-6.0);
    assert!(
        (out_l[0] - expected).abs() < 1e-5,
        "changed gain left: {}",
        out_l[0]
    );
    assert!(
        (out_r[511] - 0.5 * db_to_linear(-6.0)).abs() < 1e-5,
        "changed gain right: {}",
        out_r[511]
    );

    // Mono, with the toggle turned off between two runs: the second run is
    // silent.
    (descriptor.connect_port)(instance, 1, std::ptr::null_mut());
    (descriptor.connect_port)(instance, 3, std::ptr::null_mut());
    gain = 0.0;
    on = 1.0;
    black_box(gain);
    black_box(on);
    measured("mono 512 frames, toggle changed between runs", || {
        (descriptor.activate)(instance);
        (descriptor.run)(instance, 512);
        on = 0.0;
        (descriptor.run)(instance, 512);
        (descriptor.deactivate)(instance);
        black_box(out_l[0]);
    });
    assert_eq!(out_l[0], 0.0, "toggle off left: {}", out_l[0]);

    // Deactivate and clean up after the last measured block.
    (descriptor.cleanup)(instance);
}
