//! The audio paths of the time and pitch blocks must not allocate, deallocate
//! or reallocate: `PitchShifter::process`, `TimeStretch::push` / `pop`,
//! `DelayPitchShifter::process` and both detectors' `detect`, with hostile
//! input (NaN, infinity), odd block sizes and ratio changes in the middle.
//!
//! A counting `#[global_allocator]` as in `spectral_no_alloc.rs`; a probe first
//! proves the counter sees an allocation.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use openplugin_dsp::{
    DelayPitchShifter, MpmDetector, PitchDetector, PitchShifter, TimeStretch, VocoderConfig, YinDetector,
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
fn time_and_pitch_audio_paths_do_not_allocate() {
    arm();
    drop(std::hint::black_box(Box::new([0u8; 64])));
    let (a, d) = disarm();
    assert!(a >= 1 && d >= 1, "allocator hook is not live");

    let fs = 48_000.0;
    let mut input: Vec<f32> = (0..30_000).map(|i| (i as f32 * 0.05).sin() * 0.4 + if i % 5000 == 0 { 0.8 } else { 0.0 }).collect();
    input[1000] = f32::NAN;
    input[1001] = f32::INFINITY;
    input[1002] = f32::NEG_INFINITY;

    let mut cfg = VocoderConfig::for_rate(fs);
    cfg.transients = true;
    let mut shifter = PitchShifter::new(2, fs, cfg);
    let mut stretch = TimeStretch::new(2, fs, cfg);
    let mut delay = DelayPitchShifter::new(2, fs, 30.0, 3);
    let mut yin = YinDetector::new(fs, 50.0, 2000.0);
    let mut mpm = MpmDetector::new(fs, 50.0, 2000.0);
    let mut buf = input.clone();
    let mut b2 = input.clone();
    let mut tmp = vec![0.0f32; 700];
    let mut found = 0.0f32;

    arm();
    // Pitch shifter: odd block sizes, ratio changes, both channels.
    let mut i = 0;
    let mut k = 0;
    while i < buf.len() {
        let n = [1usize, 7, 333, 64, 1024][k % 5].min(buf.len() - i);
        if k % 11 == 0 {
            shifter.set_semitones((k % 25) as f32 - 12.0);
        }
        shifter.process(k % 2, &mut buf[i..i + n]);
        i += n;
        k += 1;
    }
    // Time stretch: push and pop with changing ratio.
    for (j, chunk) in input.chunks(256).enumerate() {
        if j % 17 == 0 {
            stretch.set_ratio(0.5 + (j % 5) as f32 * 0.4);
        }
        let mut rest = chunk;
        while !rest.is_empty() {
            let took = stretch.push(j % 2, rest);
            rest = &rest[took..];
            while stretch.pop(j % 2, &mut tmp) > 0 {}
            if took == 0 {
                break;
            }
        }
    }
    // Delay-line shifter, plain and period-synchronous.
    delay.set_semitones(5.0);
    delay.process(0, &mut b2[..9000]);
    delay.set_period_hz(Some(220.0));
    delay.set_semitones(-9.0);
    delay.process(1, &mut b2[9000..]);
    // Detectors.
    found += yin.detect(&input[..yin.window()]).frequency;
    found += mpm.detect(&input[..mpm.window()]).frequency;
    found += yin.detect(&input[..10]).frequency;
    let (a, d) = disarm();
    assert!(found.is_finite());
    assert!(buf.iter().chain(b2.iter()).all(|v| v.is_finite()));
    assert_eq!((a, d), (0, 0), "audio path allocated {a} / freed {d}");
}
