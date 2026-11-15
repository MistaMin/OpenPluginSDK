//! The synthesis blocks must not allocate, deallocate or reallocate once built.
//!
//! A counting `#[global_allocator]` forwards to `System` and counts only while
//! `ARMED` is set. A probe first proves the counter sees an allocation, so the
//! test cannot pass vacuously. Construction (tables, kernels) happens before
//! arming: it is allowed to allocate, which is why it is done off the audio thread.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use openplugin_dsp::{
    Adsr, BandlimitedOsc, BlWave, BlepOrder, Curve, Gate, GlideMode, PinkNoise, Quality,
    RetriggerMode, SincKernel, SincPlayer, StealPolicy, VoiceAllocator, VoiceMode, WavetableOsc,
    WavetableSet, WhiteNoise,
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
fn synthesis_blocks_do_not_allocate() {
    // Probe: the counter must see an allocation and a free.
    arm();
    drop(std::hint::black_box(Box::new([0u8; 64])));
    let (a, d) = disarm();
    assert!(a >= 1 && d >= 1, "allocator hook is not live");

    // Everything that may allocate is built first.
    let table = WavetableSet::saw(2048).unwrap();
    let kernel = SincKernel::new(Quality::Medium);
    let sample: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.05).sin()).collect();
    let mut out = vec![0.0f32; 512];
    let mut env_out = vec![0.0f32; 512];
    let mut osc: Vec<BandlimitedOsc> = [BlWave::Saw, BlWave::Square, BlWave::Pulse, BlWave::Triangle]
        .into_iter()
        .map(BandlimitedOsc::new)
        .collect();
    let mut wt = WavetableOsc::new();
    let mut env = Adsr::new(48_000.0);
    let mut white = WhiteNoise::new(1);
    let mut pink = PinkNoise::new(48_000.0, 2);
    let mut alloc = VoiceAllocator::<[f32; 4], 6>::new(48_000.0);
    let mut player = SincPlayer::new();
    player.start(1.37);
    player.set_loop(Some((100, 3000)));

    arm();
    // Band-limited oscillators, all orders, with sync, PWM and hostile input.
    for order in [BlepOrder::Two, BlepOrder::Four, BlepOrder::Six, BlepOrder::Eight] {
        for o in osc.iter_mut() {
            *o = o.with_order(order);
            for i in 0..512 {
                o.set_pulse_width(0.1 + 0.8 * (i as f32 / 512.0));
                let hz = if i % 97 == 0 { f32::NAN } else { 100.0 + i as f32 * 37.0 };
                let sync = if i % 5 == 0 { Some(0.37) } else { None };
                out[i] = o.next_sync(hz, 44_100.0, sync);
            }
        }
    }
    // Wavetable.
    for i in 0..512 {
        out[i] = wt.next(&table, 20.0 + i as f32 * 40.0, 48_000.0);
    }
    // Envelope: retrigger, legato, both curves, events.
    for mode in [RetriggerMode::Retrigger, RetriggerMode::Legato] {
        env.set_mode(mode);
        for curve in [Curve::Linear, Curve::Exponential] {
            env.set_curves(curve, curve, curve);
            env.set_ahdsr(0.002, 0.001, 0.003, 0.5, 0.004);
            env.process(&mut env_out, &[(3, Gate::On), (200, Gate::On), (300, Gate::Off), (400, Gate::On)]);
        }
    }
    // Noise.
    white.fill(&mut out);
    pink.fill(&mut out);
    pink.set_sample_rate(96_000.0);
    pink.fill(&mut out);
    // Allocator: fill, steal under each policy, mono, legato, glide, pedal.
    for policy in [StealPolicy::Oldest, StealPolicy::Quietest, StealPolicy::SameNote] {
        alloc.set_policy(policy);
        for n in 40..70u8 {
            alloc.note_on(n, 100);
            if n % 3 == 0 {
                alloc.note_off(n - 2);
            }
        }
        alloc.set_sustain_pedal(true, |_| {});
        alloc.note_off(69);
        alloc.set_sustain_pedal(false, |_| {});
        alloc.all_notes_off(|_| {});
        for k in 0..6 {
            alloc.free(k);
        }
    }
    for mode in [VoiceMode::Mono, VoiceMode::Legato, VoiceMode::Poly] {
        alloc.set_mode(mode);
        alloc.set_glide(0.05, GlideMode::Always);
        for n in [60u8, 64, 67, 72] {
            alloc.note_on(n, 90);
            for k in 0..6 {
                alloc.voice_mut(k).unwrap().advance(100);
            }
        }
        for n in [72u8, 67, 64, 60] {
            alloc.note_off(n);
        }
        alloc.reset();
    }
    // Resampler.
    for ratio in [0.25, 1.0, 1.5, 7.0] {
        kernel.resample(&sample, ratio, &mut out);
        out[0] = kernel.interpolate(&sample, 1234.5, ratio, Some((10, 4000)));
    }
    for _ in 0..2000 {
        out[0] = player.next(&kernel, &sample);
    }
    let (a, d) = disarm();
    assert_eq!((a, d), (0, 0), "audio path allocated {a} / freed {d}");
    assert!(out.iter().all(|v| v.is_finite()) && env_out.iter().all(|v| v.is_finite()));
}
