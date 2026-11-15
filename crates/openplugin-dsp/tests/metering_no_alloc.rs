//! The utility and metering blocks must not allocate, deallocate or
//! reallocate on the audio path, and the scope feed must not block the writer
//! while a reader is busy.
//!
//! A counting `#[global_allocator]` forwards to `System` and counts only while
//! `ARMED` is set, as in `dither_no_alloc.rs`. A probe first proves the
//! counter sees an allocation, so the test cannot pass vacuously.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use openplugin_dsp::utility::*;
use openplugin_dsp::{
    scope_feed, EbuMeter, KMeter, KScale, Loudness, PanLaw, Panner, Ppm, PpmType, TruePeakMeter, VuIec,
};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
fn utility_and_metering_do_not_allocate() {
    let _serial = SERIAL.lock().unwrap();
    arm();
    drop(std::hint::black_box(Box::new([0u8; 64])));
    let (a, d) = disarm();
    assert!(a >= 1 && d >= 1, "allocator hook is not live");

    let rate = 48_000.0;
    let l0: Vec<f32> = (0..4800).map(|i| 0.5 * (i as f32 * 0.07).sin()).collect();
    let r0: Vec<f32> = (0..4800).map(|i| 0.4 * (i as f32 * 0.05).cos()).collect();
    let (mut l, mut r) = (l0.clone(), r0.clone());
    let (mut ol, mut or) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);

    let mut gain = Gain::new(rate, 0.0);
    let mut panner = Panner::new(rate, PanLaw::Minus4p5);
    let mut peak = TruePeakMeter::new(rate, 2);
    let mut ppms = [
        Ppm::new(rate, PpmType::Ebu),
        Ppm::new(rate, PpmType::Din),
        Ppm::new(rate, PpmType::Nordic),
        Ppm::new(rate, PpmType::Bbc),
    ];
    let mut k = KMeter::new(rate, KScale::K20, 2);
    let mut vu = VuIec::new(rate);
    let mut loud = Loudness::new(rate);
    let mut ebu = EbuMeter::stereo(rate);
    let (mut writer, reader) = scope_feed(rate);

    arm();
    {
        let _guard = DenormalGuard::new();
        ms_encode_in_place(&mut l, &mut r);
        ms_decode_in_place(&mut l, &mut r);
        stereo_width_in_place(&mut l, &mut r, 1.5);
        gain.set_db(-6.0);
        gain.process_stereo(&mut l, &mut r);
        panner.set_pan(0.3);
        panner.pan_mono(&l0, &mut ol, &mut or);
        panner.balance(&mut l, &mut r);
        peak.process(0, &l);
        peak.process(1, &r);
        for p in &mut ppms {
            p.process(&l);
        }
        k.process(0, &l);
        k.process(1, &r);
        vu.process(&l);
        loud.push(&[&l, &r]);
        ebu.push(&[&l, &r]);
        for _ in 0..20 {
            writer.push(&l, &r);
        }
        let _ = (peak.max_all_dbtp(), k.rms_k(0), loud.loudness_range_lu(), ebu.integrated_lu(), reader.read().correlation);
    }
    let (a, d) = disarm();
    assert_eq!((a, d), (0, 0), "audio path allocated {a} / freed {d}");
}

/// The writer never waits for a reader: with readers hammering the slot the
/// writer still finishes a fixed amount of work in a bounded time and
/// publishes every frame.
#[test]
fn scope_writer_is_not_blocked_by_readers() {
    let _serial = SERIAL.lock().unwrap();
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    let (mut writer, reader) = scope_feed(48_000.0);
    let stop = Arc::new(AtomicBool::new(false));
    let readers: Vec<_> = (0..3)
        .map(|_| {
            let reader = reader.clone();
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    std::hint::black_box(reader.read());
                    n += 1;
                }
                n
            })
        })
        .collect();
    let block = vec![0.25f32; 4800];
    let start = std::time::Instant::now();
    for _ in 0..2000 {
        writer.push(&block, &block);
    }
    let took = start.elapsed();
    stop.store(true, Ordering::Relaxed);
    let reads: u64 = readers.into_iter().map(|h| h.join().unwrap()).sum();
    assert!(reads > 0);
    assert!(took.as_secs_f32() < 5.0, "writer took {took:?}");
    assert!(reader.read().sequence > 0);
}
