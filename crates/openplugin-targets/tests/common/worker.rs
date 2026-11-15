//! A fake LV2 host worker (`worker:schedule` and the host side of
//! `worker:interface`) and a plugin that loads through `openplugin_core::worker`.
#![allow(dead_code)]

use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::thread::ThreadId;

use openplugin_core::worker::{loader, Loader, Slot};
use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_targets::lv2::{Lv2Descriptor, Lv2Feature};

pub static LOAD_PARAMS: [ParamInfo; 1] = [ParamInfo::new("level", "Level", 0.0, 1.0, 0.5, Unit::None)];

pub static LOADING_INFO: PluginInfo = PluginInfo {
    id: "urn:openplugin:test-loading",
    name: "Test Loading",
    vendor: "Test",
    url: "https://example.com",
    email: "a@b.c",
    version: "0.1.0",
    category: Category::Utility,
    params: &LOAD_PARAMS,
    is_instrument: false,
    has_editor: false,
};

/// Where each job ran: thread id and name.
pub static JOB_RAN: Mutex<Vec<(ThreadId, String)>> = Mutex::new(Vec::new());
pub static NEXT: AtomicU32 = AtomicU32::new(10);

/// An effect whose gain is a loaded value: moving `level` queues a job that
/// returns the next number (11, 12, ...), and the output is the input times
/// the value in force. The job closure captures nothing (so boxing it does not
/// allocate); the loader is made in the constructor.
pub struct Loading {
    level: f32,
    loader: Loader<u32>,
    slot: Slot<u32>,
    pub polled: u32,
}

impl Default for Loading {
    fn default() -> Self {
        let (loader, slot) = loader::<u32>(1, 4);
        Self { level: 0.5, loader, slot, polled: 0 }
    }
}

fn job() -> u32 {
    let current = std::thread::current();
    if let Ok(mut ran) = JOB_RAN.lock() {
        ran.push((current.id(), current.name().unwrap_or("").to_string()));
    }
    NEXT.fetch_add(1, Ordering::SeqCst) + 1
}

impl Plugin for Loading {
    fn info(&self) -> &'static PluginInfo {
        &LOADING_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id != "level" {
            return false;
        }
        if value != self.level {
            // A full queue just drops the request.
            let _ = self.loader.submit(job);
        }
        self.level = value;
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        (id == "level").then_some(self.level)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if self.slot.poll() {
            self.polled += 1;
        }
        let gain = *self.slot.get() as f32;
        for channel in io.iter_mut() {
            for s in channel[..frames].iter_mut() {
                *s *= gain;
            }
        }
    }
}

type Respond = unsafe extern "C" fn(*mut c_void, u32, *const c_void) -> u32;

#[repr(C)]
pub struct WorkerInterface {
    pub work: unsafe extern "C" fn(*mut c_void, Option<Respond>, *mut c_void, u32, *const c_void) -> u32,
    pub work_response: unsafe extern "C" fn(*mut c_void, u32, *const c_void) -> u32,
    pub end_run: unsafe extern "C" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct ScheduleFeature {
    handle: *mut c_void,
    schedule_work: unsafe extern "C" fn(*mut c_void, u32, *const c_void) -> u32,
}

const MAX_REQUESTS: usize = 64;

struct Requests {
    sizes: [u32; MAX_REQUESTS],
    data: [[u8; 16]; MAX_REQUESTS],
    len: usize,
    scheduled_on: Option<ThreadId>,
}

struct Responses {
    len: usize,
    data: [[u8; 16]; MAX_REQUESTS],
    sizes: [u32; MAX_REQUESTS],
}

/// The host's side: records `schedule_work`, runs `work` on a thread of its
/// own, and queues the responses for `work_response`.
pub struct FakeWorker {
    requests: Mutex<Requests>,
    responses: Mutex<Responses>,
    pub refuse: AtomicBool,
    pub worked_on: Mutex<Vec<ThreadId>>,
    feature: Box<ScheduleFeature>,
    uri: std::ffi::CString,
    lv2: Box<Lv2Feature>,
}

unsafe extern "C" fn schedule(handle: *mut c_void, size: u32, data: *const c_void) -> u32 {
    let host = unsafe { &*(handle as *const FakeWorker) };
    if host.refuse.load(Ordering::SeqCst) {
        return 1;
    }
    let mut r = host.requests.lock().unwrap();
    if r.len == MAX_REQUESTS {
        return 2;
    }
    let at = r.len;
    r.sizes[at] = size;
    let n = (size as usize).min(16);
    let mut copy = [0u8; 16];
    if n > 0 && !data.is_null() {
        copy[..n].copy_from_slice(unsafe { std::slice::from_raw_parts(data as *const u8, n) });
    }
    r.data[at] = copy;
    r.len += 1;
    r.scheduled_on = Some(std::thread::current().id());
    0
}

unsafe extern "C" fn respond(handle: *mut c_void, size: u32, data: *const c_void) -> u32 {
    let host = unsafe { &*(handle as *const FakeWorker) };
    let mut r = host.responses.lock().unwrap();
    let at = r.len;
    if at == MAX_REQUESTS {
        return 2;
    }
    let n = (size as usize).min(16);
    let mut copy = [0u8; 16];
    if n > 0 && !data.is_null() {
        copy[..n].copy_from_slice(unsafe { std::slice::from_raw_parts(data as *const u8, n) });
    }
    r.data[at] = copy;
    r.sizes[at] = size;
    r.len += 1;
    0
}

impl FakeWorker {
    pub fn new() -> Box<Self> {
        let mut host = Box::new(Self {
            requests: Mutex::new(Requests { sizes: [0; MAX_REQUESTS], data: [[0; 16]; MAX_REQUESTS], len: 0, scheduled_on: None }),
            responses: Mutex::new(Responses { len: 0, data: [[0; 16]; MAX_REQUESTS], sizes: [0; MAX_REQUESTS] }),
            refuse: AtomicBool::new(false),
            worked_on: Mutex::new(Vec::new()),
            feature: Box::new(ScheduleFeature { handle: std::ptr::null_mut(), schedule_work: schedule }),
            uri: std::ffi::CString::new("http://lv2plug.in/ns/ext/worker#schedule").unwrap(),
            lv2: Box::new(Lv2Feature { uri: std::ptr::null(), data: std::ptr::null_mut() }),
        });
        let me = (&*host) as *const FakeWorker as *mut c_void;
        host.feature.handle = me;
        host.lv2.uri = host.uri.as_ptr();
        host.lv2.data = (&*host.feature) as *const ScheduleFeature as *mut c_void;
        host
    }

    /// The feature array: the given `urid:map` feature (if any) then this.
    pub fn features(&self, urid: Option<*const Lv2Feature>) -> Vec<*const Lv2Feature> {
        let mut out = Vec::new();
        out.extend(urid);
        out.push(&*self.lv2 as *const Lv2Feature);
        out.push(std::ptr::null());
        out
    }

    pub fn requests(&self) -> usize {
        self.requests.lock().unwrap().len
    }

    pub fn request(&self, at: usize) -> (u32, [u8; 16]) {
        let r = self.requests.lock().unwrap();
        (r.sizes[at], r.data[at])
    }

    pub fn scheduled_on(&self) -> Option<ThreadId> {
        self.requests.lock().unwrap().scheduled_on
    }

    pub fn clear(&self) {
        self.requests.lock().unwrap().len = 0;
    }

    /// Run every scheduled request through `work` on a new thread. Returns the
    /// status of each call.
    pub fn run_worker(&self, desc: &'static Lv2Descriptor, instance: *mut c_void) -> Vec<u32> {
        let taken: Vec<(u32, [u8; 16])> = {
            let mut r = self.requests.lock().unwrap();
            let items = (0..r.len).map(|i| (r.sizes[i], r.data[i])).collect();
            r.len = 0;
            items
        };
        let me = self as *const FakeWorker as usize;
        let instance = instance as usize;
        std::thread::scope(|scope| {
            scope
                .spawn(move || {
                    let iface = interface(desc);
                    let host = unsafe { &*(me as *const FakeWorker) };
                    host.worked_on.lock().unwrap().push(std::thread::current().id());
                    taken
                        .iter()
                        .map(|(size, data)| unsafe { (iface.work)(instance as *mut c_void, Some(respond), me as *mut c_void, *size, data.as_ptr().cast()) })
                        .collect()
                })
                .join()
                .unwrap()
        })
    }

    /// What the host does at the start of a block: `work_response` for each
    /// response the worker made. Returns how many.
    pub fn deliver(&self, desc: &'static Lv2Descriptor, instance: *mut c_void) -> usize {
        let iface = interface(desc);
        let (count, data, sizes) = {
            let mut r = self.responses.lock().unwrap();
            let out = (r.len, r.data, r.sizes);
            r.len = 0;
            out
        };
        for i in 0..count {
            let status = unsafe { (iface.work_response)(instance, sizes[i], data[i].as_ptr().cast()) };
            assert_eq!(status, 0);
        }
        count
    }

    pub fn end_run(&self, desc: &'static Lv2Descriptor, instance: *mut c_void) {
        assert_eq!(unsafe { (interface(desc).end_run)(instance) }, 0);
    }
}

pub fn interface(desc: &Lv2Descriptor) -> &'static WorkerInterface {
    let uri: &CStr = c"http://lv2plug.in/ns/ext/worker#interface";
    let ptr = (desc.extension_data)(uri.as_ptr() as *const c_char) as *const WorkerInterface;
    assert!(!ptr.is_null(), "worker interface missing");
    unsafe { &*ptr }
}
