//! `Model::from_json` refuses a text with more array elements than any model
//! can hold before parsing it, so a hostile file costs no memory.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use openplugin_dsp::neural::{Model, ModelError};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Peak;

fn grow(n: usize) {
    PEAK.fetch_max(LIVE.fetch_add(n, Ordering::SeqCst) + n, Ordering::SeqCst);
}

unsafe impl GlobalAlloc for Peak {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        grow(l.size());
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::SeqCst);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        // As the system does it at worst: both blocks alive during the copy.
        grow(n);
        LIVE.fetch_sub(l.size(), Ordering::SeqCst);
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static GLOBAL: Peak = Peak;

// One test in this file, so no other thread's allocations are in the peak.
#[test]
fn sixteen_megabytes_of_numbers_under_an_unknown_key_cost_nothing() {
    let text = format!("{{\"x\":[{}0]}}", "0,".repeat(8 << 20));
    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let result = Model::from_json(&text);
    let peak = PEAK.load(Ordering::SeqCst) - before;
    assert!(matches!(result, Err(ModelError::Json(_))));
    assert!(peak < 1 << 20, "parsing peaked at {peak} bytes");
}
