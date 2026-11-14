//! Parameter-only state: a plugin that answers
//! `Plugin::state_is_parameters_only` with `true` is saved and loaded
//! through the parameter plane, so a concurrent `process` never skips a
//! block. A plugin with a chunk keeps the gated path and keeps its chunk.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_ffi::FfiInstance;

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, -6.0, Unit::Db),
    ParamInfo::new("mix", "Mix", 0.0, 1.0, 1.0, Unit::None),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.example.param-only",
    name: "Param Only",
    vendor: "Test",
    url: "https://example.com",
    email: "a@b.c",
    version: "0.1.0",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

/// Gain and mix. With `chunk` set it also owns a tag the parameters do not
/// hold, saved through `save_state`.
struct Tone {
    gain_db: f32,
    mix: f32,
    chunk: bool,
    tag: u32,
}

impl Tone {
    fn params_only() -> Self {
        Self { gain_db: -6.0, mix: 1.0, chunk: false, tag: 0 }
    }
    fn with_chunk(tag: u32) -> Self {
        Self { gain_db: -6.0, mix: 1.0, chunk: true, tag }
    }
}

impl Plugin for Tone {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "gain" => self.gain_db = PARAMS[0].clamp(value),
            "mix" => self.mix = PARAMS[1].clamp(value),
            _ => return false,
        }
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "gain" => Some(self.gain_db),
            "mix" => Some(self.mix),
            _ => None,
        }
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let gain = self.mix * 10f32.powf(self.gain_db / 20.0) + (1.0 - self.mix);
        for channel in io {
            for sample in &mut channel[..frames] {
                *sample *= gain;
            }
        }
    }
    fn state_is_parameters_only(&self) -> bool {
        !self.chunk
    }
    fn save_state(&mut self) -> Option<Vec<u8>> {
        self.chunk.then(|| self.tag.to_le_bytes().to_vec())
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        if !self.chunk {
            return false;
        }
        match <[u8; 4]>::try_from(data) {
            Ok(bytes) => {
                self.tag = u32::from_le_bytes(bytes);
                true
            }
            Err(_) => false,
        }
    }
}

const BLOCK: usize = 64;
const BLOCKS: usize = 4_000;

fn input(block: usize) -> ([f32; BLOCK], [f32; BLOCK]) {
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    for i in 0..BLOCK {
        let t = (block * BLOCK + i) as f32;
        left[i] = (t * 0.013).sin() * 0.8;
        right[i] = (t * 0.021).cos() * 0.6;
    }
    (left, right)
}

fn render(instance: &FfiInstance) -> Vec<u32> {
    let mut out = Vec::with_capacity(BLOCKS * BLOCK * 2);
    for block in 0..BLOCKS {
        let (mut left, mut right) = input(block);
        assert!(instance.process_stereo(&mut left, &mut right));
        out.extend(left.iter().chain(right.iter()).map(|s| s.to_bits()));
    }
    out
}

fn fresh(plugin: Tone) -> FfiInstance {
    FfiInstance::new(Box::new(plugin), 48_000.0, BLOCK, 2)
}

#[test]
fn saves_and_loads_during_playback_never_skip_a_block() {
    let reference = render(&fresh(Tone::params_only()));
    // The reference is processed audio, not the dry input.
    let (dry_left, _) = input(0);
    assert_ne!(f32::from_bits(reference[3]), dry_left[3]);

    let instance = fresh(Tone::params_only());
    let stop = AtomicBool::new(false);
    let (rendered, saves) = thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let mut count = 0u64;
            while !stop.load(Ordering::Acquire) {
                let bytes = instance.save_bytes();
                assert!(instance.load_bytes(&bytes));
                count += 1;
            }
            count
        });
        let rendered = render(&instance);
        stop.store(true, Ordering::Release);
        (rendered, worker.join().unwrap())
    });
    assert!(saves > 0, "the other thread never saved");
    assert_eq!(rendered, reference, "a block differed from an untouched run");
}

#[test]
fn parameter_only_round_trip_applies_on_the_next_block() {
    let a = fresh(Tone::params_only());
    assert!(a.store_param(0, 9.5));
    assert!(a.store_param(1, 0.25));
    let bytes = a.save_bytes();

    let b = fresh(Tone::params_only());
    assert!(b.load_bytes(&bytes));
    assert_eq!(b.load_param(0), 9.5);
    assert_eq!(b.load_param(1), 0.25);
    let (mut l1, mut r1) = input(3);
    let (mut l2, mut r2) = input(3);
    assert!(a.process_stereo(&mut l1, &mut r1));
    assert!(b.process_stereo(&mut l2, &mut r2));
    assert_eq!(l1.map(f32::to_bits), l2.map(f32::to_bits));
    assert_eq!(r1.map(f32::to_bits), r2.map(f32::to_bits));
    assert_eq!(b.save_bytes(), bytes);
}

#[test]
fn chunk_plugin_keeps_its_chunk() {
    let a = fresh(Tone::with_chunk(0x00C0_FFEE));
    assert!(a.store_param(0, -12.0));
    let bytes = a.save_bytes();

    let b = fresh(Tone::with_chunk(1));
    assert!(b.load_bytes(&bytes));
    assert_eq!(b.load_param(0), -12.0);
    assert_eq!(b.save_bytes(), bytes, "the chunk did not survive the round trip");

    // A parameter-only build ignores the chunk, keeps the values, and its
    // own save carries no chunk.
    let c = fresh(Tone::params_only());
    assert!(c.load_bytes(&bytes));
    assert_eq!(c.load_param(0), -12.0);
    assert!(c.save_bytes().len() < bytes.len());

    // A chunk the plugin refuses stops the load before any value changes.
    let mut bad = bytes.clone();
    bad.pop();
    let d = fresh(Tone::with_chunk(1));
    let before = d.save_bytes();
    assert!(!d.load_bytes(&bad));
    assert_eq!(d.save_bytes(), before);
}

#[test]
fn chunk_plugin_state_calls_during_playback_stay_sound() {
    // The gated path may pass a block through dry, but must never corrupt
    // audio or the chunk, and every call returns.
    let instance = fresh(Tone::with_chunk(7));
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        let worker = scope.spawn(|| {
            while !stop.load(Ordering::Acquire) {
                let bytes = instance.save_bytes();
                assert!(instance.load_bytes(&bytes));
            }
        });
        for out in render(&instance) {
            assert!(f32::from_bits(out).is_finite());
        }
        stop.store(true, Ordering::Release);
        worker.join().unwrap();
    });
    assert_eq!(fresh(Tone::with_chunk(7)).save_bytes(), instance.save_bytes());
}

/// Small deterministic generator; no dependency.
fn next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

fn hostile(make: fn() -> Tone) {
    let instance = fresh(make());
    assert!(instance.store_param(0, 3.0));
    let valid = instance.save_bytes();
    let snapshot = |i: &FfiInstance| (i.load_param(0).to_bits(), i.load_param(1).to_bits());
    let before = snapshot(&instance);

    assert!(!instance.load_bytes(&[]));
    for len in 0..valid.len() {
        assert!(!instance.load_bytes(&valid[..len]), "truncation to {len} accepted");
        assert_eq!(snapshot(&instance), before);
    }
    let mut extended = valid.clone();
    extended.push(0);
    assert!(!instance.load_bytes(&extended));
    assert_eq!(snapshot(&instance), before);

    let mut seed = 0x5eed_u64;
    for round in 0..4_000 {
        let mut bytes = valid.clone();
        for _ in 0..1 + next(&mut seed) % 4 {
            let at = (next(&mut seed) as usize) % bytes.len();
            bytes[at] = next(&mut seed) as u8;
        }
        if round % 5 == 0 {
            let len = next(&mut seed) % 64;
            bytes = (0..len).map(|_| next(&mut seed) as u8).collect();
        }
        let before = snapshot(&instance);
        if instance.load_bytes(&bytes) {
            for (index, spec) in PARAMS.iter().enumerate() {
                let value = instance.load_param(index);
                assert!(value.is_finite(), "a loaded value is not finite");
                assert!(value >= spec.min && value <= spec.max, "a loaded value is out of range");
            }
        } else {
            assert_eq!(snapshot(&instance), before, "a refused load changed a value");
        }
    }
}

#[test]
fn hostile_input_is_refused_for_both_kinds() {
    hostile(Tone::params_only);
    hostile(|| Tone::with_chunk(5));
}
