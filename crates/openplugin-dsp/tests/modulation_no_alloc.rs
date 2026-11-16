//! The modulation and delay blocks must not allocate, deallocate or reallocate
//! after construction.
//!
//! A counting `#[global_allocator]` forwards to `System` and counts only on the
//! thread that armed it, as in the targets crate's `*_no_alloc` tests. A probe first
//! proves the counter sees an allocation, so the test cannot pass vacuously.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use openplugin_core::Transport;
use openplugin_dsp::{
    Chorus, Feel, Flanger, Interpolation, Lfo, LfoRate, LfoShape, ModDelay, NoteDivision, Phaser,
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
    (
        ALLOCATIONS.load(Ordering::SeqCst),
        DEALLOCATIONS.load(Ordering::SeqCst),
    )
}

#[test]
fn modulation_blocks_do_not_allocate() {
    // Probe: the counter must see an allocation and a free.
    arm();
    drop(std::hint::black_box(Box::new([0u8; 64])));
    let (a, d) = disarm();
    assert!(a >= 1 && d >= 1, "allocator hook is not live");

    let mut lines: Vec<ModDelay> = [
        Interpolation::Linear,
        Interpolation::Hermite,
        Interpolation::Lagrange(3),
        Interpolation::Lagrange(7),
        Interpolation::Thiran(1),
        Interpolation::Thiran(4),
    ]
    .iter()
    .map(|i| ModDelay::new(48_000.0, 50.0, *i))
    .collect();
    let mut lfo = Lfo::new(48_000.0);
    let mut chorus = Chorus::new(2, 48_000.0);
    let mut flanger = Flanger::new(2, 48_000.0);
    let mut phaser = Phaser::new(2, 48_000.0);
    let mut transport = Transport::default();
    transport.playing = true;
    transport.tempo = 133.0;
    transport.quarter_notes = 17.25;
    let mut buf = vec![0.0f32; 256];
    for (i, s) in buf.iter_mut().enumerate() {
        *s = (i as f32 * 0.1).sin();
    }

    arm();
    for round in 0..4 {
        for (k, line) in lines.iter_mut().enumerate() {
            for (i, s) in buf.iter().enumerate() {
                let d = 30.0 + 20.0 * ((i + k) as f32 * 0.05).sin();
                std::hint::black_box(line.process(*s, d));
            }
            line.set_interpolation(Interpolation::Hermite);
            line.set_interpolation(Interpolation::Thiran(3));
            line.reset();
        }
        for shape in [
            LfoShape::Sine,
            LfoShape::Triangle,
            LfoShape::Saw,
            LfoShape::Square,
            LfoShape::SampleHold,
            LfoShape::SmoothRandom,
        ] {
            lfo.set_shape(shape);
            lfo.set_rate(LfoRate::Sync(NoteDivision::new(8, Feel::Triplet)));
            lfo.set_transport(&transport);
            lfo.fill(&mut buf);
            lfo.set_rate(LfoRate::Hz(2.5));
            lfo.fill(&mut buf);
        }
        for s in buf.iter_mut() {
            *s = (*s * 0.5).clamp(-1.0, 1.0);
        }
        chorus.set_transport(&transport);
        flanger.set_transport(&transport);
        phaser.set_transport(&transport);
        for ch in 0..2 {
            chorus.set_voices(1 + round * 2);
            chorus.set_rate(LfoRate::Sync(NoteDivision::new(4, Feel::Straight)));
            chorus.set_interpolation(Interpolation::Lagrange(3));
            chorus.process(ch, &mut buf);
            flanger.set_through_zero(round % 2 == 0);
            flanger.set_feedback(0.7);
            flanger.set_rate(LfoRate::Quarters(8.0));
            flanger.process(ch, &mut buf);
            phaser.set_stages(2 + round * 2);
            phaser.set_feedback(-0.6);
            phaser.set_range(100.0, 5000.0);
            phaser.process(ch, &mut buf);
        }
        chorus.reset();
        flanger.reset();
        phaser.reset();
    }
    let (a, d) = disarm();
    assert_eq!((a, d), (0, 0), "audio path allocated {a} / freed {d}");
}
