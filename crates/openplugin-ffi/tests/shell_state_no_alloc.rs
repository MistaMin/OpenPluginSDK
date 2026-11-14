//! The audio thread does not allocate while a state call runs on another.
//!
//! The counting allocator counts only on threads that armed themselves, so
//! the main thread may allocate freely (a save does) while the audio thread is
//! measured. One `#[test]` in this binary. Three phases:
//!
//! 1. blocks that start while a (held) save owns the gate: all skipped;
//! 2. blocks that start while a (held) load owns the gate, on the same terms;
//! 3. a churn of saves and loads against a running audio thread.
//!
//! A control first shows the counter sees an allocation on an armed thread
//! and ignores one on an unarmed thread.

#[path = "common/state_plugin.rs"]
mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use common::*;
use openplugin_ffi::shell::{self, ShellInstance};

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

fn armed() -> bool {
    ARMED.try_with(Cell::get).unwrap_or(false)
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
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
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if armed() {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
            FREES.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if armed() {
            FREES.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[derive(Clone, Copy)]
struct Handle(usize);

impl Handle {
    fn ptr(self) -> *mut ShellInstance {
        self.0 as *mut ShellInstance
    }
}

fn counts() -> (usize, usize) {
    (ALLOCATIONS.load(Ordering::SeqCst), FREES.load(Ordering::SeqCst))
}

fn assert_allocator_counts() {
    let before = counts();
    ARMED.with(|armed| armed.set(true));
    let probe = vec![0u8; 64];
    drop(probe);
    ARMED.with(|armed| armed.set(false));
    let after = counts();
    assert!(after.0 > before.0 && after.1 > before.1, "the counter missed an armed allocation");
    // An unarmed thread (this one, now) is not counted.
    let before = counts();
    drop(vec![0u8; 64]);
    assert_eq!(counts(), before, "the counter counted an unarmed thread");
}

fn wait_for(flag: &AtomicBool) {
    let start = Instant::now();
    while !flag.load(Ordering::SeqCst) {
        assert!(start.elapsed() < Duration::from_secs(5), "timed out waiting for a flag");
        thread::yield_now();
    }
}

/// `blocks` calls to `process` and `reset` on an armed thread; returns how many
/// allocations and frees that thread made and how many blocks were dry.
fn armed_blocks(instance: Handle, blocks: usize) -> (usize, usize, usize) {
    thread::spawn(move || {
        let input = [1.0f32; 128];
        let mut out = [0.0f32; 128];
        let mut dry = 0;
        let mut results = [0u32; 1];
        let before = counts();
        ARMED.with(|armed| armed.set(true));
        for round in 0..blocks {
            results[0] = unsafe {
                shell::process(
                    instance.ptr(),
                    input.as_ptr(),
                    input.as_ptr(),
                    out.as_mut_ptr(),
                    out.as_mut_ptr(),
                    128,
                )
            };
            if results[0] != 1 {
                break;
            }
            if out[127] == 1.0 {
                dry += 1;
            }
            if round % 16 == 0 {
                unsafe { shell::reset(instance.ptr()) };
            }
        }
        ARMED.with(|armed| armed.set(false));
        let after = counts();
        assert_eq!(results[0], 1, "process failed");
        (after.0 - before.0, after.1 - before.1, dry)
    })
    .join()
    .unwrap()
}

#[test]
fn the_audio_thread_does_not_allocate_during_a_save_or_a_load() {
    assert_allocator_counts();
    let instance = shell::create::<Chunky>(48_000.0, 256, 2);
    assert!(!instance.is_null());
    let a = Handle(instance as usize);
    let state = session(&[6.0, 0.25], &chunk(3, b"three"));
    assert!(load(a.ptr(), &state));

    // Phase 1: a slow save holds the gate; every block in that time is skipped.
    HOLD_SAVE.store(true, Ordering::SeqCst);
    let saver = thread::spawn(move || save(a.ptr()));
    wait_for(&IN_SAVE);
    let (allocations, frees, dry) = armed_blocks(a, 400);
    HOLD_SAVE.store(false, Ordering::SeqCst);
    saver.join().unwrap();
    assert_eq!((allocations, frees), (0, 0), "allocation on the audio thread during a save");
    assert_eq!(dry, 400, "every block during the save is skipped");

    // The plugin runs again, still without allocating.
    let (allocations, frees, dry) = armed_blocks(a, 100);
    assert_eq!((allocations, frees, dry), (0, 0, 0));

    // Phase 2: a load holds the gate. `load_state` of the plugin does not
    // spin, so hold the gate with a second save queued behind a slow one:
    // the load waits on the state lock while the save owns the gate.
    HOLD_SAVE.store(true, Ordering::SeqCst);
    let saver = thread::spawn(move || save(a.ptr()));
    wait_for(&IN_SAVE);
    let loader = {
        let state = state.clone();
        thread::spawn(move || load(a.ptr(), &state))
    };
    thread::sleep(Duration::from_millis(50));
    let (allocations, frees, dry) = armed_blocks(a, 400);
    HOLD_SAVE.store(false, Ordering::SeqCst);
    saver.join().unwrap();
    assert!(loader.join().unwrap());
    assert_eq!((allocations, frees), (0, 0), "allocation on the audio thread during a load");
    assert_eq!(dry, 400);

    // Phase 3: churn. Saves and loads allocate on this thread; the audio
    // thread, running the whole time, does not.
    let stop = Arc::new(AtomicBool::new(false));
    let audio = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            let input = [1.0f32; 128];
            let mut out = [0.0f32; 128];
            let mut ran = 0usize;
            let before = counts();
            ARMED.with(|armed| armed.set(true));
            while !stop.load(Ordering::SeqCst) {
                let result = unsafe {
                    shell::process(
                        a.ptr(),
                        input.as_ptr(),
                        input.as_ptr(),
                        out.as_mut_ptr(),
                        out.as_mut_ptr(),
                        128,
                    )
                };
                assert_eq!(result, 1);
                ran += 1;
            }
            ARMED.with(|armed| armed.set(false));
            let after = counts();
            (after.0 - before.0, after.1 - before.1, ran)
        })
    };
    let unarmed_before = counts();
    for _ in 0..300 {
        assert!(load(a.ptr(), &state));
        assert!(!save(a.ptr()).is_empty());
    }
    stop.store(true, Ordering::SeqCst);
    let (allocations, frees, ran) = audio.join().unwrap();
    assert!(ran > 0);
    assert_eq!((allocations, frees), (0, 0), "allocation on the audio thread during churn");
    assert_eq!(counts(), unarmed_before, "the main thread was counted");
    unsafe { shell::destroy(instance) };
}
