//! Per-note expression through the Rust entries (roadmap 0.7 task 4): CLAP
//! note expressions, CLAP polyphonic modulation, MPE over CLAP's MIDI dialect,
//! MPE through the LV2 entry, and the shell ABI the VST3, Audio Unit and AAX
//! shells call (ABI 7 note ids and expressions, and MPE over plain MIDI as the
//! Audio Unit and AAX pass it). The VST3 half is the native
//! `vst3_expression_proof`.
//!
//! Each proof plays two notes, sends pitch, pressure and timbre (brightness)
//! to one of them, and reads every voice back from the synth's [`VoiceView`]:
//! only that voice changed. Each has a negative control (the same events for
//! another note id, or another channel, leave every voice as it was), hostile
//! input is refused, and the audio path is measured for allocations. CLAP
//! `voice-info` is read, and every finished voice sends one
//! `CLAP_EVENT_NOTE_END` with its note's id, port, channel and key.

use std::cell::{Cell, RefCell};
use std::ffi::{c_void, CStr};
use std::sync::Arc;

use example_expression_synth::{ExpressionSynth, VoiceSnapshot, VoiceView, TONE};
use openplugin::core::NoteRef;
use openplugin::targets::clap_abi::*;
use openplugin::targets::lv2::{self, Lv2Feature, PortMap};
use openplugin::host::shell::{self, OpEvent, OpExpressionInfo, OpProcess, ShellInstance};
use openplugin::targets::ClapAdapter;

openplugin_testing::install_allocator!();

const FRAMES: usize = 512;
const RATE: f64 = 48_000.0;

fn view_of(state: Option<openplugin::core::EditorState>) -> Arc<VoiceView> {
    state.expect("the synth shares its voices").downcast::<VoiceView>().expect("a VoiceView")
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// Everything but `active`, for comparing voices across blocks.
fn sound(v: &VoiceSnapshot) -> [f32; 6] {
    [v.pitch, v.pressure, v.brightness, v.volume, v.pan, v.tone_mod]
}

// ---- CLAP -----------------------------------------------------------------------

/// One event in a 64-byte, 8-aligned slot.
#[derive(Clone, Copy)]
#[repr(C, align(8))]
struct Slot([u8; 64]);

impl Slot {
    fn of<T>(event: &T) -> Self {
        assert!(std::mem::size_of::<T>() <= 64);
        let mut slot = Slot([0; 64]);
        unsafe { std::ptr::copy_nonoverlapping(event as *const T as *const u8, slot.0.as_mut_ptr(), std::mem::size_of::<T>()) };
        slot
    }
}

struct Events {
    slots: Vec<Slot>,
    list: clap_input_events,
}

unsafe extern "C" fn events_size(list: *const clap_input_events) -> u32 {
    unsafe { (*((*list).ctx as *const Vec<Slot>)).len() as u32 }
}

unsafe extern "C" fn events_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let slots = unsafe { &*((*list).ctx as *const Vec<Slot>) };
    slots.get(index as usize).map_or(std::ptr::null(), |s| s.0.as_ptr() as *const clap_event_header)
}

impl Events {
    fn new(slots: Vec<Slot>) -> Box<Self> {
        let mut events = Box::new(Self {
            slots,
            list: clap_input_events { ctx: std::ptr::null_mut(), size: Some(events_size), get: Some(events_get) },
        });
        events.list.ctx = (&mut events.slots) as *mut Vec<Slot> as *mut c_void;
        events
    }
}

fn header(type_: u16, size: usize) -> clap_event_header {
    clap_event_header { size: size as u32, time: 0, space_id: 0, type_, flags: 0 }
}

fn note(type_: u16, id: i32, channel: i16, key: i16) -> Slot {
    Slot::of(&clap_event_note {
        header: header(type_, std::mem::size_of::<clap_event_note>()),
        note_id: id,
        port_index: 0,
        channel,
        key,
        velocity: 0.8,
    })
}

fn expression(expression_id: i32, id: i32, channel: i16, key: i16, value: f64) -> Slot {
    Slot::of(&clap_event_note_expression {
        header: header(EVENT_NOTE_EXPRESSION, std::mem::size_of::<clap_event_note_expression>()),
        expression_id,
        note_id: id,
        port_index: -1,
        channel,
        key,
        value,
    })
}

fn modulation(param_id: u32, id: i32, key: i16, amount: f64) -> Slot {
    Slot::of(&clap_event_param_mod {
        header: header(EVENT_PARAM_MOD, std::mem::size_of::<clap_event_param_mod>()),
        param_id,
        cookie: std::ptr::null_mut(),
        note_id: id,
        port_index: -1,
        channel: -1,
        key,
        amount,
    })
}

fn midi(data: [u8; 3]) -> Slot {
    Slot::of(&clap_event_midi { header: header(EVENT_MIDI, std::mem::size_of::<clap_event_midi>()), port_index: 0, data })
}

const TUNING: i32 = 2;
const BRIGHTNESS: i32 = 5;
const PRESSURE: i32 = 6;

struct Clap {
    adapter: ClapAdapter<ExpressionSynth>,
    plugin: *const clap_plugin,
    view: Arc<VoiceView>,
    left: Box<[f32; FRAMES]>,
    right: Box<[f32; FRAMES]>,
    ptrs: Box<[*mut f32; 2]>,
}

impl Clap {
    fn new() -> Self {
        let mut adapter = ClapAdapter::new(ExpressionSynth::default());
        let plugin = adapter.as_plugin();
        assert!(unsafe { (*plugin).init.unwrap()(plugin) });
        assert!(adapter.activate(RATE, 1, FRAMES as u32));
        assert!(unsafe { (*plugin).start_processing.unwrap()(plugin) });
        let view = view_of(adapter.editor_state());
        let mut left = Box::new([0.0; FRAMES]);
        let mut right = Box::new([0.0; FRAMES]);
        let ptrs = Box::new([left.as_mut_ptr(), right.as_mut_ptr()]);
        Self { adapter, plugin, view, left, right, ptrs }
    }

    fn ext<T>(&self, id: &CStr) -> *const T {
        unsafe { (*self.plugin).get_extension.unwrap()(self.plugin, id.as_ptr()) as *const T }
    }

    /// One block carrying `events`; returns the process status.
    fn block(&mut self, events: &Events) -> i32 {
        self.block_out(events, std::ptr::null())
    }

    /// The same, with the host's output event list.
    fn block_out(&mut self, events: &Events, out: *const clap_output_events) -> i32 {
        let mut output = clap_audio_buffer {
            data32: self.ptrs.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let process = clap_process {
            steady_time: 0,
            frames_count: FRAMES as u32,
            transport: std::ptr::null(),
            audio_inputs: std::ptr::null(),
            audio_outputs: &mut output,
            audio_inputs_count: 0,
            audio_outputs_count: 1,
            in_events: &events.list,
            out_events: out,
        };
        let _ = (&self.left, &self.right);
        unsafe { (*self.plugin).process.unwrap()(self.plugin, &process) }
    }

    fn play(&mut self, slots: Vec<Slot>) -> Vec<VoiceSnapshot> {
        let events = Events::new(slots);
        assert_eq!(self.block(&events), PROCESS_CONTINUE);
        let _ = &self.adapter;
        self.view.active()
    }
}

/// The host's output list: records every `CLAP_EVENT_NOTE_END` (and counts the rest) into
/// room reserved up front, so pushing never allocates.
struct Out {
    ends: RefCell<Vec<(u32, NoteRef)>>,
    others: Cell<usize>,
    list: clap_output_events,
}

unsafe extern "C" fn out_push(list: *const clap_output_events, event: *const clap_event_header) -> bool {
    let out = unsafe { &*((*list).ctx as *const Out) };
    let header = unsafe { &*event };
    if header.type_ != EVENT_NOTE_END {
        out.others.set(out.others.get() + 1);
        return true;
    }
    assert_eq!(header.size as usize, std::mem::size_of::<clap_event_note>());
    assert_eq!(header.space_id, 0);
    let note = unsafe { &*(event as *const clap_event_note) };
    let mut ends = out.ends.borrow_mut();
    if ends.len() == ends.capacity() {
        return false;
    }
    ends.push((header.time, NoteRef::new(note.note_id, note.port_index, note.channel, note.key)));
    true
}

impl Out {
    fn new() -> Box<Self> {
        let mut out = Box::new(Self {
            ends: RefCell::new(Vec::with_capacity(64)),
            others: Cell::new(0),
            list: clap_output_events { ctx: std::ptr::null_mut(), try_push: Some(out_push) },
        });
        out.list.ctx = (&*out) as *const Out as *mut c_void;
        out
    }

    fn ends(&self) -> Vec<(u32, NoteRef)> {
        self.ends.borrow().clone()
    }
}

fn by_id(voices: &[VoiceSnapshot], id: i32) -> VoiceSnapshot {
    *voices.iter().find(|v| v.id == id).unwrap_or_else(|| panic!("no voice with note id {id}: {voices:?}"))
}

#[test]
fn clap_note_expressions_change_only_the_voice_with_that_note_id() {
    let mut clap = Clap::new();
    let start = clap.play(vec![note(EVENT_NOTE_ON, 1, 0, 60), note(EVENT_NOTE_ON, 2, 0, 64)]);
    assert_eq!(start.len(), 2);
    let other = by_id(&start, 2);
    assert!(close(other.pitch, 64.0) && close(other.pressure, 0.0) && close(other.brightness, 0.0));

    let voices = clap.play(vec![
        expression(TUNING, 1, -1, -1, 2.0),
        expression(PRESSURE, 1, -1, -1, 0.75),
        expression(BRIGHTNESS, 1, -1, -1, 0.5),
    ]);
    let one = by_id(&voices, 1);
    assert!(close(one.pitch, 62.0) && close(one.pressure, 0.75) && close(one.brightness, 0.5), "{one:?}");
    assert_eq!(sound(&by_id(&voices, 2)), sound(&other), "the other voice is untouched");

    // Negative control: the same expressions for a note id no voice has.
    let before: Vec<_> = voices.iter().map(sound).collect();
    let after = clap.play(vec![
        expression(TUNING, 99, -1, -1, -12.0),
        expression(PRESSURE, 99, -1, -1, 1.0),
        expression(BRIGHTNESS, 99, -1, -1, 1.0),
    ]);
    assert_eq!(after.iter().map(sound).collect::<Vec<_>>(), before);

    // No note id: the key decides (the CLAP wildcard rule).
    let voices = clap.play(vec![expression(TUNING, -1, 0, 64, -1.0)]);
    assert!(close(by_id(&voices, 2).pitch, 63.0) && close(by_id(&voices, 1).pitch, 62.0));

    // Hostile: NaN, an unknown expression id, an impossible key or channel,
    // a note id below -1, and a header too short for the struct change nothing.
    let before: Vec<_> = voices.iter().map(sound).collect();
    let mut short = expression(TUNING, 1, -1, -1, 40.0);
    short.0[0] = 20;
    let after = clap.play(vec![
        expression(TUNING, 1, -1, -1, f64::NAN),
        expression(PRESSURE, 1, -1, -1, f64::INFINITY),
        expression(42, 1, -1, -1, 1.0),
        expression(TUNING, -1, 0, 500, 3.0),
        expression(TUNING, -1, 99, 60, 3.0),
        expression(TUNING, -7, -1, 60, 3.0),
        short,
    ]);
    assert_eq!(after.iter().map(sound).collect::<Vec<_>>(), before);
    // Out of range is clamped: +500 semitones is +120.
    let voices = clap.play(vec![expression(TUNING, 1, -1, -1, 500.0)]);
    assert!(close(by_id(&voices, 1).pitch, 180.0));

    // The note-off by id releases its own voice only.
    clap.play(vec![note(EVENT_NOTE_OFF, 1, 0, 60)]);
    let silent = Events::new(Vec::new());
    for _ in 0..8 {
        clap.block(&silent);
    }
    let left = clap.view.active();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, 2);
}

#[test]
fn clap_polyphonic_modulation_reaches_one_voice_and_the_flags_say_so() {
    let mut clap = Clap::new();
    let params = clap.ext::<clap_plugin_params>(c"clap.params");
    let mut info: clap_param_info = unsafe { std::mem::zeroed() };
    assert!(unsafe { (*params).get_info.unwrap()(clap.plugin, TONE as u32, &mut info) });
    let poly = PARAM_IS_MODULATABLE
        | PARAM_IS_MODULATABLE_PER_NOTE_ID
        | PARAM_IS_MODULATABLE_PER_KEY
        | PARAM_IS_MODULATABLE_PER_CHANNEL
        | PARAM_IS_MODULATABLE_PER_PORT;
    assert_eq!(info.flags & poly, poly);
    assert!(unsafe { (*params).get_info.unwrap()(clap.plugin, 0, &mut info) });
    assert_eq!(info.flags & poly, 0, "gain does not offer modulation");

    let start = clap.play(vec![note(EVENT_NOTE_ON, 1, 0, 60), note(EVENT_NOTE_ON, 2, 0, 64)]);
    let other = by_id(&start, 2);
    let voices = clap.play(vec![modulation(TONE as u32, 1, -1, 0.5)]);
    assert!(close(by_id(&voices, 1).tone_mod, 0.5));
    assert_eq!(sound(&by_id(&voices, 2)), sound(&other));
    // Negative control and refused input: another note id, a parameter that is
    // not modulatable, an unknown parameter, NaN.
    let before: Vec<_> = voices.iter().map(sound).collect();
    let after = clap.play(vec![
        modulation(TONE as u32, 99, -1, 0.9),
        modulation(0, 1, -1, 0.9),
        modulation(77, 1, -1, 0.9),
        modulation(TONE as u32, 1, -1, f64::NAN),
    ]);
    assert_eq!(after.iter().map(sound).collect::<Vec<_>>(), before);
    // Per key, without an id.
    let voices = clap.play(vec![modulation(TONE as u32, -1, 64, 0.25)]);
    assert!(close(by_id(&voices, 2).tone_mod, 0.25) && close(by_id(&voices, 1).tone_mod, 0.5));
}

#[test]
fn clap_mpe_over_midi_bends_presses_and_brightens_one_channel() {
    let mut clap = Clap::new();
    let ports = clap.ext::<clap_plugin_note_ports>(c"clap.note-ports");
    let mut info: clap_note_port_info = unsafe { std::mem::zeroed() };
    assert!(unsafe { (*ports).get.unwrap()(clap.plugin, 0, true, &mut info) });
    assert_eq!(info.supported_dialects & NOTE_DIALECT_MIDI_MPE, NOTE_DIALECT_MIDI_MPE);
    assert_eq!(info.preferred_dialect, NOTE_DIALECT_CLAP);

    // The same key on member channels 2 and 3 (indices 1 and 2): two voices.
    let start = clap.play(vec![midi([0x91, 60, 100]), midi([0x92, 60, 100])]);
    assert_eq!(start.len(), 2);
    let other = clap.view.find(2, 60).unwrap();
    let voices = clap.play(vec![midi([0xE1, 0x7F, 0x7F]), midi([0xD1, 127, 0]), midi([0xB1, 74, 127])]);
    let one = voices.iter().find(|v| v.channel == 1).copied().unwrap();
    assert!(close(one.pitch, 108.0) && close(one.pressure, 1.0) && close(one.brightness, 1.0), "{one:?}");
    assert_eq!(sound(&clap.view.find(2, 60).unwrap()), sound(&other));
    // Negative control: a member channel with no note.
    let before: Vec<_> = voices.iter().map(sound).collect();
    let after = clap.play(vec![midi([0xE5, 0, 0]), midi([0xD5, 64, 0]), midi([0xB5, 74, 0])]);
    assert_eq!(after.iter().map(sound).collect::<Vec<_>>(), before);
}

fn note_at(type_: u16, id: i32, port: i16, channel: i16, key: i16) -> Slot {
    Slot::of(&clap_event_note {
        header: header(type_, std::mem::size_of::<clap_event_note>()),
        note_id: id,
        port_index: port,
        channel,
        key,
        velocity: 0.8,
    })
}

/// Blocks with no events until `done` holds for the recorded output, or `limit` blocks.
fn run_until(clap: &mut Clap, out: &Out, limit: usize, done: impl Fn(&Out) -> bool) {
    let quiet = Events::new(Vec::new());
    for _ in 0..limit {
        if done(out) {
            return;
        }
        assert_eq!(clap.block_out(&quiet, &out.list), PROCESS_CONTINUE);
    }
}

#[test]
fn clap_voice_info_reports_the_voices() {
    let clap = Clap::new();
    let ext = clap.ext::<clap_plugin_voice_info>(c"clap.voice-info");
    assert!(!ext.is_null(), "a plugin that describes its voices offers voice-info");
    let mut info = clap_voice_info { voice_count: 99, voice_capacity: 99, flags: 99 };
    assert!(unsafe { (*ext).get.unwrap()(clap.plugin, &mut info) });
    assert_eq!(info, clap_voice_info { voice_count: 8, voice_capacity: 8, flags: 0 });
    assert!(!unsafe { (*ext).get.unwrap()(clap.plugin, std::ptr::null_mut()) });
}

#[test]
fn clap_output_of_an_instrument_has_no_in_place_pair() {
    let clap = Clap::new();
    let ports = clap.ext::<clap_plugin_audio_ports>(c"clap.audio-ports");
    assert_eq!(unsafe { (*ports).count.unwrap()(clap.plugin, true) }, 0);
    let mut info: clap_audio_port_info = unsafe { std::mem::zeroed() };
    assert!(unsafe { (*ports).get.unwrap()(clap.plugin, 0, false, &mut info) });
    assert_eq!(info.in_place_pair, CLAP_INVALID_ID, "there is no input port to pair with");
}

#[test]
fn clap_sends_one_note_end_per_finished_voice_and_none_for_a_held_one() {
    let mut clap = Clap::new();
    let out = Out::new();
    let on = Events::new(vec![note_at(EVENT_NOTE_ON, 11, 0, 3, 60), note_at(EVENT_NOTE_ON, 12, 1, 9, 64)]);
    assert_eq!(clap.block_out(&on, &out.list), PROCESS_CONTINUE);
    assert_eq!(clap.view.active().len(), 2);
    // Release note 11 only; its 50 ms release ends within a few blocks.
    let off = Events::new(vec![note_at(EVENT_NOTE_OFF, 11, 0, 3, 60)]);
    assert_eq!(clap.block_out(&off, &out.list), PROCESS_CONTINUE);
    run_until(&mut clap, &out, 40, |o| !o.ends.borrow().is_empty());
    // Ten blocks more: a second end for 11 would show, and one for 12 must not.
    for _ in 0..10 {
        assert_eq!(clap.block_out(&Events::new(Vec::new()), &out.list), PROCESS_CONTINUE);
    }
    assert_eq!(out.ends(), vec![(FRAMES as u32 - 1, NoteRef::new(11, 0, 3, 60))], "one end, with the note's id, port, channel and key");
    // Negative control: the held voice sent nothing and still sounds.
    let held = clap.view.active();
    assert_eq!(held.len(), 1);
    assert_eq!((held[0].id, held[0].channel, held[0].key), (12, 9, 64));

    out.ends.borrow_mut().clear();
    let off = Events::new(vec![note_at(EVENT_NOTE_OFF, 12, 1, 9, 64)]);
    assert_eq!(clap.block_out(&off, &out.list), PROCESS_CONTINUE);
    run_until(&mut clap, &out, 40, |o| !o.ends.borrow().is_empty());
    for _ in 0..10 {
        assert_eq!(clap.block_out(&Events::new(Vec::new()), &out.list), PROCESS_CONTINUE);
    }
    assert_eq!(out.ends(), vec![(FRAMES as u32 - 1, NoteRef::new(12, 1, 9, 64))]);
    assert!(clap.view.active().is_empty());
    assert_eq!(out.others.get(), 0);
}

#[test]
fn clap_sends_a_note_end_for_a_stolen_voice_and_for_mpe_notes() {
    let mut clap = Clap::new();
    let out = Out::new();
    // Nine held notes on eight voices: the oldest (id 100) is stolen at once.
    let notes: Vec<Slot> = (0..9).map(|i| note_at(EVENT_NOTE_ON, 100 + i, 0, 0, 48 + i as i16)).collect();
    assert_eq!(clap.block_out(&Events::new(notes), &out.list), PROCESS_CONTINUE);
    assert_eq!(out.ends(), vec![(FRAMES as u32 - 1, NoteRef::new(100, 0, 0, 48))]);
    assert_eq!(clap.view.active().len(), 8);
    // MPE over the MIDI dialect: a note on member channel 2 ends with no id, its channel and key.
    let mut clap = Clap::new();
    let out = Out::new();
    assert_eq!(clap.block_out(&Events::new(vec![midi([0x91, 67, 100])]), &out.list), PROCESS_CONTINUE);
    assert_eq!(clap.block_out(&Events::new(vec![midi([0x81, 67, 0])]), &out.list), PROCESS_CONTINUE);
    run_until(&mut clap, &out, 40, |o| !o.ends.borrow().is_empty());
    assert_eq!(out.ends().len(), 1);
    let (_, end) = out.ends()[0];
    assert_eq!((end.id, end.channel, end.key), (-1, 1, 67));
    // A host with no output list loses the end, and a later block does not send it late.
    let mut clap = Clap::new();
    let quiet = Events::new(Vec::new());
    clap.block(&Events::new(vec![note(EVENT_NOTE_ON, 7, 0, 60)]));
    clap.block(&Events::new(vec![note(EVENT_NOTE_OFF, 7, 0, 60)]));
    for _ in 0..20 {
        clap.block(&quiet);
    }
    let out = Out::new();
    clap.block_out(&quiet, &out.list);
    assert!(out.ends().is_empty());
}

// ---- LV2 ------------------------------------------------------------------------

#[repr(C)]
struct UridMap {
    handle: *mut c_void,
    map: unsafe extern "C" fn(*mut c_void, *const std::ffi::c_char) -> u32,
}

unsafe extern "C" fn map_uri(handle: *mut c_void, uri: *const std::ffi::c_char) -> u32 {
    let table = unsafe { &mut *(handle as *mut Vec<String>) };
    let uri = unsafe { CStr::from_ptr(uri) }.to_string_lossy().into_owned();
    match table.iter().position(|known| *known == uri) {
        Some(index) => index as u32 + 1,
        None => {
            table.push(uri);
            table.len() as u32
        }
    }
}

struct Lv2 {
    desc: &'static lv2::Lv2Descriptor,
    instance: *mut c_void,
    #[allow(clippy::box_collection)] // the Vec header must keep its address for the map handle
    table: Box<Vec<String>>,
    _map: Box<UridMap>,
    _feature: Box<Lv2Feature>,
    _features: Box<[*const Lv2Feature; 2]>,
    audio: Box<[[f32; FRAMES]; 2]>,
    controls: Box<[f32; 4]>,
    atom: Box<[u64; 512]>,
}

impl Lv2 {
    fn new() -> Self {
        let mut table: Box<Vec<String>> = Box::default();
        let map = Box::new(UridMap { handle: (&mut *table) as *mut Vec<String> as *mut c_void, map: map_uri });
        let feature = Box::new(Lv2Feature { uri: c"http://lv2plug.in/ns/ext/urid#map".as_ptr(), data: (&*map) as *const UridMap as *mut c_void });
        let features = Box::new([&*feature as *const Lv2Feature, std::ptr::null()]);
        let desc = unsafe { &*example_expression_synth::lv2_descriptor(0) };
        let instance = (desc.instantiate)(desc, RATE, std::ptr::null(), features.as_ptr());
        assert!(!instance.is_null());
        let mut rig = Self {
            desc,
            instance,
            table,
            _map: map,
            _feature: feature,
            _features: features,
            audio: Box::new([[0.0; FRAMES]; 2]),
            controls: Box::new([-6.0, 0.0, 0.0, 0.0]),
            atom: Box::new([0; 512]),
        };
        let ports = PortMap::of(&ExpressionSynth::default());
        (desc.connect_port)(instance, 0, rig.audio[0].as_mut_ptr().cast());
        (desc.connect_port)(instance, 1, rig.audio[1].as_mut_ptr().cast());
        (desc.connect_port)(instance, ports.first_param(), (&mut rig.controls[0] as *mut f32).cast());
        (desc.connect_port)(instance, ports.first_param() + 1, (&mut rig.controls[1] as *mut f32).cast());
        (desc.connect_port)(instance, ports.freewheel(), (&mut rig.controls[2] as *mut f32).cast());
        (desc.connect_port)(instance, ports.latency(), (&mut rig.controls[3] as *mut f32).cast());
        (desc.connect_port)(instance, ports.atom_in().expect("an atom input"), rig.atom.as_mut_ptr().cast());
        (desc.activate)(instance);
        rig
    }

    fn urid(&mut self, uri: &str) -> u32 {
        let c = std::ffi::CString::new(uri).unwrap();
        unsafe { map_uri((&mut *self.table) as *mut Vec<String> as *mut c_void, c.as_ptr()) }
    }

    /// Write an atom sequence of MIDI messages into the input port's buffer.
    fn sequence(&mut self, messages: &[[u8; 3]]) {
        let sequence = self.urid("http://lv2plug.in/ns/ext/atom#Sequence");
        let kind = self.urid("http://lv2plug.in/ns/ext/midi#MidiEvent");
        let mut body = Vec::new();
        for m in messages {
            body.extend_from_slice(&0i64.to_ne_bytes());
            body.extend_from_slice(&3u32.to_ne_bytes());
            body.extend_from_slice(&kind.to_ne_bytes());
            body.extend_from_slice(m);
            body.resize(body.len().div_ceil(8) * 8, 0);
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(8 + body.len() as u32).to_ne_bytes());
        bytes.extend_from_slice(&sequence.to_ne_bytes());
        bytes.extend_from_slice(&[0u8; 8]);
        bytes.extend_from_slice(&body);
        let dst = unsafe { std::slice::from_raw_parts_mut(self.atom.as_mut_ptr() as *mut u8, 512 * 8) };
        dst.fill(0);
        dst[..bytes.len()].copy_from_slice(&bytes);
    }

    fn run(&mut self) {
        (self.desc.run)(self.instance, FRAMES as u32);
    }

    fn view(&self) -> Arc<VoiceView> {
        view_of(unsafe { lv2::instance_state(self.instance) })
    }
}

impl Drop for Lv2 {
    fn drop(&mut self) {
        (self.desc.deactivate)(self.instance);
        (self.desc.cleanup)(self.instance);
    }
}

#[test]
fn lv2_mpe_reaches_one_voice() {
    let mut lv2 = Lv2::new();
    let view = lv2.view();
    lv2.sequence(&[[0x91, 60, 100], [0x92, 67, 100]]);
    lv2.run();
    assert_eq!(view.active().len(), 2);
    let other = view.find(2, 67).unwrap();
    // RPN 0 on a member channel: a ±12 range for every member; then bend,
    // press and brighten channel 2's note.
    lv2.sequence(&[
        [0xB3, 101, 0],
        [0xB3, 100, 0],
        [0xB3, 6, 12],
        [0xE1, 0x7F, 0x7F],
        [0xD1, 64, 0],
        [0xB1, 74, 127],
    ]);
    lv2.run();
    let one = view.find(1, 60).unwrap();
    assert!(close(one.pitch, 72.0), "{one:?}");
    assert!(close(one.pressure, 64.0 / 127.0) && close(one.brightness, 1.0), "{one:?}");
    assert_eq!(sound(&view.find(2, 67).unwrap()), sound(&other));
    // The master channel bends every note of the zone by its ±2 range.
    lv2.sequence(&[[0xE0, 0x7F, 0x7F]]);
    lv2.run();
    assert!(close(view.find(1, 60).unwrap().pitch, 74.0) && close(view.find(2, 67).unwrap().pitch, 69.0));
    // Negative control: a channel with no note, and a channel outside the zone
    // after an MCM shrinks it to three members (channel 9 is plain MIDI).
    let before: Vec<_> = view.active().iter().map(sound).collect();
    lv2.sequence(&[[0xE6, 0, 0], [0xD6, 127, 0]]);
    lv2.run();
    assert_eq!(view.active().iter().map(sound).collect::<Vec<_>>(), before);
    lv2.sequence(&[[0xB0, 101, 0], [0xB0, 100, 6], [0xB0, 6, 3], [0x98, 50, 90], [0xE8, 0, 0x7F]]);
    lv2.run();
    let plain = view.find(8, 50).unwrap();
    assert!(close(plain.pitch, 50.0), "a channel in no zone is plain MIDI: {plain:?}");
}

// ---- The shell ABI (VST3, Audio Unit, AAX) ---------------------------------------

struct Shell {
    instance: *mut c_void,
    view: Arc<VoiceView>,
    out: Box<[[f32; FRAMES]; 2]>,
}

impl Shell {
    fn new() -> Self {
        let instance = unsafe { example_expression_synth::op_shell_create(RATE, FRAMES as u32, 2) };
        assert!(!instance.is_null());
        let view = view_of(shell::editor_state(instance as *const ShellInstance));
        Self { instance, view, out: Box::new([[0.0; FRAMES]; 2]) }
    }

    fn block(&mut self, events: &[OpEvent]) -> u32 {
        let outputs = [self.out[0].as_mut_ptr(), self.out[1].as_mut_ptr()];
        let args = OpProcess {
            struct_size: std::mem::size_of::<OpProcess>() as u32,
            frames: FRAMES as u32,
            inputs: std::ptr::null(),
            input_channels: 0,
            output_channels: 2,
            outputs: outputs.as_ptr(),
            sidechain: std::ptr::null(),
            sidechain_channels: 0,
            event_count: events.len() as u32,
            events: events.as_ptr(),
            transport: std::ptr::null(),
        };
        unsafe { example_expression_synth::op_shell_process_events(self.instance, &args) }
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        unsafe { example_expression_synth::op_shell_destroy(self.instance) };
    }
}

fn op_note(on: bool, id: i32, key: u8) -> OpEvent {
    OpEvent {
        kind: if on { shell::EVENT_NOTE_ON_ID } else { shell::EVENT_NOTE_OFF_ID },
        index: id as u32,
        note: key,
        velocity: 100,
        ..OpEvent::default()
    }
}

fn op_expression(id: i32, expression: u8, value: f32) -> OpEvent {
    OpEvent {
        kind: shell::EVENT_NOTE_EXPRESSION,
        index: id as u32,
        channel: shell::NOTE_ANY,
        note: shell::NOTE_ANY,
        midi: [expression, 0, 0, 0],
        value,
        ..OpEvent::default()
    }
}

fn op_midi(bytes: [u8; 3]) -> OpEvent {
    OpEvent { kind: shell::EVENT_MIDI, midi: [bytes[0], bytes[1], bytes[2], 0], ..OpEvent::default() }
}

#[test]
fn shell_note_ids_and_expressions_reach_one_voice() {
    let mut s = Shell::new();
    assert_eq!(s.block(&[op_note(true, 11, 60), op_note(true, 12, 64)]), 1);
    let voices = s.view.active();
    assert_eq!(voices.len(), 2);
    let other = by_id(&voices, 12);
    assert_eq!(s.block(&[op_expression(11, 2, 7.0), op_expression(11, 6, 0.5), op_expression(11, 5, 0.25)]), 1);
    let voices = s.view.active();
    let one = by_id(&voices, 11);
    assert!(close(one.pitch, 67.0) && close(one.pressure, 0.5) && close(one.brightness, 0.25), "{one:?}");
    assert_eq!(sound(&by_id(&voices, 12)), sound(&other));
    // Negative control, then hostile input (an unknown expression, NaN, a
    // note id below -1, a channel past 15): nothing changes, the block runs.
    let before: Vec<_> = voices.iter().map(sound).collect();
    let mut bad_channel = op_expression(11, 2, 1.0);
    bad_channel.channel = 40;
    assert_eq!(
        s.block(&[
            op_expression(13, 2, -5.0),
            op_expression(13, 6, 1.0),
            op_expression(11, 9, 1.0),
            op_expression(11, 2, f32::NAN),
            op_expression(-9, 2, 1.0),
            bad_channel,
        ]),
        1
    );
    assert_eq!(s.view.active().iter().map(sound).collect::<Vec<_>>(), before);
    // The note-off by id.
    s.block(&[op_note(false, 11, 60)]);
    for _ in 0..8 {
        s.block(&[]);
    }
    assert_eq!(s.view.active().iter().map(|v| v.id).collect::<Vec<_>>(), vec![12]);
}

#[test]
fn shell_mpe_over_plain_midi_is_the_audio_unit_and_aax_path() {
    let mut s = Shell::new();
    s.block(&[op_midi([0x91, 60, 100]), op_midi([0x92, 60, 100])]);
    let other = s.view.find(2, 60).unwrap();
    s.block(&[op_midi([0xE1, 0, 0]), op_midi([0xD1, 127, 0]), op_midi([0xB1, 74, 0])]);
    let one = s.view.find(1, 60).unwrap();
    assert!(close(one.pitch, 12.0) && close(one.pressure, 1.0) && close(one.brightness, 0.0), "{one:?}");
    assert_eq!(sound(&s.view.find(2, 60).unwrap()), sound(&other));
    let before: Vec<_> = s.view.active().iter().map(sound).collect();
    s.block(&[op_midi([0xE4, 0x7F, 0x7F])]);
    assert_eq!(s.view.active().iter().map(sound).collect::<Vec<_>>(), before, "negative control");
    // The legacy note kinds the Audio Unit and AAX shells send go through MPE too.
    let on = OpEvent { kind: shell::EVENT_NOTE_ON, channel: 3, note: 72, velocity: 90, ..OpEvent::default() };
    s.block(&[on, op_midi([0xE3, 0x7F, 0x7F])]);
    assert!(close(s.view.find(3, 72).unwrap().pitch, 120.0));
}

#[test]
fn shell_describes_the_expressions_for_the_vst3_controller() {
    assert_eq!(unsafe { example_expression_synth::op_shell_expressions() }, 0x7F);
    let mut info: OpExpressionInfo = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { example_expression_synth::op_shell_expression_info(2, &mut info) }, 1);
    assert_eq!((info.min, info.max, info.default_value, info.flags), (-120.0, 120.0, 0.0, shell::EXPRESSION_BIPOLAR));
    assert_eq!(&info.name[..7], b"Tuning\0");
    assert_eq!(unsafe { example_expression_synth::op_shell_expression_info(0, &mut info) }, 1);
    assert_eq!((info.min, info.max, info.default_value), (0.0, 4.0, 1.0));
    assert_eq!(unsafe { example_expression_synth::op_shell_expression_info(7, &mut info) }, 0);
    assert_eq!(unsafe { example_expression_synth::op_shell_expression_info(2, std::ptr::null_mut()) }, 0);
    let mut buf = [0 as std::ffi::c_char; 32];
    let n = unsafe { example_expression_synth::op_shell_expression_format(2, 7.0, buf.as_mut_ptr(), buf.len()) };
    assert_eq!(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_str().unwrap(), "+7.00 st");
    assert_eq!(n, 8);
    let mut value = 0.0f64;
    assert_eq!(unsafe { example_expression_synth::op_shell_expression_parse(6, c"40%".as_ptr(), &mut value) }, 1);
    assert!((value - 0.4).abs() < 1e-6);
    assert_eq!(unsafe { example_expression_synth::op_shell_expression_parse(6, c"loud".as_ptr(), &mut value) }, 0);
    assert_eq!(unsafe { example_expression_synth::op_shell_expression_parse(99, c"1".as_ptr(), &mut value) }, 0);
    assert_eq!(unsafe { example_expression_synth::op_shell_expression_parse(6, std::ptr::null(), &mut value) }, 0);
}

// ---- No allocation on the audio thread ---------------------------------------------

#[test]
fn the_expression_paths_do_not_allocate() {
    assert!(openplugin_testing::alloc::is_installed());

    let mut clap = Clap::new();
    let notes = Events::new(vec![note(EVENT_NOTE_ON, 1, 0, 60), note(EVENT_NOTE_ON, 2, 0, 64)]);
    let busy = Events::new(vec![
        expression(TUNING, 1, -1, -1, 1.0),
        expression(PRESSURE, 1, -1, -1, 0.5),
        expression(BRIGHTNESS, -1, 0, 64, 0.5),
        modulation(TONE as u32, 2, -1, 0.3),
        modulation(TONE as u32, -1, -1, 0.1),
        midi([0x91, 62, 100]),
        midi([0xE1, 0x10, 0x50]),
        midi([0xD1, 30, 0]),
        midi([0xB1, 74, 90]),
        note(EVENT_NOTE_OFF, 2, 0, 64),
    ]);
    let out = Out::new();
    let quiet = Events::new(Vec::new());
    let (_, counts) = openplugin_testing::alloc::measure(|| {
        clap.block_out(&notes, &out.list);
        for _ in 0..4 {
            clap.block_out(&busy, &out.list);
        }
        // Note 2's release ends here: NOTE_END goes out without allocating.
        for _ in 0..10 {
            clap.block_out(&quiet, &out.list);
        }
    });
    assert_eq!((counts.allocs, counts.frees), (0, 0), "CLAP");
    assert!(out.ends().iter().any(|(_, n)| n.id == 2), "a voice ended inside the measured blocks: {:?}", out.ends());

    let mut lv2 = Lv2::new();
    lv2.sequence(&[[0x91, 60, 100], [0xE1, 0, 0x50], [0xD1, 40, 0], [0xB1, 74, 10], [0xB0, 101, 0], [0xB0, 100, 6], [0xB0, 6, 9]]);
    let (_, counts) = openplugin_testing::alloc::measure(|| {
        for _ in 0..4 {
            lv2.run();
        }
    });
    assert_eq!((counts.allocs, counts.frees), (0, 0), "LV2");

    let mut s = Shell::new();
    let events = [
        op_note(true, 1, 60),
        op_expression(1, 2, 3.0),
        op_expression(1, 6, 0.5),
        op_midi([0x92, 61, 90]),
        op_midi([0xE2, 0, 0x60]),
        op_note(false, 1, 60),
    ];
    let (_, counts) = openplugin_testing::alloc::measure(|| {
        for _ in 0..4 {
            s.block(&events);
        }
    });
    assert_eq!((counts.allocs, counts.frees), (0, 0), "shell");
    let _ = NoteRef::ALL;
}

#[test]
fn the_synth_passes_the_conformance_kit() {
    openplugin_testing::conformance(ExpressionSynth::default, Default::default()).assert_pass();
}
