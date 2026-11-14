//! `op_shell_save_state` / `op_shell_load_state` beside `op_shell_process` on
//! another thread.
//!
//! The rule under test (see `docs/SHELL.md`): a state call takes the plugin
//! object through a gate the audio thread only try-locks. A block that starts
//! while a call holds the gate is skipped (dry copy, still success); a call
//! waits for a block already running, at most `STATE_WAIT`, and then fails
//! without changing anything. No block ever runs against a half-restored
//! plugin, and no saved blob is ever torn.

#[path = "common/state_plugin.rs"]
mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use common::*;
use openplugin_ffi::shell::{self, ShellInstance};

/// The tests share the hold flags of the plugin, so they run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy)]
struct Handle(usize);

impl Handle {
    fn new() -> Self {
        let instance = shell::create::<Chunky>(48_000.0, 256, 2);
        assert!(!instance.is_null());
        Self(instance as usize)
    }
    fn ptr(self) -> *mut ShellInstance {
        self.0 as *mut ShellInstance
    }
}

fn block(instance: Handle) -> (u32, Vec<f32>) {
    let input = [1.0f32; 64];
    let mut out = vec![-9.0f32; 64];
    let ran = unsafe {
        shell::process(instance.ptr(), input.as_ptr(), input.as_ptr(), out.as_mut_ptr(), out.as_mut_ptr(), 64)
    };
    (ran, out)
}

fn wait_for(flag: &AtomicBool) {
    let start = Instant::now();
    while !flag.load(Ordering::SeqCst) {
        assert!(start.elapsed() < Duration::from_secs(5), "timed out waiting for a flag");
        thread::yield_now();
    }
}

#[test]
fn a_block_that_starts_during_a_save_is_dry_and_does_not_wait() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let a = Handle::new();
    assert!(load(a.ptr(), &session(&[0.0, 0.5], &chunk(3, b"x"))));
    assert_eq!(block(a).1[63], 3.0, "program 3 processes");

    HOLD_SAVE.store(true, Ordering::SeqCst);
    let saver = thread::spawn(move || save(a.ptr()));
    wait_for(&IN_SAVE);

    let started = Instant::now();
    let (ran, out) = block(a);
    assert_eq!(ran, 1, "a skipped block still reports success");
    assert!(out.iter().all(|sample| *sample == 1.0), "a skipped block is the input, not the plugin");
    for _ in 0..50 {
        assert_eq!(block(a).0, 1);
    }
    // A reset during the save is deferred, not performed under the saver.
    unsafe { shell::reset(a.ptr()) };
    assert!(started.elapsed() < Duration::from_millis(500), "the audio side waited");

    HOLD_SAVE.store(false, Ordering::SeqCst);
    let saved = saver.join().unwrap();
    assert!(saved.starts_with(b"OPCH"));
    assert_eq!(block(a).1[63], 3.0, "the plugin runs again once the call ends");
    unsafe { shell::destroy(a.ptr()) };
}

#[test]
fn a_save_waits_for_a_block_in_flight() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let a = Handle::new();
    HOLD_PROCESS.store(true, Ordering::SeqCst);
    let audio = thread::spawn(move || block(a));
    wait_for(&IN_PROCESS);

    let done = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&done);
    let saver = thread::spawn(move || {
        let bytes = save(a.ptr());
        flag.store(true, Ordering::SeqCst);
        bytes
    });
    thread::sleep(Duration::from_millis(200));
    assert!(!done.load(Ordering::SeqCst), "the save ran inside a block");
    assert!(!IN_SAVE.load(Ordering::SeqCst), "save_state entered the plugin during process");

    HOLD_PROCESS.store(false, Ordering::SeqCst);
    let (ran, out) = audio.join().unwrap();
    assert_eq!(ran, 1);
    assert!(out.iter().all(|sample| *sample == 1.0));
    assert!(saver.join().unwrap().starts_with(b"OPCH"));
    unsafe { shell::destroy(a.ptr()) };
}

#[test]
fn a_call_gives_up_after_a_bounded_wait_and_changes_nothing() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let a = Handle::new();
    let before = save(a.ptr());
    HOLD_PROCESS.store(true, Ordering::SeqCst);
    let audio = thread::spawn(move || block(a));
    wait_for(&IN_PROCESS);

    let started = Instant::now();
    let loaded = load(a.ptr(), &session(&[9.0, 0.1], &chunk(7, b"late")));
    let waited = started.elapsed();
    assert!(!loaded, "a load that could not reach the plugin must fail");
    assert!(waited >= Duration::from_millis(1_500), "gave up early: {waited:?}");
    assert!(waited < Duration::from_secs(4), "the wait is not bounded: {waited:?}");
    let mut needed = 0u32;
    assert_eq!(
        unsafe { shell::save_state(a.ptr(), std::ptr::null_mut(), 0, &mut needed) },
        0,
        "a save that could not reach the plugin fails"
    );

    HOLD_PROCESS.store(false, Ordering::SeqCst);
    audio.join().unwrap();
    assert_eq!(save(a.ptr()), before, "the timed-out load changed the state");
    unsafe { shell::destroy(a.ptr()) };
}

/// The main thread alternates two states while another thread renders. Every
/// block is whole (one value across the block) and is the dry input or one of
/// the two states; every saved blob is exactly one of the two states.
#[test]
fn saves_and_loads_beside_process_never_tear() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let a = Handle::new();
    let state_a = session(&[0.0, 0.25], &chunk(2, b"alpha"));
    let state_b = session(&[6.0, 0.75], &chunk(3, b"beta"));
    let value_a = 2.0f32;
    let value_b = 3.0 * 10f32.powf(6.0 / 20.0);
    assert!(load(a.ptr(), &state_a));
    // The first loads settle the plane; sync once so the saved bytes are canonical.
    let canonical_a = save(a.ptr());
    assert!(load(a.ptr(), &state_b));
    let canonical_b = save(a.ptr());

    let stop = Arc::new(AtomicBool::new(false));
    let blocks = Arc::new(AtomicUsize::new(0));
    let dry = Arc::new(AtomicUsize::new(0));
    let audio = {
        let (stop, blocks, dry) = (Arc::clone(&stop), Arc::clone(&blocks), Arc::clone(&dry));
        thread::spawn(move || {
            let mut seen_a = 0usize;
            let mut seen_b = 0usize;
            let mut count = 0usize;
            while !stop.load(Ordering::SeqCst) {
                let (ran, out) = block(a);
                assert_eq!(ran, 1, "process failed during a state call");
                let first = out[0];
                assert!(out.iter().all(|sample| *sample == first), "a torn block: {out:?}");
                if (first - 1.0).abs() < 1e-4 {
                    dry.fetch_add(1, Ordering::Relaxed);
                } else if (first - value_a).abs() < 1e-3 {
                    seen_a += 1;
                } else if (first - value_b).abs() < 1e-3 {
                    seen_b += 1;
                } else {
                    panic!("a block ran with neither state: {first}");
                }
                count += 1;
                if count % 97 == 0 {
                    unsafe { shell::reset(a.ptr()) };
                }
                thread::yield_now();
            }
            blocks.store(count, Ordering::SeqCst);
            (seen_a, seen_b)
        })
    };

    let workers: Vec<_> = (0..2)
        .map(|_| {
            let (state_a, state_b) = (state_a.clone(), state_b.clone());
            let (canonical_a, canonical_b) = (canonical_a.clone(), canonical_b.clone());
            thread::spawn(move || {
                for round in 0..300 {
                    assert!(load(a.ptr(), &state_a), "load A, round {round}");
                    let saved = save(a.ptr());
                    assert!(saved == canonical_a || saved == canonical_b, "torn save, round {round}");
                    assert!(load(a.ptr(), &state_b), "load B, round {round}");
                    let saved = save(a.ptr());
                    assert!(saved == canonical_a || saved == canonical_b, "torn save, round {round}");
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    stop.store(true, Ordering::SeqCst);
    let (seen_a, seen_b) = audio.join().unwrap();
    assert!(blocks.load(Ordering::SeqCst) > 0);
    assert!(seen_a + seen_b > 0, "no block ran the plugin while the states changed");
    eprintln!(
        "blocks {}, dry {}, state A {seen_a}, state B {seen_b}",
        blocks.load(Ordering::SeqCst),
        dry.load(Ordering::SeqCst)
    );
    unsafe { shell::destroy(a.ptr()) };
}
