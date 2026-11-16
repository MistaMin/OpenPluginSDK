//! Scheduling work from `run`, `work_response` and the swap in `Slot::poll`
//! do not allocate or free on the audio thread. The host worker's own `work`
//! call runs outside the measured parts (it is not the audio thread). One
//! `#[test]` in this binary, so nothing else allocates while the counter is on.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use common::worker::{FakeWorker, Loading, NEXT};
use common::*;
use openplugin_core::Plugin;
use openplugin_targets::lv2;

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

fn measured<T>(shape: &str, body: impl FnOnce() -> T) -> T {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    FREES.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
    let value = body();
    ARMED.store(false, Ordering::SeqCst);
    let (a, f) = (ALLOCATIONS.load(Ordering::SeqCst), FREES.load(Ordering::SeqCst));
    assert_eq!((a, f), (0, 0), "{shape}: {a} allocations and {f} frees");
    value
}

#[test]
fn worker_scheduling_and_response_do_not_allocate_on_the_audio_thread() {
    // The control experiment: the counter sees a heap block while armed.
    ARMED.store(true, Ordering::SeqCst);
    let probe = vec![0u8; 64];
    ARMED.store(false, Ordering::SeqCst);
    assert!(ALLOCATIONS.load(Ordering::SeqCst) >= 1);
    drop(probe);

    let host = Host::new();
    let fw = FakeWorker::new();
    let urid = unsafe { *host.features() };
    let features = fw.features(Some(urid));
    lv2::set_creator(|| Box::new(Loading::default()) as Box<dyn Plugin>);
    let desc = unsafe { &*lv2::descriptor(0) };
    let instance = (desc.instantiate)(desc, 48_000.0, std::ptr::null(), features.as_ptr());
    assert!(!instance.is_null());

    let mut input = [vec![1.0f32; 256], vec![1.0f32; 256]];
    let mut output = [vec![0.0f32; 256], vec![0.0f32; 256]];
    for c in 0..2 {
        (desc.connect_port)(instance, c as u32, input[c].as_mut_ptr().cast());
        (desc.connect_port)(instance, c as u32 + 2, output[c].as_mut_ptr().cast());
    }
    let level = std::cell::Cell::new(0.5f32);
    (desc.connect_port)(instance, 4, level.as_ptr().cast());
    (desc.activate)(instance);
    (desc.run)(instance, 256);

    for round in 0..5 {
        // The fake host's std mutexes box their pthread mutex on first use.
        fw.requests();
        fw.deliver(desc, instance);
        level.set(0.6 + round as f32 * 0.05);
        measured("run that queues a job and schedules it", || {
            (desc.run)(instance, 256);
            (desc.run)(instance, 256);
        });
        assert_eq!(fw.requests(), 1, "round {round}");
        // The host's worker thread runs the job (not measured: not the audio thread).
        assert_eq!(fw.run_worker(desc, instance), [0]);
        // Response, the swap in poll (retires the old value) and the follow-up request.
        measured("work_response, swap, retire, end_run", || {
            fw.deliver(desc, instance);
            (desc.run)(instance, 256);
            fw.end_run(desc, instance);
            (desc.run)(instance, 256);
        });
        assert_eq!(output[0][255], NEXT.load(Ordering::SeqCst) as f32, "round {round}");
        // The host frees the retired value.
        assert_eq!(fw.run_worker(desc, instance), [0]);
        fw.clear();
    }
    (desc.deactivate)(instance);
    (desc.cleanup)(instance);
}
