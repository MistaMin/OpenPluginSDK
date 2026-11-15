//! `Model::process_*` and `reset` must not allocate, deallocate or reallocate.
//! A probe first proves the counter sees an allocation.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use openplugin_dsp::neural::Model;

thread_local! {
    // Armed per thread: the test harness's own threads (output capture, the
    // "running for over 60 seconds" notice) never add to the count. `const`
    // initialisation, so reading it inside the allocator never allocates.
    static ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn armed() -> bool {
    ARMED.try_with(|a| a.get()).unwrap_or(false)
}
static COUNT: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if armed() {
            COUNT.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        if armed() {
            COUNT.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if armed() {
            COUNT.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if armed() {
            COUNT.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn count(f: impl FnOnce()) -> usize {
    COUNT.store(0, Ordering::SeqCst);
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    COUNT.load(Ordering::SeqCst)
}

fn arr(n: usize, v: f32) -> String {
    format!("[{}]", vec![format!("{v}"); n].join(","))
}

#[test]
fn inference_does_not_allocate() {
    let h = 24;
    let text = format!(
        r#"{{"version":1,"sample_rate":48000,"input_size":2,"residual":true,"layers":[
        {{"type":"conv1d","in":2,"out":4,"kernel":3,"dilation":5,"activation":"tanh","weights":{},"bias":{}}},
        {{"type":"lstm","in":4,"hidden":{h},"w_x":{},"w_h":{},"b":{}}},
        {{"type":"gru","in":{h},"hidden":{h},"w_x":{},"w_h":{},"b":{}}},
        {{"type":"dense","in":{h},"out":1,"activation":"identity","weights":{},"bias":[0.0]}}]}}"#,
        arr(4 * 3 * 2, 0.05),
        arr(4, 0.0),
        arr(4 * h * 4, 0.05),
        arr(4 * h * h, 0.01),
        arr(4 * h, 0.0),
        arr(3 * h * h, 0.05),
        arr(3 * h * h, 0.01),
        arr(3 * h, 0.0),
        arr(h, 0.1),
    );
    let mut m = Model::from_json(&text).unwrap();
    let mut buf = vec![0.3f32; 4096];

    // Probe: the counter must see a Vec allocation.
    assert!(count(|| drop(std::hint::black_box(vec![0u8; 64]))) > 0);

    let n = count(|| {
        m.reset();
        m.process_block(&mut buf, 0.5);
        for _ in 0..1000 {
            std::hint::black_box(m.process_sample(0.1, 0.2));
        }
        m.reset();
    });
    assert_eq!(n, 0, "inference allocated {n} times");
    assert!(buf.iter().all(|x| x.is_finite()));
}
