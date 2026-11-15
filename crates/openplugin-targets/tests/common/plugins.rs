//! Test plugins. Neither allocates after construction.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use openplugin_core::{
    Category, IoConfig, NotePorts, ParamInfo, Plugin, PluginInfo, SpeakerLayout, Transport, Unit,
};

/// The last transport a plugin was given.
pub static TRANSPORT: Mutex<Option<Transport>> = Mutex::new(None);
/// The last `set_offline` value.
pub static OFFLINE: AtomicBool = AtomicBool::new(false);
/// Note and MIDI messages that reached a plugin.
pub static EVENTS: AtomicUsize = AtomicUsize::new(0);

pub static PARAMS: [ParamInfo; 1] = [ParamInfo::new("level", "Level", 0.0, 1.0, 0.5, Unit::None)];

pub static SYNTH_INFO: PluginInfo = PluginInfo {
    id: "urn:openplugin:test-synth",
    name: "Test Synth",
    vendor: "Test",
    url: "https://example.com",
    email: "a@b.c",
    version: "0.1.0",
    category: Category::Instrument,
    params: &PARAMS,
    is_instrument: true,
    has_editor: false,
};

pub static DUCK_INFO: PluginInfo = PluginInfo {
    id: "urn:openplugin:test-duck",
    name: "Test Duck",
    vendor: "Test",
    url: "https://example.com",
    email: "a@b.c",
    version: "0.1.0",
    category: Category::Dynamics,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

static DUCK_LAYOUTS: [IoConfig; 1] =
    [IoConfig { input: SpeakerLayout::Stereo, output: SpeakerLayout::Stereo, sidechain: Some(SpeakerLayout::Stereo) }];

/// Fixed-size queue of outgoing MIDI, so the plugin never allocates.
pub struct Queue {
    items: [(u32, [u8; 3]); 64],
    head: usize,
    len: usize,
    pos: u32,
}

impl Queue {
    pub const fn new() -> Self {
        Self { items: [(0, [0; 3]); 64], head: 0, len: 0, pos: 0 }
    }

    fn push(&mut self, msg: [u8; 3]) {
        if self.len < self.items.len() {
            let at = (self.head + self.len) % self.items.len();
            self.items[at] = (self.pos, msg);
            self.len += 1;
        }
    }

    fn pull(&mut self) -> Option<(u32, [u8; 3])> {
        if self.len == 0 {
            self.pos = 0;
            return None;
        }
        let item = self.items[self.head];
        self.head = (self.head + 1) % self.items.len();
        self.len -= 1;
        Some(item)
    }
}

/// An instrument: holds `level` on both outputs while a note is down, and
/// sends every note and message it gets back out an octave up.
pub struct Synth {
    pub level: f32,
    pub held: bool,
    pub queue: Queue,
}

impl Default for Synth {
    fn default() -> Self {
        Self { level: 0.5, held: false, queue: Queue::new() }
    }
}

impl Plugin for Synth {
    fn info(&self) -> &'static PluginInfo {
        &SYNTH_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {
        self.held = false;
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id == "level" {
            self.level = value;
            true
        } else {
            false
        }
    }
    fn param(&self, id: &str) -> Option<f32> {
        (id == "level").then_some(self.level)
    }
    fn note_ports(&self) -> NotePorts {
        NotePorts { input: true, output: true }
    }
    fn note_on_channel(&mut self, _offset: usize, channel: u8, note: u8, velocity: u8) {
        EVENTS.fetch_add(1, Ordering::SeqCst);
        self.held = true;
        self.queue.push([0x90 | channel, note.wrapping_add(12) & 0x7F, velocity]);
    }
    fn note_off(&mut self, _offset: usize, note: u8) {
        EVENTS.fetch_add(1, Ordering::SeqCst);
        self.held = false;
        self.queue.push([0x80, note.wrapping_add(12) & 0x7F, 0]);
    }
    fn midi(&mut self, _offset: usize, msg: [u8; 3]) {
        EVENTS.fetch_add(1, Ordering::SeqCst);
        self.queue.push(msg);
    }
    fn pull_midi(&mut self) -> Option<(u32, [u8; 3])> {
        self.queue.pull()
    }
    fn set_transport(&mut self, transport: &Transport) {
        if let Ok(mut slot) = TRANSPORT.lock() {
            *slot = Some(*transport);
        }
    }
    fn set_offline(&mut self, offline: bool) {
        OFFLINE.store(offline, Ordering::SeqCst);
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let value = if self.held { self.level } else { 0.0 };
        for channel in io.iter_mut() {
            channel[..frames].fill(value);
        }
        self.queue.pos += frames as u32;
    }
}

/// An effect with a stereo sidechain: output is input plus sidechain. It
/// echoes MIDI like [`Synth`] and records transport and freewheel.
pub struct Duck {
    pub level: f32,
    pub queue: Queue,
}

impl Default for Duck {
    fn default() -> Self {
        Self { level: 0.5, queue: Queue::new() }
    }
}

impl Plugin for Duck {
    fn info(&self) -> &'static PluginInfo {
        &DUCK_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id == "level" {
            self.level = value;
            true
        } else {
            false
        }
    }
    fn param(&self, id: &str) -> Option<f32> {
        (id == "level").then_some(self.level)
    }
    fn supported_layouts(&self) -> &'static [IoConfig] {
        &DUCK_LAYOUTS
    }
    fn note_ports(&self) -> NotePorts {
        NotePorts { input: true, output: true }
    }
    fn note_on_channel(&mut self, _offset: usize, channel: u8, note: u8, velocity: u8) {
        EVENTS.fetch_add(1, Ordering::SeqCst);
        self.queue.push([0x90 | channel, note, velocity]);
    }
    fn midi(&mut self, _offset: usize, msg: [u8; 3]) {
        EVENTS.fetch_add(1, Ordering::SeqCst);
        self.queue.push(msg);
    }
    fn pull_midi(&mut self) -> Option<(u32, [u8; 3])> {
        self.queue.pull()
    }
    fn set_transport(&mut self, transport: &Transport) {
        if let Ok(mut slot) = TRANSPORT.lock() {
            *slot = Some(*transport);
        }
    }
    fn set_offline(&mut self, offline: bool) {
        OFFLINE.store(offline, Ordering::SeqCst);
    }
    fn process(&mut self, _io: &mut [Vec<f32>], frames: usize) {
        self.queue.pos += frames as u32;
    }
    fn process_buses(&mut self, io: &mut [Vec<f32>], sidechain: Option<&[Vec<f32>]>, frames: usize) {
        if let Some(side) = sidechain {
            for (channel, samples) in io.iter_mut().enumerate() {
                if let Some(side) = side.get(channel) {
                    for (out, s) in samples[..frames].iter_mut().zip(&side[..frames]) {
                        *out += *s;
                    }
                }
            }
        }
        self.queue.pos += frames as u32;
    }
}
