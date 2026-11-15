//! Usage example and proof: design FIR taps on a worker thread, hand them to a
//! running `LongConvolver` through `openplugin_core::worker`, and swap without
//! allocating or freeing on the audio thread. The counting allocator counts
//! only on a thread that armed it, so the worker building and dropping
//! impulses is not counted. A probe proves the counter works.

use openplugin_core::worker::loader;
use openplugin_dsp::partitioned::{ConvolverConfig, Layout};
use openplugin_dsp::*;
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
    (r, ALLOCS.load(Ordering::SeqCst), FREES.load(Ordering::SeqCst))
}

const FS: f64 = 48_000.0;

fn direct(h: &[f32], x: &[f32]) -> Vec<f32> {
    (0..x.len())
        .map(|n| {
            (0..h.len().min(n + 1))
                .map(|k| h[k] as f64 * x[n - k] as f64)
                .sum::<f64>() as f32
        })
        .collect()
}

#[test]
fn designed_taps_swap_into_a_running_convolver_without_allocation() {
    let (_, a, f) = measure(|| drop(std::hint::black_box(vec![1u8; 64])));
    assert!(a >= 1 && f >= 1, "allocator probe saw {a} allocs, {f} frees");

    let config = ConvolverConfig::new(Layout::Mono, 4096);
    let mut conv = config.build().unwrap();
    let mut unit = vec![0.0f32; 1];
    unit[0] = 1.0;
    let (mut loader, mut slot) = loader(config.build_ir(&[&unit]).unwrap(), 4);

    let input: Vec<f32> = (0..16_384)
        .map(|i| ((i as f32 * 0.37).sin() + (i as f32 * 1.9).cos()) * 0.3)
        .collect();

    // The UI asks for a minimum-phase EQ; the worker designs it and transforms it.
    let cfg = config.clone();
    let curve: Vec<EqPoint> = vec![(100.0, 0.0), (1000.0, 9.0), (6000.0, -6.0)];
    let design = move |curve: &[EqPoint]| minimum_phase_eq(curve, FS, 1024).unwrap();
    let expected = design(&curve);
    loader
        .submit(move || {
            let taps = design(&curve);
            cfg.build_ir(&[&taps]).unwrap()
        })
        .unwrap();
    assert!(loader.wait_idle(Duration::from_secs(30)));

    // Audio side. Poll (armed) until the designed taps arrive; the old impulse
    // is retired to the worker, never freed here.
    let start = Instant::now();
    let (mut allocs, mut frees) = (0, 0);
    loop {
        let (swapped, a, f) = measure(|| slot.poll());
        allocs += a;
        frees += f;
        if swapped {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "designed taps never arrived");
        std::thread::yield_now();
    }
    // Then process the whole input in blocks, armed.
    let mut out = Vec::new();
    for chunk in input.chunks(256) {
        let mut block = chunk.to_vec();
        let ((), a, f) = measure(|| {
            slot.poll();
            conv.process_mono(slot.get(), &mut block);
        });
        allocs += a;
        frees += f;
        out.extend_from_slice(&block);
    }
    println!("audio side allocs {allocs}, frees {frees}");
    assert_eq!((allocs, frees), (0, 0), "audio side allocated or freed");

    // The convolver output equals direct convolution with the designed taps.
    let want = direct(&expected, &input);
    let worst = out
        .iter()
        .zip(&want)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    println!("max |convolver - direct| = {worst:.2e}");
    assert!(worst < 1e-3);
}
