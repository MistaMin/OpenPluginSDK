//! LV2 `lv2_descriptor` for one stereo insert, its editor, and the two
//! description files a host reads.
//!
//! Ports: 0 and 1 are the audio inputs, 2 and 3 the audio outputs (the map
//! OpenGridder's shell uses), and every parameter is a control input from
//! port 4 on, in descriptor order. `run` stores each connected control port
//! in the [`ParamPlane`] and the plugin pulls the slots that changed, so it
//! does not lock. The control ports are the whole state, as the host saves
//! them.
//!
//! After the parameters, in this order, [`PortMap`] adds: the `freeWheeling`
//! control port (always; maps to [`Plugin::set_offline`]), an `atom:Sequence`
//! input when the plugin takes notes (`midi:MidiEvent` and `time:Position`),
//! an `atom:Sequence` output when it sends MIDI, and one `lv2:isSideChain`
//! audio input per sidechain channel, and last the `latency` control output
//! (`lv2:designation lv2:latency`, `lv2:reportsLatency`), which `run` writes
//! with [`Plugin::latency`] after every block, so a host that reads it after
//! `run` sees a new latency at once. An instrument has no main audio input:
//! its outputs are ports 0 and 1 and its parameters start at port 2. Every
//! port an effect had before keeps its index.
//! MIDI is delivered at its frame offsets by rendering in segments, the way
//! the CLAP entry does. The atoms are read from the host's own buffers with
//! bounds checks and no allocation; truncated, oversized or mistyped atoms
//! are skipped.
//!
//! [`export_lv2!`] emits `lv2_descriptor` from a cdylib, and with
//! `editor = make_editor` also `lv2ui_descriptor`: a `ui:CocoaUI` on macOS.
//! Its gestures are control-port writes plus `ui:touch` grabs, values come
//! back through `port_event`, and with `instance-access` it is bound to the
//! engine's [`EditorState`]. [`manifest_ttl`] and [`plugin_ttl`] write the
//! description files from the parameter table ([`write_bundle`]).
//!
//! Size: a fixed editor lists `ui:noUserResize`; a resizable one lists
//! `ui:resize` as extension data instead ([`write_bundle_for_editor`] reads
//! the editor's range). The host resizes the editor with `ui:resize`
//! (`extension_data`), and the editor asks the host for a size through the
//! host's `ui:resize` feature.
//! [`Lv2Harness`] drives the same render path in-process.
//!
//! # Worker
//!
//! When the host offers `worker:schedule`, the plugin's
//! [`openplugin_core::worker`] loaders run their jobs on the host's worker
//! thread instead of one of their own, through the `worker:interface`
//! extension data (`work`, `work_response`, `end_run`). A plugin needs no LV2
//! code for this: the entry builds the plugin inside
//! [`openplugin_core::worker::with_executor`], so a `loader` created in the
//! constructor or `prepare` is bridged. `submit` and `Slot::poll` only bump
//! atomic counters; the end of `run` turns them into `schedule_work` calls
//! (a four-byte task number each), so scheduling happens on the audio thread
//! and does not allocate. The host calls `work` on its thread, which runs one
//! job and answers with `respond`; the host then calls `work_response` at the
//! start of a later `run` (it is a notice; the result itself is taken by
//! `Slot::poll`, which swaps without locking). A host that refuses
//! `schedule_work` keeps the request for the next `run`; one that has no
//! worker feature gets the loader's own thread. More than eight loaders in
//! one instance: the extra ones use their own threads.

use std::ffi::{CStr, CString, c_char, c_void};
use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use openplugin_core::worker::{self, Executor, Step, Task, Waker};

use openplugin_core::midi::{self, Message, Parser};
use openplugin_core::{
    sample_rate_supported, Category, EditorFactory, IoConfig, MeterFrame, ParamCursor, ParamInfo, ParamPlane,
    Plugin, Seqlock, Taper, Transport, Unit,
};

#[cfg(any(target_os = "macos", test))]
use openplugin_core::EditorState;

type CreateFn = fn() -> Box<dyn Plugin>;

static CREATOR: Mutex<Option<CreateFn>> = Mutex::new(None);
static EDITOR: Mutex<Option<EditorFactory>> = Mutex::new(None);

/// Audio ports in front of the control ports.
pub const AUDIO_PORTS: u32 = 4;

/// Most sidechain channels given a port each.
const MAX_SIDECHAIN: usize = 8;
/// An atom sequence larger than this is not a host buffer; it is ignored.
const MAX_ATOM_BYTES: usize = 1 << 24;
/// Bytes one MIDI event takes in an output sequence: time, atom header, and
/// up to three data bytes padded to eight.
const OUT_EVENT: usize = 24;

/// The layout the LV2 entry runs: the first stereo-output layout, preferring
/// one with a sidechain. LV2 ports are fixed, so the choice is made once from
/// the plugin and the description file and the instance agree.
pub fn layout_of(plugin: &dyn Plugin) -> IoConfig {
    let fits = |l: &&IoConfig| l.output_channels() == 2 && (l.input_channels() == 2 || l.input_channels() == 0);
    let layouts = plugin.supported_layouts();
    layouts
        .iter()
        .filter(fits)
        .find(|l| l.sidechain_channels() > 0)
        .or_else(|| layouts.iter().find(fits))
        .copied()
        .unwrap_or(IoConfig::STEREO)
}

/// Where each port is. Computed from the plugin alone, so the instance, the
/// editor and the description file agree.
///
/// Order: audio, parameters, `freeWheeling`, note input, note output,
/// sidechain inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortMap {
    pub instrument: bool,
    pub params: u32,
    pub notes_in: bool,
    pub notes_out: bool,
    pub sidechain: u32,
}

impl PortMap {
    pub fn of(plugin: &dyn Plugin) -> Self {
        let info = plugin.info();
        let ports = plugin.note_ports();
        Self {
            instrument: info.is_instrument,
            params: info.params.len() as u32,
            notes_in: ports.input,
            notes_out: ports.output,
            sidechain: layout_of(plugin).sidechain_channels().min(MAX_SIDECHAIN) as u32,
        }
    }

    /// Port of the first parameter: 4 for an effect, 2 for an instrument.
    pub fn first_param(self) -> u32 {
        if self.instrument { 2 } else { AUDIO_PORTS }
    }

    pub fn freewheel(self) -> u32 {
        self.first_param() + self.params
    }

    pub fn atom_in(self) -> Option<u32> {
        self.notes_in.then(|| self.freewheel() + 1)
    }

    pub fn atom_out(self) -> Option<u32> {
        self.notes_out.then(|| self.freewheel() + 1 + u32::from(self.notes_in))
    }

    pub fn sidechain_first(self) -> u32 {
        self.freewheel() + 1 + u32::from(self.notes_in) + u32::from(self.notes_out)
    }

    /// The latency control output, after every other port.
    pub fn latency(self) -> u32 {
        self.sidechain_first() + self.sidechain
    }

    /// Slot in the four-entry audio table (in L, in R, out L, out R) of an
    /// audio port, if `port` is one.
    fn audio_slot(self, port: u32) -> Option<usize> {
        if self.instrument {
            (port < 2).then(|| port as usize + 2)
        } else {
            (port < AUDIO_PORTS).then_some(port as usize)
        }
    }
}

/// URIDs the host mapped for us. All zero when the host gave no `urid:map`;
/// then no atom is read or written.
#[derive(Clone, Copy, Default)]
struct Urids {
    sequence: u32,
    frame_time: u32,
    float: u32,
    double: u32,
    int: u32,
    long: u32,
    midi_event: u32,
    position: u32,
    speed: u32,
    bpm: u32,
    bar: u32,
    bar_beat: u32,
    beats_per_bar: u32,
    beat_unit: u32,
    frame: u32,
    object: u32,
    blank: u32,
}

#[repr(C)]
struct UridMap {
    handle: *mut c_void,
    map: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> u32>,
}

impl Urids {
    /// Ask the host's `urid:map` for every URI used. Not realtime.
    fn from_features(features: *const *const Lv2Feature) -> Self {
        let mut found: *const UridMap = std::ptr::null();
        let mut next = features;
        while !next.is_null() && !unsafe { *next }.is_null() {
            let feature = unsafe { &**next };
            if !feature.uri.is_null() && unsafe { CStr::from_ptr(feature.uri) }.to_bytes() == b"http://lv2plug.in/ns/ext/urid#map" {
                found = feature.data as *const UridMap;
            }
            next = unsafe { next.add(1) };
        }
        let Some(map) = (unsafe { found.as_ref() }) else { return Self::default() };
        let Some(call) = map.map else { return Self::default() };
        let get = |uri: &CStr| unsafe { call(map.handle, uri.as_ptr()) };
        Self {
            sequence: get(c"http://lv2plug.in/ns/ext/atom#Sequence"),
            frame_time: get(c"http://lv2plug.in/ns/ext/atom#frameTime"),
            float: get(c"http://lv2plug.in/ns/ext/atom#Float"),
            double: get(c"http://lv2plug.in/ns/ext/atom#Double"),
            int: get(c"http://lv2plug.in/ns/ext/atom#Int"),
            long: get(c"http://lv2plug.in/ns/ext/atom#Long"),
            midi_event: get(c"http://lv2plug.in/ns/ext/midi#MidiEvent"),
            position: get(c"http://lv2plug.in/ns/ext/time#Position"),
            speed: get(c"http://lv2plug.in/ns/ext/time#speed"),
            bpm: get(c"http://lv2plug.in/ns/ext/time#beatsPerMinute"),
            bar: get(c"http://lv2plug.in/ns/ext/time#bar"),
            bar_beat: get(c"http://lv2plug.in/ns/ext/time#barBeat"),
            beats_per_bar: get(c"http://lv2plug.in/ns/ext/time#beatsPerBar"),
            beat_unit: get(c"http://lv2plug.in/ns/ext/time#beatUnit"),
            frame: get(c"http://lv2plug.in/ns/ext/time#frame"),
            object: get(c"http://lv2plug.in/ns/ext/atom#Object"),
            blank: get(c"http://lv2plug.in/ns/ext/atom#Blank"),
        }
    }

    /// The host mapped the URIs, so a zero never matches a real type.
    fn usable(&self) -> bool {
        self.sequence != 0 && self.midi_event != 0
    }
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    Some(u32::from_ne_bytes(bytes.get(at..end)?.try_into().ok()?))
}

fn i64_at(bytes: &[u8], at: usize) -> Option<i64> {
    let end = at.checked_add(8)?;
    Some(i64::from_ne_bytes(bytes.get(at..end)?.try_into().ok()?))
}

fn pad8(n: usize) -> Option<usize> {
    n.checked_add(7).map(|v| v & !7)
}

/// One event of an input sequence: frame, atom type, atom body.
struct AtomEvent<'a> {
    frame: i64,
    kind: u32,
    body: &'a [u8],
}

/// The next well-formed event of a sequence body (`seq` starts at the
/// sequence's unit field). A truncated or oversized event ends the walk.
fn next_event<'a>(seq: &'a [u8], pos: &mut usize) -> Option<AtomEvent<'a>> {
    let start = *pos;
    let frame = i64_at(seq, start)?;
    let size = u32_at(seq, start.checked_add(8)?)? as usize;
    let kind = u32_at(seq, start.checked_add(12)?)?;
    let body_at = start.checked_add(16)?;
    let body = seq.get(body_at..body_at.checked_add(size)?)?;
    *pos = body_at.checked_add(pad8(size)?)?;
    Some(AtomEvent { frame, kind, body })
}

/// A number from any of the four atom number types.
fn atom_number(urids: &Urids, kind: u32, value: &[u8]) -> Option<f64> {
    let v = if kind == urids.float && value.len() >= 4 {
        f64::from(f32::from_ne_bytes(value[..4].try_into().ok()?))
    } else if kind == urids.double && value.len() >= 8 {
        f64::from_ne_bytes(value[..8].try_into().ok()?)
    } else if kind == urids.int && value.len() >= 4 {
        f64::from(i32::from_ne_bytes(value[..4].try_into().ok()?))
    } else if kind == urids.long && value.len() >= 8 {
        i64::from_ne_bytes(value[..8].try_into().ok()?) as f64
    } else {
        return None;
    };
    v.is_finite().then_some(v)
}

/// What a `time:Position` object said. `None` for a field it left out.
#[derive(Clone, Copy, Default)]
struct Position {
    speed: Option<f64>,
    bpm: Option<f64>,
    bar: Option<f64>,
    bar_beat: Option<f64>,
    beats_per_bar: Option<f64>,
    beat_unit: Option<f64>,
    frame: Option<f64>,
}

/// Read a `time:Position` object body (after the atom header). `None` if it
/// is not one.
fn parse_position(urids: &Urids, object: &[u8]) -> Option<Position> {
    if urids.position == 0 || u32_at(object, 4)? != urids.position {
        return None;
    }
    let mut out = Position::default();
    let mut pos = 8usize;
    while let Some(key) = u32_at(object, pos) {
        let Some(size) = u32_at(object, pos + 8).map(|s| s as usize) else { break };
        let Some(kind) = u32_at(object, pos + 12) else { break };
        let Some(at) = pos.checked_add(16) else { break };
        let Some(value) = at.checked_add(size).and_then(|end| object.get(at..end)) else { break };
        let number = atom_number(urids, kind, value);
        if key == urids.speed {
            out.speed = number;
        } else if key == urids.bpm {
            out.bpm = number;
        } else if key == urids.bar {
            out.bar = number;
        } else if key == urids.bar_beat {
            out.bar_beat = number;
        } else if key == urids.beats_per_bar {
            out.beats_per_bar = number;
        } else if key == urids.beat_unit {
            out.beat_unit = number;
        } else if key == urids.frame {
            out.frame = number;
        }
        let Some(next) = pad8(size).and_then(|p| at.checked_add(p)) else { break };
        pos = next;
    }
    Some(out)
}

/// Fold a position into the running transport.
fn apply_position(transport: &mut Transport, p: &Position, rate: f32) {
    if let Some(speed) = p.speed {
        transport.playing = speed != 0.0;
    }
    if let Some(bpm) = p.bpm.filter(|v| *v > 0.0) {
        transport.tempo = bpm;
    }
    if let Some(n) = p.beats_per_bar.filter(|v| *v >= 1.0 && *v <= f64::from(u32::MAX)) {
        transport.numerator = n as u32;
    }
    if let Some(d) = p.beat_unit.filter(|v| *v >= 1.0 && *v <= f64::from(u32::MAX)) {
        transport.denominator = d as u32;
    }
    if let Some(bar) = p.bar.filter(|v| *v >= 0.0 && *v <= i64::MAX as f64) {
        transport.bar_number = bar as i64;
    }
    if let Some(frame) = p.frame.filter(|v| *v >= 0.0 && *v <= i64::MAX as f64) {
        transport.sample_pos = frame as i64;
        if rate > 0.0 {
            transport.seconds = frame / f64::from(rate);
        }
    }
    // Beats are in the time signature's unit; a quarter note is 4 / unit of them.
    let quarters_per_beat = if transport.denominator > 0 { 4.0 / f64::from(transport.denominator) } else { 1.0 };
    let per_bar = f64::from(transport.numerator.max(1)) * quarters_per_beat;
    if p.bar.is_some() || p.beats_per_bar.is_some() || p.beat_unit.is_some() {
        transport.bar_start = transport.bar_number as f64 * per_bar;
    }
    if let Some(beat) = p.bar_beat.filter(|v| *v >= 0.0) {
        transport.quarter_notes = transport.bar_start + beat * quarters_per_beat;
    }
}

/// Bytes in a MIDI message with this status byte (three at most).
fn midi_len(status: u8) -> usize {
    match status {
        0x80..=0xBF | 0xE0..=0xEF | 0xF2 => 3,
        0xC0..=0xDF | 0xF1 | 0xF3 => 2,
        _ => 1,
    }
}

/// The input sequence's events, as a slice from its `unit` field. `None` if
/// the port is unconnected or what it points at is not a sequence we can read.
///
/// # Safety
/// `atom` is null or the host's connected input atom port: 8-byte aligned,
/// and `size` bytes follow the 8-byte atom header.
unsafe fn input_sequence<'a>(urids: &Urids, atom: *const u8) -> Option<&'a [u8]> {
    if atom.is_null() || !urids.usable() || atom as usize % 8 != 0 {
        return None;
    }
    let header = unsafe { std::slice::from_raw_parts(atom, 8) };
    let size = u32_at(header, 0)? as usize;
    if u32_at(header, 4)? != urids.sequence || !(8..=MAX_ATOM_BYTES).contains(&size) {
        return None;
    }
    let body = unsafe { std::slice::from_raw_parts(atom.add(8), size) };
    // Unit 0 and atom:frameTime are frames; beat-timed sequences are not read.
    let unit = u32_at(body, 0)?;
    (unit == 0 || unit == urids.frame_time).then_some(body)
}

/// A host-provided output sequence being filled without allocation.
struct OutSequence {
    atom: *mut u8,
    /// Body bytes the host gave us.
    capacity: usize,
    events: usize,
}

impl OutSequence {
    /// Start an empty sequence in the port. `None` if the port is unusable.
    ///
    /// # Safety
    /// `atom` is null or the host's connected output atom port: 8-byte
    /// aligned, and `size` (as the host set it) body bytes follow the header.
    unsafe fn begin(urids: &Urids, atom: *mut u8) -> Option<Self> {
        if atom.is_null() || !urids.usable() || atom as usize % 8 != 0 {
            return None;
        }
        let capacity = unsafe { std::ptr::read(atom as *const u32) } as usize;
        if !(8..=MAX_ATOM_BYTES).contains(&capacity) {
            return None;
        }
        let mut out = Self { atom, capacity, events: 0 };
        out.finish(urids);
        Some(out)
    }

    fn finish(&mut self, urids: &Urids) {
        let mut head = [0u8; 16];
        head[0..4].copy_from_slice(&((8 + self.events * OUT_EVENT) as u32).to_ne_bytes());
        head[4..8].copy_from_slice(&urids.sequence.to_ne_bytes());
        // unit 0 (frames) and padding stay zero.
        unsafe { std::ptr::copy_nonoverlapping(head.as_ptr(), self.atom, 16) };
    }

    /// Append one MIDI event. False when it does not fit.
    fn push(&mut self, urids: &Urids, frame: i64, data: &[u8]) -> bool {
        if 8 + (self.events + 1) * OUT_EVENT > self.capacity {
            return false;
        }
        let mut event = [0u8; OUT_EVENT];
        event[0..8].copy_from_slice(&frame.to_ne_bytes());
        event[8..12].copy_from_slice(&(data.len() as u32).to_ne_bytes());
        event[12..16].copy_from_slice(&urids.midi_event.to_ne_bytes());
        event[16..16 + data.len()].copy_from_slice(data);
        unsafe { std::ptr::copy_nonoverlapping(event.as_ptr(), self.atom.add(16 + self.events * OUT_EVENT), OUT_EVENT) };
        self.events += 1;
        true
    }
}

const WORKER_SCHEDULE: &[u8] = b"http://lv2plug.in/ns/ext/worker#schedule";
const WORKER_INTERFACE: &[u8] = b"http://lv2plug.in/ns/ext/worker#interface";
/// Loaders one instance bridges to the host; further ones use their own thread.
const WORKER_SLOTS: usize = 8;
/// `schedule_work` calls one slot makes per `run`; the rest wait for the next.
const SCHEDULE_PER_RUN: u32 = 16;
const WORKER_OK: u32 = 0;
const WORKER_ERR_UNKNOWN: u32 = 1;

#[repr(C)]
struct WorkerSchedule {
    handle: *mut c_void,
    schedule_work: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void) -> u32>,
}

type RespondFn = unsafe extern "C" fn(*mut c_void, u32, *const c_void) -> u32;

#[repr(C)]
struct WorkerInterface {
    work: unsafe extern "C" fn(*mut c_void, Option<RespondFn>, *mut c_void, u32, *const c_void) -> u32,
    work_response: unsafe extern "C" fn(*mut c_void, u32, *const c_void) -> u32,
    end_run: unsafe extern "C" fn(*mut c_void) -> u32,
}

unsafe impl Sync for WorkerInterface {}

static WORKER_IFACE: WorkerInterface = WorkerInterface { work: lv2_work, work_response: lv2_work_response, end_run: lv2_end_run };

/// Counts of what the worker bridge did, for tests and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkerStats {
    /// `schedule_work` calls the host accepted.
    pub scheduled: usize,
    /// `schedule_work` calls the host refused (kept and retried).
    pub refused: usize,
    /// `work` calls that ran a task.
    pub worked: usize,
    /// `work` calls rejected as malformed.
    pub rejected: usize,
    /// `work_response` calls.
    pub responses: usize,
}

struct BridgeSlot {
    /// Requests for `work` not yet passed to the host.
    pending: AtomicU32,
    used: AtomicBool,
    task: Mutex<Option<Box<dyn Task>>>,
}

/// Runs loader tasks on the host's worker. Shared by the instance (audio
/// thread: counters only), the loaders' wakers and the host's worker thread.
struct Bridge {
    slots: [BridgeSlot; WORKER_SLOTS],
    scheduled: AtomicUsize,
    refused: AtomicUsize,
    worked: AtomicUsize,
    rejected: AtomicUsize,
    responses: AtomicUsize,
}

struct SlotWaker {
    bridge: Arc<Bridge>,
    index: usize,
    live: AtomicBool,
}

impl Bridge {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            slots: std::array::from_fn(|_| BridgeSlot { pending: AtomicU32::new(0), used: AtomicBool::new(false), task: Mutex::new(None) }),
            scheduled: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
            worked: AtomicUsize::new(0),
            rejected: AtomicUsize::new(0),
            responses: AtomicUsize::new(0),
        })
    }

    fn stats(&self) -> WorkerStats {
        WorkerStats {
            scheduled: self.scheduled.load(Ordering::Relaxed),
            refused: self.refused.load(Ordering::Relaxed),
            worked: self.worked.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
            responses: self.responses.load(Ordering::Relaxed),
        }
    }

    /// Turn the requests loaders made into `schedule_work` calls. Audio
    /// thread: atomics and the host's call only. A refusal keeps the rest.
    fn flush(&self, schedule: &WorkerSchedule) {
        let Some(call) = schedule.schedule_work else { return };
        for (index, slot) in self.slots.iter().enumerate() {
            if !slot.used.load(Ordering::Acquire) {
                continue;
            }
            let wanted = slot.pending.swap(0, Ordering::AcqRel);
            if wanted == 0 {
                continue;
            }
            let now = wanted.min(SCHEDULE_PER_RUN);
            let mut left = wanted;
            let payload = (index as u32).to_ne_bytes();
            for _ in 0..now {
                if unsafe { call(schedule.handle, 4, payload.as_ptr().cast()) } != WORKER_OK {
                    self.refused.fetch_add(1, Ordering::Relaxed);
                    break;
                }
                self.scheduled.fetch_add(1, Ordering::Relaxed);
                left -= 1;
            }
            if left > 0 {
                slot.pending.fetch_add(left, Ordering::AcqRel);
            }
        }
    }
}

/// The `Arc<Bridge>` as an [`Executor`].
struct BridgeHandle(Arc<Bridge>);

impl Executor for BridgeHandle {
    fn spawn(&self, task: Box<dyn Task>) -> Result<Arc<dyn Waker>, Box<dyn Task>> {
        for (index, slot) in self.0.slots.iter().enumerate() {
            if slot.used.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                slot.pending.store(0, Ordering::Release);
                *slot.task.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
                return Ok(Arc::new(SlotWaker { bridge: self.0.clone(), index, live: AtomicBool::new(true) }));
            }
        }
        Err(task)
    }
}

impl Waker for SlotWaker {
    fn wake(&self) {
        if self.live.load(Ordering::Acquire) {
            self.bridge.slots[self.index].pending.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn detach(&self) {
        if self.live.swap(false, Ordering::AcqRel) {
            let slot = &self.bridge.slots[self.index];
            // Waits for a `work` call in progress, then frees the task here.
            let task = slot.task.lock().unwrap_or_else(|e| e.into_inner()).take();
            drop(task);
            slot.pending.store(0, Ordering::Release);
            slot.used.store(false, Ordering::Release);
        }
    }
}

/// Live instances' bridges by handle address. The host calls `work` on its
/// own thread with the instance handle; looking the bridge up here keeps that
/// thread away from the audio thread's `Instance`.
static BRIDGES: Mutex<Vec<(usize, Arc<Bridge>)>> = Mutex::new(Vec::new());

fn bridge_of(handle: *mut c_void) -> Option<Arc<Bridge>> {
    let list = BRIDGES.lock().unwrap_or_else(|e| e.into_inner());
    list.iter().find(|(key, _)| *key == handle as usize).map(|(_, bridge)| bridge.clone())
}

/// Worker counters of a live instance handle (for tests and diagnostics).
pub fn worker_stats(handle: *mut c_void) -> Option<WorkerStats> {
    bridge_of(handle).map(|bridge| bridge.stats())
}

/// `worker:interface` `work`: runs one task step on the host's thread.
unsafe extern "C" fn lv2_work(handle: *mut c_void, respond: Option<RespondFn>, respond_handle: *mut c_void, size: u32, data: *const c_void) -> u32 {
    guarded(0, || {
        let Some(bridge) = bridge_of(handle) else { return WORKER_ERR_UNKNOWN };
        // Our requests are exactly one `u32` task number; anything else is not ours.
        let index = if size == 4 && !data.is_null() && data as usize % 4 == 0 { Some(unsafe { *(data as *const u32) } as usize) } else { None };
        let Some(slot) = index.and_then(|i| bridge.slots.get(i)) else {
            bridge.rejected.fetch_add(1, Ordering::Relaxed);
            return WORKER_ERR_UNKNOWN;
        };
        let step = {
            let mut guard = slot.task.lock().unwrap_or_else(|e| e.into_inner());
            match guard.as_mut() {
                Some(task) => task.work(),
                None => {
                    bridge.rejected.fetch_add(1, Ordering::Relaxed);
                    return WORKER_ERR_UNKNOWN;
                }
            }
        };
        bridge.worked.fetch_add(1, Ordering::Relaxed);
        match step {
            Step::Idle => {}
            Step::Done => {
                if let Some(respond) = respond {
                    unsafe { respond(respond_handle, size, data) };
                }
            }
            // A result is waiting for room: ask for another call on the next run.
            Step::Retry => {
                slot.pending.fetch_add(1, Ordering::AcqRel);
            }
        }
        WORKER_OK
    })
}

/// `worker:interface` `work_response`: runs on the audio thread before `run`
/// when a task finished. The result itself is taken by `Slot::poll`.
unsafe extern "C" fn lv2_work_response(handle: *mut c_void, _size: u32, _data: *const c_void) -> u32 {
    guarded(0, || {
        if let Some(bridge) = bridge_of(handle) {
            bridge.responses.fetch_add(1, Ordering::Relaxed);
        }
        WORKER_OK
    })
}

unsafe extern "C" fn lv2_end_run(_handle: *mut c_void) -> u32 {
    WORKER_OK
}

/// Install the plugin `lv2_descriptor` will instantiate.
pub fn set_creator(create: CreateFn) {
    *CREATOR.lock().unwrap_or_else(|e| e.into_inner()) = Some(create);
}

fn creator() -> Option<CreateFn> {
    *CREATOR.lock().unwrap_or_else(|e| e.into_inner())
}

/// Install the editor `lv2ui_descriptor` will open.
pub fn set_editor(factory: EditorFactory) {
    if let Ok(mut slot) = EDITOR.lock() {
        *slot = Some(factory);
    }
}

#[cfg(target_os = "macos")]
fn editor_factory() -> Option<EditorFactory> {
    EDITOR.lock().ok().and_then(|slot| *slot)
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

/// Marks an [`Instance`] of this binary, so an editor given an
/// `instance-access` handle can tell one of ours from anything else.
const MAGIC: u64 = 0x4f50_4c56_3249_4e53;

struct Instance {
    magic: u64,
    inner: Box<dyn Plugin>,
    plane: ParamPlane,
    cursor: ParamCursor,
    meters: Seqlock<MeterFrame>,
    map: PortMap,
    urids: Urids,
    rate: f32,
    ports: [*mut f32; 4],
    controls: Vec<*const f32>,
    freewheel: *const f32,
    /// The host's latency output port, written after every `run`.
    latency_port: *mut f32,
    offline: bool,
    atom_in: *const u8,
    atom_out: *mut u8,
    side_ports: Vec<*const f32>,
    side: Vec<Vec<f32>>,
    transport: Transport,
    has_position: bool,
    scratch: [Vec<f32>; 2],
    prepared: usize,
    #[cfg(any(target_os = "macos", test))]
    editor_state: Option<EditorState>,
    bridge: Arc<Bridge>,
    /// The host's `worker:schedule`, if it offered one that works.
    schedule: Option<WorkerSchedule>,
    handle_key: usize,
    /// A callback into the plugin panicked; `run` outputs silence until `activate`.
    poisoned: bool,
}

const PREPARED: usize = 8192;

/// The plugin's URI: [`Plugin::lv2_uri`], else its id.
pub fn plugin_uri(plugin: &dyn Plugin) -> &'static str {
    plugin.lv2_uri().unwrap_or(plugin.info().id)
}

fn uri() -> &'static CStr {
    static URI: OnceLock<CString> = OnceLock::new();
    URI.get_or_init(|| {
        let id = creator().and_then(|create| CString::new(plugin_uri(create().as_ref())).ok());
        id.unwrap_or_else(|| CString::new("urn:openplugin:plugin").expect("static uri"))
    })
    .as_c_str()
}

/// Parameters of the installed plugin, read once.
#[cfg(target_os = "macos")]
fn param_count() -> usize {
    static COUNT: OnceLock<usize> = OnceLock::new();
    *COUNT.get_or_init(|| creator().map(|create| create().info().params.len()).unwrap_or(0))
}

/// The rate a new LV2 instance is prepared at.
///
/// An LV2 host that does not know the rate yet passes `0.0`, which uses the
/// 48 kHz default. Every other rate outside the shared range fails
/// instantiation instead of being replaced silently.
fn instantiate_rate(sample_rate: f64) -> Option<f32> {
    if sample_rate == 0.0 {
        return Some(48_000.0);
    }
    sample_rate_supported(sample_rate).then_some(sample_rate as f32)
}

fn new_instance(sample_rate: f64, features: *const *const Lv2Feature) -> Option<Box<Instance>> {
    let create = creator()?;
    let bridge = Bridge::new();
    let schedule = worker_schedule(features);
    // With the host's worker, loaders made while building the plugin run on it.
    let mut inner = if schedule.is_some() {
        worker::with_executor(Arc::new(BridgeHandle(bridge.clone())), create)
    } else {
        create()
    };
    // LV2 has no call for new parameter info, and the latency port is written
    // after every block, so the notifier has no one to tell: nobody reads it.
    inner.set_host_notifier(openplugin_core::HostNotifier::new());
    let info = inner.info();
    let plane = ParamPlane::new(info);
    plane.seed_from(inner.as_ref());
    let mut cursor = ParamCursor::new(plane.len());
    let rate = instantiate_rate(sample_rate)?;
    if !inner.accepts_sample_rate(rate) {
        return None;
    }
    let map = PortMap::of(inner.as_ref());
    let layout = layout_of(inner.as_ref());
    let prepare = |inner: &mut Box<dyn Plugin>| {
        if inner.set_layout(layout) {
            inner.prepare_layout(rate, PREPARED, layout);
        } else {
            inner.prepare(rate, PREPARED, 2);
        }
    };
    if schedule.is_some() {
        worker::with_executor(Arc::new(BridgeHandle(bridge.clone())), || prepare(&mut inner));
    } else {
        prepare(&mut inner);
    }
    cursor.invalidate();
    cursor.pull(&plane, inner.as_mut());
    #[cfg(any(target_os = "macos", test))]
    let editor_state = inner.editor_state();
    let controls = vec![std::ptr::null(); plane.len()];
    let sidechain = map.sidechain as usize;
    Some(Box::new(Instance {
        magic: MAGIC,
        poisoned: false,
        inner,
        plane,
        cursor,
        meters: Seqlock::new(MeterFrame::SILENT),
        map,
        urids: Urids::from_features(features),
        rate,
        ports: [std::ptr::null_mut(); 4],
        controls,
        freewheel: std::ptr::null(),
        latency_port: std::ptr::null_mut(),
        offline: false,
        atom_in: std::ptr::null(),
        atom_out: std::ptr::null_mut(),
        side_ports: vec![std::ptr::null(); sidechain],
        side: vec![vec![0.0; PREPARED]; sidechain],
        transport: Transport::default(),
        has_position: false,
        scratch: [vec![0.0; PREPARED], vec![0.0; PREPARED]],
        prepared: PREPARED,
        #[cfg(any(target_os = "macos", test))]
        editor_state,
        bridge,
        schedule,
        handle_key: 0,
    }))
}

/// The host's `worker:schedule` feature, if it is usable.
fn worker_schedule(features: *const *const Lv2Feature) -> Option<WorkerSchedule> {
    let mut next = features;
    while !next.is_null() && !unsafe { *next }.is_null() {
        let feature = unsafe { &**next };
        if !feature.uri.is_null() && unsafe { CStr::from_ptr(feature.uri) }.to_bytes() == WORKER_SCHEDULE {
            let schedule = unsafe { (feature.data as *const WorkerSchedule).as_ref()? };
            return schedule.schedule_work.map(|call| WorkerSchedule { handle: schedule.handle, schedule_work: Some(call) });
        }
        next = unsafe { next.add(1) };
    }
    None
}

/// Copy the connected control ports into the plane. A port the host never
/// connected keeps the plugin's value.
fn read_controls(instance: &mut Instance) {
    for (index, port) in instance.controls.iter().enumerate() {
        if !port.is_null() {
            instance.plane.store(index, unsafe { **port });
        }
    }
}

/// `freeWheeling` above one half means the host is rendering offline.
fn read_freewheel(instance: &mut Instance) {
    if instance.freewheel.is_null() {
        return;
    }
    let value = unsafe { *instance.freewheel };
    let offline = value.is_finite() && value > 0.5;
    if offline != instance.offline {
        instance.offline = offline;
        instance.inner.set_offline(offline);
    }
}

/// Render `frames` frames starting `offset` frames into the host's buffers.
fn render(instance: &mut Instance, offset: usize, frames: usize) {
    let frames = frames.min(instance.prepared);
    if frames == 0 {
        return;
    }
    instance.cursor.pull(&instance.plane, instance.inner.as_mut());
    for channel in 0..2 {
        let src = instance.ports[channel];
        if src.is_null() || instance.map.instrument {
            instance.scratch[channel][..frames].fill(0.0);
        } else {
            let samples = unsafe { std::slice::from_raw_parts(src.add(offset), frames) };
            instance.scratch[channel][..frames].copy_from_slice(samples);
        }
    }
    for (channel, buffer) in instance.side.iter_mut().enumerate() {
        let src = instance.side_ports[channel];
        if src.is_null() {
            buffer[..frames].fill(0.0);
        } else {
            let samples = unsafe { std::slice::from_raw_parts(src.add(offset), frames) };
            buffer[..frames].copy_from_slice(samples);
        }
    }
    let side = if instance.side.is_empty() { None } else { Some(instance.side.as_slice()) };
    instance.inner.process_buses(&mut instance.scratch, side, frames);
    for channel in 0..2 {
        let dst = instance.ports[channel + 2];
        if !dst.is_null() {
            let out = unsafe { std::slice::from_raw_parts_mut(dst.add(offset), frames) };
            out.copy_from_slice(&instance.scratch[channel][..frames]);
        }
    }
    instance.meters.publish(instance.inner.meters());
}

/// Render `start..end` of the block in pieces no longer than the prepared size.
fn render_range(instance: &mut Instance, start: usize, end: usize) {
    let mut done = start;
    while done < end {
        let n = (end - done).min(instance.prepared);
        render(instance, done, n);
        done += n;
    }
}

/// Deliver one input event at the current position.
fn handle_event(instance: &mut Instance, event: &AtomEvent<'_>) {
    let urids = instance.urids;
    if event.kind == urids.midi_event {
        // A fresh parser per event: LV2 MIDI events carry whole messages.
        if let Ok(Some((message, _))) = Parser::new().pull(event.body) {
            match message {
                Message::NoteOn { channel, note, velocity } => instance.inner.note_on_channel(0, channel.index(), note, velocity),
                other => midi::deliver(instance.inner.as_mut(), 0, other),
            }
        }
    } else if event.kind != 0 && (event.kind == urids.object || event.kind == urids.blank) {
        if let Some(position) = parse_position(&urids, event.body) {
            apply_position(&mut instance.transport, &position, instance.rate);
            instance.has_position = true;
            instance.inner.set_transport(&instance.transport);
        }
    }
}

/// Move the running transport on by a block, for hosts that send a position
/// only when it changes.
fn advance_transport(instance: &mut Instance, frames: usize) {
    let transport = &mut instance.transport;
    if !instance.has_position || !transport.playing || instance.rate <= 0.0 {
        return;
    }
    let seconds = frames as f64 / f64::from(instance.rate);
    transport.sample_pos = transport.sample_pos.saturating_add(frames as i64);
    transport.seconds += seconds;
    if transport.tempo > 0.0 {
        transport.quarter_notes += seconds * transport.tempo / 60.0;
    }
}

/// Write the plugin's queued MIDI into the host's output sequence. Events
/// that do not fit are dropped; the queue is always emptied.
fn drain_midi(instance: &mut Instance, frames: usize) {
    if !instance.map.notes_out {
        return;
    }
    let urids = instance.urids;
    let mut out = unsafe { OutSequence::begin(&urids, instance.atom_out) };
    let last = frames.saturating_sub(1) as i64;
    let mut previous = 0i64;
    let mut guard = 0u32;
    while let Some((time, data)) = instance.inner.pull_midi() {
        guard += 1;
        if guard > 65_536 {
            break;
        }
        if let Some(sequence) = out.as_mut() {
            let frame = i64::from(time).clamp(previous, last.max(previous));
            let len = midi_len(data[0]);
            if sequence.push(&urids, frame, &data[..len]) {
                previous = frame;
            }
        }
    }
    if let Some(sequence) = out.as_mut() {
        sequence.finish(&urids);
    }
}

/// Runs one `extern "C"` entry so a panic never unwinds into the host. A panic returns `fail`,
/// the format's failure value. `catch_unwind` allocates nothing unless a panic happens; the
/// default panic hook still prints to stderr, so install your own hook if that matters on the
/// audio thread.
fn guarded<R>(fail: R, body: impl FnOnce() -> R) -> R {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(fail)
}

/// [`guarded`] for a call on an instance: a panic also poisons it, so `run` outputs
/// silence until the host activates the instance again.
fn guarded_on<R>(handle: *mut c_void, fail: R, body: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(_) => {
            if let Some(instance) = unsafe { (handle as *mut Instance).as_mut() } {
                instance.poisoned = true;
            }
            fail
        }
    }
}

extern "C" fn lv2_instantiate(
    _descriptor: *const Lv2Descriptor,
    sample_rate: f64,
    _bundle_path: *const c_char,
    features: *const *const Lv2Feature,
) -> *mut c_void {
    guarded(std::ptr::null_mut(), || {
        match new_instance(sample_rate, features) {
            Some(mut instance) => {
                instance.handle_key = &*instance as *const Instance as usize;
                BRIDGES.lock().unwrap_or_else(|e| e.into_inner()).push((instance.handle_key, instance.bridge.clone()));
                Box::into_raw(instance) as *mut c_void
            }
            None => std::ptr::null_mut(),
        }
    })
}

extern "C" fn lv2_connect(instance: *mut c_void, port: u32, data: *mut c_void) {
    guarded_on(instance, (), || {
        if instance.is_null() {
            return;
        }
        let instance = unsafe { &mut *(instance as *mut Instance) };
        let map = instance.map;
        if let Some(slot) = map.audio_slot(port) {
            instance.ports[slot] = data as *mut f32;
        } else if port >= map.first_param() && port < map.freewheel() {
            if let Some(slot) = instance.controls.get_mut((port - map.first_param()) as usize) {
                *slot = data as *const f32;
            }
        } else if port == map.freewheel() {
            instance.freewheel = data as *const f32;
        } else if Some(port) == map.atom_in() {
            instance.atom_in = data as *const u8;
        } else if Some(port) == map.atom_out() {
            instance.atom_out = data as *mut u8;
        } else if port == map.latency() {
            instance.latency_port = data as *mut f32;
            write_latency(instance);
        } else if port >= map.sidechain_first() {
            if let Some(slot) = instance.side_ports.get_mut((port - map.sidechain_first()) as usize) {
                *slot = data as *const f32;
            }
        }
    })
}

extern "C" fn lv2_activate(instance: *mut c_void) {
    guarded_on(instance, (), || {
        if instance.is_null() {
            return;
        }
        unsafe {
            let instance = &mut *(instance as *mut Instance);
            instance.poisoned = false;
            read_controls(instance);
            read_freewheel(instance);
            instance.inner.reset();
            instance.cursor.invalidate();
            instance.cursor.pull(&instance.plane, instance.inner.as_mut());
            write_latency(instance);
        }
    })
}

/// Write [`Plugin::latency`] to the host's latency port, if it connected one.
/// Audio thread (and `connect_port`, `activate`); one store.
fn write_latency(instance: &mut Instance) {
    if !instance.latency_port.is_null() {
        let latency = instance.inner.latency().min(1 << 24) as f32;
        unsafe { *instance.latency_port = latency };
    }
}

extern "C" fn lv2_run(instance: *mut c_void, sample_count: u32) {
    if instance.is_null() {
        return;
    }
    if unsafe { &*(instance as *const Instance) }.poisoned {
        silence_outputs(instance as *mut Instance, sample_count as usize);
        return;
    }
    let ran = catch_unwind(AssertUnwindSafe(|| run_block(instance as *mut Instance, sample_count as usize)));
    if ran.is_err() {
        if let Some(instance) = unsafe { (instance as *mut Instance).as_mut() } {
            instance.poisoned = true;
        }
        silence_outputs(instance as *mut Instance, sample_count as usize);
    }
}

/// Zero the host's output audio buffers for `frames` frames.
fn silence_outputs(instance: *mut Instance, frames: usize) {
    let instance = unsafe { &*instance };
    for dst in &instance.ports[2..4] {
        if !dst.is_null() {
            unsafe { std::ptr::write_bytes(*dst, 0, frames) };
        }
    }
}

fn run_block(instance: *mut Instance, frames: usize) {
    unsafe {
        let instance = &mut *instance;
        read_controls(instance);
        read_freewheel(instance);
        if frames == 0 {
            // An empty output sequence is still a valid one.
            drain_midi(instance, 0);
            instance.cursor.pull(&instance.plane, instance.inner.as_mut());
            write_latency(instance);
            flush_worker(instance);
            return;
        }
        if instance.has_position {
            instance.inner.set_transport(&instance.transport);
        }
        let sequence = input_sequence(&instance.urids, instance.atom_in);
        let mut done = 0;
        if let Some(sequence) = sequence {
            let mut pos = 8;
            while let Some(event) = next_event(sequence, &mut pos) {
                // Out-of-block times are junk; in-block times never go back.
                let Ok(frame) = usize::try_from(event.frame) else { continue };
                if frame >= frames {
                    continue;
                }
                let at = frame.max(done);
                render_range(instance, done, at);
                done = at;
                handle_event(instance, &event);
            }
        }
        render_range(instance, done, frames);
        advance_transport(instance, frames);
        drain_midi(instance, frames);
        write_latency(instance);
        flush_worker(instance);
    }
}

/// Pass the loaders' requests to the host's worker. Audio thread.
fn flush_worker(instance: &Instance) {
    if let Some(schedule) = &instance.schedule {
        instance.bridge.flush(schedule);
    }
}

extern "C" fn lv2_deactivate(instance: *mut c_void) {
    guarded_on(instance, (), || {
        if instance.is_null() {
            return;
        }
        unsafe {
            (*(instance as *mut Instance)).inner.reset();
        }
    })
}

extern "C" fn lv2_cleanup(instance: *mut c_void) {
    guarded((), || {
        if instance.is_null() {
            return;
        }
        unsafe {
            let mut instance = Box::from_raw(instance as *mut Instance);
            instance.magic = 0;
            BRIDGES.lock().unwrap_or_else(|e| e.into_inner()).retain(|(key, _)| *key != instance.handle_key);
            drop(instance);
        }
    })
}

extern "C" fn lv2_extension(uri: *const c_char) -> *const c_void {
    guarded(std::ptr::null(), || {
        if !uri.is_null() && unsafe { CStr::from_ptr(uri) }.to_bytes() == WORKER_INTERFACE {
            return (&WORKER_IFACE as *const WorkerInterface).cast();
        }
        std::ptr::null()
    })
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
    // `published` builds the URI, which may run the plugin's constructor.
    guarded(std::ptr::null(), || if index == 0 && creator().is_some() { published() } else { std::ptr::null() })
}

/// The editor state of an `instance-access` handle, if it is a live
/// instance of this binary.
///
/// # Safety
/// `handle` is null or the handle a host passes as `instance-access`, which
/// is a plugin instance in the same process.
#[cfg(any(target_os = "macos", test))]
unsafe fn instance_state(handle: *const c_void) -> Option<EditorState> {
    let instance = unsafe { (handle as *const Instance).as_ref()? };
    if instance.magic != MAGIC {
        return None;
    }
    instance.editor_state.clone()
}

#[cfg(target_os = "macos")]
pub use ui::{lv2ui_descriptor, Lv2UiDescriptor};

#[cfg(target_os = "macos")]
mod ui {
    //! `ui:CocoaUI`: the editor in an `NSView`, handed to the host as the widget.
    use super::*;
    use openplugin_core::{Editor, EditorHost, Gesture};
    use openplugin_dri::cocoa::EditorView;
    use std::cell::Cell;
    use std::rc::Rc;

    pub type WriteFn = unsafe extern "C" fn(controller: *mut c_void, port: u32, size: u32, protocol: u32, buffer: *const c_void);

    #[repr(C)]
    pub struct Lv2UiDescriptor {
        pub uri: *const c_char,
        pub instantiate: unsafe extern "C" fn(
            *const Lv2UiDescriptor,
            *const c_char,
            *const c_char,
            Option<WriteFn>,
            *mut c_void,
            *mut *mut c_void,
            *const *const Lv2Feature,
        ) -> *mut c_void,
        pub cleanup: unsafe extern "C" fn(*mut c_void),
        pub port_event: unsafe extern "C" fn(*mut c_void, u32, u32, u32, *const c_void),
        pub extension_data: unsafe extern "C" fn(*const c_char) -> *const c_void,
    }

    unsafe impl Send for Lv2UiDescriptor {}
    unsafe impl Sync for Lv2UiDescriptor {}

    #[repr(C)]
    struct Touch {
        handle: *mut c_void,
        touch: Option<unsafe extern "C" fn(handle: *mut c_void, port: u32, grabbed: bool)>,
    }

    #[repr(C)]
    struct Resize {
        handle: *mut c_void,
        ui_resize: Option<unsafe extern "C" fn(handle: *mut c_void, width: i32, height: i32) -> i32>,
    }

    /// The `ui:resize` the UI itself offers the host through `extension_data`.
    struct OfferedResize(Resize);
    // Plain function pointer and a null handle; the host passes the UI's own handle.
    unsafe impl Sync for OfferedResize {}
    static OFFERED: OfferedResize = OfferedResize(Resize { handle: std::ptr::null_mut(), ui_resize: Some(host_resizes) });

    /// The host sets the editor's size. 0 on success; non-zero if the size is
    /// outside the editor's range (a fixed editor takes only its own size).
    unsafe extern "C" fn host_resizes(handle: *mut c_void, width: i32, height: i32) -> i32 {
        guarded(1, || {
            let Some(ui) = (unsafe { (handle as *mut Ui).as_mut() }) else { return 1 };
            let (Ok(w), Ok(h)) = (u32::try_from(width), u32::try_from(height)) else { return 1 };
            if !ui.view.size_range().contains(w, h) {
                return 1;
            }
            ui.view.resize(w, h);
            0
        })
    }

    struct Ui {
        view: EditorView,
        count: usize,
        first: u32,
    }

    #[derive(Clone, Copy)]
    struct Link {
        write: Option<WriteFn>,
        controller: *mut c_void,
        touch: *const Touch,
        first: u32,
    }

    fn host_for(link: Link) -> EditorHost {
        EditorHost::new(move |gesture| unsafe {
            let link = link;
            match gesture {
                Gesture::Begin(i) | Gesture::End(i) => {
                    if let Some(t) = link.touch.as_ref() {
                        if let Some(f) = t.touch {
                            f(t.handle, link.first + i, matches!(gesture, Gesture::Begin(_)));
                        }
                    }
                }
                Gesture::Perform(i, value) => {
                    if let Some(write) = link.write {
                        // Protocol 0: a float written to a control port.
                        write(link.controller, link.first + i, 4, 0, &value as *const f32 as *const c_void);
                    }
                }
            }
        })
    }

    fn ui_uri() -> &'static CStr {
        static URI: OnceLock<CString> = OnceLock::new();
        URI.get_or_init(|| CString::new(format!("{}#ui", uri().to_string_lossy())).unwrap_or_default())
            .as_c_str()
    }

    unsafe extern "C" fn instantiate(
        _descriptor: *const Lv2UiDescriptor,
        plugin_uri: *const c_char,
        _bundle_path: *const c_char,
        write: Option<WriteFn>,
        controller: *mut c_void,
        widget: *mut *mut c_void,
        features: *const *const Lv2Feature,
    ) -> *mut c_void {
        guarded(std::ptr::null_mut(), || {
            let Some(factory) = editor_factory() else { return std::ptr::null_mut() };
            if plugin_uri.is_null() || unsafe { CStr::from_ptr(plugin_uri) } != uri() {
                return std::ptr::null_mut();
            }
            let mut parent = std::ptr::null_mut();
            let mut instance: *const c_void = std::ptr::null();
            let mut touch: *const Touch = std::ptr::null();
            let mut resize: *const Resize = std::ptr::null();
            let mut next = features;
            while !next.is_null() && !unsafe { *next }.is_null() {
                let feature = unsafe { &**next };
                if !feature.uri.is_null() {
                    match unsafe { CStr::from_ptr(feature.uri) }.to_bytes() {
                        b"http://lv2plug.in/ns/extensions/ui#parent" => parent = feature.data,
                        b"http://lv2plug.in/ns/ext/instance-access" => instance = feature.data,
                        b"http://lv2plug.in/ns/extensions/ui#touch" => touch = feature.data as *const Touch,
                        b"http://lv2plug.in/ns/extensions/ui#resize" => resize = feature.data as *const Resize,
                        _ => {}
                    }
                }
                next = unsafe { next.add(1) };
            }
            // A size the host agreed to, for the view to apply on its next tick.
            let granted: Rc<Cell<Option<(u32, u32)>>> = Rc::new(Cell::new(None));
            let ask = {
                let granted = granted.clone();
                let (handle, call) = match unsafe { resize.as_ref() } {
                    Some(r) => (r.handle as usize, r.ui_resize),
                    None => (0, None),
                };
                move |w: u32, h: u32| -> bool {
                    let Some(call) = call else { return false };
                    let (Ok(wi), Ok(hi)) = (i32::try_from(w), i32::try_from(h)) else { return false };
                    if unsafe { call(handle as *mut c_void, wi, hi) } != 0 {
                        return false;
                    }
                    granted.set(Some((w, h)));
                    true
                }
            };
            let first = creator().map(|create| PortMap::of(create().as_ref()).first_param()).unwrap_or(AUDIO_PORTS);
            let Some(mut editor) = factory(host_for(Link { write, controller, touch, first }).with_resize(ask)) else {
                return std::ptr::null_mut();
            };
            editor.bind(unsafe { instance_state(instance) });
            let count = param_count();
            let poll = Box::new(move |editor: &mut dyn Editor| {
                if let Some((w, h)) = granted.take() {
                    let (w, h) = editor.size_range().clamp(w, h);
                    editor.resize(w, h);
                }
            });
            let Some(view) = EditorView::new(editor, Some(poll)) else { return std::ptr::null_mut() };
            if !parent.is_null() {
                unsafe { view.attach(parent) };
            }
            if let Some(r) = unsafe { resize.as_ref() } {
                if let Some(f) = r.ui_resize {
                    let (w, h) = view.size();
                    unsafe { f(r.handle, w as i32, h as i32) };
                }
            }
            if !widget.is_null() {
                unsafe { *widget = view.raw() };
            }
            Box::into_raw(Box::new(Ui { view, count, first })) as *mut c_void
        })
    }

    unsafe extern "C" fn cleanup(handle: *mut c_void) {
        guarded((), || {
            if !handle.is_null() {
                drop(unsafe { Box::from_raw(handle as *mut Ui) });
            }
        })
    }

    unsafe extern "C" fn port_event(handle: *mut c_void, port: u32, size: u32, format: u32, buffer: *const c_void) {
        guarded((), || {
            let Some(ui) = (unsafe { (handle as *mut Ui).as_mut() }) else { return };
            if format != 0 || size != 4 || buffer.is_null() || port < ui.first {
                return;
            }
            let index = (port - ui.first) as usize;
            let value = unsafe { *(buffer as *const f32) };
            if index >= ui.count || !value.is_finite() {
                return;
            }
            if let Some(editor) = ui.view.editor() {
                editor.param_changed(index, value);
            }
        })
    }

    unsafe extern "C" fn extension_data(uri: *const c_char) -> *const c_void {
        guarded(std::ptr::null(), || {
            if !uri.is_null() && unsafe { CStr::from_ptr(uri) }.to_bytes() == b"http://lv2plug.in/ns/extensions/ui#resize" {
                return &OFFERED.0 as *const Resize as *const c_void;
            }
            std::ptr::null()
        })
    }

    fn published() -> &'static Lv2UiDescriptor {
        static DESCRIPTOR: OnceLock<Lv2UiDescriptor> = OnceLock::new();
        DESCRIPTOR.get_or_init(|| Lv2UiDescriptor { uri: ui_uri().as_ptr(), instantiate, cleanup, port_event, extension_data })
    }

    /// `lv2ui_descriptor`. Index 0 is the editor, once [`set_editor`] ran.
    pub fn lv2ui_descriptor(index: u32) -> *const Lv2UiDescriptor {
        guarded(std::ptr::null(), || {
            if index == 0 && creator().is_some() && editor_factory().is_some() {
                published()
            } else {
                std::ptr::null()
            }
        })
    }
}

/// The plugin library's file name in an LV2 bundle on this system.
pub fn binary_file(stem: &str) -> String {
    let ext = if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(target_os = "windows") {
        "dll"
    } else {
        "so"
    };
    format!("{stem}.{ext}")
}

fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A number the way Turtle reads a decimal: always a point, never an exponent.
fn number(v: f32) -> String {
    let text = format!("{v}");
    if text.contains('.') || text.contains("inf") || text.contains("NaN") {
        text
    } else {
        format!("{text}.0")
    }
}

/// A port symbol: letters, digits and underscores, not starting with a digit.
pub fn symbol(id: &str) -> String {
    let mut s: String = id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if s.chars().next().is_none_or(|c| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

/// The parameter groups as LV2 port groups: every path (parents first, see
/// [`openplugin_core::group_paths`]) and its `lv2:symbol`, which is the path
/// as a [`symbol`] made unique with a number (`Filter/Env` is `Filter_Env`).
struct PortGroups {
    paths: Vec<&'static str>,
    symbols: Vec<String>,
}

impl PortGroups {
    fn of(params: &[ParamInfo]) -> Self {
        let paths = openplugin_core::group_paths(params);
        let mut symbols: Vec<String> = Vec::with_capacity(paths.len());
        for path in &paths {
            let base = symbol(path);
            let mut candidate = base.clone();
            let mut n = 2;
            while symbols.contains(&candidate) {
                candidate = format!("{base}_{n}");
                n += 1;
            }
            symbols.push(candidate);
        }
        Self { paths, symbols }
    }

    fn symbol_of(&self, path: &str) -> Option<&str> {
        if path.is_empty() {
            return None;
        }
        self.paths.iter().position(|p| *p == path).map(|at| self.symbols[at].as_str())
    }
}

fn class_of(category: Category, instrument: bool) -> &'static str {
    if instrument {
        return "lv2:InstrumentPlugin";
    }
    match category {
        Category::Eq => "lv2:EQPlugin",
        Category::Dynamics => "lv2:DynamicsPlugin",
        Category::PitchShift => "lv2:PitchPlugin",
        Category::Reverb => "lv2:ReverbPlugin",
        Category::Delay => "lv2:DelayPlugin",
        Category::Modulation => "lv2:ModulatorPlugin",
        Category::Harmonic => "lv2:DistortionPlugin",
        Category::Instrument => "lv2:InstrumentPlugin",
        Category::Dither | Category::Utility => "lv2:UtilityPlugin",
        Category::Other => "lv2:Plugin",
    }
}

/// `minor.micro` for LV2 from a version such as `1.2.3-beta`. LV2 orders
/// releases by these two numbers only.
fn lv2_version(version: &str) -> (u32, u32) {
    let numbers: Vec<u32> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    (numbers.get(1).copied().unwrap_or(0), numbers.get(2).copied().unwrap_or(0))
}

/// `manifest.ttl`: what a host reads to find the plugin and its editor.
/// `binary_stem` is the library's name in the bundle without its extension.
pub fn manifest_ttl(plugin: &dyn Plugin, binary_stem: &str, with_ui: bool) -> String {
    let uri = plugin_uri(plugin);
    let binary = binary_file(binary_stem);
    let mut out = String::new();
    out.push_str("@prefix lv2:  <http://lv2plug.in/ns/lv2core#> .\n");
    out.push_str("@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n");
    out.push_str("@prefix ui:   <http://lv2plug.in/ns/extensions/ui#> .\n\n");
    let _ = writeln!(out, "<{uri}>\n    a lv2:Plugin ;\n    lv2:binary <{binary}> ;\n    rdfs:seeAlso <{binary_stem}.ttl> .");
    if with_ui {
        let _ = writeln!(out, "\n<{uri}#ui>\n    a ui:CocoaUI ;\n    ui:binary <{binary}> ;\n    rdfs:seeAlso <{binary_stem}.ttl> .");
    }
    out
}

/// `<binary_stem>.ttl`: the plugin, its ports, and its editor.
pub fn plugin_ttl(plugin: &dyn Plugin, with_ui: bool) -> String {
    plugin_ttl_sized(plugin, with_ui.then_some(false))
}

/// [`plugin_ttl`] for an editor of known size behaviour: `ui` is `None` for no
/// editor, `Some(false)` for a fixed-size one (`ui:noUserResize`), and
/// `Some(true)` for one the host can resize (`ui:resize` extension data).
pub fn plugin_ttl_sized(plugin: &dyn Plugin, ui: Option<bool>) -> String {
    let with_ui = ui.is_some();
    let resizable = ui == Some(true);
    let info = plugin.info();
    let uri = plugin_uri(plugin);
    let groups = PortGroups::of(info.params);
    let mut out = String::new();
    for (prefix, iri) in [
        ("atom", "http://lv2plug.in/ns/ext/atom#"),
        ("doap", "http://usefulinc.com/ns/doap#"),
        ("foaf", "http://xmlns.com/foaf/0.1/"),
        ("lv2", "http://lv2plug.in/ns/lv2core#"),
        ("midi", "http://lv2plug.in/ns/ext/midi#"),
        ("pg", "http://lv2plug.in/ns/ext/port-groups#"),
        ("pprop", "http://lv2plug.in/ns/ext/port-props#"),
        ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
        ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
        ("time", "http://lv2plug.in/ns/ext/time#"),
        ("ui", "http://lv2plug.in/ns/extensions/ui#"),
        ("units", "http://lv2plug.in/ns/extensions/units#"),
        ("urid", "http://lv2plug.in/ns/ext/urid#"),
        ("work", "http://lv2plug.in/ns/ext/worker#"),
    ] {
        // The port-groups prefix only when there is a group, so a plugin
        // without groups is described exactly as before.
        if prefix == "pg" && groups.paths.is_empty() {
            continue;
        }
        let _ = writeln!(out, "@prefix {prefix}: <{iri}> .");
    }
    out.push('\n');
    let (minor, micro) = lv2_version(info.version);
    let _ = writeln!(out, "<{uri}>");
    let _ = writeln!(out, "    a {} , doap:Project ;", class_of(info.category, info.is_instrument));
    let _ = writeln!(out, "    doap:name {} ;", quoted(info.name));
    if info.url.is_empty() {
        let _ = writeln!(out, "    doap:maintainer [ foaf:name {} ] ;", quoted(info.vendor));
    } else {
        let _ = writeln!(out, "    doap:maintainer [ foaf:name {} ; foaf:homepage <{}> ] ;", quoted(info.vendor), info.url);
    }
    let _ = writeln!(out, "    rdfs:comment {} ;", quoted(&format!("{} {}", info.name, info.version)));
    let _ = writeln!(out, "    lv2:minorVersion {minor} ;");
    let _ = writeln!(out, "    lv2:microVersion {micro} ;");
    out.push_str("    lv2:optionalFeature lv2:hardRTCapable , work:schedule ;\n");
    out.push_str("    lv2:extensionData work:interface ;\n");
    let map = PortMap::of(plugin);
    if map.notes_in || map.notes_out {
        out.push_str("    lv2:requiredFeature urid:map ;\n");
    }
    if with_ui {
        let _ = writeln!(out, "    ui:ui <{uri}#ui> ;");
    }
    out.push_str("    lv2:port\n");
    let mut audio = Vec::new();
    if !map.instrument {
        audio.push(("lv2:InputPort", "in_l", "Input L"));
        audio.push(("lv2:InputPort", "in_r", "Input R"));
    }
    audio.push(("lv2:OutputPort", "out_l", "Output L"));
    audio.push(("lv2:OutputPort", "out_r", "Output R"));
    let mut ports: Vec<String> = audio
        .iter()
        .enumerate()
        .map(|(index, (kind, sym, name))| {
            format!("    [ a lv2:AudioPort , {kind} ; lv2:index {index} ; lv2:symbol \"{sym}\" ; lv2:name \"{name}\" ]")
        })
        .collect();
    for (i, p) in info.params.iter().enumerate() {
        let mut lines = vec![
            "    [ a lv2:ControlPort , lv2:InputPort".to_string(),
            format!("      lv2:index {} ; lv2:symbol \"{}\" ; lv2:name {}", map.first_param() as usize + i, symbol(p.id), quoted(p.name)),
            format!("      lv2:default {} ; lv2:minimum {} ; lv2:maximum {}", number(p.clamp(p.default)), number(p.min), number(p.max)),
        ];
        let whole = |v: f32| v.fract() == 0.0;
        if p.unit == Unit::Toggle {
            lines.push("      lv2:portProperty lv2:toggled".into());
        } else if p.unit == Unit::Choice {
            lines.push("      lv2:portProperty lv2:integer , lv2:enumeration".into());
            for (value, label) in p.choices.iter().enumerate() {
                lines.push(format!("      lv2:scalePoint [ rdfs:label {} ; rdf:value {} ]", quoted(label), number(value as f32)));
            }
        } else if p.steps > 1 && whole(p.min) && (p.max - p.min) == (p.steps - 1) as f32 {
            lines.push("      lv2:portProperty lv2:integer".into());
        } else if p.taper == Taper::Log && p.min > 0.0 {
            lines.push("      lv2:portProperty pprop:logarithmic".into());
        }
        let unit = match p.unit {
            Unit::Seconds => Some("units:s"),
            Unit::Hz => Some("units:hz"),
            Unit::Db => Some("units:db"),
            Unit::Ms => Some("units:ms"),
            Unit::Percent => Some("units:pc"),
            Unit::Semitones => Some("units:semitone12TET"),
            Unit::Cents => Some("units:cent"),
            _ => None,
        };
        if let Some(unit) = unit {
            lines.push(format!("      units:unit {unit}"));
        }
        if let Some(group) = groups.symbol_of(p.host_group()) {
            lines.push(format!("      pg:group <{uri}#group_{group}>"));
        }
        ports.push(format!("{} ]", lines.join(" ;\n")));
    }
    ports.push(format!(
        "    [ a lv2:ControlPort , lv2:InputPort ;\n      lv2:index {} ; lv2:symbol \"freewheel\" ; lv2:name \"Freewheel\" ;\n      lv2:default 0.0 ; lv2:minimum 0.0 ; lv2:maximum 1.0 ;\n      lv2:designation lv2:freeWheeling ;\n      lv2:portProperty lv2:toggled , lv2:connectionOptional ]",
        map.freewheel()
    ));
    if let Some(index) = map.atom_in() {
        ports.push(format!(
            "    [ a atom:AtomPort , lv2:InputPort ;\n      atom:bufferType atom:Sequence ;\n      atom:supports midi:MidiEvent , time:Position ;\n      lv2:index {index} ; lv2:symbol \"events_in\" ; lv2:name \"Events In\" ;\n      lv2:portProperty lv2:connectionOptional ]"
        ));
    }
    if let Some(index) = map.atom_out() {
        ports.push(format!(
            "    [ a atom:AtomPort , lv2:OutputPort ;\n      atom:bufferType atom:Sequence ;\n      atom:supports midi:MidiEvent ;\n      lv2:index {index} ; lv2:symbol \"events_out\" ; lv2:name \"Events Out\" ;\n      lv2:portProperty lv2:connectionOptional ]"
        ));
    }
    for channel in 0..map.sidechain {
        ports.push(format!(
            "    [ a lv2:AudioPort , lv2:InputPort ;\n      lv2:index {} ; lv2:symbol \"sidechain_{}\" ; lv2:name \"Sidechain {}\" ;\n      lv2:portProperty lv2:isSideChain , lv2:connectionOptional ]",
            map.sidechain_first() + channel,
            channel + 1,
            channel + 1
        ));
    }
    ports.push(format!(
        "    [ a lv2:ControlPort , lv2:OutputPort ;\n      lv2:index {} ; lv2:symbol \"latency\" ; lv2:name \"Latency\" ;\n      lv2:minimum 0 ; units:unit units:frame ;\n      lv2:designation lv2:latency ;\n      lv2:portProperty lv2:reportsLatency , lv2:integer , lv2:connectionOptional ]",
        map.latency()
    ));
    out.push_str(&ports.join(" ,\n"));
    out.push_str(" .\n");
    // One pg:Group per group path, parents included: the last name is its
    // label, and a nested group is pg:subGroupOf its parent.
    for (path, sym) in groups.paths.iter().zip(&groups.symbols) {
        let _ = writeln!(out, "\n<{uri}#group_{sym}>");
        out.push_str("    a pg:Group , pg:InputGroup ;\n");
        let _ = write!(out, "    lv2:symbol \"{sym}\" ;\n    lv2:name {}", quoted(openplugin_core::group_name(path)));
        if let Some(parent) = groups.symbol_of(openplugin_core::group_parent(path)) {
            let _ = write!(out, " ;\n    pg:subGroupOf <{uri}#group_{parent}>");
        }
        out.push_str(" .\n");
    }
    if with_ui {
        let _ = writeln!(out, "\n<{uri}#ui>");
        out.push_str("    a ui:CocoaUI ;\n");
        if resizable {
            out.push_str("    lv2:optionalFeature <http://lv2plug.in/ns/ext/instance-access> , ui:parent , ui:touch , ui:resize ;\n");
            out.push_str("    lv2:extensionData ui:resize .\n");
        } else {
            out.push_str(
                "    lv2:optionalFeature <http://lv2plug.in/ns/ext/instance-access> , ui:parent , ui:touch , ui:resize , ui:noUserResize .\n",
            );
        }
    }
    out
}

/// Write `manifest.ttl` and `<binary_stem>.ttl` into an LV2 bundle folder.
/// The editor is described on macOS when `with_ui` is set.
pub fn write_bundle(plugin: &dyn Plugin, dir: &std::path::Path, binary_stem: &str, with_ui: bool) -> std::io::Result<()> {
    let with_ui = with_ui && cfg!(target_os = "macos");
    std::fs::write(dir.join("manifest.ttl"), manifest_ttl(plugin, binary_stem, with_ui))?;
    std::fs::write(dir.join(format!("{binary_stem}.ttl")), plugin_ttl(plugin, with_ui))
}

/// [`write_bundle`] for a plugin whose editor is made by `editor`: the editor
/// is asked for its size range, and the description lists `ui:noUserResize`
/// only if it is fixed. Without an editor (`None`, or one that cannot be made)
/// no editor is described.
pub fn write_bundle_for_editor(
    plugin: &dyn Plugin,
    dir: &std::path::Path,
    binary_stem: &str,
    editor: Option<EditorFactory>,
) -> std::io::Result<()> {
    let resizable = editor
        .and_then(|make| make(openplugin_core::EditorHost::detached()))
        .map(|e| !e.size_range().is_fixed());
    let ui = if cfg!(target_os = "macos") { resizable } else { None };
    std::fs::write(dir.join("manifest.ttl"), manifest_ttl(plugin, binary_stem, ui.is_some()))?;
    std::fs::write(dir.join(format!("{binary_stem}.ttl")), plugin_ttl_sized(plugin, ui))
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
        // The in-process harness has no failure path, so an unsupported rate
        // prepares at 48 kHz. The instantiate path above rejects it instead.
        let rate = instantiate_rate(f64::from(sample_rate)).unwrap_or(48_000.0);
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
///
/// `export_lv2!(P, editor = make_editor)` also exports `lv2ui_descriptor`
/// (a `ui:CocoaUI`, macOS). Describe it with [`write_bundle`]`(.., true)`.
#[macro_export]
macro_rules! export_lv2 {
    ($plugin:ty, editor = $editor:path) => {
        $crate::export_lv2!($plugin);

        #[cfg(target_os = "macos")]
        #[unsafe(no_mangle)]
        pub extern "C" fn lv2ui_descriptor(index: u32) -> *const $crate::lv2::Lv2UiDescriptor {
            $crate::lv2::set_creator(__openplugin_lv2_create);
            $crate::lv2::set_editor($editor);
            $crate::lv2::lv2ui_descriptor(index)
        }
    };
    ($plugin:ty) => {
        fn __openplugin_lv2_create() -> ::std::boxed::Box<dyn $crate::PluginForExport> {
            ::std::boxed::Box::new(<$plugin as ::std::default::Default>::default())
        }

        #[unsafe(no_mangle)]
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
    fn control_ports_reach_the_plugin_and_audio_runs_in_place() {
        set_creator(|| Box::new(Gain { db: 0.0, linear: 1.0 }));
        let desc = descriptor(0);
        unsafe {
            let raw = ((*desc).instantiate)(desc, 48_000.0, std::ptr::null(), std::ptr::null());
            assert!(!raw.is_null());
            let mut audio = vec![0.5f32; 10_000];
            let gain: *mut f32 = Box::into_raw(Box::new(6.0f32));
            ((*desc).connect_port)(raw, 0, audio.as_mut_ptr() as *mut c_void);
            ((*desc).connect_port)(raw, 2, audio.as_mut_ptr() as *mut c_void);
            ((*desc).connect_port)(raw, AUDIO_PORTS, gain as *mut c_void);
            ((*desc).connect_port)(raw, AUDIO_PORTS + 9, std::ptr::null_mut()); // no such port: ignored
            ((*desc).activate)(raw);
            // Longer than one internal block, in place.
            ((*desc).run)(raw, 10_000);
            let expected = 0.5 * 10f32.powf(6.0 / 20.0);
            assert!((audio[0] - expected).abs() < 1e-4);
            assert!((audio[9_999] - expected).abs() < 1e-4);
            *gain = 99.0; // clamped by the plane
            ((*desc).run)(raw, 1);
            assert_eq!((*(raw as *const Instance)).plane.load(0), Some(24.0));
            assert!(instance_state(raw).is_none(), "this plugin shares nothing");
            ((*desc).cleanup)(raw);
            drop(Box::from_raw(gain));
        }
    }

    #[test]
    fn instance_access_is_checked() {
        let not_ours = [0u64; 32];
        assert!(unsafe { instance_state(not_ours.as_ptr() as *const c_void) }.is_none());
        assert!(unsafe { instance_state(std::ptr::null()) }.is_none());
    }

    #[test]
    fn ttl_describes_every_port() {
        static MODES: [&str; 2] = ["Soft", "Hard"];
        static MORE: [ParamInfo; 4] = [
            ParamInfo::new("gain", "Gain \"Out\"", -24.0, 24.0, 0.0, Unit::Db),
            ParamInfo::new("mode", "Mode", 0.0, 1.0, 0.0, Unit::None).with_choices(&MODES),
            ParamInfo::new("on", "On", 0.0, 1.0, 1.0, Unit::Toggle),
            ParamInfo::new("2freq", "Freq", 20.0, 20_000.0, 1_000.0, Unit::Hz).with_taper(openplugin_core::Taper::Log),
        ];
        static MORE_INFO: PluginInfo = PluginInfo { params: &MORE, category: Category::Reverb, version: "1.4.2-beta", ..INFO };
        struct Wide;
        impl Plugin for Wide {
            fn info(&self) -> &'static PluginInfo {
                &MORE_INFO
            }
            fn prepare(&mut self, _: f32, _: usize, _: usize) {}
            fn reset(&mut self) {}
            fn set_param(&mut self, _: &str, _: f32) -> bool {
                true
            }
            fn param(&self, _: &str) -> Option<f32> {
                None
            }
            fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
            fn lv2_uri(&self) -> Option<&'static str> {
                Some("https://example.com/plugins/wide")
            }
        }
        let ttl = plugin_ttl(&Wide, true);
        assert!(ttl.contains("<https://example.com/plugins/wide>"));
        assert!(ttl.contains("lv2:ReverbPlugin"));
        assert!(ttl.contains("lv2:minorVersion 4 ;"));
        assert!(ttl.contains("lv2:microVersion 2 ;"));
        assert!(ttl.contains(r#"lv2:name "Gain \"Out\"""#));
        assert!(ttl.contains(r#"lv2:index 7 ; lv2:symbol "_2freq""#));
        assert!(ttl.contains("pprop:logarithmic"));
        assert!(ttl.contains("lv2:toggled"));
        assert!(ttl.contains(r#"rdfs:label "Hard" ; rdf:value 1.0"#));
        assert!(ttl.contains("units:unit units:db"));
        assert!(ttl.contains("a ui:CocoaUI"));
        assert!(ttl.trim_end().ends_with('.'));
        // A fixed editor lists ui:noUserResize; a resizable one offers ui:resize instead.
        assert!(ttl.contains("ui:noUserResize") && !ttl.contains("lv2:extensionData ui:resize"));
        let sized = plugin_ttl_sized(&Wide, Some(true));
        assert!(!sized.contains("noUserResize") && sized.contains("lv2:extensionData ui:resize .") && sized.trim_end().ends_with('.'));
        assert!(!plugin_ttl_sized(&Wide, None).contains("a ui:CocoaUI"));
        // Every port block is closed and separated.
        assert_eq!(ttl.matches("lv2:index").count(), 11);
        assert_eq!(ttl.matches(" ] ,\n").count(), 10);
        // The latency output comes last, after the freewheel port (index 9 here).
        assert!(ttl.contains("lv2:index 10 ; lv2:symbol \"latency\"") && ttl.contains("lv2:designation lv2:latency"));
        let manifest = manifest_ttl(&Wide, "Wide", false);
        assert!(manifest.contains("lv2:binary <Wide."));
        assert!(!manifest.contains("CocoaUI"));
        assert_eq!(lv2_version("2"), (0, 0));
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
