//! Shared by the shell state tests: a plugin with a parameter plane and its
//! own chunk. Included with `#[path]` by each state test.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_host::shell::{self, ShellInstance, STATE_OK, STATE_TOO_SMALL};

pub static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
    ParamInfo::new("tone", "Tone", 0.0, 1.0, 0.5, Unit::None),
];

pub static INFO: PluginInfo = PluginInfo::new(
    "com.example.shell-state",
    "Shell State",
    "Test",
    "0.1.0",
    Category::Utility,
    &PARAMS,
)
.with_url("https://example.com")
.with_email("a@b.c");

/// While true, `save_state` spins (a slow save), up to a safety limit.
pub static HOLD_SAVE: AtomicBool = AtomicBool::new(false);
/// Set by `save_state` once it is running.
pub static IN_SAVE: AtomicBool = AtomicBool::new(false);
/// While true, `process` spins (a long block).
pub static HOLD_PROCESS: AtomicBool = AtomicBool::new(false);
/// Set by `process` once it is running.
pub static IN_PROCESS: AtomicBool = AtomicBool::new(false);
/// Times `load_state` ran.
pub static LOADS: AtomicU32 = AtomicU32::new(0);

pub const PROGRAM_MAX: u8 = 8;

/// Gain, tone, and a `program` and `name` kept in the plugin's own chunk.
/// The audio is `input * program * gain`, so a block shows which state it ran with.
pub struct Chunky {
    pub gain_db: f32,
    pub tone: f32,
    pub program: u8,
    pub name: Vec<u8>,
}

impl Default for Chunky {
    fn default() -> Self {
        Self { gain_db: 0.0, tone: 0.5, program: 1, name: b"init".to_vec() }
    }
}

fn spin_while(flag: &AtomicBool) {
    let start = std::time::Instant::now();
    while flag.load(Ordering::SeqCst) && start.elapsed().as_secs() < 5 {
        std::hint::spin_loop();
    }
}

impl Plugin for Chunky {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "gain" => {
                self.gain_db = PARAMS[0].clamp(value);
                true
            }
            "tone" => {
                self.tone = PARAMS[1].clamp(value);
                true
            }
            _ => false,
        }
    }
    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "gain" => Some(self.gain_db),
            "tone" => Some(self.tone),
            _ => None,
        }
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        IN_PROCESS.store(true, Ordering::SeqCst);
        spin_while(&HOLD_PROCESS);
        let factor = f32::from(self.program) * 10f32.powf(self.gain_db / 20.0);
        for channel in io.iter_mut() {
            for sample in &mut channel[..frames] {
                *sample *= factor;
            }
        }
        IN_PROCESS.store(false, Ordering::SeqCst);
    }
    fn save_state(&mut self) -> Option<Vec<u8>> {
        IN_SAVE.store(true, Ordering::SeqCst);
        spin_while(&HOLD_SAVE);
        let mut out = b"PRG1".to_vec();
        out.push(self.program);
        out.push(self.name.len() as u8);
        out.extend_from_slice(&self.name);
        IN_SAVE.store(false, Ordering::SeqCst);
        Some(out)
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        LOADS.fetch_add(1, Ordering::SeqCst);
        if data.len() < 6 || &data[..4] != b"PRG1" {
            return false;
        }
        let program = data[4];
        let len = data[5] as usize;
        if !(1..=PROGRAM_MAX).contains(&program) || data.len() != 6 + len {
            return false;
        }
        self.program = program;
        self.name = data[6..].to_vec();
        true
    }
}

/// Two-call save the way a C++ caller does it: ask the size, then copy.
pub fn save(instance: *mut ShellInstance) -> Vec<u8> {
    let mut needed = 0u32;
    loop {
        let size = unsafe { shell::save_state(instance, std::ptr::null_mut(), 0, &mut needed) };
        assert_eq!(size, STATE_TOO_SMALL, "a size query reports the size");
        let mut buffer = vec![0u8; needed as usize];
        let mut again = 0u32;
        let code = unsafe { shell::save_state(instance, buffer.as_mut_ptr(), needed, &mut again) };
        match code {
            STATE_OK => {
                buffer.truncate(again as usize);
                return buffer;
            }
            STATE_TOO_SMALL => continue,
            other => panic!("save_state failed with {other}"),
        }
    }
}

pub fn load(instance: *mut ShellInstance, blob: &[u8]) -> bool {
    unsafe { shell::load_state(instance, blob.as_ptr(), blob.len() as u32) == 1 }
}

/// Session bytes for a given chunk and parameter values, the way the shell writes them.
pub fn session(values: &[f32], chunk: &[u8]) -> Vec<u8> {
    openplugin_core::encode_session(&INFO, values, chunk)
}

pub fn chunk(program: u8, name: &[u8]) -> Vec<u8> {
    let mut out = b"PRG1".to_vec();
    out.push(program);
    out.push(name.len() as u8);
    out.extend_from_slice(name);
    out
}
