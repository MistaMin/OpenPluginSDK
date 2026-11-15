//! The LV2 latency port (roadmap 0.6 task 7): the description file lists a
//! `lv2:latency` control output, and `run` writes the plugin's latency to it
//! after every block, so a host that reads the port after `run` sees a change
//! in the same block. Driven through the exported descriptor as a host does,
//! with the test plugin of `common::shift` (latency = its `lookahead`
//! parameter, or a jump after N blocks).
//!
//! One `#[test]`: the counting allocator below is armed around the blocks that
//! change the latency, and no other test runs beside it.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use common::shift::{Shift, AFTER_LATENCY};
use openplugin_targets::lv2::{self, PortMap};

static ARMED: AtomicBool = AtomicBool::new(false);
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
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if ARMED.load(Ordering::SeqCst) {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
            FREES.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, size) }
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

fn measured(body: impl FnOnce()) -> (usize, usize) {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    FREES.store(0, Ordering::SeqCst);
    ARMED.store(true, Ordering::SeqCst);
    body();
    ARMED.store(false, Ordering::SeqCst);
    (ALLOCATIONS.load(Ordering::SeqCst), FREES.load(Ordering::SeqCst))
}

const FRAMES: usize = 64;

/// One instance with every port connected to memory this struct owns.
struct Host {
    desc: *const lv2::Lv2Descriptor,
    handle: *mut c_void,
    audio: [Vec<f32>; 4],
    lookahead: Box<f32>,
    names: Box<f32>,
    freewheel: Box<f32>,
    latency: Box<f32>,
}

impl Host {
    fn new(map: PortMap) -> Self {
        let desc = lv2::descriptor(0);
        assert!(!desc.is_null());
        let handle = unsafe { ((*desc).instantiate)(desc, 48_000.0, std::ptr::null(), std::ptr::null()) };
        assert!(!handle.is_null());
        let mut host = Self {
            desc,
            handle,
            audio: [vec![0.25; FRAMES], vec![0.25; FRAMES], vec![0.0; FRAMES], vec![0.0; FRAMES]],
            lookahead: Box::new(0.0),
            names: Box::new(0.0),
            freewheel: Box::new(0.0),
            // A value the plugin never reports, to see the first write.
            latency: Box::new(-1.0),
        };
        unsafe {
            let connect = (*desc).connect_port;
            for (port, buffer) in host.audio.iter_mut().enumerate() {
                connect(handle, port as u32, buffer.as_mut_ptr() as *mut c_void);
            }
            connect(handle, map.first_param(), &mut *host.lookahead as *mut f32 as *mut c_void);
            connect(handle, map.first_param() + 1, &mut *host.names as *mut f32 as *mut c_void);
            connect(handle, map.freewheel(), &mut *host.freewheel as *mut f32 as *mut c_void);
            connect(handle, map.latency(), &mut *host.latency as *mut f32 as *mut c_void);
            ((*desc).activate)(handle);
        }
        host
    }

    fn run(&mut self, blocks: usize) {
        for _ in 0..blocks {
            unsafe { ((*self.desc).run)(self.handle, FRAMES as u32) };
        }
    }

    fn latency(&self) -> f32 {
        *self.latency
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        unsafe {
            ((*self.desc).deactivate)(self.handle);
            ((*self.desc).cleanup)(self.handle);
        }
    }
}

#[test]
fn the_latency_port_follows_the_plugin_without_allocating() {
    let (probe_alloc, probe_free) = measured(|| drop(std::hint::black_box(vec![0u8; 64])));
    assert!(probe_alloc >= 1 && probe_free >= 1, "the counting allocator is not live");

    let map = PortMap::of(&Shift::default());
    assert_eq!(map.latency(), map.freewheel() + 1, "a stereo effect without notes: the port follows freewheel");

    // The description lists the port where the instance expects it.
    let ttl = lv2::plugin_ttl(&Shift::default(), false);
    assert!(ttl.contains(&format!("lv2:index {} ; lv2:symbol \"latency\"", map.latency())));
    assert!(ttl.contains("lv2:designation lv2:latency") && ttl.contains("lv2:reportsLatency"));

    // 1. A parameter: the port reads the new latency after the block that applied it.
    lv2::set_creator(|| Box::new(Shift::default()));
    {
        let mut host = Host::new(map);
        assert_eq!(host.latency(), 0.0, "activate writes the port");
        host.run(1);
        assert_eq!(host.latency(), 0.0);
        *host.lookahead = 64.0;
        let (allocations, frees) = measured(|| host.run(1));
        assert_eq!((allocations, frees), (0, 0), "a block that changes the latency allocated");
        assert_eq!(host.latency(), 64.0);
        // The audio is delayed by what the port says: an impulse comes out 64 samples later.
        host.audio[0].fill(0.0);
        host.run(2);
        host.audio[0][0] = 1.0;
        host.run(1);
        host.audio[0][0] = 0.0;
        assert!(host.audio[2].iter().all(|s| *s == 0.0), "no output before the latency");
        host.run(1);
        assert_eq!(host.audio[2][0], 1.0, "the impulse arrives 64 samples on");
        *host.lookahead = 17.0;
        host.run(1);
        assert_eq!(host.latency(), 17.0);
        // A zero-length run (a host reading the latency before playing) also updates it.
        *host.lookahead = 5.0;
        unsafe { ((*host.desc).run)(host.handle, 0) };
        assert_eq!(host.latency(), 5.0);
    }

    // 2. A latency that moves by itself after three blocks.
    lv2::set_creator(|| Box::new(Shift::default().after_blocks(3)));
    {
        let mut host = Host::new(map);
        host.run(2);
        assert_eq!(host.latency(), 0.0);
        let (allocations, frees) = measured(|| host.run(1));
        assert_eq!((allocations, frees), (0, 0));
        assert_eq!(host.latency(), AFTER_LATENCY as f32);
    }

    // 3. A host that never connects the port is fine.
    lv2::set_creator(|| Box::new(Shift::default()));
    {
        let desc = lv2::descriptor(0);
        let handle = unsafe { ((*desc).instantiate)(desc, 48_000.0, std::ptr::null(), std::ptr::null()) };
        let mut audio = [vec![0.0f32; FRAMES], vec![0.0; FRAMES], vec![0.0; FRAMES], vec![0.0; FRAMES]];
        unsafe {
            for (port, buffer) in audio.iter_mut().enumerate() {
                ((*desc).connect_port)(handle, port as u32, buffer.as_mut_ptr() as *mut c_void);
            }
            ((*desc).activate)(handle);
            ((*desc).run)(handle, FRAMES as u32);
            ((*desc).deactivate)(handle);
            ((*desc).cleanup)(handle);
        }
    }
}
