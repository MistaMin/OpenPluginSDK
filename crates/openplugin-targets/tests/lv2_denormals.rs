//! The LV2 entry runs the plugin with denormals flushed in `run` and gives
//! the host its own floating-point mode back, also after a panic; a plugin
//! that opts out sees the host's mode untouched.
//!
//! `activate`, `deactivate`, `instantiate` and `cleanup` are not audio-thread
//! calls in LV2 and are left alone; `work_response` and `end_run` do not call
//! into the plugin; `work` runs a loader on the host's worker thread, not the
//! audio thread.

mod common;
#[path = "common/fp_probe.rs"]
mod fp_probe;

use std::ffi::c_void;

use common::*;
use fp_probe::*;
use openplugin_core::denormal::host_mode;
use openplugin_core::Plugin;
use openplugin_targets::lv2::{self, Lv2Descriptor, PortMap};

const FRAMES: usize = 64;

struct Rig {
    desc: &'static Lv2Descriptor,
    instance: *mut c_void,
    _host: Host,
    input: [Vec<f32>; 2],
    out: [Vec<f32>; 2],
    control: Box<f32>,
}

fn create() -> Box<dyn Plugin> {
    Box::new(FpProbe::default())
}

impl Rig {
    /// An instance made on this thread, so [`opt_out`] applies to it.
    fn new() -> Self {
        lv2::set_creator(create);
        let host = Host::new();
        let desc = unsafe { &*lv2::descriptor(0) };
        let instance = (desc.instantiate)(desc, 48_000.0, std::ptr::null(), host.features());
        assert!(!instance.is_null());
        let map = PortMap::of(&FpProbe::default());
        let mut rig = Self {
            desc,
            instance,
            _host: host,
            input: [vec![0.5; FRAMES], vec![0.5; FRAMES]],
            out: [vec![0.0; FRAMES], vec![0.0; FRAMES]],
            control: Box::new(0.5),
        };
        (desc.connect_port)(instance, 0, rig.input[0].as_mut_ptr().cast());
        (desc.connect_port)(instance, 1, rig.input[1].as_mut_ptr().cast());
        (desc.connect_port)(instance, 2, rig.out[0].as_mut_ptr().cast());
        (desc.connect_port)(instance, 3, rig.out[1].as_mut_ptr().cast());
        (desc.connect_port)(instance, map.first_param(), (&mut *rig.control as *mut f32).cast());
        (desc.activate)(instance);
        rig
    }

    /// One `run` with the host's control word set to `word`; the word after it.
    fn run_as(&mut self, word: u64) -> u64 {
        let (desc, instance) = (self.desc, self.instance);
        as_host(word, || (desc.run)(instance, FRAMES as u32)).0
    }

    fn sounding(&self) -> bool {
        self.out.iter().all(|c| c.iter().all(|s| *s == OUT))
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        (self.desc.cleanup)(self.instance);
    }
}

#[test]
fn run_is_flushed_and_the_host_mode_comes_back() {
    let mut rig = Rig::new();
    for (name, word) in host_mode::hosts() {
        let after = rig.run_as(word);
        assert_flushed(take_process(), word, name);
        assert!(rig.sounding(), "{name}");
        assert_eq!(after, word, "{name}: the host's mode was not restored after run");
    }
}

#[test]
fn a_control_port_change_reaches_set_param_flushed() {
    let mut rig = Rig::new();
    for (step, (name, word)) in host_mode::hosts().into_iter().enumerate() {
        *rig.control = 0.1 + 0.1 * step as f32;
        let after = rig.run_as(word);
        assert_flushed(take_set_param(), word, name);
        assert_eq!(after, word, "{name}");
    }
}

#[test]
fn a_panic_in_run_still_restores_the_host_mode() {
    let mut rig = Rig::new();
    for (name, word) in host_mode::hosts() {
        panic_next_process(true);
        let after = rig.run_as(word);
        panic_next_process(false);
        assert_flushed(take_process(), word, name);
        assert!(rig.out.iter().all(|c| c.iter().all(|s| *s == 0.0)), "{name}: a panicking run is silent");
        assert_eq!(after, word, "{name}: the host's mode was not restored after a panic");
        // Poisoned until activated again.
        (rig.desc.activate)(rig.instance);
    }
}

#[test]
fn a_plugin_that_opts_out_sees_the_host_mode_untouched() {
    opt_out(true);
    let mut rig = Rig::new();
    opt_out(false);
    for (step, (name, word)) in host_mode::hosts().into_iter().enumerate() {
        *rig.control = 0.1 + 0.1 * step as f32;
        let after = rig.run_as(word);
        assert_eq!(take_process().map(|seen| seen.control), Some(word), "{name}: run");
        assert_eq!(take_set_param().map(|seen| seen.control), Some(word), "{name}: set_param");
        assert_eq!(after, word, "{name}");
    }
}
