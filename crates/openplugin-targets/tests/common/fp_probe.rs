//! A plugin that records the floating-point mode it runs in: whether every
//! flush bit of `DenormalGuard` is set, the control word, and whether a
//! multiply with a subnormal result really came out as zero. Shared by the
//! CLAP and LV2 denormal tests.
//!
//! Everything is thread-local: a test calls the entry on its own thread, so
//! the tests of one binary can run in parallel without seeing each other.
#![allow(dead_code)]

use std::cell::Cell;
use std::hint::black_box;

use openplugin_core::denormal::host_mode;
use openplugin_core::{Category, DenormalGuard, NotePorts, ParamInfo, Plugin, PluginInfo, Unit};

/// What one callback saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    /// Every flush bit was set.
    pub flushing: bool,
    /// The control word without status flags.
    pub control: u64,
    /// `subnormal * 0.5` gave exactly zero.
    pub flushed_math: bool,
}

thread_local! {
    static PROCESS: Cell<Option<Seen>> = const { Cell::new(None) };
    static SET_PARAM: Cell<Option<Seen>> = const { Cell::new(None) };
    static RESET: Cell<Option<Seen>> = const { Cell::new(None) };
    /// The next `process` panics after recording.
    static PANIC: Cell<bool> = const { Cell::new(false) };
    /// Instances made on this thread opt out of flushing.
    static OPT_OUT: Cell<bool> = const { Cell::new(false) };
}

fn look() -> Seen {
    Seen {
        flushing: DenormalGuard::flushing(),
        control: host_mode::control(),
        flushed_math: black_box(f32::from_bits(2)) * black_box(0.5f32) == 0.0,
    }
}

pub fn take_process() -> Option<Seen> {
    PROCESS.with(|c| c.take())
}

pub fn take_set_param() -> Option<Seen> {
    SET_PARAM.with(|c| c.take())
}

pub fn take_reset() -> Option<Seen> {
    RESET.with(|c| c.take())
}

pub fn panic_next_process(on: bool) {
    PANIC.with(|c| c.set(on));
}

/// Instances made on this thread from now on opt out (or not).
pub fn opt_out(on: bool) {
    OPT_OUT.with(|c| c.set(on));
}

/// The value `process` writes.
pub const OUT: f32 = 0.25;

pub static PARAMS: [ParamInfo; 1] = [ParamInfo::new("level", "Level", 0.0, 1.0, 0.5, Unit::None)];

pub static INFO: PluginInfo = PluginInfo {
    id: "urn:openplugin:test-fp-probe",
    name: "FP Probe",
    vendor: "Test",
    url: "https://example.com",
    email: "a@b.c",
    version: "0.1.0",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

pub struct FpProbe {
    level: f32,
    flush: bool,
}

impl Default for FpProbe {
    fn default() -> Self {
        Self { level: 0.5, flush: !OPT_OUT.with(|c| c.get()) }
    }
}

impl Plugin for FpProbe {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {
        RESET.with(|c| c.set(Some(look())));
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        SET_PARAM.with(|c| c.set(Some(look())));
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
        NotePorts { input: false, output: false }
    }
    fn flush_denormals(&self) -> bool {
        self.flush
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        PROCESS.with(|c| c.set(Some(look())));
        if PANIC.with(|c| c.get()) {
            panic!("fp probe: process");
        }
        for channel in io.iter_mut() {
            channel[..frames].fill(OUT);
        }
    }
}

/// Run `call` on this thread with the host's control word set to `word`,
/// then put the thread's own word back. Returns the control word right
/// after `call` and what `call` returned.
pub fn as_host<R>(word: u64, call: impl FnOnce() -> R) -> (u64, R) {
    let old = unsafe { host_mode::set(word) };
    let value = call();
    let after = host_mode::control();
    unsafe { host_mode::set(old) };
    (after, value)
}

/// What a plugin that flushes must see inside the entry, for host word `word`:
/// every flush bit, and the rest of the host's word (its rounding mode) kept.
pub fn assert_flushed(seen: Option<Seen>, word: u64, what: &str) {
    let seen = seen.unwrap_or_else(|| panic!("{what}: the plugin was not called"));
    assert!(seen.flushing, "{what}: flush bits not set inside the plugin ({:#x})", seen.control);
    assert!(seen.flushed_math, "{what}: a subnormal result was not flushed");
    assert_eq!(
        seen.control & !DenormalGuard::flush_bits(),
        word & !DenormalGuard::flush_bits(),
        "{what}: the rest of the host's mode changed inside the plugin"
    );
}
