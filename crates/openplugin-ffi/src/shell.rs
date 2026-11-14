//! C ABI a native VST3, Audio Unit, or AAX adapter calls.
//!
//! One plugin binary exports these symbols through `openplugin_targets::export_shell!`.
//! `create` allocates. `process` does not.
//! Parameter writes go to the plane. The audio thread applies them inside `process`.

use std::panic::AssertUnwindSafe;
use std::sync::{Mutex, OnceLock};

use openplugin_core::{AaxIds, Category, EditorState, ParamFlags, ParamInfo, PluginInfo, Taper, Unit};

use crate::{FfiInstance, Plugin};

pub const ABI: u32 = 1;
pub const PARAM_LOG: u32 = 1;
pub const PARAM_DISCRETE: u32 = 2;
pub const PARAM_AUTOMATABLE: u32 = 4;
/// Plain values in one AAX packet. VST3 and Audio Unit are not capped by this.
pub const AAX_MAX_PARAMS: u32 = 64;

#[repr(C)]
pub struct OpShellInfo {
    pub abi: u32,
    pub param_count: u32,
    pub processor_cid: [u8; 16],
    pub controller_cid: [u8; 16],
    pub manufacturer_id: u32,
    pub product_id: u32,
    pub plugin_id_mono: u32,
    pub plugin_id_stereo: u32,
    pub name: [u8; 64],
    pub vendor: [u8; 64],
    pub version: [u8; 32],
    pub category: [u8; 128],
    pub id: [u8; 128],
}

#[repr(C)]
pub struct OpParamDesc {
    pub min: f64,
    pub max: f64,
    pub default_plain: f64,
    pub default_normalized: f64,
    pub steps: u32,
    pub flags: u32,
    pub name: [u8; 64],
    pub unit: [u8; 16],
    pub id: [u8; 64],
}

pub struct ShellInstance {
    ffi: FfiInstance,
}

fn put(dst: &mut [u8], src: &str) {
    dst.fill(0);
    let bytes = src.as_bytes();
    let count = bytes.len().min(dst.len().saturating_sub(1));
    dst[..count].copy_from_slice(&bytes[..count]);
}

fn mix(text: &str, salt: u64) -> [u8; 16] {
    let mut high = 0xcbf29ce484222325u64 ^ salt;
    let mut low = 0x100000001b3u64 ^ salt.wrapping_mul(0x9e3779b97f4a7c15);
    for byte in text.bytes() {
        high ^= u64::from(byte);
        high = high.wrapping_mul(0x100000001b3);
        low ^= u64::from(byte);
        low = low.wrapping_mul(0xcbf29ce484222325);
    }
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&high.to_le_bytes());
    out[8..].copy_from_slice(&low.to_le_bytes());
    if out == [0; 16] {
        out[0] = 1;
    }
    out
}

fn fourcc(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([
        bytes[0] | 0x20,
        bytes[1] | 0x20,
        bytes[2] | 0x20,
        bytes[3] | 0x20,
    ])
}

fn plugin_info<P: Plugin + Default>() -> &'static PluginInfo {
    static SLOT: OnceLock<&'static PluginInfo> = OnceLock::new();
    SLOT.get_or_init(|| P::default().info())
}

/// [`Plugin::aax_ids`], when the plugin sets valid ones.
fn plugin_aax_ids<P: Plugin + Default>() -> Option<AaxIds> {
    static SLOT: OnceLock<Option<AaxIds>> = OnceLock::new();
    *SLOT.get_or_init(|| P::default().aax_ids().filter(AaxIds::is_valid))
}

/// A four-character code the way AAX reads one (`'OpGd'`, first byte highest).
fn code(bytes: [u8; 4]) -> u32 {
    u32::from_be_bytes(bytes)
}

/// Live instances, by address, so an editor can tell a live instance from a
/// destroyed one. Message thread.
static LIVE: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Run `f` on the instance at `ptr` if it is live. The table stays locked
/// while `f` runs, so [`destroy`] cannot free the instance meanwhile.
fn with_live<T>(ptr: *const ShellInstance, f: impl FnOnce(&ShellInstance) -> T) -> Option<T> {
    if ptr.is_null() {
        return None;
    }
    let list = LIVE.lock().ok()?;
    if !list.contains(&(ptr as usize)) {
        return None;
    }
    // In the table and not destroyed: `destroy` takes the lock before it frees.
    Some(f(unsafe { &*ptr }))
}

/// What the engine `ptr` shares with its editor, or `None` when `ptr` is
/// null, not a live instance, or shares nothing. Message thread. Any pointer
/// is accepted: it is only dereferenced once found in the live table.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn editor_state(ptr: *const ShellInstance) -> Option<EditorState> {
    with_live(ptr, |instance| instance.ffi.editor_state()).flatten()
}

/// Seconds of tail as of the last block; infinity for an endless tail. Any
/// thread, never blocks.
///
/// # Safety
/// `ptr` is null or a live [`ShellInstance`].
pub unsafe fn tail_seconds(ptr: *const ShellInstance) -> f64 {
    if ptr.is_null() {
        0.0
    } else {
        unsafe { (*ptr).ffi.tail_seconds() }
    }
}

fn category_name(category: Category) -> &'static str {
    match category {
        Category::Eq => "Fx|EQ",
        Category::Dynamics => "Fx|Dynamics",
        Category::PitchShift => "Fx|Pitch Shift",
        Category::Reverb => "Fx|Reverb",
        Category::Delay => "Fx|Delay",
        Category::Modulation => "Fx|Modulation",
        Category::Harmonic => "Fx|Distortion",
        Category::Dither => "Fx",
        Category::Instrument => "Instrument",
        Category::Utility => "Fx|Tools",
        Category::Other => "Fx",
    }
}

fn unit_name(unit: Unit) -> &'static str {
    match unit {
        Unit::Db => "dB",
        Unit::Hz => "Hz",
        Unit::Ms => "ms",
        Unit::Percent => "%",
        Unit::Ratio => ":1",
        Unit::Semitones => "st",
        Unit::Cents => "ct",
        Unit::Seconds => "s",
        Unit::Toggle | Unit::Choice | Unit::None => "",
    }
}

fn steps_of(param: &ParamInfo) -> u32 {
    if param.steps > 1 {
        param.steps
    } else if param.unit == Unit::Choice && !param.choices.is_empty() {
        param.choices.len() as u32
    } else if param.unit == Unit::Toggle {
        2
    } else {
        0
    }
}

fn flags_of(param: &ParamInfo, steps: u32) -> u32 {
    let mut flags = 0;
    if param.taper == Taper::Log && param.min > 0.0 {
        flags |= PARAM_LOG;
    }
    if steps > 1
        || param.flags.contains(ParamFlags::STEPPED)
        || matches!(param.unit, Unit::Choice | Unit::Toggle)
    {
        flags |= PARAM_DISCRETE;
    }
    if param.flags.contains(ParamFlags::AUTOMATABLE) {
        flags |= PARAM_AUTOMATABLE;
    }
    flags
}

fn fill_desc(param: &ParamInfo, out: &mut OpParamDesc) {
    let steps = steps_of(param);
    *out = OpParamDesc {
        min: f64::from(param.min),
        max: f64::from(param.max),
        default_plain: f64::from(param.default),
        default_normalized: f64::from(param.to_normalized(param.default)),
        steps,
        flags: flags_of(param, steps),
        name: [0; 64],
        unit: [0; 16],
        id: [0; 64],
    };
    put(&mut out.name, param.name);
    put(&mut out.unit, unit_name(param.unit));
    put(&mut out.id, param.id);
}

pub fn write_info<P: Plugin + Default>(out: *mut OpShellInfo) -> u32 {
    if out.is_null() {
        return 0;
    }
    let filled = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let info = plugin_info::<P>();
        let processor = mix(info.id, 1);
        let controller = mix(info.id, 2);
        let mono = fourcc(&processor);
        let mut stereo = fourcc(&controller);
        if stereo == mono {
            stereo ^= 0x0100_0000;
        }
        let (manufacturer_id, product_id, plugin_id_mono, plugin_id_stereo) = match plugin_aax_ids::<P>() {
            Some(ids) => (code(ids.manufacturer), code(ids.product), code(ids.mono), code(ids.stereo)),
            None => (fourcc(&mix(info.vendor, 3)), fourcc(&mix(info.id, 4)), mono, stereo),
        };
        let header = OpShellInfo {
            abi: ABI,
            param_count: info.params.len().min(u32::MAX as usize) as u32,
            processor_cid: processor,
            controller_cid: controller,
            manufacturer_id,
            product_id,
            plugin_id_mono,
            plugin_id_stereo,
            name: [0; 64],
            vendor: [0; 64],
            version: [0; 32],
            category: [0; 128],
            id: [0; 128],
        };
        unsafe {
            out.write(header);
            let info_out = &mut *out;
            put(&mut info_out.name, info.name);
            put(&mut info_out.vendor, info.vendor);
            put(&mut info_out.version, info.version);
            put(&mut info_out.category, category_name(info.category));
            put(&mut info_out.id, info.id);
        }
    }));
    u32::from(filled.is_ok())
}

pub fn write_param<P: Plugin + Default>(index: u32, out: *mut OpParamDesc) -> u32 {
    if out.is_null() {
        return 0;
    }
    let Some(param) = plugin_info::<P>().params.get(index as usize) else {
        return 0;
    };
    unsafe { fill_desc(param, &mut *out) };
    1
}

pub fn to_plain<P: Plugin + Default>(index: u32, normalized: f64) -> f64 {
    let Some(param) = plugin_info::<P>().params.get(index as usize) else {
        return f64::NAN;
    };
    let travel = if normalized.is_finite() {
        normalized as f32
    } else {
        0.0
    };
    f64::from(param.from_normalized(travel))
}

pub fn to_normalized<P: Plugin + Default>(index: u32, plain: f64) -> f64 {
    let Some(param) = plugin_info::<P>().params.get(index as usize) else {
        return f64::NAN;
    };
    let value = if plain.is_finite() { plain as f32 } else { param.default };
    f64::from(param.to_normalized(value))
}

pub fn create<P: Plugin + Default>(rate: f64, max_block: u32, channels: u32) -> *mut ShellInstance {
    let created = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if !rate.is_finite() || !(8_000.0..=768_000.0).contains(&rate) {
            return None;
        }
        if max_block == 0 || max_block > 65_536 || !(1..=2).contains(&channels) {
            return None;
        }
        let plugin = Box::new(P::default());
        if !plugin.accepts_sample_rate(rate as f32) {
            return None;
        }
        let ffi = FfiInstance::new(plugin, rate, max_block as usize, channels as usize);
        Some(Box::into_raw(Box::new(ShellInstance { ffi })))
    }));
    match created {
        Ok(Some(ptr)) => {
            if let Ok(mut list) = LIVE.lock() {
                list.push(ptr as usize);
            }
            trace::created(plugin_info::<P>(), ptr as usize);
            ptr
        }
        _ => std::ptr::null_mut(),
    }
}

/// # Safety
/// `ptr` is null or a pointer returned by [`create`] that has not been destroyed.
pub unsafe fn destroy(ptr: *mut ShellInstance) {
    if !ptr.is_null() {
        if let Ok(mut list) = LIVE.lock() {
            list.retain(|key| *key != ptr as usize);
        }
        trace::destroyed(ptr as usize);
        drop(unsafe { Box::from_raw(ptr) });
    }
}

/// # Safety
/// `ptr` is null or a live [`ShellInstance`]. Exclusive with [`process`].
pub unsafe fn reset(ptr: *mut ShellInstance) {
    if ptr.is_null() {
        return;
    }
    trace::reset();
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe { (*ptr).ffi.reset() }));
}

/// # Safety
/// `ptr` is null or a live [`ShellInstance`].
pub unsafe fn latency(ptr: *const ShellInstance) -> u32 {
    if ptr.is_null() {
        0
    } else {
        unsafe { (*ptr).ffi.latency() }
    }
}

/// # Safety
/// `ptr` is null or a live [`ShellInstance`].
pub unsafe fn set_plain(ptr: *mut ShellInstance, index: u32, plain: f32) -> u32 {
    if ptr.is_null() || !plain.is_finite() {
        return 0;
    }
    u32::from(unsafe { (*ptr).ffi.store_param(index as usize, plain) })
}

/// # Safety
/// `ptr` is null or a live [`ShellInstance`].
pub unsafe fn get_plain(ptr: *const ShellInstance, index: u32) -> f32 {
    if ptr.is_null() {
        0.0
    } else {
        unsafe { (*ptr).ffi.load_param(index as usize) }
    }
}

/// # Safety
/// `ptr` is null or a live [`ShellInstance`].
pub unsafe fn set_normalized(ptr: *mut ShellInstance, index: u32, normalized: f32) -> u32 {
    if ptr.is_null() {
        return 0;
    }
    u32::from(unsafe { (*ptr).ffi.store_normalized(index as usize, normalized) })
}

/// # Safety
/// `ptr` is null or a live [`ShellInstance`].
pub unsafe fn get_normalized(ptr: *const ShellInstance, index: u32) -> f32 {
    if ptr.is_null() {
        0.0
    } else {
        unsafe { (*ptr).ffi.load_normalized(index as usize) }
    }
}

/// # Safety
/// Pointers are null or address `frames` floats. An input may alias an output.
/// A null right pointer is mono. `ptr` is a live [`ShellInstance`].
pub unsafe fn process(
    ptr: *mut ShellInstance,
    in_left: *const f32,
    in_right: *const f32,
    out_left: *mut f32,
    out_right: *mut f32,
    frames: u32,
) -> u32 {
    if ptr.is_null() {
        return 0;
    }
    let tracing = trace::on();
    let input = if tracing { unsafe { trace::peak(in_left, frames).max(trace::peak(in_right, frames)) } } else { 0.0 };
    let ran = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        (*ptr).ffi.process_io(
            in_left,
            in_right,
            out_left,
            out_right,
            frames as usize,
        )
    }));
    let ok = matches!(ran, Ok(true));
    if tracing {
        let output = unsafe { trace::peak(out_left, frames).max(trace::peak(out_right, frames)) };
        trace::block(ptr as usize, frames, ok, input, output);
    }
    u32::from(ok)
}

/// A diagnostic trace of what a host does with the shell, for a host that
/// goes silent and says nothing. Off unless the file `<name>-trace.on` is in
/// the temporary folder when the first instance is created (`<name>` is the
/// plugin name in lower case, spaces as dashes). When on, the audio thread
/// only updates counters, and [`trace::write`] appends a line to
/// `<name>-trace.log` there at most four times a second: creates, destroys,
/// resets, blocks and failed blocks since the last line, the block size, the
/// input and output peaks, and the first eight parameter values the engine
/// holds. The editor shells call it from their main-thread value updates.
pub mod trace {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

    static STATE: AtomicU8 = AtomicU8::new(0); // 0 undecided, 1 off, 2 on
    static CREATES: AtomicU32 = AtomicU32::new(0);
    static DESTROYS: AtomicU32 = AtomicU32::new(0);
    static RESETS: AtomicU32 = AtomicU32::new(0);
    static BLOCKS: AtomicU32 = AtomicU32::new(0);
    static FAILED: AtomicU32 = AtomicU32::new(0);
    static FRAMES: AtomicU32 = AtomicU32::new(0);
    static PEAK_IN: AtomicU32 = AtomicU32::new(0);
    static PEAK_OUT: AtomicU32 = AtomicU32::new(0);
    static INSTANCE: AtomicUsize = AtomicUsize::new(0);
    static LAST_WRITE: AtomicU64 = AtomicU64::new(0);
    static LOG: OnceLock<(PathBuf, &'static PluginInfo)> = OnceLock::new();

    /// The file stem for a plugin: `My Plugin` is `my-plugin`.
    pub fn stem(name: &str) -> String {
        name.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
            .collect()
    }

    pub(super) fn created(info: &'static PluginInfo, instance: usize) {
        if STATE.load(Ordering::Relaxed) == 0 {
            let stem = stem(info.name);
            let dir = std::env::temp_dir();
            let found = dir.join(format!("{stem}-trace.on")).exists();
            if found {
                let _ = LOG.set((dir.join(format!("{stem}-trace.log")), info));
            }
            STATE.store(if found { 2 } else { 1 }, Ordering::Relaxed);
        }
        if on() {
            CREATES.fetch_add(1, Ordering::Relaxed);
            INSTANCE.store(instance, Ordering::Relaxed);
        }
    }

    pub(super) fn destroyed(instance: usize) {
        if on() {
            DESTROYS.fetch_add(1, Ordering::Relaxed);
            let _ = INSTANCE.compare_exchange(instance, 0, Ordering::Relaxed, Ordering::Relaxed);
        }
    }

    pub(super) fn reset() {
        if on() {
            RESETS.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// True when the trace file was there at the first create.
    pub fn on() -> bool {
        STATE.load(Ordering::Relaxed) == 2
    }

    /// # Safety
    /// `p` is null or addresses `frames` floats.
    pub(super) unsafe fn peak(p: *const f32, frames: u32) -> f32 {
        if p.is_null() {
            return 0.0;
        }
        let samples = unsafe { std::slice::from_raw_parts(p, frames as usize) };
        samples.iter().fold(0.0f32, |m, v| if v.is_finite() { m.max(v.abs()) } else { f32::INFINITY })
    }

    fn raise(slot: &AtomicU32, v: f32) {
        if v > f32::from_bits(slot.load(Ordering::Relaxed)) {
            slot.store(v.to_bits(), Ordering::Relaxed);
        }
    }

    pub(super) fn block(instance: usize, frames: u32, ok: bool, input: f32, output: f32) {
        BLOCKS.fetch_add(1, Ordering::Relaxed);
        if !ok {
            FAILED.fetch_add(1, Ordering::Relaxed);
        }
        FRAMES.store(frames, Ordering::Relaxed);
        INSTANCE.store(instance, Ordering::Relaxed);
        raise(&PEAK_IN, input);
        raise(&PEAK_OUT, output);
    }

    /// Append one line if the trace is on and a quarter second has passed.
    /// Message thread.
    pub fn write(note: &str) {
        if !on() {
            return;
        }
        let Some((path, info)) = LOG.get() else { return };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if now.saturating_sub(LAST_WRITE.load(Ordering::Relaxed)) < 250 {
            return;
        }
        LAST_WRITE.store(now, Ordering::Relaxed);
        let instance = INSTANCE.load(Ordering::Relaxed) as *const ShellInstance;
        let mut line = format!(
            "{now} {note} creates={} destroys={} resets={} blocks={} failed={} frames={} peak_in={:.5} peak_out={:.5}",
            CREATES.load(Ordering::Relaxed),
            DESTROYS.load(Ordering::Relaxed),
            RESETS.load(Ordering::Relaxed),
            BLOCKS.swap(0, Ordering::Relaxed),
            FAILED.swap(0, Ordering::Relaxed),
            FRAMES.load(Ordering::Relaxed),
            f32::from_bits(PEAK_IN.swap(0, Ordering::Relaxed)),
            f32::from_bits(PEAK_OUT.swap(0, Ordering::Relaxed)),
        );
        // What the engine holds, not what the host last reported.
        for (index, param) in info.params.iter().take(8).enumerate() {
            let value = with_live(instance, |live| live.ffi.load_param(index)).unwrap_or(f32::NAN);
            line.push_str(&format!(" {}={value:.3}", param.id));
        }
        line.push('\n');
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openplugin_core::Unit;

    struct Gain {
        db: f32,
    }

    static PARAMS: [ParamInfo; 2] = [
        ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
        ParamInfo::new("freq", "Freq", 20.0, 20_000.0, 1_000.0, Unit::Hz).with_taper(Taper::Log),
    ];

    static INFO: PluginInfo = PluginInfo {
        id: "shell-gain-test",
        name: "Gain",
        vendor: "Test",
        url: "",
        email: "",
        version: "0.0.1",
        category: Category::Utility,
        params: &PARAMS,
        is_instrument: false,
        has_editor: false,
    };

    impl Default for Gain {
        fn default() -> Self {
            Self { db: 0.0 }
        }
    }

    impl Plugin for Gain {
        fn info(&self) -> &'static PluginInfo {
            &INFO
        }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, id: &str, value: f32) -> bool {
            if id == "gain" {
                self.db = PARAMS[0].clamp(value);
                true
            } else {
                id == "freq"
            }
        }
        fn param(&self, id: &str) -> Option<f32> {
            match id {
                "gain" => Some(self.db),
                "freq" => Some(1_000.0),
                _ => None,
            }
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            let linear = 10f32.powf(self.db / 20.0);
            for channel in io.iter_mut() {
                for sample in channel.iter_mut().take(frames) {
                    *sample *= linear;
                }
            }
        }
    }

    #[test]
    fn shell_describes_a_stable_identity_and_a_log_taper() {
        let mut info = unsafe { std::mem::zeroed::<OpShellInfo>() };
        assert_eq!(write_info::<Gain>(&mut info), 1);
        assert_eq!(info.abi, ABI);
        assert_eq!(info.param_count, 2);
        assert_ne!(info.processor_cid, [0; 16]);
        assert_ne!(info.processor_cid, info.controller_cid);
        assert_ne!(info.plugin_id_mono, info.plugin_id_stereo);
    }

    #[test]
    fn shell_identity_fields_are_nonzero() {
        let mut info = unsafe { std::mem::zeroed::<OpShellInfo>() };
        assert_eq!(write_info::<Gain>(&mut info), 1);
        assert_ne!(info.manufacturer_id, 0);
        assert_ne!(info.product_id, 0);
        assert_ne!(info.plugin_id_mono, 0);
        assert_ne!(info.plugin_id_stereo, 0);
        let mut again = unsafe { std::mem::zeroed::<OpShellInfo>() };
        assert_eq!(write_info::<Gain>(&mut again), 1);
        assert_eq!(info.processor_cid, again.processor_cid);
        assert!(info.name.starts_with(b"Gain"));
        assert!(info.category.starts_with(b"Fx|Tools"));
    }

    #[test]
    fn shell_log_roundtrip_and_six_db_inplace() {
        let mut desc = unsafe { std::mem::zeroed::<OpParamDesc>() };
        assert_eq!(write_param::<Gain>(1, &mut desc), 1);
        assert_ne!(desc.flags & PARAM_LOG, 0);
        let normalized = to_normalized::<Gain>(1, 1_000.0);
        let plain = to_plain::<Gain>(1, normalized);
        assert!((plain - 1_000.0).abs() < 0.05, "{plain}");

        let instance = create::<Gain>(48_000.0, 512, 2);
        assert!(!instance.is_null());
        unsafe {
            assert_eq!(set_normalized(instance, 0, to_normalized::<Gain>(0, 6.0) as f32), 1);
            let mut audio = [0.25f32; 32];
            let input = audio;
            assert_eq!(
                process(
                    instance,
                    audio.as_ptr(),
                    audio.as_ptr(),
                    audio.as_mut_ptr(),
                    audio.as_mut_ptr(),
                    32
                ),
                1
            );
            let ratio = audio[0] / input[0];
            let db = 20.0 * ratio.log10();
            assert!((db - 6.0).abs() < 0.02, "{db}");
            destroy(instance);
        }
    }
}
