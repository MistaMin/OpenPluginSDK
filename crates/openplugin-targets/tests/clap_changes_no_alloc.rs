//! Flagging a change on the audio thread allocates nothing in CLAP: the
//! plugin's `HostNotifier` calls inside `process`, the comparison of the
//! latency after the block, and the host's `request_restart` and
//! `request_callback` (the fake host only counts). A counting global allocator
//! is armed around the measured blocks only; a probe first proves it counts.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use common::clap_host::{mark_main_thread, Audio, ChangeHost, Events};
use common::shift::{Calls, Shift};
use openplugin_targets::clap_abi::*;
use openplugin_targets::ClapAdapter;

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
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.load(Ordering::SeqCst) {
            DEALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if ARMED.load(Ordering::SeqCst) {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn measured(body: impl FnOnce()) -> (usize, usize) {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    DEALLOCATIONS.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
    body();
    ARMED.store(false, Ordering::SeqCst);
    (ALLOCATIONS.load(Ordering::SeqCst), DEALLOCATIONS.load(Ordering::SeqCst))
}

#[test]
fn flagging_latency_and_parameter_info_in_process_does_not_allocate() {
    let (probe_alloc, probe_free) = measured(|| drop(std::hint::black_box(Box::new([0u8; 64]))));
    assert!(probe_alloc >= 1 && probe_free >= 1, "the counting allocator is not live");

    mark_main_thread();
    let host = ChangeHost::new(true);
    let ch = host.clap_host();
    let calls = Arc::new(Calls::default());
    let mut adapter = unsafe { ClapAdapter::with_host(Shift::new(calls.clone()), &ch) };
    let p = adapter.as_plugin();
    assert!(adapter.activate(48_000.0, 1, 64));
    let mut audio = Audio::new(64);
    let both = Events::new(&[(0, 64.0), (1, 1.0)]);
    let latency_only = Events::new(&[(0, 12.0)]);
    let names_only = Events::new(&[(1, 0.0)]);
    // Warm up once outside the measured section (first touches of lazily built statics).
    assert_eq!(unsafe { audio.run(p, None) }, PROCESS_CONTINUE);

    let (allocations, frees) = measured(|| {
        for events in [Some(&*both), None, Some(&*latency_only), Some(&*names_only), None] {
            assert_eq!(unsafe { audio.run(p, events) }, PROCESS_CONTINUE);
        }
    });
    assert_eq!((allocations, frees), (0, 0), "process allocated while flagging changes");
    // The measured blocks did flag the changes and the host was asked.
    assert_eq!(calls.latency.load(Ordering::SeqCst), 2);
    assert_eq!(calls.param_info.load(Ordering::SeqCst), 2);
    assert_eq!(host.count(&host.restarts), 1);
    assert_eq!(host.count(&host.callbacks), 1);
}
