//! Hostile input fuzzing for the `op_shell_*` C ABI.
//!
//! A deterministic LCG (the fixed seed is [`SEED`]) drives hundreds of boundary
//! `create` calls and thousands of generated parameter and process calls into
//! `openplugin_ffi::shell`, with garbage sample rates, block sizes, channel
//! counts, parameter indices and values, and audio buffers full of NaN,
//! infinities, denormals and arbitrary bit patterns. The test binary dying is
//! the failure signal: a panic, an abort or a null dereference fails the run.
//!
//! The assertions that go beyond "it survived" are the documented contract:
//!
//! - `create` accepts 1 kHz..=768 kHz, 1..=65_536 frames and 1..=2 channels and
//!   returns null outside those ranges.
//! - `process` returns 0 for a null mandatory pointer or a block larger than the
//!   prepared one, and non-zero for every block it can run. Zero frames is a
//!   successful no-op, even with null pointers (the empty block returns before
//!   the pointers are read).
//! - `set_plain` rejects a non-finite value or a missing parameter with 0 and
//!   leaves the plane untouched. `set_normalized` rejects only a missing
//!   parameter: non-finite travel falls back to the parameter default.
//! - `get_plain`/`get_normalized` return 0 for a null instance or a missing
//!   parameter and stay inside the descriptor range otherwise.
//! - `latency`/`tail_seconds` are 0 for a null instance; `write_info` and
//!   `write_param` are 0 for a null output or a missing parameter.
//!
//! Seed and iteration counts are fixed, so a failure reproduces exactly.

use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Tail, Unit};
use openplugin_ffi::shell::{self, OpParamDesc, OpShellInfo, ShellInstance, ABI, PARAM_DISCRETE};

fn write_info(out: *mut OpShellInfo) -> u32 {
    unsafe { shell::write_info::<FuzzGain>(out) }
}

fn write_param(index: u32, out: *mut OpParamDesc) -> u32 {
    unsafe { shell::write_param::<FuzzGain>(index, out) }
}

/// Fixed fuzz seed. Do not randomise it: every generated case must reproduce.
const SEED: u64 = 0x5EED_0F2A_1700_0103;

/// Generated calls per fuzz test. A few thousand reaches every branch quickly.
const PARAM_ITERATIONS: u32 = 3_000;
const PROCESS_ITERATIONS: u32 = 3_000;

/// Descriptor order. `PARAMS` below decides it; the literals keep loops cheap.
const GAIN: u32 = 0;
const MUTE: u32 = 1;
const PARAM_COUNT: u32 = 2;

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
    ParamInfo::new("mute", "Mute", 0.0, 1.0, 0.0, Unit::Toggle),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.test.shell-fuzz",
    name: "Shell Fuzz",
    vendor: "OpenGrid",
    url: "",
    email: "",
    version: "0.3.0",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

/// Two parameters and two states: a gain path and a mute switch. `tail` is one
/// second at every accepted rate, so the host property can be checked before
/// and after hostile blocks without a per-rate expectation.
struct FuzzGain {
    db: f32,
    linear: f32,
    mute: bool,
    rate: f32,
}

impl Default for FuzzGain {
    fn default() -> Self {
        Self {
            db: 0.0,
            linear: 1.0,
            mute: false,
            rate: 48_000.0,
        }
    }
}

impl Plugin for FuzzGain {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.rate = sample_rate;
        self.linear = 10f32.powf(self.db / 20.0);
    }

    fn reset(&mut self) {}

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "gain" => {
                self.db = PARAMS[GAIN as usize].clamp(value);
                self.linear = 10f32.powf(self.db / 20.0);
                true
            }
            "mute" => {
                self.mute = PARAMS[MUTE as usize].clamp(value) > 0.5;
                true
            }
            _ => false,
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "gain" => Some(self.db),
            "mute" => Some(f32::from(u8::from(self.mute))),
            _ => None,
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for channel in io {
            let samples = &mut channel[..frames];
            if self.mute {
                samples.fill(0.0);
            } else {
                for sample in samples {
                    *sample *= self.linear;
                }
            }
        }
    }

    /// One second of tail. `rate` is exact for every accepted rate (all are
    /// integers below 2^24), so this is 1.0 s at 8 kHz through 768 kHz.
    fn tail(&self) -> Tail {
        Tail::Finite(self.rate as u32)
    }

    fn latency(&self) -> usize {
        7
    }
}

/// PCG's 64-bit LCG step; the high half is the output. Good enough for fuzzing
/// and, unlike `rand`, it needs no dependency and cannot change under us.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 32) as u32
    }

    fn below(&mut self, n: u32) -> u32 {
        self.next_u32() % n
    }

    fn chance(&mut self, one_in: u32) -> bool {
        self.below(one_in) == 0
    }

    /// Mostly the parameter boundary indices the ABI promises to handle.
    fn index(&mut self) -> u32 {
        match self.below(8) {
            0 => 0,
            1 => PARAM_COUNT - 1,
            2 => PARAM_COUNT,
            3 => u32::MAX,
            4 => self.below(4),
            _ => self.next_u32(),
        }
    }
}

/// Garbage f32 for plain and normalized parameter writes: NaN, infinities,
/// huge, negative, subnormal and arbitrary bit patterns.
fn junk_value(rng: &mut Lcg) -> f32 {
    match rng.below(12) {
        0 => f32::NAN,
        1 => f32::INFINITY,
        2 => f32::NEG_INFINITY,
        3 => 0.0,
        4 => -0.0,
        5 => 1.0,
        6 => -1.0,
        7 => f32::MAX,
        8 => f32::MIN,
        9 => f32::MIN_POSITIVE,
        10 => 7.5,
        _ => f32::from_bits(rng.next_u32()),
    }
}

/// Frame counts near and past the prepared block. `max_block` is at most
/// 65_536 here, so `max_block + 1` cannot overflow.
fn junk_frames(rng: &mut Lcg, max_block: u32) -> u32 {
    match rng.below(9) {
        0 => 0,
        1 => 1,
        2 => max_block,
        3 => max_block + 1,
        4 => rng.below(max_block + 2),
        5 => rng.below(8).min(max_block + 1),
        6 => max_block / 2,
        7 => max_block.saturating_sub(1),
        _ => rng.below(2),
    }
}

/// Fill `dst[..frames]` with one hostile audio pattern.
fn fill(rng: &mut Lcg, dst: &mut [f32], frames: usize, pattern: u32) {
    let slice = &mut dst[..frames];
    match pattern {
        0 => {
            for sample in slice.iter_mut() {
                *sample = f32::from_bits(rng.next_u32());
            }
        }
        1 => slice.fill(f32::NAN),
        2 => {
            for (index, sample) in slice.iter_mut().enumerate() {
                *sample = if index % 2 == 0 {
                    f32::INFINITY
                } else {
                    f32::NEG_INFINITY
                };
            }
        }
        3 => {
            // Subnormals: the values a denormal guard must not choke on.
            for (index, sample) in slice.iter_mut().enumerate() {
                let denormal = f32::from_bits(1 + (index as u32 % 7));
                *sample = if index % 2 == 0 { denormal } else { -denormal };
            }
        }
        4 => {
            for sample in slice.iter_mut() {
                let unit = (rng.next_u32() >> 8) as f32 / (u32::MAX >> 8) as f32;
                *sample = unit * 2.0 - 1.0;
            }
        }
        _ => slice.fill(0.0),
    }
}

fn approx(actual: f32, expected: f32) -> bool {
    actual == expected || (actual - expected).abs() <= 1e-5 * expected.abs().max(1.0)
}

fn assert_desc(index: u32, desc: &OpParamDesc) {
    let param = &PARAMS[index as usize];
    assert_eq!(desc.min, f64::from(param.min), "param {index} min");
    assert_eq!(desc.max, f64::from(param.max), "param {index} max");
    assert_eq!(
        desc.default_plain,
        f64::from(param.default),
        "param {index} default"
    );
    assert!(
        desc.name.starts_with(param.name.as_bytes()),
        "param {index} name"
    );
    assert!(desc.id.starts_with(param.id.as_bytes()), "param {index} id");
    if param.unit == Unit::Toggle {
        assert_ne!(desc.flags & PARAM_DISCRETE, 0, "toggle {index} is discrete");
        assert_eq!(desc.steps, 2, "toggle {index} steps");
    } else {
        assert_eq!(
            desc.flags & PARAM_DISCRETE,
            0,
            "continuous {index} is not discrete"
        );
    }
}

/// One live instance with scratch buffers of the largest legal size plus one
/// frame, so the rejected `max_block + 1` calls still address `frames` floats
/// as `process`'s safety contract requires.
struct Rig {
    ptr: *mut ShellInstance,
    max_block: u32,
    channels: u32,
    left: Vec<f32>,
    right: Vec<f32>,
    out_left: Vec<f32>,
    out_right: Vec<f32>,
}

impl Rig {
    fn new(rate: f64, max_block: u32, channels: u32) -> Self {
        let ptr = shell::create::<FuzzGain>(rate, max_block, channels);
        assert!(
            !ptr.is_null(),
            "create({rate}, {max_block}, {channels}) was rejected"
        );
        let capacity = max_block as usize + 1;
        Self {
            ptr,
            max_block,
            channels,
            left: vec![0.0; capacity],
            right: vec![0.0; capacity],
            out_left: vec![0.0; capacity],
            out_right: vec![0.0; capacity],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Separate,
    InPlace,
    Crossed,
    MonoRight,
    NullOutRight,
    NullInLeft,
    NullOutLeft,
    HugeNull,
}

impl Kind {
    const COUNT: u32 = 8;

    fn from(value: u32) -> Self {
        match value % Self::COUNT {
            0 => Self::Separate,
            1 => Self::InPlace,
            2 => Self::Crossed,
            3 => Self::MonoRight,
            4 => Self::NullOutRight,
            5 => Self::NullInLeft,
            6 => Self::NullOutLeft,
            _ => Self::HugeNull,
        }
    }
}

/// The return value `process` documents for one call: every call with a live
/// instance and legal pointers succeeds unless the block is oversized, a
/// mandatory pointer is null, or the block is empty (which succeeds before the
/// null check).
fn expected_result(kind: Kind, frames: u32, max_block: u32) -> u32 {
    match kind {
        Kind::NullInLeft | Kind::NullOutLeft => u32::from(frames == 0),
        Kind::HugeNull => 0,
        _ => u32::from(frames == 0 || frames <= max_block),
    }
}

fn call(rig: &mut Rig, kind: Kind, frames: u32) -> u32 {
    let ptr = rig.ptr;
    let left_in = rig.left.as_ptr();
    let right_in = rig.right.as_ptr();
    let out_left = rig.out_left.as_mut_ptr();
    let out_right = rig.out_right.as_mut_ptr();
    let inplace_left = rig.left.as_mut_ptr();
    let inplace_right = rig.right.as_mut_ptr();
    unsafe {
        match kind {
            Kind::Separate => shell::process(ptr, left_in, right_in, out_left, out_right, frames),
            Kind::InPlace => {
                shell::process(ptr, left_in, right_in, inplace_left, inplace_right, frames)
            }
            Kind::Crossed => {
                shell::process(ptr, left_in, right_in, inplace_right, inplace_left, frames)
            }
            Kind::MonoRight => {
                shell::process(ptr, left_in, std::ptr::null(), out_left, out_right, frames)
            }
            Kind::NullOutRight => shell::process(
                ptr,
                left_in,
                right_in,
                out_left,
                std::ptr::null_mut(),
                frames,
            ),
            Kind::NullInLeft => {
                shell::process(ptr, std::ptr::null(), right_in, out_left, out_right, frames)
            }
            Kind::NullOutLeft => shell::process(
                ptr,
                left_in,
                right_in,
                std::ptr::null_mut(),
                out_right,
                frames,
            ),
            Kind::HugeNull => shell::process(
                ptr,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                frames,
            ),
        }
    }
}

/// Check the buffers a successful call had to write. The gain is finite and
/// never zero and `copy_ptr`/`copy_in` replace non-finite input with 0, so a
/// block of NaN/infinities must come out silent and no finite input can make
/// the plugin emit NaN.
fn check_outputs(rig: &Rig, kind: Kind, frames: u32, pattern: u32) {
    let n = frames as usize;
    let non_finite_input = matches!(pattern, 1 | 2);
    let written: [&[f32]; 2] = match kind {
        Kind::Separate | Kind::MonoRight => [&rig.out_left[..n], &rig.out_right[..n]],
        Kind::InPlace | Kind::Crossed => [&rig.left[..n], &rig.right[..n]],
        Kind::NullOutRight => [&rig.out_left[..n], &[]],
        Kind::NullInLeft | Kind::NullOutLeft | Kind::HugeNull => [&[], &[]],
    };
    for slice in written {
        for (frame, &sample) in slice.iter().enumerate() {
            assert!(
                !sample.is_nan(),
                "kind {kind:?}: NaN output at frame {frame}"
            );
            if non_finite_input {
                assert_eq!(
                    sample, 0.0,
                    "kind {kind:?}: non-finite input leaked to frame {frame}"
                );
            }
        }
    }
}

/// A clean block after all the junk must still apply the plane: this is the
/// check a crash-only test cannot make.
fn clean_block_after_junk(rigs: &mut [Rig]) {
    for rig in rigs.iter_mut() {
        let frames = 32u32.min(rig.max_block).max(1);
        let n = frames as usize;
        let linear = 10f32.powf(6.0 / 20.0);
        unsafe {
            assert_eq!(shell::set_plain(rig.ptr, GAIN, 6.0), 1, "gain write");
            assert_eq!(shell::set_plain(rig.ptr, MUTE, 0.0), 1, "mute release");
            rig.left[..n].fill(0.25);
            rig.right[..n].fill(-0.125);
            assert_eq!(
                shell::process(
                    rig.ptr,
                    rig.left.as_ptr(),
                    rig.right.as_ptr(),
                    rig.out_left.as_mut_ptr(),
                    rig.out_right.as_mut_ptr(),
                    frames,
                ),
                1,
                "clean block after junk was rejected"
            );
            assert!(
                (rig.out_left[0] - 0.25 * linear).abs() < 1e-5,
                "left gain after junk: {}",
                rig.out_left[0]
            );
            // A mono engine copies its single channel into both outputs.
            let right = if rig.channels == 1 {
                0.25 * linear
            } else {
                -0.125 * linear
            };
            assert!(
                (rig.out_right[0] - right).abs() < 1e-5,
                "right gain after junk: {}",
                rig.out_right[0]
            );
            assert!((rig.out_left[n - 1] - 0.25 * linear).abs() < 1e-5);
        }
    }
}

#[test]
fn shell_create_accepts_only_the_documented_ranges() {
    const GOOD_RATES: [f64; 5] = [1_000.0, 7_999.0, 44_100.0, 48_000.0, 768_000.0];
    const BAD_RATES: [f64; 13] = [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        0.0,
        -0.0,
        -48_000.0,
        999.0,
        999.9,
        768_000.1,
        768_001.0,
        1.0e300,
        f64::MAX,
        f64::MIN_POSITIVE,
    ];
    const BLOCKS: [u32; 6] = [0, 1, 64, 65_536, 65_537, u32::MAX];
    const CHANNELS: [u32; 4] = [0, 1, 2, 3];

    for &rate in GOOD_RATES.iter().chain(BAD_RATES.iter()) {
        for &max_block in &BLOCKS {
            for &channels in &CHANNELS {
                let accepted = rate.is_finite()
                    && (1_000.0..=768_000.0).contains(&rate)
                    && (1..=65_536).contains(&max_block)
                    && (1..=2).contains(&channels);
                let ptr = shell::create::<FuzzGain>(rate, max_block, channels);
                if accepted {
                    assert!(
                        !ptr.is_null(),
                        "create({rate}, {max_block}, {channels}) was rejected"
                    );
                    unsafe {
                        assert_eq!(shell::latency(ptr), 7);
                        assert!((shell::tail_seconds(ptr) - 1.0).abs() < 1e-9);
                        shell::destroy(ptr);
                    }
                } else {
                    assert!(
                        ptr.is_null(),
                        "create({rate}, {max_block}, {channels}) was accepted"
                    );
                }
            }
        }
    }
}

#[test]
fn shell_null_pointers_are_rejected_calls() {
    let null = std::ptr::null_mut::<ShellInstance>();
    let null_const = std::ptr::null::<ShellInstance>();
    let mut desc = unsafe { std::mem::zeroed::<OpParamDesc>() };
    assert_eq!(
        write_param(GAIN, std::ptr::null_mut()),
        0
    );
    assert_eq!(write_param(GAIN, &mut desc), 1);
    assert_eq!(write_info(std::ptr::null_mut()), 0);
    unsafe {
        shell::destroy(null);
        shell::reset(null);
        assert_eq!(shell::latency(null_const), 0);
        assert_eq!(shell::tail_seconds(null_const), 0.0);
        assert_eq!(shell::set_plain(null, GAIN, 1.0), 0);
        assert_eq!(shell::set_normalized(null, GAIN, 0.5), 0);
        assert_eq!(shell::get_plain(null_const, GAIN), 0.0);
        assert_eq!(shell::get_normalized(null_const, GAIN), 0.0);
        assert_eq!(
            shell::process(
                null,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1,
            ),
            0
        );
        // A null *instance* is rejected before the empty-block shortcut.
        assert_eq!(
            shell::process(
                null,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            ),
            0
        );
    }
}

#[test]
fn shell_parameter_surface_survives_generated_junk() {
    // One plugin type per test binary keeps the shell's cached descriptor and
    // live-instance table unambiguous.
    assert_eq!(PARAMS.len() as u32, PARAM_COUNT);
    let mut rng = Lcg::new(SEED ^ 0x5041_5241); // "PARA"
    let instances = [
        shell::create::<FuzzGain>(48_000.0, 512, 2),
        shell::create::<FuzzGain>(8_000.0, 1, 1),
        shell::create::<FuzzGain>(192_000.0, 256, 2),
    ];
    for (slot, ptr) in instances.iter().enumerate() {
        assert!(!ptr.is_null(), "valid instance {slot} was rejected");
    }
    for _ in 0..PARAM_ITERATIONS {
        let ptr = instances[rng.below(instances.len() as u32) as usize];
        let index = rng.index();
        let valid = index < PARAM_COUNT;
        match rng.below(6) {
            0 => {
                let value = junk_value(&mut rng);
                // Documented: a finite value on an existing parameter is
                // accepted; a non-finite value or a missing parameter is
                // rejected with 0 and must leave the plane alone.
                let accepted = u32::from(valid && value.is_finite());
                let before = unsafe { shell::get_plain(ptr, index) };
                assert_eq!(
                    unsafe { shell::set_plain(ptr, index, value) },
                    accepted,
                    "set_plain({index}, {value})"
                );
                let stored = unsafe { shell::get_plain(ptr, index) };
                if accepted == 0 {
                    assert_eq!(
                        stored.to_bits(),
                        before.to_bits(),
                        "rejected set_plain changed the plane"
                    );
                } else {
                    let expected = PARAMS[index as usize].clamp(value);
                    assert!(
                        approx(stored, expected),
                        "set_plain({index}, {value}) stored {stored}, expected {expected}"
                    );
                    let normalized = unsafe { shell::get_normalized(ptr, index) };
                    assert!(
                        (0.0..=1.0).contains(&normalized),
                        "normalized {normalized} outside 0..=1"
                    );
                }
            }
            1 => {
                let value = junk_value(&mut rng);
                // Documented: only a missing parameter is rejected. Non-finite
                // travel falls back to the default (FfiInstance::store_normalized).
                let accepted = u32::from(valid);
                assert_eq!(
                    unsafe { shell::set_normalized(ptr, index, value) },
                    accepted,
                    "set_normalized({index}, {value})"
                );
                if accepted == 1 {
                    let param = &PARAMS[index as usize];
                    let expected = if value.is_finite() {
                        param.from_normalized(value)
                    } else {
                        param.default
                    };
                    let stored = unsafe { shell::get_plain(ptr, index) };
                    assert!(
                        approx(stored, expected),
                        "set_normalized({index}, {value}) stored {stored}, expected {expected}"
                    );
                    let normalized = unsafe { shell::get_normalized(ptr, index) };
                    assert!(
                        (0.0..=1.0).contains(&normalized),
                        "normalized {normalized} outside 0..=1"
                    );
                }
            }
            2 => {
                let plain = unsafe { shell::get_plain(ptr, index) };
                if valid {
                    let param = &PARAMS[index as usize];
                    assert!(plain.is_finite(), "plain {plain} is not finite");
                    assert!(
                        plain >= param.min && plain <= param.max,
                        "plain {plain} outside {}..={}",
                        param.min,
                        param.max
                    );
                } else {
                    // Documented: `load_param` is `unwrap_or(0.0)`.
                    assert_eq!(plain, 0.0);
                }
            }
            3 => {
                let normalized = unsafe { shell::get_normalized(ptr, index) };
                if valid {
                    assert!(
                        normalized.is_finite(),
                        "normalized {normalized} is not finite"
                    );
                    assert!(
                        (0.0..=1.0).contains(&normalized),
                        "normalized {normalized} outside 0..=1"
                    );
                } else {
                    assert_eq!(normalized, 0.0);
                }
            }
            4 => {
                let mut desc = unsafe { std::mem::zeroed::<OpParamDesc>() };
                assert_eq!(
                    write_param(index, &mut desc),
                    u32::from(valid),
                    "write_param({index})"
                );
                if valid {
                    assert_desc(index, &desc);
                }
            }
            _ => {
                // Null outputs are rejected with 0 and never written.
                assert_eq!(
                    write_param(index, std::ptr::null_mut()),
                    0
                );
                assert_eq!(write_info(std::ptr::null_mut()), 0);
            }
        }
    }
    // Descriptors and the header are static data: junk values cannot change
    // the real parameter count.
    let mut info = unsafe { std::mem::zeroed::<OpShellInfo>() };
    assert_eq!(write_info(&mut info), 1);
    assert_eq!(info.param_count, PARAM_COUNT);
    assert_eq!(info.abi, ABI);
    for ptr in instances {
        unsafe {
            assert_eq!(shell::latency(ptr), 7);
            assert!((shell::tail_seconds(ptr) - 1.0).abs() < 1e-9);
            shell::destroy(ptr);
        }
    }
}

#[test]
fn shell_process_survives_generated_junk_and_keeps_the_gain_path() {
    let mut rng = Lcg::new(SEED ^ 0x5052_4F43); // "PROC"
    let mut rigs = vec![
        Rig::new(48_000.0, 512, 2),
        Rig::new(8_000.0, 1, 1),
        Rig::new(768_000.0, 64, 2),
        Rig::new(44_100.0, 7, 2),
    ];
    for _ in 0..PROCESS_ITERATIONS {
        let index = rng.below(rigs.len() as u32) as usize;
        let kind = Kind::from(rng.below(Kind::COUNT));
        let frames = match kind {
            Kind::HugeNull => u32::MAX,
            _ => junk_frames(&mut rng, rigs[index].max_block),
        };
        let pattern = rng.below(6);
        let rig = &mut rigs[index];
        let fill_frames = if kind == Kind::HugeNull {
            0
        } else {
            frames as usize
        };
        fill(&mut rng, &mut rig.left, fill_frames, pattern);
        fill(&mut rng, &mut rig.right, fill_frames, pattern);
        // `reset` at an arbitrary valid point must not disturb the next block.
        if rng.chance(8) {
            unsafe { shell::reset(rig.ptr) };
        }
        let got = call(rig, kind, frames);
        let want = expected_result(kind, frames, rig.max_block);
        assert_eq!(
            got, want,
            "kind {kind:?}, frames {frames}, max_block {}",
            rig.max_block
        );
        if got == 1 && frames > 0 {
            check_outputs(rig, kind, frames, pattern);
        }
        // Host properties are refreshed after every block, not corrupted by it.
        if rng.chance(16) {
            unsafe {
                assert_eq!(shell::latency(rig.ptr), 7, "latency after junk");
                assert!(
                    (shell::tail_seconds(rig.ptr) - 1.0).abs() < 1e-9,
                    "tail after junk"
                );
            }
        }
    }
    clean_block_after_junk(&mut rigs);
    for rig in &rigs {
        unsafe { shell::destroy(rig.ptr) };
    }
}

#[test]
fn shell_process_handles_the_block_boundaries() {
    // Smallest accepted block: zero frames is a no-op success, one frame runs,
    // two frames is rejected before the pointers are read.
    let small = shell::create::<FuzzGain>(8_000.0, 1, 1);
    assert!(!small.is_null());
    let mut mono = [0.25f32; 2];
    unsafe {
        assert_eq!(
            shell::process(
                small,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            ),
            1
        );
        assert_eq!(
            shell::process(
                small,
                mono.as_ptr(),
                std::ptr::null(),
                mono.as_mut_ptr(),
                std::ptr::null_mut(),
                1,
            ),
            1
        );
        assert!(
            (mono[0] - 0.25).abs() < 1e-6,
            "the default gain is 0 dB, got {}",
            mono[0]
        );
        assert_eq!(
            shell::process(
                small,
                mono.as_ptr(),
                mono.as_ptr(),
                mono.as_mut_ptr(),
                mono.as_mut_ptr(),
                2,
            ),
            0
        );
        shell::destroy(small);
    }

    // Largest accepted block, and one frame past it.
    let big = shell::create::<FuzzGain>(48_000.0, 65_536, 2);
    assert!(!big.is_null());
    let left = vec![0.25f32; 65_537];
    let right = vec![-0.125f32; 65_537];
    let mut out_left = vec![0.0f32; 65_537];
    let mut out_right = vec![0.0f32; 65_537];
    unsafe {
        assert_eq!(
            shell::process(
                big,
                left.as_ptr(),
                right.as_ptr(),
                out_left.as_mut_ptr(),
                out_right.as_mut_ptr(),
                65_536,
            ),
            1
        );
        assert!((out_left[65_535] - 0.25).abs() < 1e-6);
        assert!((out_right[65_535] + 0.125).abs() < 1e-6);
        // One frame over the prepared block is rejected; the buffers are
        // `frames` long under the # Safety contract, so the call is legal.
        assert_eq!(
            shell::process(
                big,
                left.as_ptr(),
                right.as_ptr(),
                out_left.as_mut_ptr(),
                out_right.as_mut_ptr(),
                65_537,
            ),
            0
        );
        // u32::MAX frames cannot have buffers that long here, so every pointer
        // is null: the call must still be rejected, never dereferenced.
        assert_eq!(
            shell::process(
                big,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                u32::MAX,
            ),
            0
        );
        assert_eq!(
            shell::process(
                big,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            ),
            1
        );
        shell::destroy(big);
    }
}

#[test]
fn shell_reset_keeps_the_parameter_plane() {
    // `reset` at valid points: the plane is not DSP state, so a clean block
    // afterwards must still apply the gain written before the resets.
    let mut rng = Lcg::new(SEED ^ 0x5245_5345); // "RESE"
    let ptr = shell::create::<FuzzGain>(48_000.0, 32, 2);
    assert!(!ptr.is_null());
    let mut left = vec![0.0f32; 33];
    let mut right = vec![0.0f32; 33];
    unsafe {
        assert_eq!(shell::set_plain(ptr, GAIN, 6.0), 1);
        assert_eq!(shell::set_plain(ptr, MUTE, 0.0), 1);
        // Before any block: latency and tail come from the created engine.
        assert_eq!(shell::latency(ptr), 7);
        assert!((shell::tail_seconds(ptr) - 1.0).abs() < 1e-9);
        for _ in 0..16 {
            let frames = junk_frames(&mut rng, 32);
            let n = frames as usize;
            let pattern = rng.below(6);
            fill(&mut rng, &mut left, n, pattern);
            fill(&mut rng, &mut right, n, pattern);
            let want = u32::from(n == 0 || n <= 32);
            let got = shell::process(
                ptr,
                left.as_ptr(),
                right.as_ptr(),
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                frames,
            );
            assert_eq!(got, want, "frames {frames}");
            shell::reset(ptr);
            assert_eq!(shell::get_plain(ptr, GAIN), 6.0, "reset changed the plane");
            assert_eq!(shell::latency(ptr), 7);
            assert!((shell::tail_seconds(ptr) - 1.0).abs() < 1e-9);
        }
        let linear = 10f32.powf(6.0 / 20.0);
        left[..8].fill(0.25);
        right[..8].fill(-0.125);
        assert_eq!(
            shell::process(
                ptr,
                left.as_ptr(),
                right.as_ptr(),
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                8,
            ),
            1
        );
        assert!(
            (left[0] - 0.25 * linear).abs() < 1e-5,
            "gain lost after reset: {}",
            left[0]
        );
        assert!(
            (right[0] + 0.125 * linear).abs() < 1e-5,
            "right gain lost after reset: {}",
            right[0]
        );
        shell::destroy(ptr);
    }
}

#[test]
fn shell_info_and_descriptors_ignore_junk_plane_values() {
    let mut rng = Lcg::new(SEED ^ 0x494E_464F); // "INFO"
    let ptr = shell::create::<FuzzGain>(48_000.0, 32, 2);
    assert!(!ptr.is_null());
    let mut before = unsafe { std::mem::zeroed::<OpParamDesc>() };
    assert_eq!(write_param(GAIN, &mut before), 1);
    let mut info_before = unsafe { std::mem::zeroed::<OpShellInfo>() };
    assert_eq!(write_info(&mut info_before), 1);

    for _ in 0..512 {
        let index = rng.index();
        let value = junk_value(&mut rng);
        unsafe {
            let _ = shell::set_plain(ptr, index, value);
            let _ = shell::set_normalized(ptr, index, junk_value(&mut rng));
        }
        let mut desc = unsafe { std::mem::zeroed::<OpParamDesc>() };
        let _ = write_param(index, &mut desc);
    }

    // Real parameter count, ABI and identity survive the junk.
    let mut info = unsafe { std::mem::zeroed::<OpShellInfo>() };
    assert_eq!(write_info(&mut info), 1);
    assert_eq!(info.abi, ABI);
    assert_eq!(info.param_count, PARAM_COUNT);
    assert!(info.name.starts_with(b"Shell Fuzz"));
    assert!(info.vendor.starts_with(b"OpenGrid"));
    assert!(info.version.starts_with(b"0.3.0"));
    assert!(info.category.starts_with(b"Fx|Tools"));
    assert!(info.id.starts_with(b"com.opengrid.test.shell-fuzz"));
    assert_ne!(info.processor_cid, [0; 16]);
    assert_ne!(info.plugin_id_mono, 0);
    assert_eq!(info.processor_cid, info_before.processor_cid);

    // Descriptors are unchanged by plane writes.
    let mut after = unsafe { std::mem::zeroed::<OpParamDesc>() };
    assert_eq!(write_param(GAIN, &mut after), 1);
    assert_eq!(after.min, before.min);
    assert_eq!(after.max, before.max);
    assert_eq!(after.default_plain, before.default_plain);
    assert_eq!(after.steps, before.steps);
    assert_eq!(after.flags, before.flags);
    assert_eq!(after.name, before.name);
    assert_eq!(after.id, before.id);

    // Out-of-range descriptors are rejected, valid ones agree with `PARAMS`.
    for (index, _) in PARAMS.iter().enumerate() {
        let mut desc = unsafe { std::mem::zeroed::<OpParamDesc>() };
        assert_eq!(write_param(index as u32, &mut desc), 1);
        assert_desc(index as u32, &desc);
    }
    for index in [PARAM_COUNT, PARAM_COUNT + 1, u32::MAX] {
        let mut desc = unsafe { std::mem::zeroed::<OpParamDesc>() };
        assert_eq!(write_param(index, &mut desc), 0);
        assert_eq!(
            write_param(index, std::ptr::null_mut()),
            0
        );
    }
    unsafe { shell::destroy(ptr) };
}
