//! The reverbs must not allocate, deallocate or reallocate in `process*` or in
//! any setter. A counting `#[global_allocator]` forwards to `System` and counts
//! only while `ARMED` is set; a probe proves the counter is live first.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use openplugin_dsp::{EarlyPattern, EarlyReflections, EarlyTap, FdnMatrix, FdnReverb, PlateReverb};

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

fn arm() {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    DEALLOCATIONS.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
}

fn disarm() -> (usize, usize) {
    ARMED.store(false, Ordering::SeqCst);
    (ALLOCATIONS.load(Ordering::SeqCst), DEALLOCATIONS.load(Ordering::SeqCst))
}

#[test]
fn reverbs_do_not_allocate() {
    arm();
    drop(std::hint::black_box(Box::new([0u8; 64])));
    let (a, d) = disarm();
    assert!(a >= 1 && d >= 1, "allocator hook is not live");

    let mut fdn8 = FdnReverb::new(48_000.0, 8);
    let mut fdn16 = FdnReverb::new(96_000.0, 16);
    let mut plate = PlateReverb::new(44_100.0);
    let mut early = EarlyReflections::new(48_000.0);
    let mut l: Vec<f32> = (0..512).map(|i| (i as f32 * 0.03).sin()).collect();
    let mut r = l.clone();
    let nan = vec![f32::NAN; 512];

    arm();
    for round in 0..3 {
        for fdn in [&mut fdn8, &mut fdn16] {
            fdn.set_rt60(1.0 + round as f32, 2.0, 0.5);
            fdn.set_reference_hz(100.0, 900.0, 7000.0);
            fdn.set_size(0.3 + round as f32);
            fdn.set_matrix(if round % 2 == 0 { FdnMatrix::Hadamard } else { FdnMatrix::Householder });
            fdn.set_modulation(0.5, 1.0);
            fdn.set_predelay_ms(40.0 * round as f32);
            fdn.set_width(0.5 + round as f32 * 0.5);
            fdn.set_freeze(round == 1);
            fdn.early_mut().set_level(0.5);
            fdn.early_mut().set_pattern(EarlyPattern::Hall);
            fdn.early_mut().set_size(1.5);
            let _ = fdn.tail_samples();
            fdn.process(&mut l, &mut r);
            l.copy_from_slice(&nan);
            fdn.process(&mut l, &mut r);
            l.iter_mut().enumerate().for_each(|(i, v)| *v = (i as f32 * 0.03).sin());
            fdn.reset();
        }
        plate.set_rt60(1.0 + round as f32);
        plate.set_damping_hz(5000.0);
        plate.set_bandwidth_hz(10_000.0);
        plate.set_modulation(0.7, 1.3);
        plate.set_predelay_ms(10.0 * round as f32);
        plate.set_width(1.5);
        plate.set_freeze(round == 2);
        let _ = plate.tail_samples();
        plate.process(&mut l, &mut r);
        plate.reset();

        early.set_taps(&[EarlyTap { time_ms: 12.0, gain: 0.5, pan: 0.2 }]);
        early.set_pattern(EarlyPattern::Dense);
        early.set_size(2.0);
        early.set_level(0.7);
        early.process(&mut l, &mut r);
        early.reset();
    }
    let (a, d) = disarm();
    assert_eq!((a, d), (0, 0), "reverb allocated {a} / freed {d}");
}
