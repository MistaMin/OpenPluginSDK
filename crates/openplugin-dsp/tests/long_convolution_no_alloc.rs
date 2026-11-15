//! `LongConvolver::process` (and `Slot::poll` around an impulse swap) must
//! neither allocate nor free. A counting global allocator forwards to `System`
//! and counts only on a thread that armed it, so the worker thread building
//! and dropping impulses is not counted. A probe proves the counter works.

use openplugin_core::worker::loader;
use openplugin_dsp::partitioned::{ConvolverConfig, Layout};
use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
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
    unsafe fn alloc(&self, l: AllocLayout) -> *mut u8 {
        if armed() {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: AllocLayout) -> *mut u8 {
        if armed() {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: AllocLayout, n: usize) -> *mut u8 {
        if armed() {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(p, l, n) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: AllocLayout) {
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
    (
        r,
        ALLOCS.load(Ordering::SeqCst),
        FREES.load(Ordering::SeqCst),
    )
}

fn taps(len: usize, seed: f32) -> Vec<f32> {
    (0..len)
        .map(|i| ((i as f32 * 0.37 + seed).sin()) / (len as f32).sqrt())
        .collect()
}

fn build(config: &ConvolverConfig, len: usize, seed: f32) -> openplugin_dsp::ConvolutionIr {
    let h: Vec<Vec<f32>> = (0..config.layout().filters())
        .map(|k| taps(len, seed + k as f32))
        .collect();
    let refs: Vec<&[f32]> = h.iter().map(|v| v.as_slice()).collect();
    config.build_ir(&refs).unwrap()
}

#[test]
fn process_and_impulse_swap_never_allocate() {
    let (_, a, f) = measure(|| drop(std::hint::black_box(vec![1u8; 64])));
    assert!(
        a >= 1 && f >= 1,
        "allocator probe saw {a} allocs, {f} frees"
    );

    for layout in [Layout::Mono, Layout::Stereo, Layout::TrueStereo] {
        let config = ConvolverConfig::new(layout, 48_000);
        let mut conv = config.build().unwrap();
        let (mut loader, mut slot) = loader(build(&config, 2000, 0.0), 4);

        let mut left: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut right = left.clone();

        let sizes = [1usize, 7, 64, 512, 4096, 33, 1000];
        let mut swaps = 0;
        for round in 0..24usize {
            if round % 4 == 0 {
                let cfg = config.clone();
                let len = 5_000 + 9_000 * (round % 5);
                while loader
                    .submit({
                        let cfg = cfg.clone();
                        move || build(&cfg, len, round as f32)
                    })
                    .is_err()
                {
                    std::thread::yield_now();
                }
                loader.wait_idle(Duration::from_secs(20));
            }
            let size = sizes[round % sizes.len()];
            let start = Instant::now();
            let ((), allocs, frees) = measure(|| {
                for _ in 0..8 {
                    if slot.poll() {
                        swaps += 1;
                    }
                    let ir = slot.get();
                    match layout {
                        Layout::Mono => conv.process_mono(ir, &mut left[..size]),
                        _ => conv.process_stereo(ir, &mut left[..size], &mut right[..size]),
                    }
                }
            });
            assert_eq!(
                (allocs, frees),
                (0, 0),
                "{layout:?} block {size} allocated or freed"
            );
            assert!(start.elapsed() < Duration::from_secs(60));
        }
        assert!(swaps >= 1, "{layout:?}: no impulse was swapped in");
        assert!(left.iter().all(|v| v.is_finite()));
    }
}
