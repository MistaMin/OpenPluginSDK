//! The shell's realtime path does not allocate.
//!
//! One `#[test]` in this binary: the global allocator counts only while armed,
//! and no other test can run beside it. `create` is allowed to allocate and
//! runs before arming; `process` with every block shape the C ABI documents,
//! `set_plain` and `set_normalized` are measured; `destroy` runs after
//! disarming.
//!
//! Each shape is measured alone, so a failure names the block shape, the
//! allocation count and the free count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_ffi::shell;

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
/// nothing. `shape` names the call or block being measured.
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
    id: "com.example.shell-no-alloc",
    name: "Shell No Alloc",
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
}

impl Default for GainToggle {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            linear: 1.0,
            on: true,
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

    fn reset(&mut self) {}

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
    }
}

#[test]
fn shell_audio_and_parameter_setters_do_not_allocate() {
    assert_allocator_counts();
    // `create` is allowed to allocate: the plugin, the session scratch, the
    // live-instance table and the trace state all exist before arming.
    let instance = shell::create::<GainToggle>(48_000.0, 512, 2);
    assert!(!instance.is_null(), "shell::create returned null");

    let mut in_l = [0.25f32; 512];
    let mut in_r = [0.5f32; 512];
    let mut out_l = [-1.0f32; 512];
    let mut out_r = [-7.0f32; 512];

    // Controls reach the engine through the plane; the audio thread applies
    // them inside `process`.
    let set = measured("set_plain gain +6 dB", || unsafe {
        shell::set_plain(instance, 0, 6.0)
    });
    assert_eq!(set, 1, "set_plain gain returned {set}");
    let set = measured("set_plain toggle on", || unsafe {
        shell::set_plain(instance, 1, 1.0)
    });
    assert_eq!(set, 1, "set_plain toggle returned {set}");

    // Stereo, separate input and output buffers.
    let ran = measured("stereo 512 frames, separate buffers", || unsafe {
        shell::process(
            instance,
            in_l.as_ptr(),
            in_r.as_ptr(),
            out_l.as_mut_ptr(),
            out_r.as_mut_ptr(),
            512,
        )
    });
    assert_eq!(ran, 1, "stereo 512 process returned {ran}");
    let loud = db_to_linear(6.0);
    assert!(
        (out_l[0] - 0.25 * loud).abs() < 1e-5,
        "stereo 512 left: {}",
        out_l[0]
    );
    assert!(
        (out_r[511] - 0.5 * loud).abs() < 1e-5,
        "stereo 512 right: {}",
        out_r[511]
    );

    let ran = measured("stereo 1 frame, separate buffers", || unsafe {
        shell::process(
            instance,
            in_l.as_ptr(),
            in_r.as_ptr(),
            out_l.as_mut_ptr(),
            out_r.as_mut_ptr(),
            1,
        )
    });
    assert_eq!(ran, 1, "stereo 1 process returned {ran}");
    assert!(
        (out_l[0] - 0.25 * loud).abs() < 1e-5,
        "stereo 1 left: {}",
        out_l[0]
    );
    assert!(
        (out_r[0] - 0.5 * loud).abs() < 1e-5,
        "stereo 1 right: {}",
        out_r[0]
    );

    // Stereo in place: the same buffers are the input and the output. The
    // shell copies through scratch first, so this is defined.
    let ran = measured("stereo 512 frames, in place", || unsafe {
        in_l.fill(0.25);
        in_r.fill(0.5);
        shell::process(
            instance,
            in_l.as_ptr(),
            in_r.as_ptr(),
            in_l.as_mut_ptr(),
            in_r.as_mut_ptr(),
            512,
        )
    });
    assert_eq!(ran, 1, "stereo 512 in place returned {ran}");
    assert!(
        (in_l[511] - 0.25 * loud).abs() < 1e-5,
        "in place left: {}",
        in_l[511]
    );
    assert!(
        (in_r[511] - 0.5 * loud).abs() < 1e-5,
        "in place right: {}",
        in_r[511]
    );

    let ran = measured("stereo 1 frame, in place", || unsafe {
        in_l[0] = 0.25;
        in_r[0] = 0.5;
        shell::process(
            instance,
            in_l.as_ptr(),
            in_r.as_ptr(),
            in_l.as_mut_ptr(),
            in_r.as_mut_ptr(),
            1,
        )
    });
    assert_eq!(ran, 1, "stereo 1 in place returned {ran}");
    assert!(
        (in_l[0] - 0.25 * loud).abs() < 1e-5,
        "in place 1 left: {}",
        in_l[0]
    );
    assert!(
        (in_r[0] - 0.5 * loud).abs() < 1e-5,
        "in place 1 right: {}",
        in_r[0]
    );

    // Mono: both right pointers null. The left path still runs and the right
    // output buffer must not be touched. The in-place blocks above left the
    // inputs at the processed values, so refill them first.
    in_l.fill(0.25);
    in_r.fill(0.5);
    out_l[511] = -1.0;
    out_r[511] = -7.0;
    let ran = measured("mono 512 frames, null right pointers", || unsafe {
        shell::process(
            instance,
            in_l.as_ptr(),
            std::ptr::null(),
            out_l.as_mut_ptr(),
            std::ptr::null_mut(),
            512,
        )
    });
    assert_eq!(ran, 1, "mono 512 process returned {ran}");
    assert!(
        (out_l[0] - 0.25 * loud).abs() < 1e-5,
        "mono 512 left: {}",
        out_l[0]
    );
    assert_eq!(out_r[511], -7.0, "mono 512 wrote the null right output");

    out_l[0] = -1.0;
    let ran = measured("mono 1 frame, null right pointers", || unsafe {
        shell::process(
            instance,
            in_l.as_ptr(),
            std::ptr::null(),
            out_l.as_mut_ptr(),
            std::ptr::null_mut(),
            1,
        )
    });
    assert_eq!(ran, 1, "mono 1 process returned {ran}");
    assert!(
        (out_l[0] - 0.25 * loud).abs() < 1e-5,
        "mono 1 left: {}",
        out_l[0]
    );
    assert_eq!(out_r[511], -7.0, "mono 1 wrote the null right output");

    // Mono input with the right output connected: the shell duplicates the
    // left input onto the right output.
    let ran = measured(
        "mono input, right output connected, 512 frames",
        || unsafe {
            shell::process(
                instance,
                in_l.as_ptr(),
                std::ptr::null(),
                out_l.as_mut_ptr(),
                out_r.as_mut_ptr(),
                512,
            )
        },
    );
    assert_eq!(ran, 1, "mono-in stereo-out process returned {ran}");
    assert!(
        (out_l[511] - 0.25 * loud).abs() < 1e-5,
        "mono-in left: {}",
        out_l[511]
    );
    assert!(
        (out_r[511] - 0.25 * loud).abs() < 1e-5,
        "mono-in right: {}",
        out_r[511]
    );

    // Parameter setters with ordinary in-range values. `set_normalized` goes
    // through the parameter taper and the plane, never through the plugin.
    let set = measured("set_plain gain -6 dB", || unsafe {
        shell::set_plain(instance, 0, -6.0)
    });
    assert_eq!(set, 1, "set_plain -6 dB returned {set}");
    let set = measured("set_normalized gain 0.5", || unsafe {
        shell::set_normalized(instance, 0, 0.5)
    });
    assert_eq!(set, 1, "set_normalized gain returned {set}");
    let set = measured("set_plain toggle off", || unsafe {
        shell::set_plain(instance, 1, 0.0)
    });
    assert_eq!(set, 1, "set_plain toggle off returned {set}");
    let set = measured("set_normalized toggle 1.0", || unsafe {
        shell::set_normalized(instance, 1, 1.0)
    });
    assert_eq!(set, 1, "set_normalized toggle returned {set}");

    // After the setters the engine holds 0 dB and on: one battery of shapes
    // under a single armed section, as the whole realtime path a host drives.
    let ran = measured(
        "battery: stereo 512 separate, in place and mono",
        || unsafe {
            shell::process(
                instance,
                in_l.as_ptr(),
                in_r.as_ptr(),
                out_l.as_mut_ptr(),
                out_r.as_mut_ptr(),
                512,
            );
            shell::process(
                instance,
                in_l.as_ptr(),
                in_r.as_ptr(),
                in_l.as_mut_ptr(),
                in_r.as_mut_ptr(),
                512,
            );
            shell::process(
                instance,
                in_l.as_ptr(),
                std::ptr::null(),
                out_l.as_mut_ptr(),
                std::ptr::null_mut(),
                512,
            );
            shell::process(
                instance,
                in_l.as_ptr(),
                in_r.as_ptr(),
                out_l.as_mut_ptr(),
                out_r.as_mut_ptr(),
                1,
            )
        },
    );
    assert_eq!(ran, 1, "battery returned {ran}");
    assert!(
        (out_l[511] - 0.25).abs() < 1e-5,
        "battery 0 dB left: {}",
        out_l[511]
    );
    assert!(
        (out_r[0] - 0.5).abs() < 1e-5,
        "battery 0 dB right: {}",
        out_r[0]
    );

    // `destroy` and its table update run outside the measured section.
    unsafe { shell::destroy(instance) };
}
