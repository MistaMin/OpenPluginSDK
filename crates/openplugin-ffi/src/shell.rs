//! C ABI a native VST3, Audio Unit, or AAX adapter calls.
//!
//! One plugin binary exports these symbols through `openplugin_targets::export_shell!`.
//! `create` allocates. `process` does not.
//! Parameter writes go to the plane. The audio thread applies them inside `process`.

use std::panic::AssertUnwindSafe;
use std::sync::{Mutex, OnceLock};

use openplugin_core::{
    sample_rate_supported, AaxIds, Category, EditorState, IoConfig, NotePorts, ParamFlags,
    ParamInfo, PluginInfo, PortKind, SpeakerLayout, Taper, Unit,
};

use crate::{FfiInstance, Plugin};

/// Version of the whole `op_shell_*` ABI. Additions raise it; see docs/SHELL.md.
pub const ABI: u32 = 5;
pub use crate::block::{
    OpEvent, OpMidiEvent, OpProcess, OpTransport, EVENT_MIDI, EVENT_NOTE_OFF, EVENT_NOTE_ON,
    EVENT_PARAM_NORMALIZED, EVENT_PARAM_PLAIN, MAX_EVENTS, MAX_MIDI_OUT, TRANSPORT_HAS_BEATS,
    TRANSPORT_HAS_SECONDS, TRANSPORT_HAS_SIGNATURE, TRANSPORT_HAS_TEMPO, TRANSPORT_PLAYING,
    TRANSPORT_RECORDING,
};

/// [`track_info_from_abi`] flags: the track is a return, a bus, the master.
pub const TRACK_RETURN: u32 = 1;
pub const TRACK_BUS: u32 = 2;
pub const TRACK_MASTER: u32 = 4;
/// The colour argument is valid.
pub const TRACK_HAS_COLOUR: u32 = 0x100;

pub const KIND_EFFECT: u32 = 0;
pub const KIND_INSTRUMENT: u32 = 1;
pub const KIND_NOTE_EFFECT: u32 = 2;
pub const NOTE_PORT_INPUT: u32 = 1;
pub const NOTE_PORT_OUTPUT: u32 = 2;
/// Bus kinds in [`OpBus::kind`], the order of `PortKind`.
pub const BUS_ABSENT: u32 = 0;
pub const BUS_MONO: u32 = 1;
pub const BUS_STEREO: u32 = 2;
pub const BUS_SURROUND: u32 = 3;
pub const BUS_AMBISONIC: u32 = 4;
pub const BUS_DISCRETE: u32 = 5;
/// [`OpBus::roles`] entry for a channel with no role id.
pub const ROLE_NONE: u8 = 0xFF;
/// Channels whose role [`OpBus`] reports.
pub const ROLE_SLOTS: usize = 16;
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

/// One bus of a layout. Channels are always in the SDK's canonical order:
/// `roles[i]` is the SDK role id (`SURROUND_*` in `openplugin-core`: the CLAP
/// surround ids 0..=17, then 18..=21 for top side and wide channels) of
/// channel `i`, or [`ROLE_NONE`] for mono, ambisonic, discrete buses and
/// channels past [`ROLE_SLOTS`].
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpBus {
    pub kind: u32,
    pub channels: u32,
    pub roles: [u8; ROLE_SLOTS],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpLayout {
    pub input: OpBus,
    pub output: OpBus,
    /// `kind` is [`BUS_ABSENT`] and `channels` 0 when there is no sidechain.
    pub sidechain: OpBus,
    pub name: [u8; 64],
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

// These queries build a plugin each call instead of caching in a generic
// static (which would be shared by every plugin type in a process). They run
// on the main thread, when a shell is described, never on the audio path.
fn plugin_layouts<P: Plugin + Default>() -> &'static [IoConfig] {
    P::default().supported_layouts()
}

fn plugin_note_ports<P: Plugin + Default>() -> NotePorts {
    P::default().note_ports()
}

/// What the plugin is: an instrument (`is_instrument` or the Instrument
/// category), a note effect (not an instrument, writes notes, and no layout
/// has an audio output), else an effect.
pub fn kind<P: Plugin + Default>() -> u32 {
    let plugin = P::default();
    let info = plugin.info();
    if info.is_instrument || info.category == Category::Instrument {
        KIND_INSTRUMENT
    } else if plugin_note_ports::<P>().output
        && plugin_layouts::<P>().iter().all(|l| l.output == SpeakerLayout::Absent)
    {
        KIND_NOTE_EFFECT
    } else {
        KIND_EFFECT
    }
}

/// [`NOTE_PORT_INPUT`] and [`NOTE_PORT_OUTPUT`] bits.
pub fn note_ports<P: Plugin + Default>() -> u32 {
    let ports = plugin_note_ports::<P>();
    u32::from(ports.input) * NOTE_PORT_INPUT + u32::from(ports.output) * NOTE_PORT_OUTPUT
}

pub fn layout_count<P: Plugin + Default>() -> u32 {
    plugin_layouts::<P>().len().min(u32::MAX as usize) as u32
}

fn bus_of(layout: SpeakerLayout) -> OpBus {
    let mut roles = [ROLE_NONE; ROLE_SLOTS];
    match (layout.surround_map(), layout) {
        (Some(map), _) => {
            for (slot, id) in roles.iter_mut().zip(map) {
                *slot = *id;
            }
        }
        (None, SpeakerLayout::Stereo) => {
            roles[0] = openplugin_core::layout::SURROUND_FL;
            roles[1] = openplugin_core::layout::SURROUND_FR;
        }
        _ => {}
    }
    OpBus {
        kind: match layout.kind() {
            PortKind::Empty => BUS_ABSENT,
            PortKind::Mono => BUS_MONO,
            PortKind::Stereo => BUS_STEREO,
            PortKind::Surround => BUS_SURROUND,
            PortKind::Ambisonic => BUS_AMBISONIC,
            PortKind::Discrete => BUS_DISCRETE,
        },
        channels: u32::from(layout.channels().unwrap_or(0)),
        roles,
    }
}

/// Fill `out` with layout `index`. 0 for a null `out` or an index past the end.
///
/// # Safety
/// `out` is null or writable for one [`OpLayout`].
pub unsafe fn write_layout<P: Plugin + Default>(index: u32, out: *mut OpLayout) -> u32 {
    if out.is_null() {
        return 0;
    }
    let Some(config) = plugin_layouts::<P>().get(index as usize).copied() else {
        return 0;
    };
    let mut layout = OpLayout {
        input: bus_of(config.input),
        output: bus_of(config.output),
        sidechain: bus_of(config.sidechain.unwrap_or(SpeakerLayout::Absent)),
        name: [0; 64],
    };
    put(&mut layout.name, &config.name());
    unsafe { out.write(layout) };
    1
}

/// Select layout `index` on an instance, before the host starts blocks.
/// Allocates. 0 for a null instance, a bad index, a refusal or a timeout.
///
/// # Safety
/// `ptr` is null or a live [`ShellInstance`].
pub unsafe fn set_layout(ptr: *mut ShellInstance, index: u32) -> u32 {
    if ptr.is_null() {
        return 0;
    }
    let done = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe { (*ptr).ffi.select_layout(index) }));
    u32::from(matches!(done, Ok(true)))
}

/// # Safety
/// `ptr` is null or a live [`ShellInstance`].
pub unsafe fn layout_selected(ptr: *mut ShellInstance) -> u32 {
    if ptr.is_null() {
        0
    } else {
        unsafe { (*ptr).ffi.layout_index() }
    }
}

/// A [`TrackInfo`](openplugin_core::TrackInfo) from the ABI's arguments: `name` is a
/// NUL-terminated UTF-8 string (null or empty for none; invalid bytes are replaced; at
/// most 255 characters are kept), `rgba` is `0xRRGGBBAA` and counts only with
/// [`TRACK_HAS_COLOUR`] in `flags`, and the kind comes from the low bits of `flags`.
///
/// # Safety
/// `name` is null or a NUL-terminated string.
pub unsafe fn track_info_from_abi(name: *const std::ffi::c_char, rgba: u32, flags: u32) -> openplugin_core::TrackInfo {
    let name = if name.is_null() {
        None
    } else {
        let text = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy();
        Some(text.chars().take(255).collect::<String>()).filter(|n| !n.is_empty())
    };
    openplugin_core::TrackInfo {
        name,
        colour: (flags & TRACK_HAS_COLOUR != 0).then_some(rgba),
        kind: flags & (TRACK_RETURN | TRACK_BUS | TRACK_MASTER),
    }
}

/// Tell the plugin about the host's track. Message thread; waits for a running
/// block like a state call. Returns 1 when delivered, 0 for a null instance or when
/// the plugin could not be taken in time.
///
/// # Safety
/// `ptr` is null or a live [`ShellInstance`]; `name` is null or NUL-terminated.
pub unsafe fn set_track_info(ptr: *mut ShellInstance, name: *const std::ffi::c_char, rgba: u32, flags: u32) -> u32 {
    if ptr.is_null() {
        return 0;
    }
    let info = unsafe { track_info_from_abi(name, rgba, flags) };
    u32::from(unsafe { (*ptr).ffi.set_track_info(&info) })
}

/// Offline (bounce) flag; applied at the start of the next block.
///
/// # Safety
/// `ptr` is null or a live [`ShellInstance`].
pub unsafe fn set_offline(ptr: *mut ShellInstance, offline: u32) {
    if !ptr.is_null() {
        unsafe { (*ptr).ffi.set_offline_flag(offline != 0) };
    }
}

/// # Safety
/// `ptr` is null or a live [`ShellInstance`]; `args` is null or a valid [`OpProcess`].
pub unsafe fn process_events(ptr: *mut ShellInstance, args: *const OpProcess) -> u32 {
    if ptr.is_null() || args.is_null() {
        return 0;
    }
    let ran = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        (*ptr).ffi.process_block(&*args)
    }));
    if ran.is_err() {
        unsafe { silence_outputs(&*args) };
    }
    u32::from(matches!(ran, Ok(true)))
}

/// After a panic in the plugin: write silence to every output channel, so the host does not
/// play half a block. The block was validated before it reached the plugin, so the pointers
/// and counts are the host's.
///
/// # Safety
/// `args` is the block the host passed to `process_events`.
unsafe fn silence_outputs(args: &OpProcess) {
    if args.outputs.is_null() || args.output_channels as usize > 64 {
        return;
    }
    for channel in 0..args.output_channels as usize {
        let dst = unsafe { *args.outputs.add(channel) };
        if !dst.is_null() {
            unsafe { std::ptr::write_bytes(dst, 0, args.frames as usize) };
        }
    }
}

/// # Safety
/// `ptr` is null or a live [`ShellInstance`]; `out` is null or writable for `capacity` events.
pub unsafe fn midi_out(ptr: *mut ShellInstance, out: *mut OpMidiEvent, capacity: u32) -> u32 {
    if ptr.is_null() {
        return 0;
    }
    let got = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        (*ptr).ffi.pull_midi_out(out, capacity)
    }));
    got.unwrap_or(0)
}

/// # Safety
/// `ptr` is null or a live [`ShellInstance`].
pub unsafe fn midi_dropped(ptr: *const ShellInstance) -> u32 {
    if ptr.is_null() {
        0
    } else {
        unsafe { (*ptr).ffi.take_midi_dropped() }
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

/// Fill `out` with the plugin identity. Returns 1, or 0 when `out` is null.
///
/// # Safety
/// `out` is null or points to writable memory for one [`OpShellInfo`].
/// Writes happen only after the null check.
pub unsafe fn write_info<P: Plugin + Default>(out: *mut OpShellInfo) -> u32 {
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

/// Fill `out` with parameter `index`. Returns 1, or 0 when `out` is null or
/// `index` is past the end.
///
/// # Safety
/// `out` is null or points to writable memory for one [`OpParamDesc`].
/// Writes happen only after the null check and the index lookup.
pub unsafe fn write_param<P: Plugin + Default>(index: u32, out: *mut OpParamDesc) -> u32 {
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

/// Longest text [`param_parse`] reads, in bytes. Longer input is refused.
pub const PARSE_MAX: usize = 1024;

fn display_value(param: &ParamInfo, plain: f64) -> f32 {
    if plain.is_finite() { plain as f32 } else { param.default }
}

/// The plugin's own text for the plain value `plain` of parameter `index`
/// (`ParamInfo::format`), as UTF-8 plus a NUL in `buf` (room for `cap` bytes).
/// Returns the length of the whole text in bytes, not counting the NUL; the
/// text was cut when that is `cap` or more. A cut falls on a UTF-8 boundary
/// and the NUL is always written when `buf` is set and `cap > 0`. Returns 0
/// for an unknown `index` or a panic (and writes an empty string); `buf` NULL
/// writes nothing, so `buf` NULL with `cap` 0 asks for the length. A
/// non-finite `plain` shows the default. Main thread: it allocates.
///
/// # Safety
/// `buf` is null or points to `cap` writable bytes.
pub unsafe fn param_format<P: Plugin + Default>(index: u32, plain: f64, buf: *mut u8, cap: usize) -> usize {
    let text = std::panic::catch_unwind(AssertUnwindSafe(|| {
        plugin_info::<P>().params.get(index as usize).map(|param| param.format(display_value(param, plain)))
    }))
    .ok()
    .flatten()
    .unwrap_or_default();
    unsafe { copy_text(&text, buf, cap) }
}

/// Copies `text` and a NUL into `buf` (room for `cap` bytes), cut on a UTF-8
/// boundary, and returns `text.len()`. Writes nothing when `buf` is NULL or
/// `cap` is 0.
///
/// # Safety
/// `buf` is null or points to `cap` writable bytes.
unsafe fn copy_text(text: &str, buf: *mut u8, cap: usize) -> usize {
    if !buf.is_null() && cap > 0 {
        let mut count = text.len().min(cap - 1);
        while !text.is_char_boundary(count) {
            count -= 1;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(text.as_ptr(), buf, count);
            *buf.add(count) = 0;
        }
    }
    text.len()
}

// ---- ABI 5: parameter groups ----------------------------------------------

/// The group path of parameter `index` (`ParamInfo::host_group`: names
/// separated by `/`, outermost first, `""` for the top level and for a path
/// `check_group` refuses) as UTF-8 plus a NUL in `buf` (room for `cap`
/// bytes). Returns the length of the whole path in bytes, not counting the
/// NUL; it was cut when that is `cap` or more. A cut falls on a UTF-8
/// boundary and the NUL is always written when `buf` is set and `cap > 0`.
/// `buf` NULL with `cap` 0 asks for the length. An unknown `index` (or a
/// panic) returns 0 and writes an empty string. Does not allocate after the
/// first descriptor read; any thread.
///
/// # Safety
/// `buf` is null or points to `cap` writable bytes.
pub unsafe fn param_group<P: Plugin + Default>(index: u32, buf: *mut u8, cap: usize) -> usize {
    let group = std::panic::catch_unwind(AssertUnwindSafe(|| {
        plugin_info::<P>().params.get(index as usize).map_or("", |param| param.host_group())
    }))
    .unwrap_or("");
    unsafe { copy_text(group, buf, cap) }
}

/// Reads `text` (NUL-terminated UTF-8) as a value of parameter `index`
/// (`ParamInfo::parse`: units, choice labels, on/off, clamped) and stores the
/// plain value in `*out_plain`. Returns 1, or 0 and leaves `*out_plain`
/// alone for a NULL pointer, an unknown `index`, text that is not UTF-8 or is
/// longer than [`PARSE_MAX`] bytes, text that is not a value, or a panic.
/// Main thread.
///
/// # Safety
/// `text` is null or a NUL-terminated string; `out_plain` is null or writable.
pub unsafe fn param_parse<P: Plugin + Default>(index: u32, text: *const std::ffi::c_char, out_plain: *mut f64) -> i32 {
    if text.is_null() || out_plain.is_null() {
        return 0;
    }
    let bytes = unsafe { std::ffi::CStr::from_ptr(text) }.to_bytes();
    if bytes.len() > PARSE_MAX {
        return 0;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return 0;
    };
    let parsed = std::panic::catch_unwind(AssertUnwindSafe(|| {
        plugin_info::<P>().params.get(index as usize).and_then(|param| param.parse(text))
    }));
    match parsed {
        Ok(Some(value)) if value.is_finite() => {
            unsafe { *out_plain = f64::from(value) };
            1
        }
        _ => 0,
    }
}

pub fn create<P: Plugin + Default>(rate: f64, max_block: u32, channels: u32) -> *mut ShellInstance {
    let created = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if !sample_rate_supported(rate) {
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

/// [`save_state`] result: the state was written.
pub const STATE_OK: u32 = 1;
/// [`save_state`] result: the buffer is too small; `needed` holds the size.
pub const STATE_TOO_SMALL: u32 = 2;
/// Largest state a call hands across the ABI (sizes are `u32`).
pub const STATE_MAX: usize = u32::MAX as usize;

/// Save the whole plugin state (an OPCH session: parameters plus the
/// plugin's own chunk) into `out`.
///
/// Returns [`STATE_OK`] with the length in `*needed` after writing that many
/// bytes at `out`; [`STATE_TOO_SMALL`] with the required length in `*needed`
/// and nothing written when `capacity` is short (a null `out` with capacity 0
/// is the size query); 0 for a null instance, a null `out` with capacity above
/// 0, a plugin that panics, a state above 4 GiB, or a block that did not finish
/// within [`crate::STATE_WAIT`]. `needed` may be null.
///
/// Main thread. Allocates. Waits at most [`crate::STATE_WAIT`] for a block in
/// flight; blocks that start meanwhile are skipped (dry). The state can change
/// between a size query and the copy: loop until the call returns OK.
///
/// # Safety
/// `ptr` is null or a live [`ShellInstance`]. `out` is null or writable for
/// `capacity` bytes. `needed` is null or writable.
pub unsafe fn save_state(ptr: *mut ShellInstance, out: *mut u8, capacity: u32, needed: *mut u32) -> u32 {
    if ptr.is_null() || (out.is_null() && capacity > 0) {
        return 0;
    }
    let saved = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe { (*ptr).ffi.try_save_bytes() }));
    let Ok(Some(bytes)) = saved else { return 0 };
    if bytes.len() > STATE_MAX {
        return 0;
    }
    let length = bytes.len() as u32;
    if !needed.is_null() {
        unsafe { *needed = length };
    }
    if length > capacity {
        return STATE_TOO_SMALL;
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    STATE_OK
}

/// Restore a state written by [`save_state`] (or a bare OPST parameter
/// plane). Returns 1 on success. Returns 0 and changes nothing for a null
/// instance, a null `data`, a zero `length`, a corrupt, truncated or foreign
/// blob, a chunk the plugin refuses, or a block that did not finish within
/// [`crate::STATE_WAIT`].
///
/// Main thread. Allocates. Same threading rule as [`save_state`].
///
/// # Safety
/// `ptr` is null or a live [`ShellInstance`]. `data` is null or readable for
/// `length` bytes.
pub unsafe fn load_state(ptr: *mut ShellInstance, data: *const u8, length: u32) -> u32 {
    if ptr.is_null() || data.is_null() || length == 0 {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, length as usize) };
    let loaded = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe { (*ptr).ffi.load_bytes(bytes) }));
    u32::from(matches!(loaded, Ok(true)))
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
    if frames > 0 && (in_left.is_null() || out_left.is_null()) {
        return 0;
    }
    let tracing = trace::on();
    let input = if tracing { unsafe { trace::peak(in_left, frames).max(trace::peak(in_right, frames)) } } else { 0.0 };
    let inputs = [in_left, in_right];
    let outputs = [out_left, out_right];
    let args = OpProcess {
        struct_size: std::mem::size_of::<OpProcess>() as u32,
        frames,
        inputs: inputs.as_ptr(),
        input_channels: if in_right.is_null() { 1 } else { 2 },
        output_channels: if out_right.is_null() { 1 } else { 2 },
        outputs: outputs.as_ptr(),
        sidechain: std::ptr::null(),
        sidechain_channels: 0,
        event_count: 0,
        events: std::ptr::null(),
        transport: std::ptr::null(),
    };
    let ran = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe { (*ptr).ffi.process_block(&args) }));
    if ran.is_err() {
        unsafe { silence_outputs(&args) };
    }
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

    static CHOICES: [&str; 3] = ["Raum", "Größe ✓", "Plate"];
    static PARAMS: [ParamInfo; 4] = [
        ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
        ParamInfo::new("freq", "Freq", 20.0, 20_000.0, 1_000.0, Unit::Hz).with_taper(Taper::Log).with_group("Filter"),
        ParamInfo::new("mode", "Mode", 0.0, 2.0, 0.0, Unit::Choice).with_choices(&CHOICES).with_group("Filter/Größe"),
        ParamInfo::new("warm", "Warm", 0.0, 1.0, 0.0, Unit::Toggle).with_group("Filter//Bad"),
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

    static HEARD: Mutex<Vec<openplugin_core::TrackInfo>> = Mutex::new(Vec::new());

    /// The gain plugin, listening for the track.
    #[derive(Default)]
    struct Tracked(Gain);

    impl Plugin for Tracked {
        fn info(&self) -> &'static PluginInfo {
            self.0.info()
        }
        fn prepare(&mut self, rate: f32, block: usize, channels: usize) {
            self.0.prepare(rate, block, channels)
        }
        fn reset(&mut self) {}
        fn set_param(&mut self, id: &str, value: f32) -> bool {
            self.0.set_param(id, value)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.0.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.0.process(io, frames)
        }
        fn track_info_changed(&mut self, info: &openplugin_core::TrackInfo) {
            HEARD.lock().unwrap().push(info.clone());
        }
    }

    #[test]
    fn track_info_crosses_the_abi_and_reaches_the_plugin() {
        const { assert!(ABI >= 4, "set_track_info is ABI 4") };
        let instance = create::<Tracked>(48_000.0, 64, 2);
        assert!(!instance.is_null());
        // Name, colour as 0xRRGGBBAA and the kind bits arrive as given.
        let name = std::ffi::CString::new("Lead Vox \u{266a}").unwrap();
        assert_eq!(unsafe { set_track_info(instance, name.as_ptr(), 0x11_22_33_ff, TRACK_HAS_COLOUR | TRACK_BUS) }, 1);
        // No colour flag: the colour argument is ignored. A null or empty name is no name.
        assert_eq!(unsafe { set_track_info(instance, std::ptr::null(), 0xdead_beef, TRACK_RETURN | TRACK_MASTER) }, 1);
        let empty = std::ffi::CString::new("").unwrap();
        assert_eq!(unsafe { set_track_info(instance, empty.as_ptr(), 0, 0xffff_ffff) }, 1);
        // A null instance delivers nothing.
        assert_eq!(unsafe { set_track_info(std::ptr::null_mut(), name.as_ptr(), 0, 0) }, 0);
        // Bytes that are not UTF-8 are replaced, and a very long name is cut.
        let bad = std::ffi::CString::new(vec![b'a', 0xff, b'b']).unwrap();
        assert_eq!(unsafe { set_track_info(instance, bad.as_ptr(), 0, 0) }, 1);
        let long = std::ffi::CString::new("x".repeat(5000)).unwrap();
        assert_eq!(unsafe { set_track_info(instance, long.as_ptr(), 0, 0) }, 1);
        unsafe { destroy(instance) };
        let heard = HEARD.lock().unwrap().clone();
        assert_eq!(heard.len(), 5);
        assert_eq!(heard[0].name.as_deref(), Some("Lead Vox \u{266a}"));
        assert_eq!(heard[0].colour, Some(0x1122_33ff));
        assert!(heard[0].is_bus() && !heard[0].is_return());
        assert_eq!((heard[1].name.clone(), heard[1].colour), (None, None));
        assert!(heard[1].is_return() && heard[1].is_master());
        assert_eq!(heard[2].name, None);
        assert_eq!(heard[2].kind, openplugin_core::TrackInfo::IS_RETURN | openplugin_core::TrackInfo::IS_BUS | openplugin_core::TrackInfo::IS_MASTER);
        assert_eq!(heard[3].name.as_deref(), Some("a\u{fffd}b"));
        assert_eq!(heard[4].name.as_ref().map(String::len), Some(255));
    }

    #[test]
    fn shell_describes_a_stable_identity_and_a_log_taper() {
        let mut info = unsafe { std::mem::zeroed::<OpShellInfo>() };
        assert_eq!(unsafe { write_info::<Gain>(&mut info) }, 1);
        assert_eq!(info.abi, ABI);
        assert_eq!(info.param_count, 4);
        assert_ne!(info.processor_cid, [0; 16]);
        assert_ne!(info.processor_cid, info.controller_cid);
        assert_ne!(info.plugin_id_mono, info.plugin_id_stereo);
    }

    #[test]
    fn shell_identity_fields_are_nonzero() {
        let mut info = unsafe { std::mem::zeroed::<OpShellInfo>() };
        assert_eq!(unsafe { write_info::<Gain>(&mut info) }, 1);
        assert_ne!(info.manufacturer_id, 0);
        assert_ne!(info.product_id, 0);
        assert_ne!(info.plugin_id_mono, 0);
        assert_ne!(info.plugin_id_stereo, 0);
        let mut again = unsafe { std::mem::zeroed::<OpShellInfo>() };
        assert_eq!(unsafe { write_info::<Gain>(&mut again) }, 1);
        assert_eq!(info.processor_cid, again.processor_cid);
        assert!(info.name.starts_with(b"Gain"));
        assert!(info.category.starts_with(b"Fx|Tools"));
    }

    #[test]
    fn shell_log_roundtrip_and_six_db_inplace() {
        let mut desc = unsafe { std::mem::zeroed::<OpParamDesc>() };
        assert_eq!(unsafe { write_param::<Gain>(1, &mut desc) }, 1);
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

    fn format_text<P: Plugin + Default>(index: u32, plain: f64) -> String {
        let mut buf = [0xAAu8; 128];
        let need = unsafe { param_format::<P>(index, plain, buf.as_mut_ptr(), buf.len()) };
        assert!(need < buf.len());
        assert_eq!(buf[need], 0);
        String::from_utf8(buf[..need].to_vec()).unwrap()
    }

    fn parse_text<P: Plugin + Default>(index: u32, text: &str) -> Option<f64> {
        let c = std::ffi::CString::new(text).unwrap();
        let mut out = -777.0;
        match unsafe { param_parse::<P>(index, c.as_ptr(), &mut out) } {
            1 => Some(out),
            _ => {
                assert_eq!(out, -777.0, "a refused parse must not write");
                None
            }
        }
    }

    #[test]
    fn param_text_matches_param_info_and_round_trips() {
        assert_eq!(format_text::<Gain>(0, 6.0), "6.0 dB");
        assert_eq!(format_text::<Gain>(1, 1_500.0), "1.50 kHz");
        let n = to_normalized::<Gain>(0, 6.0);
        assert!((n - 0.625).abs() < 1e-6);
        assert_eq!(parse_text::<Gain>(0, "6 dB"), Some(6.0));
        assert_eq!(parse_text::<Gain>(1, "1.5 kHz"), Some(1_500.0));
        assert_eq!(parse_text::<Gain>(0, "99 dB"), Some(24.0), "parse clamps");
        for (index, value) in [(2u32, 0.0), (2, 1.0), (2, 2.0), (3, 0.0), (3, 1.0)] {
            let text = format_text::<Gain>(index, value);
            assert_eq!(parse_text::<Gain>(index, &text), Some(value), "{text}");
        }
        assert_eq!(format_text::<Gain>(2, 1.0), "Größe ✓");
        assert_eq!(format_text::<Gain>(3, 1.0), "On");
        assert_eq!(parse_text::<Gain>(2, "plate"), Some(2.0));
    }

    #[test]
    fn param_format_truncates_on_a_utf8_boundary_and_reports_the_length() {
        let full = "Größe ✓";
        assert_eq!(full.len(), 11);
        let mut buf = [0xAAu8; 32];
        unsafe {
            assert_eq!(param_format::<Gain>(2, 1.0, std::ptr::null_mut(), 0), 11);
            assert_eq!(param_format::<Gain>(2, 1.0, std::ptr::null_mut(), 8), 11);
            assert_eq!(param_format::<Gain>(2, 1.0, buf.as_mut_ptr(), 0), 11);
            assert_eq!(buf[0], 0xAA);
            for cap in 1..=14usize {
                buf.fill(0xAA);
                assert_eq!(param_format::<Gain>(2, 1.0, buf.as_mut_ptr(), cap), 11, "cap {cap}");
                let nul = buf.iter().position(|b| *b == 0).expect("NUL");
                assert!(nul < cap, "cap {cap}");
                let shown = std::str::from_utf8(&buf[..nul]).expect("valid UTF-8");
                assert!(full.starts_with(shown), "cap {cap}: {shown:?}");
                if cap >= 12 {
                    assert_eq!(shown, full);
                }
                assert!(buf[cap..].iter().all(|b| *b == 0xAA), "wrote past cap {cap}");
            }
            // "Gr" then a two-byte letter: three text bytes would split it.
            buf.fill(0xAA);
            param_format::<Gain>(2, 1.0, buf.as_mut_ptr(), 4);
            assert_eq!(&buf[..4], &[b'G', b'r', 0, 0xAA]);
        }
    }

    #[test]
    fn param_group_reports_the_path_and_truncates_on_a_utf8_boundary() {
        let group = |index: u32| {
            let mut buf = [0xAAu8; 64];
            let n = unsafe { param_group::<Gain>(index, buf.as_mut_ptr(), buf.len()) };
            assert_eq!(buf[n], 0, "NUL after the path");
            String::from_utf8(buf[..n].to_vec()).unwrap()
        };
        assert_eq!(group(0), "", "no group is the top level");
        assert_eq!(group(1), "Filter");
        assert_eq!(group(2), "Filter/Größe");
        assert_eq!(group(3), "", "an invalid path is the top level");
        unsafe {
            // "Filter/Größe" is 14 bytes; ö is two bytes at 9..11.
            assert_eq!(param_group::<Gain>(2, std::ptr::null_mut(), 0), 14);
            let mut buf = [0xAAu8; 16];
            assert_eq!(param_group::<Gain>(2, buf.as_mut_ptr(), 0), 14);
            assert_eq!(buf[0], 0xAA, "cap 0 writes nothing");
            assert_eq!(param_group::<Gain>(2, buf.as_mut_ptr(), 11), 14);
            assert_eq!(&buf[..10], b"Filter/Gr\0", "the cut does not split the two bytes of ö");
            assert_eq!(param_group::<Gain>(9, buf.as_mut_ptr(), buf.len()), 0);
            assert_eq!(buf[0], 0, "an unknown index writes an empty string");
            assert_eq!(param_group::<Gain>(u32::MAX, std::ptr::null_mut(), 0), 0);
        }
    }

    #[test]
    fn param_text_refuses_hostile_input_without_panicking() {
        let mut buf = [0xAAu8; 8];
        unsafe {
            assert_eq!(param_format::<Gain>(9, 0.0, buf.as_mut_ptr(), buf.len()), 0);
            assert_eq!(buf[0], 0);
            assert_eq!(param_format::<Gain>(u32::MAX, 0.0, std::ptr::null_mut(), 0), 0);
        }
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -1e300] {
            let _ = format_text::<Gain>(0, v);
        }
        assert_eq!(format_text::<Gain>(0, f64::NAN), "0.0 dB");
        let mut out = -777.0;
        let ok = std::ffi::CString::new("6 dB").unwrap();
        unsafe {
            assert_eq!(param_parse::<Gain>(0, std::ptr::null(), &mut out), 0);
            assert_eq!(param_parse::<Gain>(0, ok.as_ptr(), std::ptr::null_mut()), 0);
            assert_eq!(param_parse::<Gain>(7, ok.as_ptr(), &mut out), 0);
            assert_eq!(param_parse::<Gain>(u32::MAX, ok.as_ptr(), &mut out), 0);
            let bad = [0xFFu8, 0xFE, 0xC3, 0x28, 0];
            assert_eq!(param_parse::<Gain>(0, bad.as_ptr().cast(), &mut out), 0);
            let long = std::ffi::CString::new("9".repeat(PARSE_MAX + 1)).unwrap();
            assert_eq!(param_parse::<Gain>(0, long.as_ptr(), &mut out), 0);
        }
        assert_eq!(out, -777.0);
        for text in ["", "   ", "loud", "NaN", "inf", "1e999", "--3", "3 parsecs", "\u{1F4A5}", "\t\n"] {
            let got = parse_text::<Gain>(0, text);
            assert!(got.is_none_or(|v| v.is_finite() && (-24.0..=24.0).contains(&v)), "{text:?} {got:?}");
        }
    }
}
