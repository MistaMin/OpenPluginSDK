//! C ABI between a JUCE or native host and an [`Plugin`].
//!
//! Seven Halls, RadarVU and ArcVu ship the engine as a Rust static library.
//! The host builds its parameter list from a spec table, then the audio
//! callback only passes pointers and frame counts. [`export_openplugin!`]
//! emits that table for one plugin.
//!
//! `op_set_param`, `op_get_param`, `op_apply_preset` and `op_meters` do not
//! touch the plugin object. The audio thread applies the parameter plane
//! inside `op_process_*`. `prepare`, `reset` and `destroy` stay exclusive
//! with `process`, which is the JUCE rule.
#![allow(unsafe_code)]

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::OnceLock;

use openplugin_core::{
    decode_params, encode_params, MeterFrame, ParamCursor, ParamInfo, ParamPlane, PluginInfo,
    Seqlock, Taper, Unit,
};

pub use openplugin_core::Plugin;

const NAME: usize = 64;

/// Layout matches `OpSpec` in `include/openplugin.h`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpSpec {
    pub id: [u8; NAME],
    pub name: [u8; NAME],
    pub min: f32,
    pub max: f32,
    pub default_value: f32,
    pub is_discrete: i32,
    pub steps: i32,
    pub is_log: i32,
    pub unit: i32,
}

/// Layout matches `OpMeters` in `include/openplugin.h`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpMeters {
    pub input_db: [f32; 2],
    pub output_db: [f32; 2],
    pub reduction_db: f32,
}

impl OpMeters {
    pub const SILENT: Self = Self {
        input_db: [-100.0, -100.0],
        output_db: [-100.0, -100.0],
        reduction_db: 0.0,
    };
}

impl From<MeterFrame> for OpMeters {
    fn from(frame: MeterFrame) -> Self {
        Self {
            input_db: frame.input_db,
            output_db: frame.output_db,
            reduction_db: frame.reduction_db,
        }
    }
}

pub type CreatePlugin = fn() -> Box<dyn Plugin>;

static FACTORY: OnceLock<CreatePlugin> = OnceLock::new();
static INFO: OnceLock<&'static PluginInfo> = OnceLock::new();

struct PresetEntry {
    name: &'static str,
    values: &'static [f32],
}

static PRESETS: OnceLock<Vec<PresetEntry>> = OnceLock::new();

/// Register the plugin the exported C functions construct. A second call is ignored.
pub fn set_factory(create: CreatePlugin) {
    let _ = FACTORY.set(create);
}

fn prototype() -> Option<Box<dyn Plugin>> {
    FACTORY.get().map(|create| create())
}

fn plugin_info() -> Option<&'static PluginInfo> {
    if let Some(info) = INFO.get() {
        return Some(*info);
    }
    let plugin = prototype()?;
    Some(*INFO.get_or_init(|| plugin.info()))
}

fn preset_table() -> &'static [PresetEntry] {
    if FACTORY.get().is_none() {
        return &[];
    }
    PRESETS
        .get_or_init(|| {
            let Some(plugin) = prototype() else {
                return Vec::new();
            };
            let mut entries = Vec::new();
            for index in 0..plugin.preset_count() {
                if let Some(values) = plugin.preset_values(index) {
                    entries.push(PresetEntry {
                        name: plugin.preset_name(index),
                        values,
                    });
                }
            }
            entries
        })
        .as_slice()
}

pub fn param_count() -> i32 {
    plugin_info()
        .map(|info| info.params.len() as i32)
        .unwrap_or(0)
}

pub fn param_info(index: i32, out: &mut OpSpec) -> bool {
    let Some(info) = plugin_info() else {
        return false;
    };
    let Some(param) = info.params.get(index as usize) else {
        return false;
    };
    *out = spec_from(param);
    true
}

pub fn spec_from(param: &ParamInfo) -> OpSpec {
    let mut spec = OpSpec {
        id: [0; NAME],
        name: [0; NAME],
        min: param.min,
        max: param.max,
        default_value: param.default,
        is_discrete: 0,
        steps: 0,
        is_log: i32::from(param.taper == Taper::Log),
        unit: unit_code(param.unit),
    };
    copy_bytes(&mut spec.id, param.id);
    copy_bytes(&mut spec.name, param.name);
    let steps = if param.steps > 1 {
        param.steps as i32
    } else if param.unit == Unit::Choice && !param.choices.is_empty() {
        param.choices.len() as i32
    } else if param.unit == Unit::Toggle {
        2
    } else {
        0
    };
    spec.steps = steps;
    spec.is_discrete = i32::from(steps > 1);
    spec
}

fn unit_code(unit: Unit) -> i32 {
    match unit {
        Unit::None => 0,
        Unit::Db => 1,
        Unit::Hz => 2,
        Unit::Ms => 3,
        Unit::Percent => 4,
        Unit::Ratio => 5,
        Unit::Semitones => 6,
        Unit::Cents => 7,
        Unit::Seconds => 8,
        Unit::Toggle => 9,
        Unit::Choice => 10,
    }
}

fn copy_bytes(dst: &mut [u8], text: &str) {
    dst.fill(0);
    let limit = dst.len().saturating_sub(1);
    for (slot, byte) in dst.iter_mut().zip(text.bytes()).take(limit) {
        *slot = byte;
    }
}

pub fn copy_c_string(dst: &mut [u8], text: &str) {
    copy_bytes(dst, text);
}

struct Audio {
    inner: Box<dyn Plugin>,
    buffers: Vec<Vec<f32>>,
    cursor: ParamCursor,
    applied_offline: Option<bool>,
    channels: usize,
    max_block: usize,
}

/// One host instance. Message-thread calls use the plane and the atomics.
/// The audio thread is the only caller that enters `audio`.
pub struct FfiInstance {
    audio: UnsafeCell<Audio>,
    plane: ParamPlane,
    meters: Seqlock<MeterFrame>,
    latency: AtomicU32,
    offline: AtomicU32,
    preset: AtomicI32,
    info: &'static PluginInfo,
}

unsafe impl Send for FfiInstance {}
unsafe impl Sync for FfiInstance {}

impl FfiInstance {
    pub fn new(
        mut plugin: Box<dyn Plugin>,
        sample_rate: f64,
        max_block: usize,
        channels: usize,
    ) -> Self {
        let info = plugin.info();
        let plane = ParamPlane::new(info);
        plane.seed_from(plugin.as_ref());
        let max_block = max_block.clamp(1, 65_536);
        let channels = channels.clamp(1, 8);
        let rate = finite_rate(sample_rate);
        plugin.prepare(rate, max_block, channels);
        let mut cursor = ParamCursor::new(plane.len());
        cursor.pull(&plane, plugin.as_mut());
        let latency = plugin.latency() as u32;
        Self {
            audio: UnsafeCell::new(Audio {
                inner: plugin,
                buffers: (0..channels).map(|_| vec![0.0; max_block]).collect(),
                cursor,
                applied_offline: None,
                channels,
                max_block,
            }),
            plane,
            meters: Seqlock::new(MeterFrame::SILENT),
            latency: AtomicU32::new(latency),
            offline: AtomicU32::new(0),
            preset: AtomicI32::new(-1),
            info,
        }
    }

    pub fn store_param(&self, index: usize, value: f32) -> bool {
        self.plane.store(index, value)
    }

    pub fn load_param(&self, index: usize) -> f32 {
        self.plane.load(index).unwrap_or(0.0)
    }

    pub fn latency(&self) -> u32 {
        self.latency.load(Ordering::Acquire)
    }

    pub fn meters(&self) -> OpMeters {
        OpMeters::from(self.meters.read())
    }

    pub fn preset_index(&self) -> i32 {
        self.preset.load(Ordering::Acquire)
    }

    /// Publish a factory preset onto the plane. The audio thread applies it.
    pub fn apply_preset(&self, index: usize) -> bool {
        let Some(entry) = preset_table().get(index) else {
            return false;
        };
        if entry.values.len() != self.plane.len() {
            return false;
        }
        for (slot, value) in entry.values.iter().enumerate() {
            self.plane.store(slot, *value);
        }
        self.preset.store(index as i32, Ordering::Release);
        true
    }

    pub fn set_offline_flag(&self, offline: bool) {
        self.offline
            .store(if offline { 2 } else { 1 }, Ordering::Release);
    }

    /// Exclusive with `process`. Allocates the scratch buffers.
    pub fn prepare(&self, sample_rate: f64, max_block: usize, channels: usize) -> bool {
        if max_block == 0 || max_block > 65_536 || !(1..=8).contains(&channels) {
            return false;
        }
        let audio = unsafe { &mut *self.audio.get() };
        let rate = finite_rate(sample_rate);
        audio.inner.prepare(rate, max_block, channels);
        audio.buffers = (0..channels).map(|_| vec![0.0; max_block]).collect();
        audio.channels = channels;
        audio.max_block = max_block;
        audio.cursor.invalidate();
        audio.cursor.pull(&self.plane, audio.inner.as_mut());
        self.latency
            .store(audio.inner.latency() as u32, Ordering::Release);
        true
    }

    /// Exclusive with `process`.
    pub fn reset(&self) {
        let audio = unsafe { &mut *self.audio.get() };
        audio.inner.reset();
        for buffer in &mut audio.buffers {
            buffer.fill(0.0);
        }
    }

    pub fn save_bytes(&self) -> Vec<u8> {
        let values: Vec<f32> = (0..self.plane.len())
            .map(|index| self.load_param(index))
            .collect();
        encode_params(self.info, &values)
    }

    pub fn load_bytes(&self, bytes: &[u8]) -> bool {
        let Some(values) = decode_params(self.info, bytes) else {
            return false;
        };
        for (index, value) in values.iter().enumerate() {
            self.plane.store(index, *value);
        }
        true
    }

    pub fn process_stereo_raw(
        &self,
        in_l: *const f32,
        in_r: *const f32,
        out_l: *mut f32,
        out_r: *mut f32,
        frames: usize,
    ) -> bool {
        if in_l.is_null() || in_r.is_null() || out_l.is_null() || out_r.is_null() {
            return false;
        }
        let audio = unsafe { &mut *self.audio.get() };
        if frames > audio.max_block || audio.channels == 0 {
            return false;
        }
        if frames == 0 {
            return true;
        }
        copy_in(
            unsafe { std::slice::from_raw_parts(in_l, frames) },
            &mut audio.buffers[0][..frames],
        );
        if audio.channels > 1 {
            copy_in(
                unsafe { std::slice::from_raw_parts(in_r, frames) },
                &mut audio.buffers[1][..frames],
            );
        }
        let width = audio.channels.min(2);
        self.render(audio, width, frames);
        unsafe {
            std::ptr::copy_nonoverlapping(audio.buffers[0].as_ptr(), out_l, frames);
            let right = if audio.channels > 1 { 1 } else { 0 };
            std::ptr::copy_nonoverlapping(audio.buffers[right].as_ptr(), out_r, frames);
        }
        true
    }

    pub fn process_planar_raw(
        &self,
        input: *const *const f32,
        output: *mut *mut f32,
        channels: usize,
        frames: usize,
    ) -> bool {
        if input.is_null() || output.is_null() {
            return false;
        }
        let audio = unsafe { &mut *self.audio.get() };
        if channels == 0 || channels > audio.channels || frames > audio.max_block {
            return false;
        }
        if frames == 0 {
            return true;
        }
        for channel in 0..channels {
            let src = unsafe { *input.add(channel) };
            if src.is_null() {
                audio.buffers[channel][..frames].fill(0.0);
            } else {
                copy_in(
                    unsafe { std::slice::from_raw_parts(src, frames) },
                    &mut audio.buffers[channel][..frames],
                );
            }
        }
        self.render(audio, channels, frames);
        for channel in 0..channels {
            let dst = unsafe { *output.add(channel) };
            if dst.is_null() {
                continue;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(audio.buffers[channel].as_ptr(), dst, frames);
            }
        }
        true
    }

    pub fn process_interleaved_raw(
        &self,
        input: *const f32,
        output: *mut f32,
        channels: usize,
        frames: usize,
    ) -> bool {
        if input.is_null() || output.is_null() {
            return false;
        }
        let audio = unsafe { &mut *self.audio.get() };
        if channels == 0 || channels > audio.channels || frames > audio.max_block {
            return false;
        }
        if frames == 0 {
            return true;
        }
        for frame in 0..frames {
            for channel in 0..channels {
                let sample = unsafe { *input.add(frame * channels + channel) };
                audio.buffers[channel][frame] = if sample.is_finite() { sample } else { 0.0 };
            }
        }
        self.render(audio, channels, frames);
        for frame in 0..frames {
            for channel in 0..channels {
                unsafe {
                    *output.add(frame * channels + channel) = audio.buffers[channel][frame];
                }
            }
        }
        true
    }

    fn render(&self, audio: &mut Audio, channels: usize, frames: usize) {
        let flag = self.offline.load(Ordering::Acquire);
        let offline = match flag {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        };
        if let Some(offline) = offline {
            if audio.applied_offline != Some(offline) {
                audio.inner.set_offline(offline);
                audio.applied_offline = Some(offline);
            }
        }
        audio.cursor.pull(&self.plane, audio.inner.as_mut());
        audio.inner.process(&mut audio.buffers[..channels], frames);
        self.latency
            .store(audio.inner.latency() as u32, Ordering::Release);
        self.meters.publish(audio.inner.meters());
    }
}

fn finite_rate(sample_rate: f64) -> f32 {
    if sample_rate.is_finite() && sample_rate >= 8_000.0 && sample_rate <= 768_000.0 {
        sample_rate as f32
    } else {
        48_000.0
    }
}

fn copy_in(src: &[f32], dst: &mut [f32]) {
    for (out, sample) in dst.iter_mut().zip(src) {
        *out = if sample.is_finite() { *sample } else { 0.0 };
    }
}

/// Emit the `op_*` symbols documented in `include/openplugin.h`.
/// `$plugin` implements [`Plugin`] and [`Default`].
#[macro_export]
macro_rules! export_openplugin {
    ($plugin:ty) => {
        fn __openplugin_ffi_create() -> ::std::boxed::Box<dyn $crate::Plugin> {
            ::std::boxed::Box::new(<$plugin as ::std::default::Default>::default())
        }

        fn __openplugin_ffi_ready() {
            $crate::set_factory(__openplugin_ffi_create);
        }

        #[no_mangle]
        pub extern "C" fn op_param_count() -> i32 {
            __openplugin_ffi_ready();
            $crate::param_count()
        }

        #[no_mangle]
        pub extern "C" fn op_param_info(index: i32, out: *mut $crate::OpSpec) -> i32 {
            __openplugin_ffi_ready();
            if out.is_null() {
                return 0;
            }
            let spec = unsafe { &mut *out };
            i32::from($crate::param_info(index, spec))
        }

        #[no_mangle]
        pub extern "C" fn op_plugin_name(buf: *mut u8, cap: i32) {
            __openplugin_ffi_ready();
            if buf.is_null() || cap <= 0 {
                return;
            }
            let dst = unsafe { ::std::slice::from_raw_parts_mut(buf, cap as usize) };
            $crate::copy_c_string(dst, $crate::plugin_name());
        }

        #[no_mangle]
        pub extern "C" fn op_preset_count() -> i32 {
            __openplugin_ffi_ready();
            $crate::preset_count()
        }

        #[no_mangle]
        pub extern "C" fn op_preset_name(index: i32, buf: *mut u8, cap: i32) {
            __openplugin_ffi_ready();
            if buf.is_null() || cap <= 0 {
                return;
            }
            let dst = unsafe { ::std::slice::from_raw_parts_mut(buf, cap as usize) };
            $crate::copy_c_string(dst, $crate::preset_name(index));
        }

        #[no_mangle]
        pub extern "C" fn op_create(
            sample_rate: f64,
            max_block: u32,
            channels: u32,
        ) -> *mut $crate::FfiInstance {
            __openplugin_ffi_ready();
            let plugin = __openplugin_ffi_create();
            ::std::boxed::Box::into_raw(::std::boxed::Box::new($crate::FfiInstance::new(
                plugin,
                sample_rate,
                max_block as usize,
                channels as usize,
            )))
        }

        #[no_mangle]
        pub extern "C" fn op_destroy(plugin: *mut $crate::FfiInstance) {
            if !plugin.is_null() {
                drop(unsafe { ::std::boxed::Box::from_raw(plugin) });
            }
        }

        #[no_mangle]
        pub extern "C" fn op_prepare(
            plugin: *mut $crate::FfiInstance,
            sample_rate: f64,
            max_block: u32,
            channels: u32,
        ) -> i32 {
            match unsafe { plugin.as_mut() } {
                Some(plugin) => {
                    i32::from(plugin.prepare(sample_rate, max_block as usize, channels as usize))
                }
                None => 0,
            }
        }

        #[no_mangle]
        pub extern "C" fn op_reset(plugin: *mut $crate::FfiInstance) {
            if let Some(plugin) = unsafe { plugin.as_mut() } {
                plugin.reset();
            }
        }

        #[no_mangle]
        pub extern "C" fn op_set_offline(plugin: *mut $crate::FfiInstance, offline: i32) {
            if let Some(plugin) = unsafe { plugin.as_ref() } {
                plugin.set_offline_flag(offline != 0);
            }
        }

        #[no_mangle]
        pub extern "C" fn op_latency(plugin: *const $crate::FfiInstance) -> u32 {
            unsafe { plugin.as_ref() }
                .map(|plugin| plugin.latency())
                .unwrap_or(0)
        }

        #[no_mangle]
        pub extern "C" fn op_set_param(
            plugin: *mut $crate::FfiInstance,
            index: i32,
            value: f32,
        ) -> i32 {
            match unsafe { plugin.as_ref() } {
                Some(plugin) if index >= 0 => i32::from(plugin.store_param(index as usize, value)),
                _ => 0,
            }
        }

        #[no_mangle]
        pub extern "C" fn op_get_param(plugin: *const $crate::FfiInstance, index: i32) -> f32 {
            match unsafe { plugin.as_ref() } {
                Some(plugin) if index >= 0 => plugin.load_param(index as usize),
                _ => 0.0,
            }
        }

        #[no_mangle]
        pub extern "C" fn op_apply_preset(plugin: *mut $crate::FfiInstance, index: i32) -> i32 {
            __openplugin_ffi_ready();
            match unsafe { plugin.as_ref() } {
                Some(plugin) if index >= 0 => i32::from(plugin.apply_preset(index as usize)),
                _ => 0,
            }
        }

        #[no_mangle]
        pub extern "C" fn op_preset_index(plugin: *const $crate::FfiInstance) -> i32 {
            unsafe { plugin.as_ref() }
                .map(|plugin| plugin.preset_index())
                .unwrap_or(-1)
        }

        #[no_mangle]
        pub extern "C" fn op_process_stereo(
            plugin: *mut $crate::FfiInstance,
            in_l: *const f32,
            in_r: *const f32,
            out_l: *mut f32,
            out_r: *mut f32,
            frames: u32,
        ) -> i32 {
            match unsafe { plugin.as_ref() } {
                Some(plugin) => {
                    i32::from(plugin.process_stereo_raw(in_l, in_r, out_l, out_r, frames as usize))
                }
                None => 0,
            }
        }

        #[no_mangle]
        pub extern "C" fn op_process_planar(
            plugin: *mut $crate::FfiInstance,
            input: *const *const f32,
            output: *mut *mut f32,
            channels: u32,
            frames: u32,
        ) -> i32 {
            match unsafe { plugin.as_ref() } {
                Some(plugin) => i32::from(plugin.process_planar_raw(
                    input,
                    output,
                    channels as usize,
                    frames as usize,
                )),
                None => 0,
            }
        }

        #[no_mangle]
        pub extern "C" fn op_process_interleaved(
            plugin: *mut $crate::FfiInstance,
            input: *const f32,
            output: *mut f32,
            channels: u32,
            frames: u32,
        ) -> i32 {
            match unsafe { plugin.as_ref() } {
                Some(plugin) => i32::from(plugin.process_interleaved_raw(
                    input,
                    output,
                    channels as usize,
                    frames as usize,
                )),
                None => 0,
            }
        }

        #[no_mangle]
        pub extern "C" fn op_meters(
            plugin: *const $crate::FfiInstance,
            out: *mut $crate::OpMeters,
        ) {
            if out.is_null() {
                return;
            }
            let frame = unsafe { plugin.as_ref() }
                .map(|plugin| plugin.meters())
                .unwrap_or($crate::OpMeters::SILENT);
            unsafe { *out = frame };
        }

        #[no_mangle]
        pub extern "C" fn op_save(plugin: *mut $crate::FfiInstance, buf: *mut u8, cap: u32) -> i32 {
            let Some(plugin) = (unsafe { plugin.as_ref() }) else {
                return -1;
            };
            let bytes = plugin.save_bytes();
            if buf.is_null() || (cap as usize) < bytes.len() {
                return bytes.len() as i32;
            }
            unsafe { ::std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len()) };
            bytes.len() as i32
        }

        #[no_mangle]
        pub extern "C" fn op_load(
            plugin: *mut $crate::FfiInstance,
            buf: *const u8,
            len: u32,
        ) -> i32 {
            if buf.is_null() {
                return 0;
            }
            match unsafe { plugin.as_ref() } {
                Some(plugin) => {
                    let bytes = unsafe { ::std::slice::from_raw_parts(buf, len as usize) };
                    i32::from(plugin.load_bytes(bytes))
                }
                None => 0,
            }
        }
    };
}

pub fn plugin_name() -> &'static str {
    plugin_info().map(|info| info.name).unwrap_or("")
}

pub fn preset_count() -> i32 {
    preset_table().len() as i32
}

pub fn preset_name(index: i32) -> &'static str {
    preset_table()
        .get(index as usize)
        .map(|entry| entry.name)
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use openplugin_core::{Category, Unit};

    struct Gain {
        db: f32,
        linear: f32,
        offline: bool,
    }

    static PARAMS: [ParamInfo; 2] = [
        ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
        ParamInfo::new("mode", "Mode", 0.0, 2.0, 0.0, Unit::None).with_steps(3),
    ];
    static PRESET_FLAT: [f32; 2] = [0.0, 0.0];
    static PRESET_HOT: [f32; 2] = [6.0, 2.0];

    static INFO: PluginInfo = PluginInfo {
        id: "com.example.ffi-gain",
        name: "FFI Gain",
        vendor: "Test",
        url: "https://example.com",
        email: "a@b.c",
        version: "0",
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
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            for channel in io {
                for sample in &mut channel[..frames] {
                    *sample *= self.linear;
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

    fn gain() -> Box<dyn Plugin> {
        Box::new(Gain {
            db: 0.0,
            linear: 1.0,
            offline: false,
        })
    }

    #[test]
    fn spec_reports_steps_and_log_for_the_host() {
        set_factory(gain);
        assert_eq!(param_count(), 2);
        let mut spec = spec_from(&PARAMS[0]);
        assert!(param_info(0, &mut spec));
        assert_eq!(spec.is_log, 0);
        assert_eq!(spec.unit, 1);
        assert!(param_info(1, &mut spec));
        assert_eq!(spec.steps, 3);
        assert_eq!(spec.is_discrete, 1);
        let name = std::str::from_utf8(&spec.name).unwrap();
        assert!(name.starts_with("Mode"));
        assert_eq!(plugin_name(), "FFI Gain");
        assert_eq!(preset_count(), 2);
        assert_eq!(preset_name(1), "Hot");
    }

    #[test]
    fn inplace_stereo_uses_the_plane_and_rejects_an_oversized_block() {
        let instance = FfiInstance::new(gain(), 48_000.0, 8, 2);
        assert!(instance.store_param(0, 6.0));
        assert!((instance.load_param(0) - 6.0).abs() < 1e-6);
        let mut left = [0.5f32; 4];
        let mut right = [0.25f32; 4];
        assert!(instance.process_stereo_raw(
            left.as_ptr(),
            right.as_ptr(),
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            4
        ));
        let expected = 0.5 * 10f32.powf(6.0 / 20.0);
        assert!((left[0] - expected).abs() < 1e-4);
        assert!((right[0] - expected * 0.5).abs() < 1e-4);
        assert!(!instance.process_stereo_raw(
            left.as_ptr(),
            right.as_ptr(),
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            9
        ));
        let mut interleaved = [0.5f32, -0.5, f32::NAN, 0.0];
        assert!(instance.process_interleaved_raw(
            interleaved.as_ptr(),
            interleaved.as_mut_ptr(),
            2,
            2
        ));
        assert!(interleaved.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn preset_and_state_round_trip_without_touching_audio_early() {
        set_factory(gain);
        let instance = FfiInstance::new(gain(), 48_000.0, 16, 2);
        assert!(instance.apply_preset(1));
        assert_eq!(instance.preset_index(), 1);
        assert!((instance.load_param(0) - 6.0).abs() < 1e-4);
        assert!((instance.load_param(1) - 2.0).abs() < 1e-4);
        let bytes = instance.save_bytes();
        assert!(instance.store_param(0, -3.0));
        assert!(instance.load_bytes(&bytes));
        assert!((instance.load_param(0) - 6.0).abs() < 1e-4);
        instance.set_offline_flag(true);
        let mut left = [0.2f32; 4];
        let mut right = [0.2f32; 4];
        assert!(instance.process_stereo_raw(
            left.as_ptr(),
            right.as_ptr(),
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            4
        ));
        let expected = 0.2 * 10f32.powf(6.0 / 20.0);
        assert!((left[3] - expected).abs() < 1e-4);
    }
}
