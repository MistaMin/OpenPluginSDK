//! The audio side of `worker` (`Slot::poll`, `get`, `get_mut`) must neither
//! allocate nor free, and old values must be dropped off the audio thread.
//!
//! A counting global allocator forwards to `System`. It counts only on a
//! thread that has set its `ARMED` flag, so the worker thread's own
//! allocations (building values, freeing old ones) are not counted. The probe
//! at the start proves the counters see an allocation and a free.

use openplugin_core::worker::loader;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);

struct Counting;

fn armed() -> bool {
    ARMED.try_with(|a| a.get()).unwrap_or(false)
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if armed() {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        if armed() {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if armed() {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(p, l, n) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if armed() {
            FREES.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn measure<R>(f: impl FnOnce() -> R) -> (R, usize, usize) {
    ALLOCS.store(0, Ordering::SeqCst);
    FREES.store(0, Ordering::SeqCst);
    ARMED.with(|a| a.set(true));
    let r = f();
    ARMED.with(|a| a.set(false));
    (r, ALLOCS.load(Ordering::SeqCst), FREES.load(Ordering::SeqCst))
}

#[test]
fn audio_side_pickup_and_swap_never_allocate_or_free() {
    // Probe: the counters see a real allocation and free.
    let (_, a, f) = measure(|| drop(std::hint::black_box(vec![1u8; 64])));
    assert!(a >= 1 && f >= 1, "allocator probe saw {a} allocs, {f} frees");

    let (mut loader, mut slot) = loader(vec![0.0f32; 4096], 8);
    let mut swaps = 0;
    let mut sum = 0.0f32;
    for round in 1..=200u32 {
        while loader.submit(move || vec![round as f32; 4096]).is_err() {
            std::thread::yield_now();
        }
        // Let the result reach the ready queue (worker side, uncounted).
        let start = Instant::now();
        loop {
            let ((changed, v), allocs, frees) = measure(|| {
                let changed = slot.poll();
                (changed, slot.get()[0])
            });
            assert_eq!((allocs, frees), (0, 0), "poll allocated or freed");
            if changed {
                assert_eq!(v, round as f32);
                swaps += 1;
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::yield_now();
        }
        let (s, allocs, frees) = measure(|| slot.get_mut().iter().sum::<f32>());
        assert_eq!((allocs, frees), (0, 0));
        sum += s;
    }
    assert_eq!(swaps, 200);
    assert!(sum > 0.0);
}
