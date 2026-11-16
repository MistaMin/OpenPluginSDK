//! The CLAP entry runs the plugin with denormals flushed and gives the host its
//! own floating-point mode back.
//!
//! For each host mode from `host_mode::hosts()` (flush off, already on, only
//! FTZ on x86, each also with a non-default rounding mode) the test sets the
//! mode on its thread, calls `process`, `params.flush` or `reset` as a host
//! does, and checks what `FpProbe` recorded inside (every flush bit set, the
//! rest of the host's word kept, a subnormal product flushed) and the mode
//! after the call (exactly the host's). The same after a panic in `process`,
//! and with `Plugin::flush_denormals` returning false: then the plugin sees
//! the host's mode untouched.

mod common;
#[path = "common/fp_probe.rs"]
mod fp_probe;

use std::ffi::c_char;

use common::clap_host::{Audio, Events};
use fp_probe::*;
use openplugin_core::denormal::host_mode;
use openplugin_core::DenormalGuard;
use openplugin_targets::clap_abi::*;
use openplugin_targets::ClapAdapter;

const FRAMES: usize = 64;

fn rig() -> (ClapAdapter<FpProbe>, *const clap_plugin) {
    let mut adapter = ClapAdapter::new(FpProbe::default());
    let plugin = adapter.as_plugin();
    assert!(adapter.activate(48_000.0, 1, FRAMES as u32));
    (adapter, plugin)
}

fn params(plugin: *const clap_plugin) -> *const clap_plugin_params {
    unsafe { ((*plugin).get_extension.unwrap())(plugin, EXT_PARAMS.as_ptr() as *const c_char) as *const clap_plugin_params }
}

#[test]
fn the_platform_has_a_flush_mode_to_test() {
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    assert!(DenormalGuard::SUPPORTED && host_mode::hosts().len() >= 4);
}

#[test]
fn process_runs_flushed_and_the_host_mode_comes_back() {
    let (_adapter, plugin) = rig();
    let mut audio = Audio::new(FRAMES);
    for (name, word) in host_mode::hosts() {
        let (after, status) = as_host(word, || unsafe { audio.run(plugin, None) });
        assert_eq!(status, PROCESS_CONTINUE, "{name}");
        assert_flushed(take_process(), word, name);
        assert_eq!(after, word, "{name}: the host's mode was not restored after process");
    }
}

#[test]
fn a_block_with_parameter_events_runs_set_param_flushed() {
    let (_adapter, plugin) = rig();
    let mut audio = Audio::new(FRAMES);
    for (step, (name, word)) in host_mode::hosts().into_iter().enumerate() {
        // A new value each time, so the entry has something to apply.
        let events = Events::new(&[(0, 0.1 + 0.1 * step as f64)]);
        let (after, _) = as_host(word, || unsafe { audio.run(plugin, Some(&events)) });
        assert_flushed(take_set_param(), word, name);
        assert_eq!(after, word, "{name}");
    }
}

#[test]
fn a_panic_in_process_still_restores_the_host_mode() {
    let (mut adapter, plugin) = rig();
    let mut audio = Audio::new(FRAMES);
    for (name, word) in host_mode::hosts() {
        panic_next_process(true);
        let (after, status) = as_host(word, || unsafe { audio.run(plugin, None) });
        panic_next_process(false);
        assert_eq!(status, PROCESS_ERROR, "{name}: the panic was not reported");
        assert_flushed(take_process(), word, name);
        assert_eq!(after, word, "{name}: the host's mode was not restored after a panic");
        // The poisoned instance refuses blocks until it is activated again.
        unsafe { ((*plugin).deactivate.unwrap())(plugin) };
        assert!(adapter.activate(48_000.0, 1, FRAMES as u32));
    }
}

#[test]
fn params_flush_runs_flushed_and_restores() {
    let (_adapter, plugin) = rig();
    let table = params(plugin);
    for (step, (name, word)) in host_mode::hosts().into_iter().enumerate() {
        let events = Events::new(&[(0, 0.1 + 0.1 * step as f64)]);
        let (after, _) = as_host(word, || unsafe { (*table).flush.unwrap()(plugin, events.list(), std::ptr::null()) });
        assert_flushed(take_set_param(), word, name);
        assert_eq!(after, word, "{name}: the host's mode was not restored after params.flush");
    }
}

#[test]
fn reset_runs_flushed_and_restores() {
    let (_adapter, plugin) = rig();
    for (name, word) in host_mode::hosts() {
        let (after, _) = as_host(word, || unsafe { ((*plugin).reset.unwrap())(plugin) });
        assert_flushed(take_reset(), word, name);
        assert_eq!(after, word, "{name}: the host's mode was not restored after reset");
    }
}

#[test]
fn a_plugin_that_opts_out_sees_the_host_mode_untouched() {
    opt_out(true);
    let (_adapter, plugin) = rig();
    opt_out(false);
    let mut audio = Audio::new(FRAMES);
    let table = params(plugin);
    for (step, (name, word)) in host_mode::hosts().into_iter().enumerate() {
        let events = Events::new(&[(0, 0.1 + 0.1 * step as f64)]);
        let (after, _) = as_host(word, || unsafe { audio.run(plugin, Some(&events)) });
        assert_eq!(take_process().map(|seen| seen.control), Some(word), "{name}: process");
        assert_eq!(take_set_param().map(|seen| seen.control), Some(word), "{name}: set_param in process");
        assert_eq!(after, word, "{name}");
        let events = Events::new(&[(0, 0.05 + 0.1 * step as f64)]);
        let (after, _) = as_host(word, || unsafe { (*table).flush.unwrap()(plugin, events.list(), std::ptr::null()) });
        assert_eq!(take_set_param().map(|seen| seen.control), Some(word), "{name}: params.flush");
        assert_eq!(after, word, "{name}");
        let (after, _) = as_host(word, || unsafe { ((*plugin).reset.unwrap())(plugin) });
        assert_eq!(take_reset().map(|seen| seen.control), Some(word), "{name}: reset");
        assert_eq!(after, word, "{name}");
    }
}
