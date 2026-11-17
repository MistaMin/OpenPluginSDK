//! `op_shell_load_foreign_state` and `op_shell_foreign_kinds` (ABI 10): a plugin that replaces a
//! shipped one reads the old plugin's own bytes. Success moves the values the host reads;
//! refusal, an unknown kind, a null pointer, an empty state and a plugin that panics change
//! nothing. A plugin that reads no kind refuses every state and lists none.

#[path = "common/state_plugin.rs"]
mod common;

use std::panic::{catch_unwind, AssertUnwindSafe};

use openplugin_core::foreign_state::ForeignStateKind;
use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_host::shell;

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
    ParamInfo::new("tone", "Tone", 0.0, 1.0, 0.5, Unit::None),
];

static INFO: PluginInfo = PluginInfo::new(
    "com.example.replacement",
    "Replacement",
    "Test",
    "2.0.0",
    Category::Utility,
    &PARAMS,
)
.with_url("https://example.com")
.with_email("a@b.c");

/// Reads the old plugin's VST3 component state (two little-endian f32: gain, tone) and an AAX
/// chunk (the word "boom" makes it panic, to check the shell contains a plugin's panic).
#[derive(Default)]
struct Replacement {
    gain_db: f32,
    tone: f32,
}

impl Plugin for Replacement {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "gain" => self.gain_db = PARAMS[0].clamp(value),
            "tone" => self.tone = PARAMS[1].clamp(value),
            _ => return false,
        }
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "gain" => Some(self.gain_db),
            "tone" => Some(self.tone),
            _ => None,
        }
    }
    fn process(&mut self, _: &mut [&mut [f32]], _: usize) {}
    fn foreign_state_kinds(&self) -> &'static [ForeignStateKind] {
        &[ForeignStateKind::Vst3Component, ForeignStateKind::AaxChunk]
    }
    fn load_foreign_state(&mut self, kind: ForeignStateKind, data: &[u8]) -> bool {
        match kind {
            ForeignStateKind::Vst3Component if data.len() == 8 => {
                let gain = f32::from_le_bytes(data[..4].try_into().unwrap());
                let tone = f32::from_le_bytes(data[4..].try_into().unwrap());
                if !gain.is_finite() || !tone.is_finite() {
                    return false;
                }
                self.gain_db = PARAMS[0].clamp(gain);
                self.tone = PARAMS[1].clamp(tone);
                true
            }
            ForeignStateKind::AaxChunk if data == b"boom" => panic!("a plugin that panics while reading a foreign state"),
            _ => false,
        }
    }
}

fn make() -> *mut shell::ShellInstance {
    let instance = shell::create::<Replacement>(48_000.0, 256, 2);
    assert!(!instance.is_null());
    instance
}

fn old_state(gain: f32, tone: f32) -> Vec<u8> {
    let mut v = gain.to_le_bytes().to_vec();
    v.extend_from_slice(&tone.to_le_bytes());
    v
}

fn offer(instance: *mut shell::ShellInstance, kind: u32, bytes: &[u8]) -> u32 {
    unsafe { shell::load_foreign_state(instance, kind, bytes.as_ptr(), bytes.len() as u32) }
}

fn values(instance: *mut shell::ShellInstance) -> (f32, f32) {
    unsafe { (shell::get_plain(instance, 0), shell::get_plain(instance, 1)) }
}

#[test]
fn the_kinds_a_plugin_reads_are_one_bit_each() {
    // bit (kind - 1): VST3 component is kind 1, AAX chunk kind 4
    assert_eq!(shell::foreign_kinds::<Replacement>(), 0b1001);
    assert_eq!(shell::foreign_kinds::<common::Chunky>(), 0, "a plugin that reads none costs a host nothing");
    assert_eq!(ForeignStateKind::from_abi(1), Some(ForeignStateKind::Vst3Component));
    assert_eq!(ForeignStateKind::from_abi(0), None);
    assert_eq!(ForeignStateKind::from_abi(5), None);
    assert_eq!(ForeignStateKind::Vst3Controller.to_abi(), 2);
}

#[test]
fn an_accepted_state_moves_the_values_the_host_reads() {
    let a = make();
    assert_eq!(offer(a, 1, &old_state(-6.0, 0.75)), 1);
    let (gain, tone) = values(a);
    assert!((gain - -6.0).abs() < 1e-6 && (tone - 0.75).abs() < 1e-6);
    // the host reads normalised values too
    let normalized = unsafe { shell::get_normalized(a, 0) };
    assert!((normalized - (18.0 / 48.0)).abs() < 1e-5);
    unsafe { shell::destroy(a) };
}

#[test]
fn a_refused_state_changes_nothing() {
    let a = make();
    assert_eq!(offer(a, 1, &old_state(3.0, 0.25)), 1);
    let before = values(a);
    assert_eq!(offer(a, 1, &old_state(f32::NAN, 0.5)), 0, "not finite");
    assert_eq!(offer(a, 1, &[1, 2, 3]), 0, "the wrong size");
    assert_eq!(offer(a, 2, &old_state(9.0, 0.9)), 0, "a kind the plugin does not read");
    assert_eq!(offer(a, 3, &old_state(9.0, 0.9)), 0);
    assert_eq!(offer(a, 9, &old_state(9.0, 0.9)), 0, "a kind this build does not know");
    assert_eq!(offer(a, 0, &old_state(9.0, 0.9)), 0);
    assert_eq!(values(a), before);
    unsafe { shell::destroy(a) };
}

#[test]
fn null_empty_and_dead_arguments_are_refused() {
    let a = make();
    let bytes = old_state(1.0, 0.5);
    unsafe {
        assert_eq!(shell::load_foreign_state(std::ptr::null_mut(), 1, bytes.as_ptr(), 8), 0);
        assert_eq!(shell::load_foreign_state(a, 1, std::ptr::null(), 8), 0);
        assert_eq!(shell::load_foreign_state(a, 1, bytes.as_ptr(), 0), 0);
    }
    assert_eq!(values(a), (0.0, 0.0), "nothing was applied");
    unsafe { shell::destroy(a) };
}

#[test]
fn a_plugin_that_panics_is_contained_and_changes_nothing() {
    let a = make();
    assert_eq!(offer(a, 1, &old_state(2.0, 0.5)), 1);
    let before = values(a);
    let result = catch_unwind(AssertUnwindSafe(|| offer(a, 4, b"boom")));
    assert_eq!(result.ok(), Some(0), "the shell answers 0 and does not unwind into the host");
    assert_eq!(values(a), before);
    // the instance still works afterwards
    assert_eq!(offer(a, 1, &old_state(-1.0, 0.1)), 1);
    unsafe { shell::destroy(a) };
}

#[test]
fn a_plugin_that_reads_no_kind_refuses_every_state() {
    let c = shell::create::<common::Chunky>(48_000.0, 256, 2);
    assert!(!c.is_null());
    for kind in 1..=4 {
        assert_eq!(offer(c, kind, &old_state(1.0, 0.5)), 0, "kind {kind}");
    }
    unsafe { shell::destroy(c) };
}
