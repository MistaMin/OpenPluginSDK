//! The multiband compressor, dynamic EQ, transient shaper, de-esser and
//! clipper must not allocate while processing (counting allocator, as in
//! `dynamics_no_alloc.rs`; construction and `prepare` happen before arming).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use openplugin_dsp::{
    DeEssDetect, DeEssMode, DeEsser, DynEqKind, DynEqMode, DynamicEqBand, DynamicEqParams, DynamicsParams, LrOrder,
    MultibandCompressor, TransientShaper, TruePeakClipper,
};

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
static DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
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
        if armed() {
            DEALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn counts<R>(f: impl FnOnce() -> R) -> (usize, usize, R) {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    DEALLOCATIONS.store(0, Ordering::SeqCst);
    ARMED.with(|a| a.set(true));
    let r = f();
    ARMED.with(|a| a.set(false));
    (ALLOCATIONS.load(Ordering::SeqCst), DEALLOCATIONS.load(Ordering::SeqCst), r)
}

#[test]
fn multiband_family_processes_without_allocating() {
    let (a, d, _) = counts(|| {
        let v = vec![1u8; 64];
        std::hint::black_box(&v);
    });
    assert!(a >= 1 && d >= 1, "probe saw {a} allocations, {d} deallocations");

    let frames = 600;
    let mut l: Vec<f32> = (0..frames).map(|n| 1.5 * (n as f32 * 0.07).sin()).collect();
    let mut r: Vec<f32> = (0..frames).map(|n| 1.5 * (n as f32 * 0.11).cos()).collect();
    let key: Vec<f32> = (0..frames).map(|n| (n as f32 * 0.3).sin()).collect();
    let mut mb = MultibandCompressor::new(48_000.0, 2, 4, LrOrder::Lr4, &[200.0, 1000.0, 5000.0]);
    let mut eq = DynamicEqBand::new(48_000.0, 2);
    let mut ts = TransientShaper::new(48_000.0);
    let mut de = DeEsser::new(48_000.0, 2);
    let mut clip = TruePeakClipper::prepare(48_000.0, 4, 2, 256).unwrap();

    let (alloc, dealloc, _) = counts(|| {
        for round in 0..4 {
            mb.set_crossovers(&[150.0 + round as f32, 900.0, 4000.0]);
            mb.set_band_solo(round % 4, round % 2 == 0);
            mb.set_band_bypass((round + 1) % 4, round % 2 == 1);
            mb.set_band_link(0.5);
            mb.set_band_params(0, DynamicsParams { threshold_db: -30.0, ..DynamicsParams::default() });
            mb.process(&mut [&mut l, &mut r]);

            eq.set_params(DynamicEqParams {
                kind: if round % 2 == 0 { DynEqKind::Bell } else { DynEqKind::HighShelf },
                mode: if round < 2 { DynEqMode::Above } else { DynEqMode::Below },
                ..DynamicEqParams::default()
            });
            eq.process(&mut [&mut l, &mut r]);
            eq.process_sidechain(&mut [&mut l, &mut r], &[&key]);

            ts.set_attack(0.5);
            ts.set_sustain(-0.5);
            ts.process(&mut [&mut l, &mut r]);

            de.set_mode(if round % 2 == 0 { DeEssMode::SplitBand } else { DeEssMode::WideBand });
            de.set_detect(if round < 2 { DeEssDetect::BandPass } else { DeEssDetect::HighPass });
            de.process(&mut [&mut l, &mut r]);
            de.process_sidechain(&mut [&mut l, &mut r], &[&key]);

            clip.set_ceiling_db(-1.0 - round as f32);
            clip.set_knee_db(round as f32);
            clip.process(&mut [&mut l, &mut r]);
        }
    });
    assert_eq!((alloc, dealloc), (0, 0), "process allocated");
    assert!(l.iter().chain(&r).all(|v| v.is_finite()));
}
