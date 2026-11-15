//! The dynamics blocks must not allocate while processing.
//!
//! Same method as `crates/openplugin-targets/tests/clap_no_alloc.rs`: a
//! counting `#[global_allocator]` forwards to `System` and counts only while
//! `ARMED`. Everything that may allocate (construction, `prepare`) happens
//! before arming. A probe first proves the allocator really sees an allocation
//! and a deallocation in this binary, so the test cannot pass vacuously.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use openplugin_dsp::{
    Detection, Domain, Dynamics, DynamicsMode, DynamicsParams, EnvelopeFollower, Limiter, Link, Smoothing, Topology,
    TruePeak,
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
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ARMED.load(Ordering::SeqCst) {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.load(Ordering::SeqCst) {
            DEALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn counts<R>(f: impl FnOnce() -> R) -> (usize, usize, R) {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    DEALLOCATIONS.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
    let r = f();
    ARMED.store(false, Ordering::SeqCst);
    (ALLOCATIONS.load(Ordering::SeqCst), DEALLOCATIONS.load(Ordering::SeqCst), r)
}

#[test]
fn dynamics_and_limiter_process_without_allocating() {
    // Probe: the counters see an allocation and a deallocation.
    let (a, d, _) = counts(|| {
        let v = vec![1u8; 64];
        std::hint::black_box(&v);
    });
    assert!(a >= 1 && d >= 1, "probe saw {a} allocations, {d} deallocations");

    // Setup, unmeasured.
    let frames = 512;
    let mut sig: Vec<Vec<f32>> = (0..6)
        .map(|c| (0..frames).map(|n| 1.5 * ((n + 7 * c) as f32 * 0.07).sin()).collect())
        .collect();
    let key: Vec<f32> = (0..frames).map(|n| (n as f32 * 0.3).sin()).collect();
    let mut dynamics = Dynamics::new(48_000.0);
    let mut limiter = Limiter::prepare(48_000.0, 6, 2.0, 80.0);
    let mut follower = EnvelopeFollower::new(48_000.0);
    let mut tp = TruePeak::precise();
    let mut filter_state = [0.0f32; 6];

    let (alloc, dealloc, _) = counts(|| {
        for round in 0..4 {
            for mode in [DynamicsMode::Compressor, DynamicsMode::Expander, DynamicsMode::Gate] {
                for (smoothing, topology, detection, domain, link) in [
                    (Smoothing::Level, Topology::Branching, Detection::Peak, Domain::Log, Link::Max),
                    (Smoothing::Gain, Topology::Decoupled, Detection::Rms, Domain::Linear, Link::Average),
                ] {
                    dynamics.set_params(DynamicsParams {
                        mode,
                        smoothing,
                        topology,
                        detection,
                        domain,
                        link,
                        hold_ms: 5.0,
                        hysteresis_db: 3.0,
                        ..DynamicsParams::default()
                    });
                    let [a, b, c, d, e, f] = &mut sig[..] else { unreachable!() };
                    let mut chans: [&mut [f32]; 6] =
                        [a.as_mut_slice(), b.as_mut_slice(), c.as_mut_slice(), d.as_mut_slice(), e.as_mut_slice(), f.as_mut_slice()];
                    dynamics.process(&mut chans);
                    dynamics.process_sidechain(&mut chans, &[&key]);
                    dynamics.process_filtered(&mut chans, None, |ch, x| {
                        filter_state[ch] += 0.1 * (x - filter_state[ch]);
                        filter_state[ch]
                    });
                }
            }
            let [a, b, c, d, e, f] = &mut sig[..] else { unreachable!() };
            let mut chans: [&mut [f32]; 6] =
                [a.as_mut_slice(), b.as_mut_slice(), c.as_mut_slice(), d.as_mut_slice(), e.as_mut_slice(), f.as_mut_slice()];
            limiter.set_ceiling_db(-1.0 - round as f32);
            limiter.process(&mut chans);
            for &x in &key {
                follower.process(x);
                tp.push(x);
            }
        }
    });
    assert_eq!((alloc, dealloc), (0, 0), "process allocated");
}
