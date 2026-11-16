//! The LV2 `worker` extension through the exported descriptor, driven by a fake
//! host worker: `schedule_work` from `run`, `work` on another thread,
//! `work_response` before a later `run`. Also the fallback without the
//! feature, hostile hosts, and the TTL.

mod common;

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::worker::{interface, FakeWorker, Loading, JOB_RAN, NEXT};
use common::*;
use openplugin_core::Plugin;
use openplugin_targets::lv2::{self, Lv2Descriptor, Lv2Feature};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn loading() -> Box<dyn Plugin> {
    Box::new(Loading::default())
}

struct Rig {
    desc: &'static Lv2Descriptor,
    instance: *mut c_void,
    input: [Vec<f32>; 2],
    output: [Vec<f32>; 2],
    level: Box<std::cell::Cell<f32>>,
}

impl Rig {
    fn new(features: *const *const Lv2Feature) -> Self {
        lv2::set_creator(loading);
        let desc = unsafe { &*lv2::descriptor(0) };
        let instance = (desc.instantiate)(desc, 48_000.0, std::ptr::null(), features);
        assert!(!instance.is_null());
        let mut rig = Self {
            desc,
            instance,
            input: [vec![1.0; 64], vec![1.0; 64]],
            output: [vec![0.0; 64], vec![0.0; 64]],
            level: Box::new(std::cell::Cell::new(0.5)),
        };
        for c in 0..2 {
            (desc.connect_port)(instance, c as u32, rig.input[c].as_mut_ptr().cast());
            (desc.connect_port)(instance, c as u32 + 2, rig.output[c].as_mut_ptr().cast());
        }
        (desc.connect_port)(instance, 4, rig.level.as_ptr().cast());
        (desc.activate)(instance);
        rig
    }

    fn run(&self) -> f32 {
        (self.desc.run)(self.instance, 64);
        self.output[0][63]
    }

    fn stats(&self) -> lv2::WorkerStats {
        lv2::worker_stats(self.instance).expect("live instance")
    }

    /// Run blocks until the output differs from `old`, for a loader on its own
    /// thread: when its job lands depends on the scheduler, so the test waits
    /// for it (with a deadline far above any real delay) instead of assuming
    /// it has or has not happened by a given block.
    fn run_until_changed(&self, old: f32, what: &str) -> f32 {
        let start = Instant::now();
        loop {
            let out = self.run();
            if out != old {
                return out;
            }
            assert!(start.elapsed() < LANDING_DEADLINE, "{what}: the loader's own thread never delivered");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// How long a test waits for a loader thread's job to land.
const LANDING_DEADLINE: Duration = Duration::from_secs(30);

impl Drop for Rig {
    fn drop(&mut self) {
        (self.desc.deactivate)(self.instance);
        (self.desc.cleanup)(self.instance);
    }
}

fn urid_feature(host: &Host) -> *const Lv2Feature {
    unsafe { *host.features() }
}

fn job_threads() -> Vec<(std::thread::ThreadId, String)> {
    JOB_RAN.lock().unwrap().clone()
}

#[test]
fn jobs_run_on_the_hosts_worker_and_results_arrive_on_the_next_run() {
    let _g = serial();
    let host = Host::new();
    let fw = FakeWorker::new();
    let features = fw.features(Some(urid_feature(&host)));
    let rig = Rig::new(features.as_ptr());
    let main = std::thread::current().id();
    JOB_RAN.lock().unwrap().clear();

    assert_eq!(rig.run(), 1.0);
    assert_eq!(fw.requests(), 0, "nothing to load yet");

    // Moving the parameter inside run queues a job; run ends by asking the host.
    rig.level.set(0.8);
    assert_eq!(rig.run(), 1.0, "the old value is used until the job lands");
    assert_eq!(fw.requests(), 1);
    assert_eq!(fw.request(0).0, 4, "the request is a four-byte task number");
    assert_eq!(fw.scheduled_on(), Some(main), "scheduled from the audio thread, inside run");
    assert!(job_threads().is_empty(), "the host has not run the job yet");
    assert_eq!(rig.run(), 1.0);
    assert_eq!(fw.requests(), 1, "one request, not one per block");

    // The host's worker thread runs it and answers.
    let status = fw.run_worker(rig.desc, rig.instance);
    assert_eq!(status, [0]);
    let ran = job_threads();
    assert_eq!(ran.len(), 1);
    assert_ne!(ran[0].0, main, "work ran off the audio thread");
    assert_ne!(ran[0].1, "openplugin-worker", "no thread of the SDK's own was used");
    assert_eq!(*fw.worked_on.lock().unwrap(), [ran[0].0]);

    // work_response arrives on the audio thread before the next run.
    assert_eq!(fw.deliver(rig.desc, rig.instance), 1);
    let expected = NEXT.load(Ordering::SeqCst) as f32;
    assert_eq!(rig.run(), expected);
    fw.end_run(rig.desc, rig.instance);
    let stats = rig.stats();
    assert_eq!((stats.responses, stats.worked, stats.rejected), (1, 1, 0));

    // The swap retired the old value: one more request lets the host free it
    // (it has nothing to run, so no response).
    assert_eq!(fw.requests(), 1);
    assert_eq!(fw.run_worker(rig.desc, rig.instance), [0]);
    assert_eq!(fw.deliver(rig.desc, rig.instance), 0);
    assert_eq!(rig.run(), expected);
    assert_eq!(fw.requests(), 0);
    drop(rig);
    drop(features);
}

#[test]
fn many_changes_in_a_burst_all_settle() {
    let _g = serial();
    let host = Host::new();
    let fw = FakeWorker::new();
    let features = fw.features(Some(urid_feature(&host)));
    let rig = Rig::new(features.as_ptr());
    for i in 0..40 {
        rig.level.set(0.1 + i as f32 * 0.01);
        let out = rig.run();
        assert!(out.is_finite() && out >= 1.0);
    }
    // The job queue holds four; the rest were refused by the loader and dropped.
    for _ in 0..20 {
        fw.run_worker(rig.desc, rig.instance);
        fw.deliver(rig.desc, rig.instance);
        rig.run();
        fw.end_run(rig.desc, rig.instance);
    }
    assert_eq!(rig.run(), NEXT.load(Ordering::SeqCst) as f32);
    assert_eq!(rig.stats().rejected, 0);
}

#[test]
fn without_the_feature_the_loader_keeps_its_own_thread() {
    let _g = serial();
    let host = Host::new();
    let rig = Rig::new(host.features());
    JOB_RAN.lock().unwrap().clear();
    assert!(!(rig.desc.extension_data)(c"http://lv2plug.in/ns/ext/worker#interface".as_ptr()).is_null());
    assert_eq!(rig.run(), 1.0);
    rig.level.set(0.9);
    let landed = rig.run_until_changed(1.0, "without the feature");
    assert_eq!(landed, NEXT.load(Ordering::SeqCst) as f32);
    let ran = job_threads();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].1, "openplugin-worker");
    let stats = rig.stats();
    assert_eq!((stats.scheduled, stats.worked), (0, 0));
}

#[test]
fn a_host_with_no_urid_map_or_no_feature_array_still_works() {
    let _g = serial();
    let fw = FakeWorker::new();
    let features = fw.features(None);
    let rig = Rig::new(features.as_ptr());
    rig.level.set(0.7);
    rig.run();
    assert_eq!(fw.requests(), 1);
    // No feature array: the loader runs on its own thread. Its job may land
    // before the block that queued it polls (a busy machine can preempt the
    // test thread right after the submit), so the first block may already
    // have the new value; either way the value lands, which is what counts.
    let none = Rig::new(std::ptr::null());
    JOB_RAN.lock().unwrap().clear();
    none.level.set(0.7);
    let landed = none.run_until_changed(1.0, "no feature array");
    assert_eq!(landed, NEXT.load(Ordering::SeqCst) as f32);
    let ran = job_threads();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].1, "openplugin-worker");
    assert_eq!(none.run(), landed, "the landed value stays");
}

#[test]
fn a_refused_schedule_is_kept_and_retried() {
    let _g = serial();
    let host = Host::new();
    let fw = FakeWorker::new();
    let features = fw.features(Some(urid_feature(&host)));
    let rig = Rig::new(features.as_ptr());
    fw.refuse.store(true, Ordering::SeqCst);
    rig.level.set(0.8);
    for _ in 0..3 {
        assert_eq!(rig.run(), 1.0);
    }
    assert_eq!(fw.requests(), 0);
    let stats = rig.stats();
    assert_eq!(stats.scheduled, 0);
    assert!(stats.refused >= 3, "retried every block: {stats:?}");
    // The host recovers: the kept request goes out, and the job lands.
    fw.refuse.store(false, Ordering::SeqCst);
    rig.run();
    assert_eq!(fw.requests(), 1);
    assert_eq!(fw.run_worker(rig.desc, rig.instance), [0]);
    fw.deliver(rig.desc, rig.instance);
    assert_eq!(rig.run(), NEXT.load(Ordering::SeqCst) as f32);
}

#[test]
fn malformed_work_calls_are_refused_without_running_anything() {
    let _g = serial();
    let host = Host::new();
    let fw = FakeWorker::new();
    let features = fw.features(Some(urid_feature(&host)));
    let rig = Rig::new(features.as_ptr());
    let iface = interface(rig.desc);
    let word = [3u64, 0, 0, 0];
    let aligned = word.as_ptr() as *const u8;
    let call = |handle: *mut c_void, size: u32, data: *const u8| unsafe { (iface.work)(handle, None, std::ptr::null_mut(), size, data.cast()) };
    let h = rig.instance;
    let small = [0u32; 1];
    let index = |n: u32| n.to_ne_bytes();
    // Wrong sizes, including ones that would run far past the buffer.
    for size in [0u32, 1, 3, 5, 8, 4096, u32::MAX] {
        assert_ne!(call(h, size, small.as_ptr().cast()), 0, "size {size}");
    }
    assert_ne!(call(h, 4, std::ptr::null()), 0, "null data");
    assert_ne!(call(h, 4, unsafe { aligned.add(1) }), 0, "misaligned data");
    // Task numbers that were never handed out, and out of range.
    for n in [5u32, 7, 8, 9, 1000, u32::MAX] {
        let bytes = index(n);
        let aligned_copy = [u32::from_ne_bytes(bytes)];
        assert_ne!(call(h, 4, aligned_copy.as_ptr().cast()), 0, "task {n}");
    }
    // A handle that is not an instance.
    assert_ne!(call(0x1000 as *mut c_void, 4, small.as_ptr().cast()), 0);
    assert_ne!(call(std::ptr::null_mut(), 4, small.as_ptr().cast()), 0);
    // work_response with garbage is a harmless notice.
    assert_eq!(unsafe { (iface.work_response)(h, u32::MAX, std::ptr::null()) }, 0);
    assert_eq!(unsafe { (iface.work_response)(0x1000 as *mut c_void, 0, std::ptr::null()) }, 0);
    assert_eq!(unsafe { (iface.end_run)(std::ptr::null_mut()) }, 0);
    let stats = rig.stats();
    assert_eq!(stats.worked, 0);
    assert!(stats.rejected >= 14);
    // And the instance still works afterwards.
    rig.level.set(0.6);
    rig.run();
    assert_eq!(fw.run_worker(rig.desc, rig.instance), [0]);
    fw.deliver(rig.desc, rig.instance);
    assert_eq!(rig.run(), NEXT.load(Ordering::SeqCst) as f32);
}

#[test]
fn work_after_cleanup_is_refused() {
    let _g = serial();
    let host = Host::new();
    let fw = FakeWorker::new();
    let features = fw.features(Some(urid_feature(&host)));
    let rig = Rig::new(features.as_ptr());
    rig.level.set(0.8);
    rig.run();
    assert_eq!(fw.requests(), 1);
    let (desc, instance) = (rig.desc, rig.instance);
    drop(rig);
    // The job was still queued: the instance is gone and so is its worker.
    assert!(lv2::worker_stats(instance).is_none());
    let status = fw.run_worker(desc, instance);
    assert_ne!(status, [0]);
}

#[test]
fn the_ttl_declares_the_worker() {
    let ttl = lv2::plugin_ttl(&Loading::default(), false);
    assert!(ttl.contains("@prefix work: <http://lv2plug.in/ns/ext/worker#> ."));
    assert!(ttl.contains("lv2:optionalFeature lv2:hardRTCapable , work:schedule ;"));
    assert!(!ttl.contains("requiredFeature work:"));
    assert!(ttl.contains("lv2:extensionData work:interface , state:interface ;"));
}

/// Writes the bundle's description files and runs `lv2_validate` on them when
/// it is installed. The files go under cargo's per-target temp folder.
#[test]
fn lv2_validate_accepts_the_description() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("lv2-worker-bundle.lv2");
    std::fs::create_dir_all(&dir).unwrap();
    lv2::write_bundle(&Loading::default(), &dir, "libloading", false).unwrap();
    // Relative names from inside the bundle: the tool splits paths at spaces.
    let ran = std::process::Command::new("lv2_validate").current_dir(&dir).args(["manifest.ttl", "libloading.ttl"]).output();
    match ran {
        Ok(out) => {
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            assert!(out.status.success() && text.contains("Found 0 errors") && !text.contains("error:"), "lv2_validate said:\n{text}");
        }
        Err(_) => eprintln!("lv2_validate is not installed; skipped"),
    }
}
