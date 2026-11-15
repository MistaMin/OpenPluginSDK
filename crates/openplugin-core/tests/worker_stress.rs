//! Stress: many swaps while a simulated `process` loop runs on another thread.
//! Every table is filled with one version number, so a torn or reused value
//! shows up as a table with mixed entries. Versions must never go backwards,
//! every job result must arrive, and no table may be dropped on the audio
//! thread.

use openplugin_core::worker::loader;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

thread_local! {
    static AUDIO: Cell<bool> = const { Cell::new(false) };
}
static DROPS_ON_AUDIO: AtomicUsize = AtomicUsize::new(0);
static DROPS: AtomicUsize = AtomicUsize::new(0);

struct Table {
    version: u32,
    data: Vec<u32>,
}

impl Table {
    fn new(version: u32) -> Self {
        Self {
            version,
            data: vec![version; 256],
        }
    }
}

impl Drop for Table {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::SeqCst);
        if AUDIO.with(|a| a.get()) {
            DROPS_ON_AUDIO.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[test]
fn many_swaps_during_a_running_process_loop() {
    const TOTAL: u32 = 20_000;
    let (mut loader, mut slot) = loader(Table::new(0), 8);
    let done = Arc::new(AtomicBool::new(false));
    let audio_done = done.clone();

    let audio = std::thread::spawn(move || {
        AUDIO.with(|a| a.set(true));
        let start = Instant::now();
        let mut last = 0u32;
        let mut swaps = 0u32;
        // Simulated process loop: poll, then read the live table.
        while !audio_done.load(Ordering::Acquire) || last < TOTAL {
            if slot.poll() {
                swaps += 1;
            }
            let t = slot.get();
            assert!(t.version >= last, "version went backwards");
            assert!(t.data.iter().all(|&v| v == t.version), "torn table");
            last = t.version;
            assert!(start.elapsed() < Duration::from_secs(120), "no convergence");
        }
        // Return the slot so the main thread drops the live table.
        (slot, swaps)
    });

    let start = Instant::now();
    let mut version = 1;
    while version <= TOTAL {
        let v = version;
        if loader.submit(move || Table::new(v)).is_ok() {
            version += 1;
        } else {
            std::thread::yield_now();
        }
        assert!(start.elapsed() < Duration::from_secs(120));
    }
    done.store(true, Ordering::Release);
    let (slot, swaps) = audio.join().unwrap();
    assert_eq!(swaps, TOTAL);
    assert!(loader.wait_idle(Duration::from_secs(10)));
    drop(loader);
    drop(slot);
    assert_eq!(DROPS_ON_AUDIO.load(Ordering::SeqCst), 0);
    // The initial table plus TOTAL results, all freed by now.
    assert_eq!(DROPS.load(Ordering::SeqCst), TOTAL as usize + 1);
}
