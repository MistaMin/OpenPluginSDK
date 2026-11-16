//! Hostile-input fuzzing for the exported LV2 descriptor.
//!
//! The adapter exports the C entry points of `lv2_descriptor`: instantiate,
//! connect, activate, run, deactivate and cleanup. This test drives them with
//! generated junk: NaN and infinite control values, audio buffers full of NaN,
//! infinities, huge finite values and subnormals, out-of-range port indices,
//! odd block sizes and hostile call order. It passes when the process
//! survives, finite input under finite control still gives output that is not
//! corrupted by the junk, and a clean run after the junk still has the
//! expected gain.
//!
//! NaN and infinite *audio* samples may propagate to the output; that is IEEE
//! arithmetic, not a crash, so those samples are not compared.

use std::cell::Cell;
use std::ffi::c_void;

use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_targets::lv2::{descriptor, set_creator, AUDIO_PORTS};

/// `lv2.rs` prepares this many frames per internal block (`const PREPARED`).
const PREPARED_MAX: usize = 8_192;
/// Room for a block longer than the prepared maximum, plus a sentinel tail.
const CAPACITY: usize = PREPARED_MAX + 64;
/// Host memory is filled with this and checked after a run, so a write past
/// the requested block is visible.
const SENTINEL: f32 = 1_234.5;
/// Fixed LCG seed: every run of the test fuzzes the same input sequence.
const SEED: u32 = 0x5EED_F00D;
/// A few thousand iterations; the whole file stays well under five seconds.
const ITERATIONS: usize = 2_048;

thread_local! {
    /// Sample rate the plugin's `prepare` saw last on this thread. Each test
    /// instantiates synchronously on its own thread, so this needs no lock.
    static PREPARED_RATE: Cell<f32> = const { Cell::new(f32::NAN) };
}

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
    ParamInfo::new("mute", "Mute", 0.0, 1.0, 0.0, Unit::Toggle),
];

static INFO: PluginInfo = PluginInfo::new(
    "urn:openplugin:fuzz-gain",
    "Fuzz Gain",
    "OpenGrid",
    "0.1.0-beta.1",
    Category::Utility,
    &PARAMS,
)
.with_url("https://github.com/MistaMin/OpenPluginSDK")
.with_email("contact@mistamin.com");

#[derive(Clone, Copy)]
struct FuzzGain {
    db: f32,
    mute: bool,
}

impl Default for FuzzGain {
    fn default() -> Self {
        Self {
            db: 0.0,
            mute: false,
        }
    }
}

fn fuzz_gain() -> Box<dyn Plugin> {
    Box::new(FuzzGain::default())
}

impl FuzzGain {
    fn linear(&self) -> f32 {
        if self.mute {
            0.0
        } else {
            10f32.powf(self.db / 20.0)
        }
    }
}

impl Plugin for FuzzGain {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        PREPARED_RATE.with(|cell| cell.set(sample_rate));
        self.db = PARAMS[0].clamp(self.db);
        self.mute = PARAMS[1].clamp(if self.mute { 1.0 } else { 0.0 }) >= 0.5;
    }

    fn reset(&mut self) {}

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "gain" => {
                self.db = PARAMS[0].clamp(value);
                true
            }
            "mute" => {
                self.mute = PARAMS[1].clamp(value) >= 0.5;
                true
            }
            _ => false,
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "gain" => Some(self.db),
            "mute" => Some(if self.mute { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let linear = self.linear();
        for channel in io.iter_mut() {
            let frames = frames.min(channel.len());
            for sample in &mut channel[..frames] {
                *sample *= linear;
            }
        }
    }
}

/// What the plane and the plugin should make of a pair of raw control values.
fn expected_linear(gain: f32, mute: f32) -> f32 {
    let db = PARAMS[0].clamp(gain);
    let mute = PARAMS[1].clamp(mute) >= 0.5;
    if mute {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Deterministic linear congruential generator (Numerical Recipes constants).
struct Lcg(u32);

impl Lcg {
    fn new(seed: u32) -> Self {
        Self(seed)
    }

    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0
    }

    fn below(&mut self, bound: u32) -> u32 {
        self.next_u32() % bound
    }

    fn pick<T: Copy>(&mut self, values: &[T]) -> T {
        values[self.below(values.len() as u32) as usize]
    }

    /// An arbitrary bit pattern: covers NaN, both infinities, subnormals and
    /// anything else a host might write.
    fn bits(&mut self) -> f32 {
        f32::from_bits(self.next_u32())
    }
}

#[derive(Clone, Copy)]
enum Audio {
    Silence,
    Quiet,
    Nan,
    PosInf,
    NegInf,
    Huge,
    Subnormal,
    Mixed,
}

const AUDIO_MODES: [Audio; 8] = [
    Audio::Silence,
    Audio::Quiet,
    Audio::Nan,
    Audio::PosInf,
    Audio::NegInf,
    Audio::Huge,
    Audio::Subnormal,
    Audio::Mixed,
];

fn fill_audio(buffer: &mut [f32], mode: Audio) {
    let mixed = [
        f32::NAN,
        0.75,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MAX,
        -f32::MAX,
        f32::from_bits(1),
        -0.0,
        0.0,
        -0.25,
    ];
    for (index, sample) in buffer.iter_mut().enumerate() {
        *sample = match mode {
            Audio::Silence => 0.0,
            Audio::Quiet => ((index % 17) as f32 - 8.0) * 0.1,
            Audio::Nan => f32::NAN,
            Audio::PosInf => f32::INFINITY,
            Audio::NegInf => f32::NEG_INFINITY,
            Audio::Huge => {
                if index % 2 == 0 {
                    f32::MAX
                } else {
                    -f32::MAX
                }
            }
            Audio::Subnormal => {
                let sign = if index % 2 == 0 { 1.0 } else { -1.0 };
                f32::from_bits(1 + (index as u32 % 5)) * sign
            }
            Audio::Mixed => mixed[index % mixed.len()],
        };
    }
}

/// A thin owner of the published descriptor. All unsafe calls into the C ABI
/// live here so the tests themselves read as host actions.
struct Host(*const openplugin_targets::lv2::Lv2Descriptor);

impl Host {
    fn new() -> Self {
        set_creator(fuzz_gain);
        let raw = descriptor(0);
        assert!(!raw.is_null(), "descriptor(0) is null after set_creator");
        Self(raw)
    }

    fn try_instantiate(&self, sample_rate: f64) -> *mut c_void {
        unsafe { ((*self.0).instantiate)(self.0, sample_rate, std::ptr::null(), std::ptr::null()) }
    }

    fn instantiate(&self, sample_rate: f64) -> *mut c_void {
        let instance = self.try_instantiate(sample_rate);
        assert!(
            !instance.is_null(),
            "instantiate returned null at {sample_rate} Hz"
        );
        instance
    }

    fn connect(&self, instance: *mut c_void, port: u32, data: *mut c_void) {
        unsafe { ((*self.0).connect_port)(instance, port, data) }
    }

    fn activate(&self, instance: *mut c_void) {
        unsafe { ((*self.0).activate)(instance) }
    }

    fn run(&self, instance: *mut c_void, frames: u32) {
        unsafe { ((*self.0).run)(instance, frames) }
    }

    fn deactivate(&self, instance: *mut c_void) {
        unsafe { ((*self.0).deactivate)(instance) }
    }

    fn cleanup(&self, instance: *mut c_void) {
        unsafe { ((*self.0).cleanup)(instance) }
    }
}

/// The adapter must not write past the requested block, so everything after
/// it keeps the sentinel the host put there.
fn assert_tail(output: &[f32], frames: usize, iteration: usize) {
    for (index, &value) in output.iter().enumerate().skip(frames) {
        assert_eq!(
            value.to_bits(),
            SENTINEL.to_bits(),
            "iteration {iteration}: wrote past the {frames}-frame block at sample {index}"
        );
    }
}

/// Compare the samples the adapter wrote for one block.
///
/// The plane clamps every control value before the plugin sees it, so a
/// finite input sample can only become a finite sample or an infinity from
/// f32 overflow: it must never become NaN. Overflow is allowed (`f32::MAX`
/// times 24 dB is not representable), and non-finite input is not inspected
/// because NaN and `0 * inf` legitimately propagate.
fn check_written(output: &[f32], input: &[f32], frames: usize, linear: f32, iteration: usize) {
    assert!(
        frames <= output.len() && frames <= input.len(),
        "iteration {iteration}: block larger than the buffers"
    );
    let probes = [0usize, frames / 2, frames.saturating_sub(1)];
    for &index in &probes {
        let x = input[index];
        let y = output[index];
        if !x.is_finite() {
            continue;
        }
        assert!(
            !y.is_nan(),
            "iteration {iteration}: finite input {x} at sample {index} produced NaN"
        );
        let expected = f64::from(x) * f64::from(linear);
        if expected.abs() <= f64::from(f32::MAX) / 2.0 {
            assert!(
                y.is_finite(),
                "iteration {iteration}: finite input {x} at sample {index} produced {y}"
            );
            let tolerance = expected.abs() * 1e-4 + 1e-6;
            assert!(
                (f64::from(y) - expected).abs() <= tolerance,
                "iteration {iteration}: sample {index} is {y}, expected {expected} (gain {linear})"
            );
        }
    }
}

#[test]
fn randomized_hostile_values_and_lifecycle_orders_survive() {
    let host = Host::new();
    let mut rng = Lcg::new(SEED);

    // 0, one frame, short and typical buffers, the prepared maximum, and one
    // block that the adapter has to split because it exceeds that maximum.
    const BLOCKS: [usize; 8] = [0, 1, 7, 64, 257, 1_024, PREPARED_MAX, PREPARED_MAX + 37];
    // Explicit hostile control values, plus arbitrary bit patterns further
    // down: NaN, both infinities, far outside the -24..24 / 0..1 ranges, -0.0,
    // a subnormal, and ordinary values.
    let specials = [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        1.0e30,
        -1.0e30,
        f32::MAX,
        f32::MIN,
        -0.0,
        0.0,
        24.0,
        -24.0,
        6.5,
        f32::from_bits(1),
        0.5,
    ];

    let mut input_a = vec![0.0f32; CAPACITY];
    let mut input_b = vec![0.0f32; CAPACITY];
    let mut output = vec![SENTINEL; CAPACITY];
    let mut replaced_output = vec![SENTINEL; CAPACITY];

    for iteration in 0..ITERATIONS {
        let block = BLOCKS[iteration % BLOCKS.len()];
        // The lifecycle loop uses only rates the shared range accepts, so every
        // iteration has a live instance; rate rejection itself is covered by
        // `instantiate_rates_use_the_shared_range`.
        let sample_rate = match iteration % 97 {
            0 => 1_000.0,
            1 => 1_234.567_8,
            2 => 7_999.0,
            3 => 0.0,
            4 => 768_000.0,
            5 => 44_100.0,
            _ => 48_000.0,
        };

        let gain_value = if iteration % 11 == 0 {
            rng.bits()
        } else {
            rng.pick(&specials)
        };
        let mute_value = rng.pick(&specials);
        // Two cells hold each control so a port can be reconnected to a
        // different buffer without changing the value the block must see.
        let mut gain_a = gain_value;
        let mut gain_b = gain_value;
        let mut mute_a = mute_value;
        let mut mute_b = mute_value;
        let linear = expected_linear(gain_value, mute_value);

        let mode = AUDIO_MODES[iteration % AUDIO_MODES.len()];
        fill_audio(&mut input_a, mode);
        fill_audio(&mut input_b, mode);
        output.fill(SENTINEL);
        replaced_output.fill(SENTINEL);

        let instance = host.instantiate(sample_rate);

        // Connect the four audio ports to one set of buffers, then replace the
        // input on every third iteration with a different, equally hostile
        // buffer before activate. The replaced buffer must never be touched.
        host.connect(instance, 0, input_a.as_mut_ptr() as *mut c_void);
        host.connect(instance, 1, input_a.as_mut_ptr() as *mut c_void);
        if iteration % 3 == 0 {
            host.connect(instance, 0, input_b.as_mut_ptr() as *mut c_void);
            host.connect(instance, 1, input_b.as_mut_ptr() as *mut c_void);
        }
        // The output port is also connected twice; the second buffer wins.
        host.connect(instance, 2, replaced_output.as_mut_ptr() as *mut c_void);
        host.connect(instance, 3, replaced_output.as_mut_ptr() as *mut c_void);
        host.connect(instance, 2, output.as_mut_ptr() as *mut c_void);
        host.connect(instance, 3, output.as_mut_ptr() as *mut c_void);

        host.connect(
            instance,
            AUDIO_PORTS,
            std::ptr::addr_of_mut!(gain_a) as *mut c_void,
        );
        host.connect(
            instance,
            AUDIO_PORTS + 1,
            std::ptr::addr_of_mut!(mute_a) as *mut c_void,
        );

        // Run without activate on every fourth iteration, otherwise activate
        // first; some iterations activate twice.
        let mut activated = false;
        if iteration % 4 != 0 {
            host.activate(instance);
            activated = true;
            if iteration % 29 == 0 {
                host.activate(instance);
            }
        }
        // One to three runs. Most are repeated calls with no lifecycle change;
        // every seventh iteration deactivates before a later run.
        let runs = 1 + rng.below(3) as usize;
        for run_index in 0..runs {
            if activated && run_index == 1 && iteration % 7 == 0 {
                host.deactivate(instance);
            }
            host.run(instance, block as u32);
        }

        // Deactivate twice in a row, and connect a fresh control buffer after
        // deactivate, then run again.
        if iteration % 13 == 0 {
            host.deactivate(instance);
            host.deactivate(instance);
        }
        if iteration % 17 == 0 && block > 0 {
            host.connect(
                instance,
                AUDIO_PORTS,
                std::ptr::addr_of_mut!(gain_b) as *mut c_void,
            );
            host.connect(
                instance,
                AUDIO_PORTS + 1,
                std::ptr::addr_of_mut!(mute_b) as *mut c_void,
            );
            host.run(instance, 1);
        }

        // A zero-frame run writes nothing; any other run writes exactly the
        // requested block and no further.
        if block == 0 {
            assert_tail(&output, 0, iteration);
        } else {
            let input = if iteration % 3 == 0 {
                &input_b
            } else {
                &input_a
            };
            check_written(&output, input, block, linear, iteration);
            assert_tail(&output, block, iteration);
        }
        assert_tail(&replaced_output, 0, iteration);

        // Junk must not corrupt the plane or the gain stage: after all of the
        // above, a clean run still produces the expected 6 dB gain. Fresh
        // control cells also reconnect the control ports after the churn.
        let mut clean_gain = 6.0f32;
        let mut clean_mute = 0.0f32;
        host.connect(
            instance,
            AUDIO_PORTS,
            std::ptr::addr_of_mut!(clean_gain) as *mut c_void,
        );
        host.connect(
            instance,
            AUDIO_PORTS + 1,
            std::ptr::addr_of_mut!(clean_mute) as *mut c_void,
        );
        input_a[..64].fill(0.25);
        input_b[..64].fill(0.25);
        host.run(instance, 64);
        let expected = 0.25 * 10f32.powf(6.0 / 20.0);
        for &index in &[0usize, 31, 63] {
            assert!(
                (output[index] - expected).abs() < 1e-4 * expected.abs().max(1.0),
                "iteration {iteration}: clean run after junk is {} at {index}, expected {expected}",
                output[index]
            );
        }

        host.cleanup(instance);
    }
}

#[test]
fn reconnected_ports_and_lifecycle_order_stay_safe() {
    let host = Host::new();

    // Every entry point tolerates a null instance handle: the adapter checks.
    host.connect(std::ptr::null_mut(), 0, std::ptr::null_mut());
    host.activate(std::ptr::null_mut());
    host.run(std::ptr::null_mut(), 64);
    host.deactivate(std::ptr::null_mut());
    host.cleanup(std::ptr::null_mut());

    // Only descriptor 0 exists, and `extension_data` has nothing to return.
    assert!(descriptor(1).is_null());
    assert!(descriptor(u32::MAX).is_null());
    let extension = unsafe { ((*host.0).extension_data)(std::ptr::null()) };
    assert!(extension.is_null());
    let extension =
        unsafe { ((*host.0).extension_data)(c"http://lv2plug.in/ns/ext/state".as_ptr()) };
    assert!(extension.is_null());

    // `instantiate` does not read its descriptor argument.
    let null_descriptor = unsafe {
        ((*host.0).instantiate)(
            std::ptr::null(),
            48_000.0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert!(!null_descriptor.is_null());
    host.cleanup(null_descriptor);

    let instance = host.instantiate(48_000.0);
    let mut input_a = [0.25f32; 16];
    let mut input_b = [-0.5f32; 16];
    let mut output_a = [SENTINEL; 16];
    let mut output_b = [-7.0f32; 16];
    let mut gain_zero = 0.0f32;
    let mut gain_six = 6.0f32;
    let mut mute_off = 0.0f32;
    let mut mute_on = 1.0f32;
    let six_db = 10f32.powf(6.0 / 20.0);

    // Connect before activate. Audio port 0 is connected twice: the second
    // buffer replaces the first, which must not be written. The same happens
    // to control port 4, where the later 6 dB cell wins over 0 dB. Only the
    // left channel is connected, so each check below has one writer.
    host.connect(instance, 0, input_a.as_mut_ptr() as *mut c_void);
    host.connect(instance, 2, output_a.as_mut_ptr() as *mut c_void);
    host.connect(instance, 0, input_b.as_mut_ptr() as *mut c_void);
    host.connect(instance, 2, output_b.as_mut_ptr() as *mut c_void);
    host.connect(
        instance,
        AUDIO_PORTS,
        std::ptr::addr_of_mut!(gain_zero) as *mut c_void,
    );
    host.connect(
        instance,
        AUDIO_PORTS + 1,
        std::ptr::addr_of_mut!(mute_off) as *mut c_void,
    );
    host.connect(
        instance,
        AUDIO_PORTS,
        std::ptr::addr_of_mut!(gain_six) as *mut c_void,
    );

    host.activate(instance);
    host.run(instance, 8);
    for &sample in &output_b[..8] {
        assert!(
            (sample - -0.5 * six_db).abs() < 1e-5,
            "reconnected input or control did not win: {sample}"
        );
    }
    assert_tail(&output_a, 0, 0);
    assert_eq!(output_b[8], -7.0, "wrote past the requested block");

    // Deactivate twice, then run. The current adapter still renders; either
    // way the call must return without touching memory it does not own.
    host.deactivate(instance);
    host.deactivate(instance);
    host.run(instance, 8);

    // Connect a control after deactivate, then activate and run: the new
    // value applies.
    host.connect(
        instance,
        AUDIO_PORTS,
        std::ptr::addr_of_mut!(gain_zero) as *mut c_void,
    );
    host.activate(instance);
    host.run(instance, 8);
    for &sample in &output_b[..8] {
        assert!(
            (sample - -0.5).abs() < 1e-5,
            "control connected after deactivate was ignored: {sample}"
        );
    }

    // Out-of-range control ports are ignored, not written through.
    host.connect(
        instance,
        AUDIO_PORTS + 2,
        std::ptr::addr_of_mut!(gain_six) as *mut c_void,
    );
    host.connect(
        instance,
        u32::MAX,
        std::ptr::addr_of_mut!(gain_six) as *mut c_void,
    );
    host.run(instance, 8);
    for &sample in &output_b[..8] {
        assert!(
            (sample - -0.5).abs() < 1e-5,
            "an out-of-range control port changed the gain: {sample}"
        );
    }

    // Null audio data is checked by the adapter: missing input reads silence
    // and a missing output is not written.
    host.connect(instance, 0, std::ptr::null_mut());
    host.connect(instance, 1, std::ptr::null_mut());
    host.connect(instance, 2, std::ptr::null_mut());
    host.connect(instance, 3, std::ptr::null_mut());
    let untouched = output_b;
    host.run(instance, 8);
    assert_eq!(output_b, untouched, "a null output port was written");

    // In-place processing: input and output on the same buffer.
    let mut in_place = [0.5f32; 16];
    host.connect(
        instance,
        AUDIO_PORTS,
        std::ptr::addr_of_mut!(gain_six) as *mut c_void,
    );
    host.connect(instance, 0, in_place.as_mut_ptr() as *mut c_void);
    host.connect(instance, 2, in_place.as_mut_ptr() as *mut c_void);
    host.run(instance, 8);
    for &sample in &in_place[..8] {
        assert!(
            (sample - 0.5 * six_db).abs() < 1e-5,
            "in-place gain is {sample}"
        );
    }

    // A toggle rounds its control value to a switch: 1.0 mutes the output.
    host.connect(
        instance,
        AUDIO_PORTS,
        std::ptr::addr_of_mut!(gain_zero) as *mut c_void,
    );
    host.connect(
        instance,
        AUDIO_PORTS + 1,
        std::ptr::addr_of_mut!(mute_on) as *mut c_void,
    );
    in_place.fill(0.5);
    host.run(instance, 8);
    for &sample in &in_place[..8] {
        assert_eq!(sample, 0.0, "mute left a non-zero sample");
    }

    // A clean run after all of the above still has the expected gain.
    host.connect(
        instance,
        AUDIO_PORTS,
        std::ptr::addr_of_mut!(gain_six) as *mut c_void,
    );
    host.connect(
        instance,
        AUDIO_PORTS + 1,
        std::ptr::addr_of_mut!(mute_off) as *mut c_void,
    );
    host.connect(instance, 0, input_a.as_mut_ptr() as *mut c_void);
    host.connect(instance, 2, output_b.as_mut_ptr() as *mut c_void);
    input_a.fill(0.25);
    output_b.fill(SENTINEL);
    host.run(instance, 8);
    for &sample in &output_b[..8] {
        assert!(
            (sample - 0.25 * six_db).abs() < 1e-5,
            "clean run is {sample}"
        );
    }
    assert_eq!(output_b[8], SENTINEL, "clean run wrote past its block");

    host.cleanup(instance);
}

#[test]
fn instantiate_rates_use_the_shared_range() {
    let host = Host::new();
    let mut input = [0.25f32; 64];
    let mut output = [SENTINEL; 64];

    // Rates in the shared 1 kHz..=768 kHz range are prepared exactly.
    for &rate in &[
        1_000.0,
        1_001.0,
        1_234.567_8,
        7_999.0,
        8_000.0,
        44_100.0,
        48_000.0,
        96_000.0,
        768_000.0,
    ] {
        let instance = host.try_instantiate(rate);
        assert!(
            !instance.is_null(),
            "instantiate rejected the supported rate {rate}"
        );
        let prepared = PREPARED_RATE.with(|cell| cell.get());
        assert_eq!(
            prepared.to_bits(),
            (rate as f32).to_bits(),
            "sample rate {rate} prepared the plugin with {prepared}"
        );
        host.connect(instance, 0, input.as_mut_ptr() as *mut c_void);
        host.connect(instance, 1, input.as_mut_ptr() as *mut c_void);
        host.connect(instance, 2, output.as_mut_ptr() as *mut c_void);
        host.connect(instance, 3, output.as_mut_ptr() as *mut c_void);
        host.activate(instance);
        host.run(instance, 64);
        host.cleanup(instance);
        for &sample in &output {
            assert!(
                (sample - 0.25).abs() < 1e-6,
                "rate {rate} processed {sample}"
            );
        }
    }

    // 0.0 is LV2's "rate unknown" value: it keeps the 48 kHz fallback.
    for &rate in &[0.0, -0.0] {
        let instance = host.try_instantiate(rate);
        assert!(!instance.is_null(), "unknown rate {rate} was rejected");
        let prepared = PREPARED_RATE.with(|cell| cell.get());
        assert_eq!(
            prepared.to_bits(),
            48_000.0f32.to_bits(),
            "unknown rate {rate} prepared the plugin with {prepared}"
        );
        host.cleanup(instance);
    }

    // Non-finite rates and rates outside the shared range fail before prepare.
    for &rate in &[
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -1.0,
        -48_000.0,
        999.0,
        768_001.0,
        1.0e9,
        f64::MAX,
    ] {
        PREPARED_RATE.with(|cell| cell.set(-1.0));
        let instance = host.try_instantiate(rate);
        assert!(
            instance.is_null(),
            "instantiate accepted the hostile rate {rate}"
        );
        assert_eq!(
            PREPARED_RATE.with(|cell| cell.get()),
            -1.0,
            "rate {rate} reached prepare"
        );
    }
}

#[test]
fn repeated_instantiate_and_cleanup_has_no_reused_state() {
    let host = Host::new();
    let rates = [0.0, 1_000.0, 7_999.0, 48_000.0, 768_000.0];
    let mut input = [0.25f32; 64];
    let mut output = [SENTINEL; 64];

    for iteration in 0..256usize {
        let mut gain = if iteration % 2 == 0 {
            12.0f32
        } else {
            -12.0f32
        };
        let instance = host.instantiate(rates[iteration % rates.len()]);
        host.connect(instance, 0, input.as_mut_ptr() as *mut c_void);
        host.connect(instance, 1, input.as_mut_ptr() as *mut c_void);
        host.connect(instance, 2, output.as_mut_ptr() as *mut c_void);
        host.connect(instance, 3, output.as_mut_ptr() as *mut c_void);
        host.connect(
            instance,
            AUDIO_PORTS,
            std::ptr::addr_of_mut!(gain) as *mut c_void,
        );
        if iteration % 3 != 0 {
            host.activate(instance);
        }
        if iteration % 3 == 1 {
            host.deactivate(instance);
        }
        if iteration % 4 != 0 {
            host.run(instance, 32);
        }
        host.cleanup(instance);

        // A fresh instance starts from the descriptor defaults, not from the
        // gain the previous instance was given.
        let fresh = host.instantiate(48_000.0);
        host.connect(fresh, 0, input.as_mut_ptr() as *mut c_void);
        host.connect(fresh, 2, output.as_mut_ptr() as *mut c_void);
        host.run(fresh, 32);
        for &sample in &output[..32] {
            assert!(
                (sample - 0.25).abs() < 1e-6,
                "iteration {iteration}: a fresh instance reused state and processed {sample}"
            );
        }
        host.cleanup(fresh);
    }

    // Repeated cleanup and creation leave the descriptor intact.
    assert!(!descriptor(0).is_null());
    let last = host.instantiate(48_000.0);
    host.cleanup(last);
}
