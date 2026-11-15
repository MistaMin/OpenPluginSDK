//! The physical models must not allocate, deallocate or reallocate after
//! construction, including when their settings change on the audio path.
//!
//! A counting `#[global_allocator]` forwards to `System` and counts only while
//! `ARMED` is set, as in `modulation_no_alloc.rs`. A probe first proves the
//! counter sees an allocation, so the test cannot pass vacuously.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use openplugin_dsp::{
    BoreEnd, Exciter, ModalBank, ModalPreset, PluckedString, PresetSettings, Strike, Tuning,
    WaveguideBore, WaveguideString,
};

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
fn physical_models_do_not_allocate() {
    // Probe: the counter must see an allocation and a free.
    arm();
    drop(std::hint::black_box(Box::new([0u8; 64])));
    let (a, d) = disarm();
    assert!(a >= 1 && d >= 1, "allocator hook is not live");

    let mut ks = PluckedString::new(48_000.0, 20.0);
    let mut ws = WaveguideString::new(48_000.0, 20.0);
    let mut bore = WaveguideBore::new(48_000.0, 20.0, BoreEnd::Closed, BoreEnd::Open);
    let mut bank = ModalBank::new(48_000.0, 40);
    let mut ex = Exciter::new(48_000.0, Strike::Mallet { contact_ms: 1.0 });
    let mut out = vec![0.0f32; 512];

    arm();
    let mut acc = 0.0f32;
    for round in 0..40 {
        let hz = 55.0 * 1.07f32.powi(round % 30);
        ks.set_frequency(hz);
        ks.set_decay(0.5 + (round % 5) as f32);
        ks.set_stretch(0.05 * (round % 8) as f32);
        ks.set_stiffness(1e-4 * (round % 4) as f32);
        ks.set_tuning(if round % 2 == 0 { Tuning::Allpass } else { Tuning::Integer });
        ks.set_pick_position(0.1 + 0.05 * (round % 6) as f32);
        ks.pluck(0.3 + 0.02 * round as f32);
        ks.process(&mut out);
        acc += out[7] + ks.next_with(0.1);

        ws.set_frequency(hz);
        ws.set_stiffness(2e-4);
        ws.set_input_position(0.3);
        ws.pluck(0.8);
        ws.process(&mut out);
        acc += out[9] + ws.next_with(1.0);

        bore.set_frequency(hz);
        bore.set_ends(if round % 3 == 0 { BoreEnd::Open } else { BoreEnd::Closed }, BoreEnd::Open);
        out.iter_mut().for_each(|v| *v = 0.0);
        out[0] = 1.0;
        bore.process(&mut out);
        acc += out[11];

        bank.load_preset(ModalPreset::Membrane, &PresetSettings { f1: hz, ..PresetSettings::default() });
        bank.load_preset(
            ModalPreset::Plate { aspect: 1.4 },
            &PresetSettings { f1: hz, modes: 30, ..PresetSettings::default() },
        );
        bank.set_mode(3, hz * 2.0, 1.0, 0.5);
        ex.trigger(0.9);
        for v in out.iter_mut() {
            *v = ex.next();
        }
        bank.process(&mut out);
        acc += out[13];
    }
    let (allocs, deallocs) = disarm();
    assert!(acc.is_finite());
    assert_eq!((allocs, deallocs), (0, 0), "physical models allocated on the audio path");
}
