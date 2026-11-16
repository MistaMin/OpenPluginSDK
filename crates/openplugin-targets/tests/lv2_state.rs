//! The LV2 `state:interface` through the exported descriptor, driven by a fake
//! host store: a plugin's chunk is saved as one property and restored into a
//! new instance; a plugin without a chunk stores nothing.

mod common;

use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::*;
use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_targets::lv2::{self, Lv2Descriptor, Lv2Feature};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

static PARAMS: [ParamInfo; 1] = [ParamInfo::new("level", "Level", 0.0, 1.0, 0.5, Unit::None)];

static INFO: PluginInfo = PluginInfo {
    id: "urn:openplugin:test-state",
    name: "Test State",
    vendor: "Test",
    url: "https://example.com",
    email: "a@b.c",
    version: "0.1.0",
    category: Category::Dynamics,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

/// Holds data no parameter carries, as a loaded impulse response would be.
/// Its output is the input times the data's length, so the audio shows what was loaded.
#[derive(Default)]
struct Loaded {
    data: Vec<u8>,
    params_only: bool,
}

impl Plugin for Loaded {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        true
    }
    fn param(&self, _: &str) -> Option<f32> {
        Some(0.5)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let gain = self.data.len() as f32;
        for channel in io.iter_mut() {
            for sample in &mut channel[..frames] {
                *sample *= gain;
            }
        }
    }
    fn state_is_parameters_only(&self) -> bool {
        self.params_only
    }
    fn save_state(&mut self) -> Option<Vec<u8>> {
        if HOLD_SAVE.load(Ordering::SeqCst) {
            IN_SAVE.store(true, Ordering::SeqCst);
            while HOLD_SAVE.load(Ordering::SeqCst) {
                std::hint::spin_loop();
            }
        }
        Some(self.data.clone())
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        self.data = data.to_vec();
        true
    }
}

/// Keeps `save_state` inside the plugin until cleared; `IN_SAVE` says it got there.
static HOLD_SAVE: AtomicBool = AtomicBool::new(false);
static IN_SAVE: AtomicBool = AtomicBool::new(false);

fn with_chunk() -> Box<dyn Plugin> {
    Box::new(Loaded::default())
}

fn params_only() -> Box<dyn Plugin> {
    Box::new(Loaded { data: b"ignored".to_vec(), params_only: true })
}

type StoreFn = unsafe extern "C" fn(*mut c_void, u32, *const c_void, usize, u32, u32) -> u32;
type RetrieveFn = unsafe extern "C" fn(*mut c_void, u32, *mut usize, *mut u32, *mut u32) -> *const c_void;

#[repr(C)]
struct StateInterface {
    save: unsafe extern "C" fn(*mut c_void, Option<StoreFn>, *mut c_void, u32, *const *const Lv2Feature) -> u32,
    restore: unsafe extern "C" fn(*mut c_void, Option<RetrieveFn>, *mut c_void, u32, *const *const Lv2Feature) -> u32,
}

/// The host's saved properties: (key, value, type, flags).
type Store = Vec<(u32, Vec<u8>, u32, u32)>;

unsafe extern "C" fn store(handle: *mut c_void, key: u32, value: *const c_void, size: usize, kind: u32, flags: u32) -> u32 {
    let bytes = unsafe { std::slice::from_raw_parts(value as *const u8, size) }.to_vec();
    unsafe { &mut *(handle as *mut Store) }.push((key, bytes, kind, flags));
    0
}

unsafe extern "C" fn retrieve(handle: *mut c_void, key: u32, size: *mut usize, kind: *mut u32, flags: *mut u32) -> *const c_void {
    let saved = unsafe { &*(handle as *const Store) };
    match saved.iter().find(|entry| entry.0 == key) {
        Some((_, bytes, k, f)) => unsafe {
            *size = bytes.len();
            *kind = *k;
            *flags = *f;
            bytes.as_ptr().cast()
        },
        None => std::ptr::null(),
    }
}

struct Rig {
    desc: &'static Lv2Descriptor,
    instance: *mut c_void,
    input: [Vec<f32>; 2],
    output: [Vec<f32>; 2],
    level: Box<f32>,
}

impl Rig {
    fn new(create: fn() -> Box<dyn Plugin>, features: *const *const Lv2Feature) -> Self {
        lv2::set_creator(create);
        let desc = unsafe { &*lv2::descriptor(0) };
        let instance = (desc.instantiate)(desc, 48_000.0, std::ptr::null(), features);
        assert!(!instance.is_null());
        let mut rig = Self { desc, instance, input: [vec![1.0; 64], vec![1.0; 64]], output: [vec![0.0; 64], vec![0.0; 64]], level: Box::new(0.5) };
        for c in 0..2 {
            (desc.connect_port)(instance, c as u32, rig.input[c].as_mut_ptr().cast());
            (desc.connect_port)(instance, c as u32 + 2, rig.output[c].as_mut_ptr().cast());
        }
        (desc.connect_port)(instance, 4, (&mut *rig.level as *mut f32).cast());
        (desc.activate)(instance);
        rig
    }

    fn run(&self) -> f32 {
        (self.desc.run)(self.instance, 64);
        self.output[0][63]
    }

    fn state(&self) -> &'static StateInterface {
        let uri = c"http://lv2plug.in/ns/ext/state#interface";
        let iface = (self.desc.extension_data)(uri.as_ptr() as *const c_char) as *const StateInterface;
        unsafe { iface.as_ref() }.expect("state:interface is answered")
    }

    fn save(&self, saved: &mut Store) -> u32 {
        unsafe { (self.state().save)(self.instance, Some(store), (saved as *mut Store).cast(), 0, std::ptr::null()) }
    }

    fn restore(&self, saved: &Store) -> u32 {
        let handle = (saved as *const Store).cast_mut().cast();
        unsafe { (self.state().restore)(self.instance, Some(retrieve), handle, 0, std::ptr::null()) }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        (self.desc.deactivate)(self.instance);
        (self.desc.cleanup)(self.instance);
    }
}

#[test]
fn a_chunk_is_saved_as_one_property_and_restored_into_a_new_instance() {
    let _g = serial();
    let mut host = Host::new();
    let first = Rig::new(with_chunk, host.features());
    // Give the first instance data through its own state call, as a load would.
    let mut seed: Store = vec![(host.urid("urn:openplugin:state#chunk"), b"ir!".to_vec(), host.urid(CHUNK), 3)];
    assert_eq!(first.restore(&seed), 0);
    assert_eq!(first.run(), 3.0, "the loaded data reaches the audio");
    seed.clear();

    let mut saved = Store::new();
    assert_eq!(first.save(&mut saved), 0);
    assert_eq!(saved.len(), 1, "one property");
    let (key, bytes, kind, flags) = saved[0].clone();
    assert_eq!(key, host.urid("urn:openplugin:state#chunk"));
    assert_eq!(kind, host.urid(CHUNK));
    assert_eq!(bytes, b"ir!");
    assert_eq!(flags, 1 | 2, "POD and portable");
    drop(first);

    let second = Rig::new(with_chunk, host.features());
    assert_eq!(second.run(), 0.0, "a new instance has no data");
    assert_eq!(second.restore(&saved), 0);
    assert_eq!(second.run(), 3.0, "the reopened session has the data back");
}

#[test]
fn a_plugin_without_a_chunk_stores_nothing() {
    let _g = serial();
    let host = Host::new();
    let rig = Rig::new(params_only, host.features());
    let mut saved = Store::new();
    assert_eq!(rig.save(&mut saved), 0);
    assert!(saved.is_empty(), "its control ports are its state");
    assert_eq!(rig.restore(&saved), 0);
}

#[test]
fn a_session_without_the_property_or_with_the_wrong_type_is_handled() {
    let _g = serial();
    let mut host = Host::new();
    let rig = Rig::new(with_chunk, host.features());
    assert_eq!(rig.restore(&Store::new()), 0, "an older session restores nothing and succeeds");
    assert_eq!(rig.run(), 0.0);
    let wrong = vec![(host.urid("urn:openplugin:state#chunk"), b"ir!".to_vec(), host.urid(INT), 3)];
    assert_eq!(rig.restore(&wrong), 2, "LV2_STATE_ERR_BAD_TYPE");
    assert_eq!(rig.run(), 0.0, "nothing was loaded");
}

#[test]
fn without_urid_map_a_chunk_cannot_be_saved() {
    let _g = serial();
    let none: [*const Lv2Feature; 1] = [std::ptr::null()];
    let rig = Rig::new(with_chunk, none.as_ptr());
    let mut saved = Store::new();
    assert_eq!(rig.save(&mut saved), 4, "LV2_STATE_ERR_NO_FEATURE");
    assert!(saved.is_empty());
}

#[test]
fn a_block_during_a_save_passes_the_input_through() {
    let _g = serial();
    let mut host = Host::new();
    let rig = Rig::new(with_chunk, host.features());
    let seed = vec![(host.urid("urn:openplugin:state#chunk"), b"ir!".to_vec(), host.urid(CHUNK), 3)];
    assert_eq!(rig.restore(&seed), 0);
    assert_eq!(rig.run(), 3.0);

    HOLD_SAVE.store(true, Ordering::SeqCst);
    IN_SAVE.store(false, Ordering::SeqCst);
    let (save, instance) = (rig.state().save as usize, rig.instance as usize);
    let saver = std::thread::spawn(move || {
        let save: unsafe extern "C" fn(*mut c_void, Option<StoreFn>, *mut c_void, u32, *const *const Lv2Feature) -> u32 =
            unsafe { std::mem::transmute(save) };
        let mut saved = Store::new();
        let status = unsafe { save(instance as *mut c_void, Some(store), (&mut saved as *mut Store).cast(), 0, std::ptr::null()) };
        (status, saved)
    });
    let start = Instant::now();
    while !IN_SAVE.load(Ordering::SeqCst) {
        assert!(start.elapsed() < Duration::from_secs(30), "the save never reached the plugin");
        std::thread::yield_now();
    }
    assert_eq!(rig.run(), 1.0, "the block skipped the plugin: dry");
    HOLD_SAVE.store(false, Ordering::SeqCst);
    let (status, saved) = saver.join().unwrap();
    assert_eq!(status, 0);
    assert_eq!(saved[0].1, b"ir!");
    assert_eq!(rig.run(), 3.0, "the plugin runs again after the save");
}

const CHUNK: &str = "http://lv2plug.in/ns/ext/atom#Chunk";

#[test]
fn ttl_lists_the_state_interface() {
    let ttl = lv2::plugin_ttl(&Loaded::default(), false);
    assert!(ttl.contains("@prefix state: <http://lv2plug.in/ns/ext/state#> ."));
    assert!(ttl.contains("lv2:extensionData work:interface , state:interface ;"));
}
