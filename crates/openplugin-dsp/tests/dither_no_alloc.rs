//! `Dither::process*` must not allocate, deallocate or reallocate.
//!
//! A counting `#[global_allocator]` forwards to `System` and counts only while
//! `ARMED` is set, as in the targets crate's `*_no_alloc` tests. A probe first
//! proves the counter sees an allocation, so the test cannot pass vacuously.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use openplugin_dsp::{Dither, DitherKind, NoiseShaping};

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
    (
        ALLOCATIONS.load(Ordering::SeqCst),
        DEALLOCATIONS.load(Ordering::SeqCst),
    )
}

#[test]
fn process_does_not_allocate() {
    // Probe: the counter must see an allocation and a free.
    arm();
    drop(std::hint::black_box(Box::new([0u8; 64])));
    let (a, d) = disarm();
    assert!(a >= 1 && d >= 1, "allocator hook is not live");

    let mut dither = Dither::new(2, 48_000.0, 16, 1234);
    let input: Vec<f32> = (0..512).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();
    let silence = vec![0.0f32; 512];
    let mut f = input.clone();
    let mut s = vec![0i16; 512];
    let mut t = vec![0i32; 512];
    let mut z = silence.clone();

    arm();
    for shaping in [NoiseShaping::None, NoiseShaping::FirstOrder, NoiseShaping::Perceptual] {
        // Reconfiguring (including redesigning the filter) is allocation free too.
        dither.set_shaping(shaping);
        dither.set_sample_rate(44_100.0);
        for kind in [DitherKind::None, DitherKind::Rpdf, DitherKind::Tpdf] {
            dither.set_kind(kind);
            for bits in [8, 16, 24] {
                dither.set_bits(bits);
                for ch in 0..2 {
                    f.copy_from_slice(&input);
                    dither.process(ch, &mut f);
                    dither.process_i16(ch, &input, &mut s);
                    dither.process_i24(ch, &input, &mut t);
                    dither.process_i32(ch, &input, &mut t);
                    f.copy_from_slice(&silence);
                    dither.process(ch, &mut f);
                }
            }
        }
    }
    dither.set_blank_after(16);
    dither.process(0, &mut z);
    let (a, d) = disarm();
    assert_eq!((a, d), (0, 0), "process allocated {a} / freed {d}");
}
