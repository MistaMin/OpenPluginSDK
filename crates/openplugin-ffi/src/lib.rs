//! Rust host session for one [`Plugin`].
//!
//! A host builds its parameter list from [`ParamSpec`], writes the plane with
//! [`FfiInstance::store_param`], and renders with [`FfiInstance::process_stereo`],
//! [`FfiInstance::process_planar`], or [`FfiInstance::process_interleaved`].
//! Those calls copy through scratch allocated in [`FfiInstance::prepare`], so
//! the same slices may be the input and the output.
//!
//! `store_param`, `load_param`, `apply_preset`, and `meters` do not enter the
//! plugin object and do not allocate. The audio thread applies the plane
//! inside `process_*`. `prepare` and `reset` stay exclusive with `process`.
#![allow(unsafe_code)]

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use openplugin_core::{
    decode_params, encode_params, MeterFrame, ParamCursor, ParamInfo, ParamPlane, PluginInfo,
    Seqlock, Taper, Unit,
};

pub use openplugin_core::Plugin;

/// One parameter, the way a host builds its control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default_value: f32,
    pub is_discrete: bool,
    pub steps: u32,
    pub is_log: bool,
    pub unit: Unit,
}

struct PresetEntry {
    name: &'static str,
    values: &'static [f32],
}

/// Steps, discrete, and log skew for one descriptor.
pub fn spec_from(param: &ParamInfo) -> ParamSpec {
    let steps = if param.steps > 1 {
        param.steps
    } else if param.unit == Unit::Choice && !param.choices.is_empty() {
        param.choices.len() as u32
    } else if param.unit == Unit::Toggle {
        2
    } else {
        0
    };
    ParamSpec {
        id: param.id,
        name: param.name,
        min: param.min,
        max: param.max,
        default_value: param.default,
        is_discrete: steps > 1,
        steps,
        is_log: param.taper == Taper::Log,
        unit: param.unit,
    }
}

fn collect_presets(plugin: &dyn Plugin) -> Vec<PresetEntry> {
    (0..plugin.preset_count())
        .filter_map(|index| {
            plugin.preset_values(index).map(|values| PresetEntry {
                name: plugin.preset_name(index),
                values,
            })
        })
        .collect()
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
    presets: Vec<PresetEntry>,
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
        let presets = collect_presets(plugin.as_ref());
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
            presets,
        }
    }

    pub fn name(&self) -> &'static str {
        self.info.name
    }

    pub fn param_count(&self) -> usize {
        self.info.params.len()
    }

    pub fn param_spec(&self, index: usize) -> Option<ParamSpec> {
        self.info.params.get(index).map(spec_from)
    }

    pub fn preset_count(&self) -> usize {
        self.presets.len()
    }

    pub fn preset_name(&self, index: usize) -> &'static str {
        self.presets
            .get(index)
            .map(|entry| entry.name)
            .unwrap_or("")
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

    pub fn meters(&self) -> MeterFrame {
        self.meters.read()
    }

    pub fn preset_index(&self) -> i32 {
        self.preset.load(Ordering::Acquire)
    }

    /// Publish a factory preset onto the plane. The audio thread applies it.
    pub fn apply_preset(&self, index: usize) -> bool {
        let Some(entry) = self.presets.get(index) else {
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

    /// In-place stereo. Copies through scratch, so `left` and `right` are both the input and the output.
    pub fn process_stereo(&self, left: &mut [f32], right: &mut [f32]) -> bool {
        if left.len() != right.len() {
            return false;
        }
        let frames = left.len();
        let audio = unsafe { &mut *self.audio.get() };
        if frames > audio.max_block || audio.channels == 0 {
            return false;
        }
        if frames == 0 {
            return true;
        }
        copy_in(left, &mut audio.buffers[0][..frames]);
        if audio.channels > 1 {
            copy_in(right, &mut audio.buffers[1][..frames]);
        }
        let width = audio.channels.min(2);
        self.render(audio, width, frames);
        left.copy_from_slice(&audio.buffers[0][..frames]);
        let right_index = if audio.channels > 1 { 1 } else { 0 };
        right.copy_from_slice(&audio.buffers[right_index][..frames]);
        true
    }

    /// In-place planar audio. Each channel slice is the same length.
    pub fn process_planar(&self, channels: &mut [&mut [f32]]) -> bool {
        if channels.is_empty()
            || channels
                .iter()
                .any(|channel| channel.len() != channels[0].len())
        {
            return false;
        }
        let frames = channels[0].len();
        let count = channels.len();
        let audio = unsafe { &mut *self.audio.get() };
        if count > audio.channels || frames > audio.max_block {
            return false;
        }
        if frames == 0 {
            return true;
        }
        for (index, channel) in channels.iter().enumerate() {
            copy_in(channel, &mut audio.buffers[index][..frames]);
        }
        self.render(audio, count, frames);
        for (index, channel) in channels.iter_mut().enumerate() {
            channel.copy_from_slice(&audio.buffers[index][..frames]);
        }
        true
    }

    /// In-place interleaved audio, frame-major. `samples.len()` is `channels * frames`.
    pub fn process_interleaved(&self, samples: &mut [f32], channels: usize) -> bool {
        if channels == 0 || samples.len() % channels != 0 {
            return false;
        }
        let frames = samples.len() / channels;
        let audio = unsafe { &mut *self.audio.get() };
        if channels > audio.channels || frames > audio.max_block {
            return false;
        }
        if frames == 0 {
            return true;
        }
        for frame in 0..frames {
            for channel in 0..channels {
                let sample = samples[frame * channels + channel];
                audio.buffers[channel][frame] = if sample.is_finite() { sample } else { 0.0 };
            }
        }
        self.render(audio, channels, frames);
        for frame in 0..frames {
            for channel in 0..channels {
                samples[frame * channels + channel] = audio.buffers[channel][frame];
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
        let instance = FfiInstance::new(gain(), 48_000.0, 8, 2);
        assert_eq!(instance.param_count(), 2);
        let spec = instance.param_spec(0).unwrap();
        assert!(!spec.is_log);
        assert_eq!(spec.unit, Unit::Db);
        let spec = instance.param_spec(1).unwrap();
        assert_eq!(spec.steps, 3);
        assert!(spec.is_discrete);
        assert!(spec.name.starts_with("Mode"));
        assert_eq!(instance.name(), "FFI Gain");
        assert_eq!(instance.preset_count(), 2);
        assert_eq!(instance.preset_name(1), "Hot");
    }

    #[test]
    fn inplace_stereo_uses_the_plane_and_rejects_an_oversized_block() {
        let instance = FfiInstance::new(gain(), 48_000.0, 8, 2);
        assert!(instance.store_param(0, 6.0));
        assert!((instance.load_param(0) - 6.0).abs() < 1e-6);
        let mut left = [0.5f32; 4];
        let mut right = [0.25f32; 4];
        assert!(instance.process_stereo(&mut left, &mut right));
        let expected = 0.5 * 10f32.powf(6.0 / 20.0);
        assert!((left[0] - expected).abs() < 1e-4);
        assert!((right[0] - expected * 0.5).abs() < 1e-4);
        let mut long_left = [0.5f32; 9];
        let mut long_right = [0.25f32; 9];
        assert!(!instance.process_stereo(&mut long_left, &mut long_right));
        let mut interleaved = [0.5f32, -0.5, f32::NAN, 0.0];
        assert!(instance.process_interleaved(&mut interleaved, 2));
        assert!(interleaved.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn preset_and_state_round_trip_without_touching_audio_early() {
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
        assert!(instance.process_stereo(&mut left, &mut right));
        let expected = 0.2 * 10f32.powf(6.0 / 20.0);
        assert!((left[3] - expected).abs() < 1e-4);
    }
}
