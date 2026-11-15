//! Configuring and running every filter in the filter library must not allocate.
//!
//! A counting `#[global_allocator]` forwards to `System` and only counts while
//! `ARMED` is set (the same instrument as `openplugin-targets/tests/*_no_alloc.rs`).
//! All filters are built before the measured section; the measured section calls
//! `configure`/`set` and `process_sample` every sample, including with NaN and
//! infinite parameters and input. The probe proves the allocator sees an
//! allocation and a deallocation in this binary, so a broken hook cannot pass
//! vacuously. This file holds a single test so no other thread allocates while
//! the counters are armed.

use openplugin_dsp::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static ARMED: AtomicBool = AtomicBool::new(false);
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

fn reset_counts() {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    DEALLOCATIONS.store(0, Ordering::SeqCst);
}

fn assert_allocator_counts() {
    reset_counts();
    ARMED.store(true, Ordering::SeqCst);
    let probe = std::hint::black_box(Box::new([0u8; 64]));
    drop(std::hint::black_box(probe));
    ARMED.store(false, Ordering::SeqCst);
    assert!(ALLOCATIONS.load(Ordering::SeqCst) >= 1, "allocator did not see the probe allocation");
    assert!(DEALLOCATIONS.load(Ordering::SeqCst) >= 1, "allocator did not see the probe deallocation");
    reset_counts();
}

#[test]
fn filters_do_not_allocate() {
    assert_allocator_counts();

    let mut svf = Svf::new();
    let mut matched = MatchedBiquad::new();
    let mut one_pole = OnePoleFilter::new();
    let mut dc = DcBlocker::new(48_000.0, 20.0);
    let mut ap1 = Allpass1::new();
    let mut ap2 = Allpass2::new();
    let mut butter = Butterworth::new();
    let mut lr = LinkwitzRiley::new(LrOrder::Lr8);
    let mut xo4 = Crossover::new(LrOrder::Lr4, 4, 48_000.0, &[100.0, 1000.0, 8000.0]);
    let mut xo3 = Crossover::new(LrOrder::Lr2, 3, 48_000.0, &[200.0, 4000.0]);
    let mut sink = 0.0f32;

    reset_counts();
    ARMED.store(true, Ordering::SeqCst);
    for n in 0..20_000usize {
        let t = n as f32;
        let x = match n % 997 {
            0 => f32::NAN,
            1 => f32::INFINITY,
            _ => (t * 0.05).sin() * 0.5,
        };
        let f = 50.0 + 5000.0 * (0.5 + 0.5 * (t * 0.001).sin());
        let q = if n % 1013 == 0 { f32::NAN } else { 0.5 + 4.0 * (0.5 + 0.5 * (t * 0.002).cos()) };
        let g = 12.0 * (t * 0.003).sin();
        for mode in [
            SvfMode::LowPass,
            SvfMode::HighPass,
            SvfMode::BandPass,
            SvfMode::Notch,
            SvfMode::Peak,
            SvfMode::AllPass,
            SvfMode::Bell,
            SvfMode::LowShelf,
            SvfMode::HighShelf,
        ] {
            svf.configure(mode, 48_000.0, f, q, g);
            sink += svf.process_sample(x);
            let (l, b, h) = svf.process_split(x);
            sink += l + b + h;
        }
        for kind in [
            MatchedKind::LowPass,
            MatchedKind::HighPass,
            MatchedKind::BandPass,
            MatchedKind::Notch,
            MatchedKind::Peaking,
            MatchedKind::LowShelf,
            MatchedKind::HighShelf,
        ] {
            matched.configure(kind, 96_000.0, f, q, g);
            sink += matched.process_sample(x);
        }
        one_pole.set(OnePoleMode::HighPass, 44_100.0, f);
        sink += one_pole.process_sample(x);
        dc.set(44_100.0, f * 0.01);
        sink += dc.process_sample(x);
        ap1.set(48_000.0, f);
        sink += ap1.process_sample(x);
        ap2.set(48_000.0, f, q);
        sink += ap2.process_sample(x);
        butter.set(ButterworthKind::LowPass, 1 + n % 8, 48_000.0, f);
        sink += butter.process_sample(x);
        lr.set(48_000.0, f);
        let (lo, hi) = lr.process_sample(x);
        sink += lo + hi;
        xo4.set_frequencies(48_000.0, &[f * 0.1, f, f * 2.0]);
        sink += xo4.process_sample(x).iter().sum::<f32>();
        xo3.set_frequencies(48_000.0, &[f, f * 3.0]);
        sink += xo3.process_sample(x).iter().sum::<f32>();
    }
    ARMED.store(false, Ordering::SeqCst);
    std::hint::black_box(sink);

    assert!(sink.is_finite());
    assert_eq!(ALLOCATIONS.load(Ordering::SeqCst), 0, "a filter allocated");
    assert_eq!(DEALLOCATIONS.load(Ordering::SeqCst), 0, "a filter deallocated");
}
