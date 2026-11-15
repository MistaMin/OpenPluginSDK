//! The virtual analog blocks must not allocate on the audio path, including
//! with NaN and infinite input and parameters. Single test so no other thread
//! allocates while the counters are armed.

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

#[test]
fn virtual_analog_does_not_allocate() {
    // The probe proves the hook sees allocations in this binary.
    ARMED.store(true, Ordering::SeqCst);
    drop(std::hint::black_box(Box::new([0u8; 64])));
    ARMED.store(false, Ordering::SeqCst);
    assert!(ALLOCATIONS.load(Ordering::SeqCst) >= 1 && DEALLOCATIONS.load(Ordering::SeqCst) >= 1);

    let mut tl = TransistorLadder::new(48_000.0);
    let mut dl = DiodeLadder::new(48_000.0);
    let mut kl = Korg35::new(48_000.0);
    let mut kh = Korg35::new(48_000.0);
    kh.set_mode(Korg35Mode::HighPass);
    let mut clip = DiodeClipper::new(48_000.0);
    let mut tree = Series::new(
        Inverter::new(Resistor::new(1000.0)),
        Parallel::new(Inductor::new(0.01, 48_000.0), ResistiveVoltageSource::new(50.0)),
    );
    let root = SingleDiode::new(DiodeParams::D1N4148);
    let mut tape = TapeHysteresis::new(96_000.0);
    tape.set_dc_block(10.0);
    let mut sink = 0.0f32;

    ALLOCATIONS.store(0, Ordering::SeqCst);
    DEALLOCATIONS.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
    for n in 0..20_000usize {
        let t = n as f32;
        let x = match n % 997 {
            0 => f32::NAN,
            1 => f32::INFINITY,
            _ => (t * 0.05).sin() * 2.0,
        };
        let f = if n % 1013 == 0 { f32::NAN } else { 50.0 + 8000.0 * (0.5 + 0.5 * (t * 0.001).sin()) };
        let r = if n % 1009 == 0 { f32::INFINITY } else { 1.2 * (0.5 + 0.5 * (t * 0.002).cos()) };
        tl.set_cutoff(f);
        tl.set_resonance(r);
        dl.set_cutoff(f);
        dl.set_resonance(r);
        kl.set_cutoff(f);
        kl.set_resonance(r);
        kh.set_cutoff(f);
        kh.set_resonance(r);
        tape.set_drive(r * 10.0);
        tape.set_width(r);
        clip.set_resistance(1000.0 + 1000.0 * f64::from(r.min(1.0)));
        sink += tl.process_sample(x) + dl.process_sample(x) + kl.process_sample(x) + kh.process_sample(x);
        sink += clip.process_sample(x) + tape.process_sample(x);
        tree.a.a.set_resistance(500.0 + 500.0 * f64::from(r.min(1.0)));
        tree.refresh();
        tree.b.b.set_voltage(f64::from(x));
        sink += root.process(&mut tree) as f32;
    }
    ARMED.store(false, Ordering::SeqCst);
    std::hint::black_box(sink);
    assert!(sink.is_finite());
    assert_eq!(ALLOCATIONS.load(Ordering::SeqCst), 0, "allocated");
    assert_eq!(DEALLOCATIONS.load(Ordering::SeqCst), 0, "deallocated");
}
