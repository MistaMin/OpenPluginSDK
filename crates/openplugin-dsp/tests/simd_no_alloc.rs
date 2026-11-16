//! The SIMD kernels and `BiquadBank4::process` do not allocate.
//! One `#[test]` in this binary, and the counter is armed per thread, so nothing
//! else in the process can add to the count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use openplugin_dsp::simd::{BiquadBank4, Kernels};

thread_local! {
    // Armed per thread: the test harness's own threads (output capture, the
    // "running for over 60 seconds" notice) never add to the count. `const`
    // initialisation, so reading it inside the allocator never allocates.
    static ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn armed() -> bool {
    ARMED.try_with(|a| a.get()).unwrap_or(false)
}
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if armed() {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if armed() {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if armed() {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static A: Counting = Counting;

#[test]
fn kernels_do_not_allocate() {
    let n = 1000;
    let a: Vec<f32> = (0..n).map(|i| (i as f32 * 0.01).sin()).collect();
    let b = a.clone();
    let mut d = vec![0.0_f32; n];
    let mut acc = vec![0.0_f32; 2 * n];
    let x2 = vec![0.5_f32; 2 * n];
    let taps = vec![0.1_f32; 33];
    let xs = vec![0.2_f32; n + 32];
    let mut y = vec![0.0_f32; 10];
    let w = vec![0.3_f32; 10 * n];
    let mut io = vec![0.1_f32; 4 * 64];
    let mut p: Vec<Vec<f32>> = (0..4).map(|_| vec![0.1_f32; 100]).collect();
    let paths = Kernels::available();
    let mut banks: Vec<BiquadBank4> = paths
        .iter()
        .map(|k| BiquadBank4::with_kernels(&[[0.2, 0.4, 0.2, -0.5, 0.2]; 6], *k))
        .collect();

    // Probe: the counter sees an allocation on this thread, so the test
    // cannot pass with a dead hook.
    ARMED.with(|a| a.set(true));
    drop(std::hint::black_box(Box::new([0u8; 64])));
    ARMED.with(|a| a.set(false));
    assert!(ALLOCATIONS.load(Ordering::SeqCst) >= 1, "allocator hook is not live");
    ALLOCATIONS.store(0, Ordering::SeqCst);

    ALLOCATIONS.store(0, Ordering::SeqCst);
    ARMED.with(|a| a.set(true));
    for (k, bank) in paths.iter().zip(banks.iter_mut()) {
        k.gain(&mut d, &a, 0.5);
        k.scale(&mut d, 0.5);
        k.mix_add(&mut d, &b, 0.5);
        k.crossfade(&mut d, &a, &b, 0.0, 0.001);
        k.cmac_interleaved(&mut acc, &x2, &x2);
        k.fir(&mut d[..n], &xs, &taps);
        std::hint::black_box(k.dot(&a, &b));
        k.matvec(&mut y, &w, &a, None);
        bank.process_interleaved(&mut io);
        let [c0, c1, c2, c3] = &mut p[..] else {
            unreachable!()
        };
        bank.process([c0, c1, c2, c3]);
    }
    ARMED.with(|a| a.set(false));
    assert_eq!(ALLOCATIONS.load(Ordering::SeqCst), 0);
}
