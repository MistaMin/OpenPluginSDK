//! A block with more events than the CLAP bridge's 256-entry schedule: every
//! note-off still reaches the plugin and no note is left sounding.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use openplugin_core::{Category, NoteRef, Plugin, PluginInfo};
use openplugin_targets::clap_abi::*;
use openplugin_targets::ClapAdapter;

const FRAMES: u32 = 512;
const NOTES: usize = 150;

static INFO: PluginInfo = PluginInfo::new(
    "com.opengrid.test.overflow",
    "Event Overflow",
    "OpenGrid",
    "0.0.1",
    Category::Utility,
    &[],
)
    .with_url("https://github.com/MistaMin/OpenPluginSDK")
    .with_email("contact@mistamin.com")
    .with_instrument(true);

#[derive(Default)]
struct Seen {
    sounding: HashSet<(u8, u8)>,
    offs: usize,
}

struct Synth(Arc<Mutex<Seen>>);

impl Plugin for Synth {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [&mut [f32]], _: usize) {}
    fn note_on_ref(&mut self, _: usize, note: NoteRef, _: u8) {
        self.0.lock().unwrap().sounding.insert((note.channel as u8, note.key as u8));
    }
    fn note_off_ref(&mut self, _: usize, note: NoteRef, _: u8) {
        let mut seen = self.0.lock().unwrap();
        seen.sounding.remove(&(note.channel as u8, note.key as u8));
        seen.offs += 1;
    }
    fn midi(&mut self, _: usize, message: [u8; 3]) {
        let note = (message[0] & 0x0F, message[1]);
        let mut seen = self.0.lock().unwrap();
        if message[0] & 0xF0 == 0x90 && message[2] > 0 {
            seen.sounding.insert(note);
        } else if message[0] & 0xF0 == 0x80 {
            seen.sounding.remove(&note);
            seen.offs += 1;
        }
    }
}

#[repr(C, align(8))]
struct Slot([u8; 64]);

impl Slot {
    fn from<T>(event: &T) -> Self {
        let mut slot = Slot([0; 64]);
        assert!(std::mem::size_of::<T>() <= slot.0.len());
        unsafe { std::ptr::copy_nonoverlapping(event as *const T as *const u8, slot.0.as_mut_ptr(), std::mem::size_of::<T>()) };
        slot
    }
}

unsafe extern "C" fn size(list: *const clap_input_events) -> u32 {
    unsafe { &*((*list).ctx as *const Vec<Slot>) }.len() as u32
}

unsafe extern "C" fn get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let slots = unsafe { &*((*list).ctx as *const Vec<Slot>) };
    slots[index as usize].0.as_ptr() as *const clap_event_header
}

fn header<T>(time: u32, type_: u16) -> clap_event_header {
    clap_event_header { size: std::mem::size_of::<T>() as u32, time, space_id: 0, type_, flags: 0 }
}

/// Every note starts, then every note ends: the last ends overflow the schedule
/// while their starts sit in it.
fn note_events() -> Vec<Slot> {
    let note = |time: u32, type_: u16, n: usize| {
        Slot::from(&clap_event_note {
            header: header::<clap_event_note>(time, type_),
            note_id: -1,
            port_index: 0,
            channel: (n / 100) as i16,
            key: (n % 100) as i16,
            velocity: 0.8,
        })
    };
    let ons = (0..NOTES).map(|n| note(n as u32, EVENT_NOTE_ON, n));
    let offs = (0..NOTES).map(|n| note((NOTES + n) as u32, EVENT_NOTE_OFF, n));
    ons.chain(offs).collect()
}

fn midi_events() -> Vec<Slot> {
    let midi = |time: u32, status: u8, n: usize| {
        Slot::from(&clap_event_midi {
            header: header::<clap_event_midi>(time, EVENT_MIDI),
            port_index: 0,
            data: [status | (n / 100) as u8, (n % 100) as u8, 100],
        })
    };
    let ons = (0..NOTES).map(|n| midi(n as u32, 0x90, n));
    let offs = (0..NOTES).map(|n| midi((NOTES + n) as u32, 0x80, n));
    ons.chain(offs).collect()
}

fn play(slots: Vec<Slot>) -> Seen {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let adapter = ClapAdapter::new(Synth(seen.clone()));
    let plugin = adapter.as_plugin();
    assert!(unsafe { (*plugin).activate.unwrap()(plugin, 48_000.0, 1, FRAMES) });
    let events = clap_input_events { ctx: &slots as *const Vec<Slot> as *mut c_void, size: Some(size), get: Some(get) };
    let mut left = vec![0.0f32; FRAMES as usize];
    let mut right = vec![0.0f32; FRAMES as usize];
    let mut channels = [left.as_mut_ptr(), right.as_mut_ptr()];
    let mut output = clap_audio_buffer {
        data32: channels.as_mut_ptr(),
        data64: std::ptr::null_mut(),
        channel_count: 2,
        latency: 0,
        constant_mask: 0,
    };
    let process = clap_process {
        steady_time: 0,
        frames_count: FRAMES,
        transport: std::ptr::null(),
        audio_inputs: std::ptr::null(),
        audio_outputs: &mut output,
        audio_inputs_count: 0,
        audio_outputs_count: 1,
        in_events: &events,
        out_events: std::ptr::null(),
    };
    assert_eq!(unsafe { (*plugin).process.unwrap()(plugin, &process) }, PROCESS_CONTINUE);
    drop(adapter);
    Arc::try_unwrap(seen).ok().unwrap().into_inner().unwrap()
}

#[test]
fn every_note_off_past_the_schedule_is_delivered() {
    let seen = play(note_events());
    assert_eq!(seen.offs, NOTES);
    assert!(seen.sounding.is_empty(), "stuck notes: {:?}", seen.sounding);
}

#[test]
fn every_midi_note_off_past_the_schedule_is_delivered() {
    let seen = play(midi_events());
    assert_eq!(seen.offs, NOTES);
    assert!(seen.sounding.is_empty(), "stuck notes: {:?}", seen.sounding);
}
