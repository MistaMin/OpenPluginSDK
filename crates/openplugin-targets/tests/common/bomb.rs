//! A plugin that panics where the test says: in `process`, `set_param`, `save_state`,
//! `load_state`, a note callback or `reset`. Shared by the CLAP and LV2 panic tests.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use openplugin_core::{Category, NotePorts, ParamInfo, Plugin, PluginInfo, Unit};

pub const NONE: u32 = 0;
pub const PROCESS: u32 = 1;
pub const SET_PARAM: u32 = 2;
pub const SAVE: u32 = 3;
pub const LOAD: u32 = 4;
pub const NOTE: u32 = 5;
pub const RESET: u32 = 6;

/// Where the next call panics. Tests take turns through [`SERIAL`].
pub static MODE: AtomicU32 = AtomicU32::new(NONE);
pub static SERIAL: Mutex<()> = Mutex::new(());

/// The value `process` writes when it does not panic.
pub const OUT: f32 = 0.8;

pub fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn arm(mode: u32) {
    MODE.store(mode, Ordering::SeqCst);
}

fn armed(mode: u32) -> bool {
    MODE.load(Ordering::SeqCst) == mode
}

pub static PARAMS: [ParamInfo; 1] = [ParamInfo::new("level", "Level", 0.0, 1.0, 0.5, Unit::None)];

pub static INFO: PluginInfo = PluginInfo::new(
    "urn:openplugin:test-bomb",
    "Test Bomb",
    "Test",
    "0.1.0",
    Category::Utility,
    &PARAMS,
)
.with_url("https://example.com")
.with_email("a@b.c");

#[derive(Default)]
pub struct Bomb {
    pub level: f32,
}

impl Plugin for Bomb {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {
        if armed(RESET) {
            panic!("bomb: reset");
        }
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if armed(SET_PARAM) {
            panic!("bomb: set_param");
        }
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
        NotePorts { input: true, output: false }
    }
    fn note_on_channel(&mut self, _: usize, _: u8, _: u8, _: u8) {
        if armed(NOTE) {
            panic!("bomb: note");
        }
    }
    fn state_is_parameters_only(&self) -> bool {
        false
    }
    fn save_state(&mut self) -> Option<Vec<u8>> {
        if armed(SAVE) {
            panic!("bomb: save_state");
        }
        Some(self.level.to_le_bytes().to_vec())
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        if armed(LOAD) {
            panic!("bomb: load_state");
        }
        let Ok(bytes) = <[u8; 4]>::try_from(data) else { return false };
        self.level = f32::from_le_bytes(bytes);
        true
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        if armed(PROCESS) {
            panic!("bomb: process");
        }
        for channel in io.iter_mut() {
            channel[..frames].fill(OUT);
        }
    }
}
