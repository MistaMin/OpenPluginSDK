//! The LV2 realtime path with every new port connected does not allocate.
//!
//! Notes and a `time:Position` in, MIDI out, a sidechain, and the freewheel
//! port, all driven through the exported descriptor. Everything that may
//! allocate (instance, atoms, buffers) is built before the allocator is
//! armed. One `#[test]` in this binary, so nothing else allocates while it is.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use common::plugins::{Duck, OFFLINE, TRANSPORT};
use common::*;
use openplugin_core::Plugin;
use openplugin_targets::lv2;

// The armed flag is per thread: only allocations made on the thread that armed
// it are counted, so the test harness's other threads cannot add to the count.
// `const` initialisation: reading it inside the allocator never allocates.
thread_local! {
    static ARMED_HERE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct ThreadArm;

impl ThreadArm {
    fn load(&self, _: Ordering) -> bool {
        ARMED_HERE.try_with(|a| a.get()).unwrap_or(false)
    }

    fn store(&self, value: bool, _: Ordering) {
        ARMED_HERE.with(|a| a.set(value));
    }
}

static ARMED: ThreadArm = ThreadArm;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
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
            FREES.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.load(Ordering::SeqCst) {
            FREES.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn measured<T>(shape: &str, body: impl FnOnce() -> T) -> T {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    FREES.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
    let value = body();
    ARMED.store(false, Ordering::SeqCst);
    let (a, f) = (ALLOCATIONS.load(Ordering::SeqCst), FREES.load(Ordering::SeqCst));
    assert_eq!((a, f), (0, 0), "{shape}: {a} allocations and {f} frees");
    value
}

#[test]
fn lv2_run_with_every_port_connected_does_not_allocate() {
    // The control experiment: the counter sees a heap block while armed.
    ARMED.store(true, Ordering::SeqCst);
    let probe = vec![0u8; 64];
    ARMED.store(false, Ordering::SeqCst);
    assert!(ALLOCATIONS.load(Ordering::SeqCst) >= 1);
    drop(probe);

    // The test plugin records the transport behind a std Mutex, which boxes its
    // pthread mutex the first time it is locked on macOS. Warm it up here.
    drop(TRANSPORT.lock());
    let mut host = Host::new();
    lv2::set_creator(|| Box::new(Duck::default()) as Box<dyn Plugin>);
    let desc = unsafe { &*lv2::descriptor(0) };
    let instance = (desc.instantiate)(desc, 48_000.0, std::ptr::null(), host.features());
    assert!(!instance.is_null());

    let mut buffers: Vec<Vec<f32>> = (0..6).map(|i| vec![0.1 * (i + 1) as f32; 512]).collect();
    // 0, 1 in; 2, 3 out; 8, 9 sidechain.
    for (port, slot) in [(0u32, 0usize), (1, 1), (2, 2), (3, 3), (8, 4), (9, 5)] {
        (desc.connect_port)(instance, port, buffers[slot].as_mut_ptr().cast());
    }
    let level = Box::new(0.5f32);
    let freewheel = std::cell::Cell::new(0.0f32);
    (desc.connect_port)(instance, 4, (&*level as *const f32).cast_mut().cast());
    (desc.connect_port)(instance, 5, freewheel.as_ptr().cast());

    let mut input = Seq::new(&mut host);
    for i in 0..20 {
        input.midi(&mut host, i * 20, &[0x90 | (i % 16) as u8, 40 + i as u8, 100]);
    }
    let object = host.urid(OBJECT);
    let body = object_body(&mut host);
    input.atom(5, object, &body);
    let mut events_in = input.buf();
    // Room for 12 of the 20 echoes, so the full-buffer path is measured too.
    let mut events_out = OutSeq::new(8 + 24 * 12);
    (desc.connect_port)(instance, 6, events_in.ptr());
    (desc.connect_port)(instance, 7, events_out.buf.ptr());

    measured("512 frames, notes in, midi out, position, sidechain", || {
        (desc.activate)(instance);
        (desc.run)(instance, 512);
        freewheel.set(1.0);
        events_out.reset();
        (desc.run)(instance, 512);
        events_out.reset();
        (desc.run)(instance, 1);
        events_out.reset();
        (desc.run)(instance, 0);
        freewheel.set(0.0);
        events_out.reset();
        (desc.run)(instance, 37);
        (desc.deactivate)(instance);
    });
    assert!(events_out.sentinel_intact());
    assert!(!OFFLINE.load(Ordering::SeqCst));
    // The last run had no events fit past its 37 frames; the 512-frame run filled 12.
    events_out.reset();
    (desc.run)(instance, 512);
    assert_eq!(events_out.events().len(), 12);
    assert!(events_out.sentinel_intact());
    (desc.cleanup)(instance);
}

fn object_body(host: &mut Host) -> Vec<u8> {
    object(
        host,
        POSITION,
        &[(SPEED, Value::Float(1.0)), (BPM, Value::Float(140.0)), (FRAME, Value::Long(48_000)), (BAR, Value::Long(2))],
    )
}
