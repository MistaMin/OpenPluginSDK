//! LV2 `lv2_descriptor` for one stereo insert.
//!
//! The port map matches OpenGridder's shell: 0 and 1 are audio inputs, 2 and 3
//! are audio outputs. Parameter changes go through the [`ParamPlane`] so the
//! audio callback only pulls slots that changed. It does not lock.
//!
//! [`export_lv2!`] emits `lv2_descriptor` from a cdylib. [`Lv2Harness`] drives
//! the same render path in-process.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::{Mutex, OnceLock};

use openplugin_core::{MeterFrame, ParamCursor, ParamPlane, Plugin, Seqlock};

type CreateFn = fn() -> Box<dyn Plugin>;

static CREATOR: Mutex<Option<CreateFn>> = Mutex::new(None);

/// Install the plugin `lv2_descriptor` will instantiate.
pub fn set_creator(create: CreateFn) {
    *CREATOR.lock().expect("lv2 creator lock") = Some(create);
}

fn creator() -> Option<CreateFn> {
    *CREATOR.lock().expect("lv2 creator lock")
}

#[repr(C)]
pub struct Lv2Feature {
    pub uri: *const c_char,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct Lv2Descriptor {
    pub uri: *const c_char,
    pub instantiate: extern "C" fn(*const Lv2Descriptor, f64, *const c_char, *const *const Lv2Feature) -> *mut c_void,
    pub connect_port: extern "C" fn(*mut c_void, u32, *mut c_void),
    pub activate: extern "C" fn(*mut c_void),
    pub run: extern "C" fn(*mut c_void, u32),
    pub deactivate: extern "C" fn(*mut c_void),
    pub cleanup: extern "C" fn(*mut c_void),
    pub extension_data: extern "C" fn(*const c_char) -> *const c_void,
}

// The URI points at a process-lifetime `CString`. The function pointers do not
// capture thread-local state. The descriptor is published once and then only read.
unsafe impl Send for Lv2Descriptor {}
unsafe impl Sync for Lv2Descriptor {}

struct Instance {
    inner: Box<dyn Plugin>,
    plane: ParamPlane,
    cursor: ParamCursor,
    meters: Seqlock<MeterFrame>,
    ports: [*mut f32; 4],
    scratch: [Vec<f32>; 2],
    prepared: usize,
}

const PREPARED: usize = 8192;

fn uri() -> &'static CStr {
    static URI: OnceLock<CString> = OnceLock::new();
    URI.get_or_init(|| {
        let id = creator().and_then(|create| CString::new(create().info().id).ok());
        id.unwrap_or_else(|| CString::new("urn:openplugin:plugin").expect("static uri"))
    })
    .as_c_str()
}

fn new_instance(sample_rate: f64) -> Option<Box<Instance>> {
    let create = creator()?;
    let mut inner = create();
    let info = inner.info();
    let plane = ParamPlane::new(info);
    plane.seed_from(inner.as_ref());
    let mut cursor = ParamCursor::new(plane.len());
    let rate = if sample_rate.is_finite() && sample_rate >= 8_000.0 {
        sample_rate as f32
    } else {
        48_000.0
    };
    inner.prepare(rate, PREPARED, 2);
    cursor.invalidate();
    cursor.pull(&plane, inner.as_mut());
    Some(Box::new(Instance {
        inner,
        plane,
        cursor,
        meters: Seqlock::new(MeterFrame::SILENT),
        ports: [std::ptr::null_mut(); 4],
        scratch: [vec![0.0; PREPARED], vec![0.0; PREPARED]],
        prepared: PREPARED,
    }))
}

fn render(instance: &mut Instance, frames: usize) {
    let frames = frames.min(instance.prepared);
    if frames == 0 {
        return;
    }
    instance.cursor.pull(&instance.plane, instance.inner.as_mut());
    for channel in 0..2 {
        let src = instance.ports[channel];
        if src.is_null() {
            instance.scratch[channel][..frames].fill(0.0);
        } else {
            let samples = unsafe { std::slice::from_raw_parts(src, frames) };
            instance.scratch[channel][..frames].copy_from_slice(samples);
        }
    }
    instance.inner.process(&mut instance.scratch, frames);
    for channel in 0..2 {
        let dst = instance.ports[channel + 2];
        if !dst.is_null() {
            let out = unsafe { std::slice::from_raw_parts_mut(dst, frames) };
            out.copy_from_slice(&instance.scratch[channel][..frames]);
        }
    }
    instance.meters.publish(instance.inner.meters());
}

extern "C" fn lv2_instantiate(
    _descriptor: *const Lv2Descriptor,
    sample_rate: f64,
    _bundle_path: *const c_char,
    _features: *const *const Lv2Feature,
) -> *mut c_void {
    match new_instance(sample_rate) {
        Some(instance) => Box::into_raw(instance) as *mut c_void,
        None => std::ptr::null_mut(),
    }
}

extern "C" fn lv2_connect(instance: *mut c_void, port: u32, data: *mut c_void) {
    if instance.is_null() || port > 3 {
        return;
    }
    unsafe {
        (*(instance as *mut Instance)).ports[port as usize] = data as *mut f32;
    }
}

extern "C" fn lv2_activate(instance: *mut c_void) {
    if instance.is_null() {
        return;
    }
    unsafe {
        let instance = &mut *(instance as *mut Instance);
        instance.inner.reset();
        instance.cursor.invalidate();
        instance.cursor.pull(&instance.plane, instance.inner.as_mut());
    }
}

extern "C" fn lv2_run(instance: *mut c_void, sample_count: u32) {
    if instance.is_null() || sample_count == 0 {
        return;
    }
    unsafe {
        let instance = &mut *(instance as *mut Instance);
        let mut done = 0;
        let frames = sample_count as usize;
        while done < frames {
            let n = (frames - done).min(instance.prepared);
            let saved = instance.ports;
            for port in &mut instance.ports {
                if !port.is_null() {
                    *port = port.add(done);
                }
            }
            render(instance, n);
            instance.ports = saved;
            done += n;
        }
    }
}

extern "C" fn lv2_deactivate(instance: *mut c_void) {
    if instance.is_null() {
        return;
    }
    unsafe {
        (*(instance as *mut Instance)).inner.reset();
    }
}

extern "C" fn lv2_cleanup(instance: *mut c_void) {
    if instance.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(instance as *mut Instance));
    }
}

extern "C" fn lv2_extension(_uri: *const c_char) -> *const c_void {
    std::ptr::null()
}

fn published() -> &'static Lv2Descriptor {
    static DESCRIPTOR: OnceLock<Lv2Descriptor> = OnceLock::new();
    DESCRIPTOR.get_or_init(|| Lv2Descriptor {
        uri: uri().as_ptr(),
        instantiate: lv2_instantiate,
        connect_port: lv2_connect,
        activate: lv2_activate,
        run: lv2_run,
        deactivate: lv2_deactivate,
        cleanup: lv2_cleanup,
        extension_data: lv2_extension,
    })
}

/// `lv2_descriptor`. Index 0 is the plugin. Call [`set_creator`] first.
pub fn descriptor(index: u32) -> *const Lv2Descriptor {
    if index == 0 && creator().is_some() {
        published()
    } else {
        std::ptr::null()
    }
}

/// In-process stereo LV2 run. Ports are owned vectors: 0 and 1 in, 2 and 3 out.
pub struct Lv2Harness<P: Plugin> {
    inner: P,
    plane: ParamPlane,
    cursor: ParamCursor,
    meters: Seqlock<MeterFrame>,
    ports: [Vec<f32>; 4],
    scratch: [Vec<f32>; 2],
}

impl<P: Plugin> Lv2Harness<P> {
    pub fn new(mut plugin: P, sample_rate: f32) -> Self {
        let info = plugin.info();
        let plane = ParamPlane::new(info);
        plane.seed_from(&plugin);
        let mut cursor = ParamCursor::new(plane.len());
        let rate = if sample_rate.is_finite() && sample_rate >= 8_000.0 {
            sample_rate
        } else {
            48_000.0
        };
        plugin.prepare(rate, PREPARED, 2);
        cursor.invalidate();
        cursor.pull(&plane, &mut plugin);
        Self {
            inner: plugin,
            plane,
            cursor,
            meters: Seqlock::new(MeterFrame::SILENT),
            ports: [vec![0.0; PREPARED], vec![0.0; PREPARED], vec![0.0; PREPARED], vec![0.0; PREPARED]],
            scratch: [vec![0.0; PREPARED], vec![0.0; PREPARED]],
        }
    }

    pub fn plane(&self) -> &ParamPlane {
        &self.plane
    }

    pub fn meters(&self) -> MeterFrame {
        self.meters.read()
    }

    pub fn plugin(&self) -> &P {
        &self.inner
    }

    pub fn input(&mut self, channel: usize) -> &mut [f32] {
        &mut self.ports[channel.min(1)]
    }

    pub fn output(&self, channel: usize) -> &[f32] {
        &self.ports[2 + channel.min(1)]
    }

    pub fn run(&mut self, frames: usize) {
        let frames = frames.min(PREPARED);
        self.cursor.pull(&self.plane, &mut self.inner);
        for channel in 0..2 {
            self.scratch[channel][..frames].copy_from_slice(&self.ports[channel][..frames]);
        }
        self.inner.process(&mut self.scratch, frames);
        for channel in 0..2 {
            self.ports[channel + 2][..frames].copy_from_slice(&self.scratch[channel][..frames]);
        }
        self.meters.publish(self.inner.meters());
    }
}

/// Export `lv2_descriptor` from a cdylib. `$plugin` implements [`Plugin`] and [`Default`].
#[macro_export]
macro_rules! export_lv2 {
    ($plugin:ty) => {
        fn __openplugin_lv2_create() -> ::std::boxed::Box<dyn $crate::PluginForExport> {
            ::std::boxed::Box::new(<$plugin as ::std::default::Default>::default())
        }

        #[no_mangle]
        pub extern "C" fn lv2_descriptor(index: u32) -> *const $crate::lv2::Lv2Descriptor {
            $crate::lv2::set_creator(__openplugin_lv2_create);
            $crate::lv2::descriptor(index)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use openplugin_core::{Category, ParamInfo, PluginInfo, Unit};

    struct Gain {
        db: f32,
        linear: f32,
    }

    static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];

    static INFO: PluginInfo = PluginInfo {
        id: "urn:openplugin:test-gain",
        name: "Test Gain",
        vendor: "OpenGrid",
        url: "https://github.com/MistaMin/OpenPluginSDK",
        email: "contact@mistamin.com",
        version: "0.1.0-beta.1",
        category: Category::Utility,
        params: &PARAMS,
        is_instrument: false,
        has_editor: false,
    };

    impl Plugin for Gain {
        fn info(&self) -> &'static PluginInfo {
            &INFO
        }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {
            self.linear = 10f32.powf(self.db / 20.0);
        }
        fn reset(&mut self) {}
        fn set_param(&mut self, id: &str, value: f32) -> bool {
            if id != "gain" {
                return false;
            }
            self.db = PARAMS[0].clamp(value);
            self.linear = 10f32.powf(self.db / 20.0);
            true
        }
        fn param(&self, id: &str) -> Option<f32> {
            (id == "gain").then_some(self.db)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            for channel in io {
                for sample in &mut channel[..frames] {
                    *sample *= self.linear;
                }
            }
        }
    }

    #[test]
    fn harness_pulls_the_plane_before_audio() {
        let mut harness = Lv2Harness::new(Gain { db: 0.0, linear: 1.0 }, 48_000.0);
        harness.input(0).fill(0.5);
        harness.input(1).fill(0.25);
        assert!(harness.plane().store(0, 6.0));
        harness.run(8);
        let expected = 0.5 * 10f32.powf(6.0 / 20.0);
        assert!((harness.output(0)[0] - expected).abs() < 1e-4);
        assert!((harness.output(1)[3] - 0.25 * 10f32.powf(6.0 / 20.0)).abs() < 1e-4);
    }

    #[test]
    fn descriptor_uses_the_plugin_uri() {
        set_creator(|| Box::new(Gain { db: 0.0, linear: 1.0 }));
        let desc = descriptor(0);
        assert!(!desc.is_null());
        unsafe {
            assert_eq!(CStr::from_ptr((*desc).uri), c"urn:openplugin:test-gain");
            let raw = ((*desc).instantiate)(desc, 48_000.0, std::ptr::null(), std::ptr::null());
            assert!(!raw.is_null());
            let mut input = [0.5f32; 4];
            let mut output = [0.0f32; 4];
            ((*desc).connect_port)(raw, 0, input.as_mut_ptr() as *mut c_void);
            ((*desc).connect_port)(raw, 2, output.as_mut_ptr() as *mut c_void);
            ((*desc).run)(raw, 4);
            assert!((output[0] - 0.5).abs() < 1e-5);
            ((*desc).cleanup)(raw);
        }
        assert!(descriptor(1).is_null());
    }
}
