//! The audio path of a parameter-only plugin does not allocate while another
//! thread saves and loads its state. The counter is per thread, so only the
//! audio thread's own allocations are counted.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_host::HostSession;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

fn bump() {
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = COUNT.try_with(|count| count.set(count.get() + 1));
        }
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        bump();
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];
static INFO: PluginInfo = PluginInfo::new(
    "com.example.param-only-no-alloc",
    "Param Only No Alloc",
    "Test",
    "0.1.0",
    Category::Utility,
    &PARAMS,
)
.with_url("https://example.com")
.with_email("a@b.c");

struct Gain(f32);

impl Plugin for Gain {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id == "gain" {
            self.0 = PARAMS[0].clamp(value);
            true
        } else {
            false
        }
    }
    fn param(&self, id: &str) -> Option<f32> {
        (id == "gain").then_some(self.0)
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let gain = 10f32.powf(self.0 / 20.0);
        for channel in io {
            for sample in &mut channel[..frames] {
                *sample *= gain;
            }
        }
    }
    fn state_is_parameters_only(&self) -> bool {
        true
    }
}

#[test]
fn process_does_not_allocate_while_state_is_saved_and_loaded() {
    let instance = HostSession::new(Box::new(Gain(0.0)), 48_000.0, 64, 2).unwrap();
    let stop = AtomicBool::new(false);

    // Control: the counter sees an allocation on this thread.
    ARMED.with(|a| a.set(true));
    let probe = vec![0u8; 32];
    ARMED.with(|a| a.set(false));
    drop(probe);
    assert!(COUNT.with(Cell::get) >= 1, "the counting allocator is blind");
    COUNT.with(|c| c.set(0));

    thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let mut gain = 0.0;
            while !stop.load(Ordering::Acquire) {
                instance.store_param(0, gain);
                gain = (gain + 0.5) % 12.0;
                let bytes = instance.save_bytes().unwrap();
                assert!(instance.load_bytes(&bytes));
            }
        });
        let mut left = [0.25f32; 64];
        let mut right = [0.25f32; 64];
        ARMED.with(|a| a.set(true));
        for _ in 0..20_000 {
            assert!(instance.process_stereo(&mut left, &mut right));
            left.fill(0.25);
            right.fill(0.25);
        }
        ARMED.with(|a| a.set(false));
        stop.store(true, Ordering::Release);
        worker.join().unwrap();
    });
    assert_eq!(COUNT.with(Cell::get), 0, "the audio thread allocated");
}
