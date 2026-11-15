//! A panic in the plugin never leaves the LV2 entry points.
//!
//! `Bomb` panics in `run` (its `process`), in `set_param` (a control port changing), in a
//! note callback (a MIDI event on the atom port) and in `reset` (`activate`/`deactivate`).
//! The exported descriptor is driven as a host does: the call returns, the outputs are
//! silence, later `run` calls are silent and harmless, and `activate` recovers the instance.
//! The LV2 entries here publish no state interface; the state paths are covered for CLAP.

mod common;
#[path = "common/bomb.rs"]
mod bomb;

use std::ffi::c_void;

use bomb::*;
use common::*;
use openplugin_core::Plugin;
use openplugin_targets::lv2::{self, Lv2Descriptor, PortMap};

const FRAMES: usize = 64;

struct Rig {
    desc: &'static Lv2Descriptor,
    instance: *mut c_void,
    host: Host,
    left: Vec<f32>,
    right: Vec<f32>,
    out: [Vec<f32>; 2],
    control: Box<f32>,
    events: AtomBuf,
}

fn create() -> Box<dyn Plugin> {
    Box::new(Bomb::default())
}

impl Rig {
    fn new(midi: Option<[u8; 3]>) -> Self {
        lv2::set_creator(create);
        let mut host = Host::new();
        let desc = unsafe { &*lv2::descriptor(0) };
        let instance = (desc.instantiate)(desc, 48_000.0, std::ptr::null(), host.features());
        assert!(!instance.is_null());
        let map = PortMap::of(&Bomb::default());
        let mut seq = Seq::new(&mut host);
        if let Some(data) = midi {
            seq.midi(&mut host, 0, &data);
        }
        let mut rig = Self {
            desc,
            instance,
            host,
            left: vec![0.5; FRAMES],
            right: vec![0.5; FRAMES],
            out: [vec![0.0; FRAMES], vec![0.0; FRAMES]],
            control: Box::new(0.5),
            events: seq.buf(),
        };
        (desc.connect_port)(instance, 0, rig.left.as_mut_ptr().cast());
        (desc.connect_port)(instance, 1, rig.right.as_mut_ptr().cast());
        // The host uses separate input and output buffers: the outputs are ports 2 and 3.
        (desc.connect_port)(instance, 2, rig.out[0].as_mut_ptr().cast());
        (desc.connect_port)(instance, 3, rig.out[1].as_mut_ptr().cast());
        (desc.connect_port)(instance, map.first_param(), (&mut *rig.control as *mut f32).cast());
        let atom = map.atom_in().expect("the bomb takes notes");
        (desc.connect_port)(instance, atom, rig.events.ptr());
        let _ = &rig.host;
        rig
    }

    /// Prefill the outputs with 9.0, run one block, return both outputs.
    fn run(&mut self) -> Vec<Vec<f32>> {
        for channel in &mut self.out {
            channel.fill(9.0);
        }
        (self.desc.run)(self.instance, FRAMES as u32);
        self.out.to_vec()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        (self.desc.cleanup)(self.instance);
    }
}

fn silent(out: &[Vec<f32>]) -> bool {
    out.iter().all(|c| c.iter().all(|s| *s == 0.0))
}

fn sounding(out: &[Vec<f32>]) -> bool {
    out.iter().all(|c| c.iter().all(|s| *s == OUT))
}

fn recovers(rig: &mut Rig) {
    arm(NONE);
    assert!(silent(&rig.run()), "a poisoned instance keeps outputting silence");
    (rig.desc.activate)(rig.instance);
    assert!(sounding(&rig.run()), "activate clears the poison");
}

#[test]
fn a_healthy_run_makes_sound() {
    let _g = serial();
    arm(NONE);
    let mut rig = Rig::new(None);
    assert!(sounding(&rig.run()));
}

#[test]
fn panic_in_process() {
    let _g = serial();
    arm(PROCESS);
    let mut rig = Rig::new(None);
    assert!(silent(&rig.run()), "the panicking block is silent");
    recovers(&mut rig);
}

#[test]
fn panic_in_set_param_from_a_control_port() {
    let _g = serial();
    arm(NONE);
    let mut rig = Rig::new(None);
    assert!(sounding(&rig.run()));
    arm(SET_PARAM);
    *rig.control = 0.9;
    assert!(silent(&rig.run()));
    recovers(&mut rig);
}

#[test]
fn panic_in_a_note_callback() {
    let _g = serial();
    arm(NOTE);
    let mut rig = Rig::new(Some([0x90, 60, 100]));
    assert!(silent(&rig.run()));
    recovers(&mut rig);
}

#[test]
fn panic_in_deactivate_poisons_until_activate() {
    let _g = serial();
    arm(NONE);
    let mut rig = Rig::new(None);
    arm(RESET);
    (rig.desc.deactivate)(rig.instance);
    arm(NONE);
    assert!(silent(&rig.run()));
    (rig.desc.activate)(rig.instance);
    assert!(sounding(&rig.run()));
}

#[test]
fn panic_in_activate_leaves_a_poisoned_instance_that_is_silent() {
    let _g = serial();
    arm(NONE);
    let mut rig = Rig::new(None);
    arm(RESET);
    (rig.desc.activate)(rig.instance);
    assert!(silent(&rig.run()));
    arm(NONE);
    (rig.desc.activate)(rig.instance);
    assert!(sounding(&rig.run()));
}

#[test]
fn extension_data_and_cleanup_survive_a_panicked_instance() {
    let _g = serial();
    arm(PROCESS);
    let mut rig = Rig::new(None);
    assert!(silent(&rig.run()));
    arm(NONE);
    let uri = std::ffi::CString::new("http://example.com/unknown").unwrap();
    assert!((rig.desc.extension_data)(uri.as_ptr()).is_null());
}
