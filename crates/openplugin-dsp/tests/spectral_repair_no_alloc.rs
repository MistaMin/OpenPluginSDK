//! The `SpectralRepair` audio path (process, learning, parameter changes,
//! profile load, reset) must not allocate, deallocate or reallocate, with
//! NaN and infinity in the input and every mode and rule.
//!
//! A counting `#[global_allocator]` forwards to `System` and counts only on the
//! thread that armed it. A probe first proves the counter sees an allocation.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use openplugin_dsp::{GainRule, RepairConfig, RepairMode, RepairParams, SpectralRepair};

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

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if armed() {
            DEALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if armed() {
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
    ARMED.with(|a| a.set(true));
}

fn disarm() -> (usize, usize) {
    ARMED.with(|a| a.set(false));
    (ALLOCATIONS.load(Ordering::SeqCst), DEALLOCATIONS.load(Ordering::SeqCst))
}

#[test]
fn spectral_repair_audio_path_does_not_allocate() {
    arm();
    drop(std::hint::black_box(Box::new([0u8; 64])));
    let (a, d) = disarm();
    assert!(a >= 1 && d >= 1, "allocator hook is not live");

    let mut x: Vec<f32> = (0..16_384)
        .map(|i| (i as f32 * 0.05).sin() * 0.3 + ((i * 2_654_435_761usize) % 1000) as f32 * 1e-4)
        .collect();
    x[100] = f32::NAN;
    x[5000] = f32::INFINITY;
    x[9000] = f32::NEG_INFINITY;
    let mut out = vec![0.0f32; x.len()];
    let mut r = SpectralRepair::new(RepairConfig::new(48_000.0, 2, 1024)).unwrap();
    let bytes = {
        let mut other = SpectralRepair::new(RepairConfig::new(48_000.0, 2, 1024)).unwrap();
        other.start_learning();
        let mut o = vec![0.0f32; 8192];
        other.process(0, &x[..8192], &mut o);
        other.process(1, &x[..8192], &mut o);
        other.stop_learning();
        other.save_profile()
    };

    arm();
    r.start_learning();
    for chunk in x.chunks(37) {
        let mut tmp = [0.0f32; 37];
        let n = chunk.len();
        r.process(0, chunk, &mut tmp[..n]);
        r.process(1, chunk, &mut tmp[..n]);
    }
    r.stop_learning();
    let loaded = r.load_profile(&bytes);
    for (i, rule) in [GainRule::SpectralSubtraction, GainRule::Wiener, GainRule::MmseStsa]
        .into_iter()
        .enumerate()
    {
        for mode in [RepairMode::Reduce, RepairMode::Gate] {
            r.set_params(RepairParams {
                rule,
                mode,
                decision_directed: i != 0,
                listen_removed: i == 1,
                ..RepairParams::default()
            });
            r.process(0, &x, &mut out);
            let mut buf = [0.0f32; 64];
            buf.copy_from_slice(&x[..64]);
            r.process_in_place(1, &mut buf);
            r.tick(0, 0.1);
        }
    }
    r.reset();
    let (a, d) = disarm();
    assert_eq!(loaded, Ok(()));
    assert_eq!((a, d), (0, 0), "audio path allocated {a} / freed {d}");
}
