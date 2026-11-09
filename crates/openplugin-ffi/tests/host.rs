//! The symbols `export_openplugin!` emits, exercised the way a JUCE processor calls them.
//! The counting allocator is the Seven Halls realtime check: after prepare, process
//! and `op_set_param` must not allocate.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use openplugin_core::{Category, MeterFrame, ParamInfo, PluginInfo, Unit};
use openplugin_ffi::{export_openplugin, OpMeters, OpSpec, Plugin};

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::SeqCst);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::SeqCst);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

struct Wire {
    db: f32,
    linear: f32,
    offline: bool,
}

impl Default for Wire {
    fn default() -> Self {
        Self {
            db: 0.0,
            linear: 1.0,
            offline: false,
        }
    }
}

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
    ParamInfo::new("mode", "Mode", 0.0, 2.0, 0.0, Unit::None).with_steps(3),
];
static PRESET_FLAT: [f32; 2] = [0.0, 0.0];
static PRESET_HOT: [f32; 2] = [6.0, 2.0];

static INFO: PluginInfo = PluginInfo {
    id: "com.example.wire",
    name: "Wire",
    vendor: "Test",
    url: "https://example.com",
    email: "a@b.c",
    version: "0",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

impl Plugin for Wire {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {
        self.linear = 10f32.powf(self.db / 20.0);
    }
    fn reset(&mut self) {}
    fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "gain" => {
                self.db = PARAMS[0].clamp(value);
                self.linear = 10f32.powf(self.db / 20.0);
                true
            }
            "mode" => {
                let _ = PARAMS[1].clamp(value);
                true
            }
            _ => false,
        }
    }
    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "gain" => Some(self.db),
            "mode" => Some(0.0),
            _ => None,
        }
    }
    fn meters(&self) -> MeterFrame {
        MeterFrame {
            input_db: [self.db, self.db],
            output_db: [self.linear, self.linear],
            reduction_db: if self.offline { 1.0 } else { 0.0 },
        }
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let gain = if self.offline { 0.0 } else { self.linear };
        for channel in io {
            for sample in &mut channel[..frames] {
                *sample *= gain;
            }
        }
    }
    fn preset_count(&self) -> usize {
        2
    }
    fn preset_name(&self, index: usize) -> &'static str {
        ["Flat", "Hot"][index]
    }
    fn preset_values(&self, index: usize) -> Option<&'static [f32]> {
        match index {
            0 => Some(&PRESET_FLAT),
            1 => Some(&PRESET_HOT),
            _ => None,
        }
    }
}

export_openplugin!(Wire);

fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[test]
fn host_calls_match_seven_halls_and_process_does_not_allocate() {
    assert_eq!(op_param_count(), 2);
    assert_eq!(op_param_info(0, std::ptr::null_mut()), 0);
    let mut spec = unsafe { std::mem::zeroed::<OpSpec>() };
    assert_eq!(op_param_info(0, &mut spec), 1);
    assert_eq!(spec.unit, 1);
    assert_eq!(spec.is_discrete, 0);
    assert_eq!(op_param_info(1, &mut spec), 1);
    assert_eq!(spec.steps, 3);
    assert_eq!(spec.is_discrete, 1);
    assert!(std::str::from_utf8(&spec.name).unwrap().starts_with("Mode"));

    let mut name = [0u8; 32];
    op_plugin_name(name.as_mut_ptr(), name.len() as i32);
    assert!(std::str::from_utf8(&name).unwrap().starts_with("Wire"));
    assert_eq!(op_preset_count(), 2);
    let mut preset = [0u8; 16];
    op_preset_name(1, preset.as_mut_ptr(), preset.len() as i32);
    assert!(std::str::from_utf8(&preset).unwrap().starts_with("Hot"));

    let plugin = op_create(48_000.0, 8, 2);
    assert!(!plugin.is_null());
    assert_eq!(op_prepare(plugin, 48_000.0, 0, 2), 0);
    assert_eq!(op_latency(plugin), 0);

    assert_eq!(op_set_param(plugin, 0, 6.0), 1);
    assert!((op_get_param(plugin, 0) - 6.0).abs() < 1e-6);

    let mut left = [0.5f32; 4];
    let mut right = [0.25f32; 4];
    assert_eq!(
        op_process_stereo(
            plugin,
            left.as_ptr(),
            right.as_ptr(),
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            4
        ),
        1
    );
    let loud = db_to_linear(6.0);
    assert!((left[0] - 0.5 * loud).abs() < 1e-4);
    assert!((right[3] - 0.25 * loud).abs() < 1e-4);
    assert_eq!(
        op_process_stereo(
            plugin,
            left.as_ptr(),
            right.as_ptr(),
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            9
        ),
        0
    );

    left = [0.5; 4];
    right = [0.25; 4];
    let input: [*const f32; 2] = [left.as_ptr(), right.as_ptr()];
    let mut output: [*mut f32; 2] = [left.as_mut_ptr(), right.as_mut_ptr()];
    assert_eq!(
        op_process_planar(plugin, input.as_ptr(), output.as_mut_ptr(), 2, 4),
        1
    );
    assert!((left[1] - 0.5 * loud).abs() < 1e-4);
    assert!((right[1] - 0.25 * loud).abs() < 1e-4);

    let mut interleaved = [0.5f32, -0.5, f32::NAN, 0.25];
    assert_eq!(
        op_process_interleaved(plugin, interleaved.as_ptr(), interleaved.as_mut_ptr(), 2, 2),
        1
    );
    assert!(interleaved.iter().all(|sample| sample.is_finite()));
    assert!((interleaved[0] - 0.5 * loud).abs() < 1e-4);
    assert!((interleaved[2]).abs() < 1e-6);

    assert_eq!(op_apply_preset(plugin, 1), 1);
    assert_eq!(op_preset_index(plugin), 1);
    assert!((op_get_param(plugin, 1) - 2.0).abs() < 1e-4);
    let bytes = op_save(plugin, std::ptr::null_mut(), 0);
    assert!(bytes > 0);
    let mut blob = vec![0u8; bytes as usize];
    assert_eq!(op_save(plugin, blob.as_mut_ptr(), bytes as u32), bytes);
    assert_eq!(op_set_param(plugin, 0, -12.0), 1);
    assert_eq!(op_load(plugin, blob.as_ptr(), bytes as u32), 1);
    assert!((op_get_param(plugin, 0) - 6.0).abs() < 1e-4);

    op_set_offline(plugin, 1);
    left = [0.5; 4];
    right = [0.5; 4];
    assert_eq!(
        op_process_stereo(
            plugin,
            left.as_ptr(),
            right.as_ptr(),
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            4
        ),
        1
    );
    assert_eq!(left[0], 0.0);
    let mut meters = OpMeters::SILENT;
    op_meters(plugin, &mut meters);
    assert!((meters.reduction_db - 1.0).abs() < 1e-6);

    op_set_offline(plugin, 0);
    assert_eq!(op_prepare(plugin, 44_100.0, 8, 2), 1);
    op_reset(plugin);
    left = [0.5; 4];
    right = [0.25; 4];
    assert_eq!(
        op_process_stereo(
            plugin,
            left.as_ptr(),
            right.as_ptr(),
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            4
        ),
        1
    );

    let before = ALLOC_COUNT.load(Ordering::SeqCst);
    assert_eq!(op_set_param(plugin, 0, 3.0), 1);
    assert_eq!(
        ALLOC_COUNT.load(Ordering::SeqCst),
        before,
        "op_set_param allocated"
    );
    let before = ALLOC_COUNT.load(Ordering::SeqCst);
    assert_eq!(
        op_process_stereo(
            plugin,
            left.as_ptr(),
            right.as_ptr(),
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            4
        ),
        1
    );
    assert_eq!(
        ALLOC_COUNT.load(Ordering::SeqCst),
        before,
        "op_process_stereo allocated"
    );
    let before = ALLOC_COUNT.load(Ordering::SeqCst);
    assert_eq!(
        op_process_stereo(
            plugin,
            left.as_ptr(),
            right.as_ptr(),
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            4
        ),
        1
    );
    assert_eq!(
        ALLOC_COUNT.load(Ordering::SeqCst),
        before,
        "idle op_process_stereo allocated"
    );
    op_meters(plugin, &mut meters);
    assert_eq!(
        ALLOC_COUNT.load(Ordering::SeqCst),
        before,
        "op_meters allocated"
    );

    assert_eq!(
        op_process_stereo(
            std::ptr::null_mut(),
            left.as_ptr(),
            right.as_ptr(),
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            4
        ),
        0
    );
    assert_eq!(op_save(std::ptr::null_mut(), std::ptr::null_mut(), 0), -1);
    op_meters(std::ptr::null_mut(), &mut meters);
    assert_eq!(meters, OpMeters::SILENT);

    op_destroy(plugin);
}
