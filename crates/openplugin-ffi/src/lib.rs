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
//! inside `process_*`. `prepare` stays exclusive with `process`.
//!
//! # Session state and the audio thread
//!
//! [`FfiInstance::save_bytes`] and [`FfiInstance::load_bytes`] (and their
//! `try_` forms) need `&mut` access to the plugin object, which the audio
//! thread uses inside `process_*`. A one-word gate arbitrates, and the audio
//! thread never waits on it:
//!
//! - `process_*` and `reset` take the gate with one compare-exchange. If a
//!   state call holds or has requested the gate, the block is **skipped**: the
//!   input is copied to the output (dry), the plugin is not entered, and the
//!   call still returns success. A `reset` that cannot enter is remembered and
//!   runs at the start of the next block that can.
//! - A state call (message thread) serialises with other state calls on a
//!   mutex, raises a request bit, then waits for a block in flight to finish.
//!   The wait is bounded by [`STATE_WAIT`]; on timeout the call fails and
//!   changes nothing. New blocks see the request bit and skip, so the wait
//!   cannot be starved.
//!
//! The request bit and the in-flight bit are one atomic, so for any pair of
//! (block, state call) either the block sees the request and skips, or the
//! state call sees the block and waits. The audio side allocates nothing.
#![allow(unsafe_code)]

use std::cell::UnsafeCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use openplugin_core::{
    decode_session, encode_session, release_momentary, sample_rate_supported, BusMeters,
    DenormalGuard, EditorState, HostNotifier, HostSpace, MeterFrame, ParamCursor, ParamFlags, ParamInfo, ParamPlane,
    PluginInfo, Seqlock, Tail, Taper, Unit, MAX_AUDIO_CHANNELS,
};

pub use openplugin_core::Plugin;

pub mod block;
pub mod shell;

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
    /// [`ParamFlags::bits`].
    pub flags: u32,
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
        is_discrete: steps > 1 || param.flags.contains(ParamFlags::STEPPED),
        steps,
        is_log: param.taper == Taper::Log,
        unit: param.unit,
        flags: param.flags.bits(),
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
    sidechain: Vec<Vec<f32>>,
    cursor: ParamCursor,
    applied_offline: Option<bool>,
    channels: usize,
    side_channels: usize,
    max_block: usize,
    rate: f32,
    /// Events of the block in flight, sorted by offset. Capacity fixed at creation.
    events: Vec<block::OpEvent>,
    /// MIDI the plugin produced in the last block. Capacity fixed at creation.
    midi_out: Vec<block::OpMidiEvent>,
    /// Entries of `midi_out` already handed to the host.
    midi_read: usize,
    /// Index into `supported_layouts` last selected.
    layout_index: u32,
    /// [`Plugin::note_expressions`], read once at creation.
    expressions: openplugin_core::ExpressionSet,
    /// The MPE receiver of a plugin that asked for MPE ([`Plugin::mpe_input`]):
    /// note and raw MIDI events go through it.
    mpe: Option<openplugin_core::MpeReceiver>,
}

/// A tail in seconds: infinity for an endless tail, zero for none or unknown.
fn tail_seconds_of(tail: Tail, rate: f32) -> f64 {
    match tail {
        Tail::Infinite => f64::INFINITY,
        Tail::Finite(samples) => f64::from(samples) / f64::from(rate.max(1.0)),
        Tail::None | Tail::Unknown => 0.0,
    }
}

/// Longest a state call waits for a block in flight before it gives up.
pub const STATE_WAIT: Duration = Duration::from_secs(2);

const GATE_AUDIO: u32 = 1;
const GATE_STATE: u32 = 2;

/// Releases the audio side of the gate when a block (or reset) ends. While it
/// lives the thread runs with denormals flushed ([`Plugin::flush_denormals`]);
/// the host's mode comes back when it drops, at the end of the call or while
/// a panic unwinds out of the plugin.
struct AudioGuard<'a> {
    gate: &'a AtomicU32,
    _flush: DenormalGuard,
}

impl Drop for AudioGuard<'_> {
    fn drop(&mut self) {
        self.gate.fetch_and(!GATE_AUDIO, Ordering::SeqCst);
    }
}

/// Drops the state request when a state call ends, panic included.
struct StateGuard<'a>(&'a AtomicU32);

impl Drop for StateGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_and(!GATE_STATE, Ordering::SeqCst);
    }
}

/// One host instance. Message-thread calls use the plane and the atomics.
/// The audio thread is the only caller that enters `audio`.
pub struct FfiInstance {
    audio: UnsafeCell<Audio>,
    plane: ParamPlane,
    meters: Seqlock<MeterFrame>,
    bus_meters: Seqlock<BusMeters>,
    latency: AtomicU32,
    /// The latency [`FfiInstance::latency`] last returned: what the host knows.
    told_latency: AtomicU32,
    /// Set when a block, a reset or a state call saw [`Plugin::latency`] move.
    latency_moved: AtomicBool,
    /// The plugin's [`HostNotifier`] (a clone of the one it was given).
    notifier: HostNotifier,
    offline: AtomicU32,
    preset: AtomicI32,
    info: &'static PluginInfo,
    presets: Vec<PresetEntry>,
    /// [`Plugin::editor_state`], taken once at creation.
    editor_state: Option<EditorState>,
    /// [`Plugin::state_is_parameters_only`], read once: save and load then use
    /// the parameter plane alone and never take the gate.
    state_in_params: bool,
    /// Seconds of tail as `f64` bits, refreshed after every block.
    tail: AtomicU64,
    /// `GATE_AUDIO` while a block runs, `GATE_STATE` while a state call holds
    /// or has asked for the plugin object.
    gate: AtomicU32,
    /// Serialises state calls with each other (message thread only).
    state_lock: Mutex<()>,
    /// A `reset` that found the gate busy; the next block performs it.
    reset_pending: AtomicBool,
    /// MIDI output events lost to a full buffer since the last read.
    midi_dropped: AtomicU32,
    /// [`Plugin::flush_denormals`], read once at creation: every call that
    /// enters the audio side (blocks, `reset`, MIDI out) runs flushed.
    flush_denormals: bool,
}

unsafe impl Send for FfiInstance {}
unsafe impl Sync for FfiInstance {}

impl FfiInstance {
    /// Builds a host instance.
    ///
    /// This constructor cannot fail, so an invalid or out-of-range
    /// `sample_rate` is replaced with 48 kHz. [`FfiInstance::prepare`]
    /// rejects such a rate instead.
    pub fn new(
        mut plugin: Box<dyn Plugin>,
        sample_rate: f64,
        max_block: usize,
        channels: usize,
    ) -> Self {
        let notifier = HostNotifier::new();
        plugin.set_host_notifier(notifier.clone());
        let info = plugin.info();
        let presets = collect_presets(plugin.as_ref());
        let editor_state = plugin.editor_state();
        let state_in_params = plugin.state_is_parameters_only();
        let flush_denormals = plugin.flush_denormals();
        let plane = ParamPlane::new(info);
        plane.seed_from(plugin.as_ref());
        let max_block = max_block.clamp(1, 65_536);
        let channels = channels.clamp(1, MAX_AUDIO_CHANNELS);
        let side_channels = plugin
            .supported_layouts()
            .iter()
            .map(|layout| layout.sidechain_channels())
            .max()
            .unwrap_or(0)
            .min(MAX_AUDIO_CHANNELS);
        let rate = finite_rate(sample_rate);
        plugin.prepare(rate, max_block, channels);
        let mut cursor = ParamCursor::new(plane.len());
        cursor.pull(&plane, plugin.as_mut());
        let latency = plugin.latency() as u32;
        let tail = tail_seconds_of(plugin.tail(), rate);
        let expressions = plugin.note_expressions();
        let mpe = openplugin_core::MpeReceiver::for_plugin(plugin.as_ref());
        Self {
            audio: UnsafeCell::new(Audio {
                inner: plugin,
                buffers: (0..channels).map(|_| vec![0.0; max_block]).collect(),
                sidechain: (0..side_channels).map(|_| vec![0.0; max_block]).collect(),
                cursor,
                applied_offline: None,
                channels,
                side_channels,
                max_block,
                rate,
                events: Vec::with_capacity(block::MAX_EVENTS),
                midi_out: Vec::with_capacity(block::MAX_MIDI_OUT),
                midi_read: 0,
                layout_index: 0,
                expressions,
                mpe,
            }),
            plane,
            meters: Seqlock::new(MeterFrame::SILENT),
            bus_meters: Seqlock::new(BusMeters::SILENT),
            latency: AtomicU32::new(latency),
            told_latency: AtomicU32::new(latency),
            latency_moved: AtomicBool::new(false),
            notifier,
            offline: AtomicU32::new(0),
            preset: AtomicI32::new(-1),
            info,
            presets,
            editor_state,
            state_in_params,
            tail: AtomicU64::new(tail.to_bits()),
            gate: AtomicU32::new(0),
            state_lock: Mutex::new(()),
            reset_pending: AtomicBool::new(false),
            midi_dropped: AtomicU32::new(0),
            flush_denormals,
        }
    }

    /// Audio side of the gate. `None` when a state call holds or wants the
    /// plugin object (or another block is somehow in flight): the caller
    /// skips. Never blocks, never allocates. Sets flush-to-zero for as long as
    /// the guard lives. Runs a deferred reset first.
    fn enter_audio(&self) -> Option<AudioGuard<'_>> {
        self.gate
            .compare_exchange(0, GATE_AUDIO, Ordering::SeqCst, Ordering::SeqCst)
            .ok()?;
        let guard = AudioGuard { gate: &self.gate, _flush: DenormalGuard::new_if(self.flush_denormals) };
        if self.reset_pending.swap(false, Ordering::AcqRel) {
            let audio = unsafe { &mut *self.audio.get() };
            audio.inner.reset();
            for buffer in &mut audio.buffers {
                buffer.fill(0.0);
            }
        }
        Some(guard)
    }

    /// Run `f` with exclusive access to the audio state. Message thread.
    /// `None` when no block finished within [`STATE_WAIT`]; `f` did not run.
    fn with_exclusive<T>(&self, f: impl FnOnce(&mut Audio) -> T) -> Option<T> {
        let _serial = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.gate.fetch_or(GATE_STATE, Ordering::SeqCst);
        let _request = StateGuard(&self.gate);
        let deadline = Instant::now() + STATE_WAIT;
        let mut spins = 0u32;
        while self.gate.load(Ordering::SeqCst) & GATE_AUDIO != 0 {
            if Instant::now() >= deadline {
                return None;
            }
            spins += 1;
            if spins < 64 {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
        }
        Some(f(unsafe { &mut *self.audio.get() }))
    }

    /// What the engine shares with its editor ([`Plugin::editor_state`]).
    pub fn editor_state(&self) -> Option<EditorState> {
        self.editor_state.clone()
    }

    /// How long the output rings after the input stops, in seconds, as of
    /// the last block. Infinity for an endless tail. Any thread; never blocks.
    pub fn tail_seconds(&self) -> f64 {
        f64::from_bits(self.tail.load(Ordering::Acquire))
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

    /// Stores the value a VST3, Audio Unit or AAX host sees ([`HostSpace::SHELLS`]): plain units,
    /// or normalized travel for a parameter shown normalized there. Does not allocate.
    pub fn store_host(&self, index: usize, value: f32) -> bool {
        let Some(param) = self.info.params.get(index) else {
            return false;
        };
        self.plane.store(index, param.from_host(HostSpace::SHELLS, value))
    }

    /// The value a VST3, Audio Unit or AAX host sees ([`FfiInstance::store_host`]).
    pub fn load_host(&self, index: usize) -> f32 {
        let value = self.load_param(index);
        self.info.params.get(index).map_or(value, |param| param.to_host(HostSpace::SHELLS, value))
    }

    /// Plain value from normalized 0..1 travel, using the parameter taper.
    /// The audio thread applies it on the next `process_*`. Does not allocate.
    pub fn store_normalized(&self, index: usize, normalized: f32) -> bool {
        let Some(param) = self.info.params.get(index) else {
            return false;
        };
        let travel = if normalized.is_finite() {
            normalized
        } else {
            param.to_normalized(param.default)
        };
        self.plane.store(index, param.from_normalized(travel))
    }

    pub fn load_normalized(&self, index: usize) -> f32 {
        let Some(param) = self.info.params.get(index) else {
            return 0.0;
        };
        param.to_normalized(self.load_param(index))
    }

    /// The latency as of the last block (or `prepare`, `reset`, a state
    /// load). This is the host's read: the value is remembered as the one the
    /// host knows, so [`FfiInstance::take_changes`] does not report it again.
    /// Any thread; never blocks.
    pub fn latency(&self) -> u32 {
        let now = self.latency.load(Ordering::Acquire);
        self.told_latency.store(now, Ordering::Release);
        now
    }

    /// What changed since the last call, as [`HostNotifier`] bits: the
    /// plugin's own [`HostNotifier`] calls, plus [`HostNotifier::LATENCY`]
    /// when a block saw [`Plugin::latency`] move. A latency equal to the one
    /// the host last read through [`FfiInstance::latency`] is not reported,
    /// so a new engine that comes up with the latency the host already
    /// knows is quiet. Each change is reported once. Any thread (the shells
    /// call it on their main thread); never blocks, never allocates.
    pub fn take_changes(&self) -> u32 {
        let mut flags = self.notifier.take();
        if self.latency_moved.swap(false, Ordering::AcqRel) {
            flags |= HostNotifier::LATENCY;
        }
        if flags & HostNotifier::LATENCY != 0
            && self.latency.load(Ordering::Acquire) == self.told_latency.load(Ordering::Acquire)
        {
            flags &= !HostNotifier::LATENCY;
        }
        flags
    }

    /// Store the plugin's latency and note whether it moved. No allocation.
    fn note_latency(&self, latency: usize) {
        let latency = latency.min(u32::MAX as usize) as u32;
        if self.latency.swap(latency, Ordering::AcqRel) != latency {
            self.latency_moved.store(true, Ordering::Release);
        }
    }

    pub fn meters(&self) -> MeterFrame {
        self.meters.read()
    }

    pub fn bus_meters(&self) -> BusMeters {
        self.bus_meters.read()
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

    /// Tell the plugin about the host's track ([`Plugin::track_info_changed`]).
    /// Message thread. The plugin object is taken between blocks, as for a
    /// state call: a block that starts meanwhile is passed through dry, and
    /// the call waits at most [`STATE_WAIT`] for one already running. Returns
    /// false when it gave up (nothing was delivered) or the plugin panicked.
    pub fn set_track_info(&self, info: &openplugin_core::TrackInfo) -> bool {
        catch_unwind(AssertUnwindSafe(|| {
            self.with_exclusive(|audio| {
                audio.inner.track_info_changed(info);
                self.note_latency(audio.inner.latency());
            })
            .is_some()
        }))
        .unwrap_or(false)
    }

    pub fn set_offline_flag(&self, offline: bool) {
        self.offline
            .store(if offline { 2 } else { 1 }, Ordering::Release);
    }

    /// Exclusive with `process`. Allocates the scratch buffers.
    ///
    /// Returns false for a sample rate outside
    /// [`openplugin_core::MIN_SAMPLE_RATE`]..=[`openplugin_core::MAX_SAMPLE_RATE`],
    /// for a rate the plugin refuses, or for an out-of-range block size or
    /// channel count. It does not substitute a rate.
    pub fn prepare(&self, sample_rate: f64, max_block: usize, channels: usize) -> bool {
        if max_block == 0 || max_block > 65_536 || !(1..=MAX_AUDIO_CHANNELS).contains(&channels) {
            return false;
        }
        if !sample_rate_supported(sample_rate) {
            return false;
        }
        let audio = unsafe { &mut *self.audio.get() };
        let rate = sample_rate as f32;
        if !audio.inner.accepts_sample_rate(rate) {
            return false;
        }
        let side_channels = audio
            .inner
            .supported_layouts()
            .iter()
            .map(|layout| layout.sidechain_channels())
            .max()
            .unwrap_or(0)
            .min(MAX_AUDIO_CHANNELS);
        audio.inner.prepare(rate, max_block, channels);
        audio.buffers = (0..channels).map(|_| vec![0.0; max_block]).collect();
        audio.sidechain = (0..side_channels).map(|_| vec![0.0; max_block]).collect();
        audio.channels = channels;
        audio.side_channels = side_channels;
        audio.max_block = max_block;
        audio.rate = rate;
        audio.cursor.invalidate();
        audio.cursor.pull(&self.plane, audio.inner.as_mut());
        self.latency
            .store(audio.inner.latency() as u32, Ordering::Release);
        self.tail
            .store(tail_seconds_of(audio.inner.tail(), rate).to_bits(), Ordering::Release);
        true
    }

    /// Resets the plugin now, or, when a state call holds the gate, at the
    /// start of the next block. The Audio Unit shell calls this from its
    /// render thread, so it never waits.
    pub fn reset(&self) {
        match self.enter_audio() {
            Some(_gate) => {
                let audio = unsafe { &mut *self.audio.get() };
                // The values stored so far apply first, so the latency read
                // after a reset is the one the next block will have.
                audio.cursor.pull(&self.plane, audio.inner.as_mut());
                audio.inner.reset();
                for buffer in &mut audio.buffers {
                    buffer.fill(0.0);
                }
                self.note_latency(audio.inner.latency());
            }
            None => self.reset_pending.store(true, Ordering::Release),
        }
    }

    /// OPCH session: the parameter plane plus `Plugin::save_state` when the plugin has one.
    /// A bare OPST plane still loads.
    ///
    /// Empty when the plugin object could not be reached (see
    /// [`FfiInstance::try_save_bytes`]); an empty vector is never a valid session.
    pub fn save_bytes(&self) -> Vec<u8> {
        self.try_save_bytes().unwrap_or_default()
    }

    /// [`FfiInstance::save_bytes`], or `None` when a block did not finish within
    /// [`STATE_WAIT`] or the plugin's `save_state` panicked. Message thread;
    /// allocates; blocks at most [`STATE_WAIT`]. Safe beside the audio thread:
    /// blocks that start meanwhile are skipped (dry).
    pub fn try_save_bytes(&self) -> Option<Vec<u8>> {
        if self.state_in_params {
            // The whole state is the plane, which the audio thread only reads:
            // no gate, so no block is skipped.
            let values: Vec<f32> = (0..self.plane.len()).map(|index| self.load_param(index)).collect();
            return Some(encode_session(self.info, &values, &[]));
        }
        self.with_exclusive(|audio| {
            catch_unwind(AssertUnwindSafe(|| {
                // The chunk should describe the values the plane holds now.
                audio.cursor.pull(&self.plane, audio.inner.as_mut());
                let chunk = audio.inner.save_state().unwrap_or_default();
                let values: Vec<f32> = (0..self.plane.len())
                    .map(|index| self.load_param(index))
                    .collect();
                encode_session(self.info, &values, &chunk)
            }))
            .ok()
        })
        .flatten()
    }

    /// Restores a session (OPCH, or a bare OPST plane). All or nothing: the
    /// envelope is checked before the plugin is touched, a refused or
    /// panicking `load_state` leaves the plane as it was. Returns false for a
    /// corrupt or foreign blob, a refused chunk, or a gate timeout. Message
    /// thread; blocks at most [`STATE_WAIT`]; safe beside the audio thread.
    ///
    /// A plugin whose `load_state` returns false must itself have changed
    /// nothing; the SDK cannot undo a plugin's partial write.
    pub fn load_bytes(&self, bytes: &[u8]) -> bool {
        let Some(session) = decode_session(self.info, bytes) else {
            return false;
        };
        if self.state_in_params {
            // Validated whole above; the engine pulls the new values on its
            // next block. A chunk in the session is ignored: the plugin
            // declared it has none.
            for (index, value) in session.values.iter().enumerate() {
                self.plane.store(index, *value);
            }
            return true;
        }
        self.with_exclusive(|audio| {
            let applied = catch_unwind(AssertUnwindSafe(|| {
                session.chunk.is_empty() || audio.inner.load_state(&session.chunk)
            }));
            if !matches!(applied, Ok(true)) {
                return false;
            }
            for (index, value) in session.values.iter().enumerate() {
                self.plane.store(index, *value);
            }
            // The chunk may have moved plugin parameters; push the plane again.
            audio.cursor.invalidate();
            self.note_latency(audio.inner.latency());
            true
        })
        .unwrap_or(false)
    }

    /// In-place stereo. Copies through scratch, so `left` and `right` are both the input and the output.
    pub fn process_stereo(&self, left: &mut [f32], right: &mut [f32]) -> bool {
        if left.len() != right.len() {
            return false;
        }
        let frames = left.len();
        let Some(_gate) = self.enter_audio() else {
            return true;
        };
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
        self.render(audio, width, frames, false);
        left.copy_from_slice(&audio.buffers[0][..frames]);
        let right_index = if audio.channels > 1 { 1 } else { 0 };
        right.copy_from_slice(&audio.buffers[right_index][..frames]);
        true
    }

    /// Planar raw pointers. Copies every input into scratch before writing any output,
    /// so an input pointer may alias an output pointer. A null right pointer is mono.
    ///
    /// # Safety
    /// Each non-null pointer addresses `frames` floats. `frames` is at most the
    /// block size passed to [`FfiInstance::new`] or [`FfiInstance::prepare`].
    pub unsafe fn process_io(
        &self,
        in_left: *const f32,
        in_right: *const f32,
        out_left: *mut f32,
        out_right: *mut f32,
        frames: usize,
    ) -> bool {
        if frames == 0 {
            return true;
        }
        if in_left.is_null() || out_left.is_null() {
            return false;
        }
        let Some(_gate) = self.enter_audio() else {
            // A state call holds the plugin: pass the block through dry.
            unsafe {
                dry_copy(in_left, out_left, frames);
                if !out_right.is_null() {
                    let source = if in_right.is_null() { in_left } else { in_right };
                    dry_copy(source, out_right, frames);
                }
            }
            return true;
        };
        let audio = unsafe { &mut *self.audio.get() };
        if frames > audio.max_block || audio.channels == 0 {
            return false;
        }
        unsafe {
            copy_ptr(in_left, &mut audio.buffers[0][..frames]);
            if audio.channels > 1 {
                let source = if in_right.is_null() { in_left } else { in_right };
                copy_ptr(source, &mut audio.buffers[1][..frames]);
            }
        }
        let width = audio.channels.min(2);
        self.render(audio, width, frames, false);
        unsafe {
            copy_out(&audio.buffers[0][..frames], out_left);
            if !out_right.is_null() {
                let index = if audio.channels > 1 { 1 } else { 0 };
                copy_out(&audio.buffers[index][..frames], out_right);
            }
        }
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
        let Some(_gate) = self.enter_audio() else {
            return true;
        };
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
        self.render(audio, count, frames, false);
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
        let Some(_gate) = self.enter_audio() else {
            return true;
        };
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
        self.render(audio, channels, frames, false);
        for frame in 0..frames {
            for channel in 0..channels {
                samples[frame * channels + channel] = audio.buffers[channel][frame];
            }
        }
        true
    }

    /// Main bus plus a sidechain. The sidechain is copied into scratch and is not written back.
    pub fn process_with_sidechain(&self, main: &mut [&mut [f32]], sidechain: &[&[f32]]) -> bool {
        if main.is_empty() || main.iter().any(|channel| channel.len() != main[0].len()) {
            return false;
        }
        if sidechain.iter().any(|channel| channel.len() != main[0].len()) {
            return false;
        }
        let frames = main[0].len();
        let count = main.len();
        let Some(_gate) = self.enter_audio() else {
            return true;
        };
        let audio = unsafe { &mut *self.audio.get() };
        if count > audio.channels || sidechain.len() > audio.side_channels || frames > audio.max_block {
            return false;
        }
        if frames == 0 {
            return true;
        }
        for (index, channel) in main.iter().enumerate() {
            copy_in(channel, &mut audio.buffers[index][..frames]);
        }
        for (index, channel) in audio.sidechain.iter_mut().enumerate() {
            if let Some(source) = sidechain.get(index) {
                copy_in(source, &mut channel[..frames]);
            } else {
                channel[..frames].fill(0.0);
            }
        }
        self.render(audio, count, frames, true);
        for (index, channel) in main.iter_mut().enumerate() {
            channel.copy_from_slice(&audio.buffers[index][..frames]);
        }
        true
    }

    fn apply_offline(&self, audio: &mut Audio) {
        let flag = self.offline.load(Ordering::Acquire);
        let offline = match flag {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        };
        if let Some(offline) = offline {
            if audio.applied_offline != Some(offline) {
                audio.inner.set_offline(offline);
                if offline {
                    let params = audio.inner.info().params;
                    for (index, param) in params.iter().enumerate() {
                        if param.flags.contains(ParamFlags::MOMENTARY) {
                            self.plane.store(index, param.default);
                        }
                    }
                    release_momentary(audio.inner.as_mut());
                }
                audio.applied_offline = Some(offline);
            }
        }
    }

    fn publish(&self, audio: &mut Audio) {
        let (latency, meters, bus) = (
            audio.inner.latency(),
            audio.inner.meters(),
            audio.inner.bus_meters(),
        );
        let tail = tail_seconds_of(audio.inner.tail(), audio.rate);
        self.tail.store(tail.to_bits(), Ordering::Release);
        self.note_latency(latency);
        self.meters.publish(meters);
        self.bus_meters.publish(bus);
    }

    fn render(&self, audio: &mut Audio, channels: usize, frames: usize, with_sidechain: bool) {
        self.apply_offline(audio);
        audio.cursor.pull(&self.plane, audio.inner.as_mut());
        {
            let Audio {
                inner,
                buffers,
                sidechain,
                ..
            } = audio;
            let side = if with_sidechain && !sidechain.is_empty() {
                Some(sidechain.as_slice())
            } else {
                None
            };
            inner.process_buses(&mut buffers[..channels], side, frames);
        }
        self.publish(audio);
    }
}

fn finite_rate(sample_rate: f64) -> f32 {
    if sample_rate_supported(sample_rate) {
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

/// # Safety
/// `src` and `dst` address `frames` floats; they may be the same buffer.
unsafe fn dry_copy(src: *const f32, dst: *mut f32, frames: usize) {
    for index in 0..frames {
        let sample = unsafe { *src.add(index) };
        unsafe { *dst.add(index) = if sample.is_finite() { sample } else { 0.0 } };
    }
}

/// # Safety
/// `src` addresses `dst.len()` floats.
unsafe fn copy_ptr(src: *const f32, dst: &mut [f32]) {
    for (index, out) in dst.iter_mut().enumerate() {
        let sample = unsafe { *src.add(index) };
        *out = if sample.is_finite() { sample } else { 0.0 };
    }
}

/// # Safety
/// `dst` addresses `src.len()` floats.
unsafe fn copy_out(src: &[f32], dst: *mut f32) {
    for (index, sample) in src.iter().enumerate() {
        unsafe { *dst.add(index) = *sample };
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

    #[test]
    fn prepare_accepts_twelve_channels_and_rejects_sixty_five() {
        let instance = FfiInstance::new(gain(), 48_000.0, 32, 2);
        assert!(instance.prepare(48_000.0, 32, 12));
        assert!(!instance.prepare(48_000.0, 32, 65));
        assert!(!instance.prepare(48_000.0, 32, 0));
    }

    #[test]
    fn an_old_opst_plane_still_loads() {
        let instance = FfiInstance::new(gain(), 48_000.0, 8, 2);
        let bytes = openplugin_core::encode_params(&INFO, &[6.0, 1.0]);
        assert!(bytes.starts_with(b"OPST"));
        assert!(instance.load_bytes(&bytes));
        assert!((instance.load_param(0) - 6.0).abs() < 1e-4);
        let saved = instance.save_bytes();
        assert!(saved.starts_with(b"OPCH"));
    }
}
