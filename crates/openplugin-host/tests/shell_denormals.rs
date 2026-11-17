//! The shell ABI (what the VST3, Audio Unit and AAX shells call) runs the
//! plugin with denormals flushed and gives the host its own floating-point
//! mode back.
//!
//! Every call that enters the audio side holds the guard: `process_events`,
//! the simple `process`, `reset` (the Audio Unit calls it from its render
//! thread) and `midi_out`; the Rust `HostSession::process_*` calls go through
//! the same gate. For each host mode from `host_mode::hosts()` the test sets
//! the mode, calls the entry and checks what `FpProbe` saw inside and the mode
//! afterwards, also after a panic in `process`, and with
//! `Plugin::flush_denormals` returning false.

#[path = "../../openplugin-targets/tests/common/fp_probe.rs"]
mod fp_probe;

use fp_probe::*;
use openplugin_core::denormal::host_mode;
use openplugin_host::shell::{self, *};
use openplugin_host::HostSession;

const FRAMES: usize = 64;

fn mk() -> *mut ShellInstance {
    let instance = create::<FpProbe>(48_000.0, FRAMES as u32, 2);
    assert!(!instance.is_null());
    instance
}

/// One block through `process_events` with `events`; the status and the left output.
fn block(instance: *mut ShellInstance, events: &[OpEvent]) -> (u32, Vec<f32>) {
    let input = [0.5f32; FRAMES];
    let mut left = vec![9.0f32; FRAMES];
    let mut right = vec![9.0f32; FRAMES];
    let ins = [input.as_ptr(), input.as_ptr()];
    let outs = [left.as_mut_ptr(), right.as_mut_ptr()];
    let args = OpProcess {
        struct_size: std::mem::size_of::<OpProcess>() as u32,
        frames: FRAMES as u32,
        inputs: ins.as_ptr(),
        input_channels: 2,
        output_channels: 2,
        outputs: outs.as_ptr(),
        sidechain: std::ptr::null(),
        sidechain_channels: 0,
        event_count: events.len() as u32,
        events: if events.is_empty() { std::ptr::null() } else { events.as_ptr() },
        transport: std::ptr::null(),
    };
    let status = unsafe { process_events(instance, &args) };
    (status, left)
}

fn simple(instance: *mut ShellInstance) -> u32 {
    let input = [0.5f32; FRAMES];
    let mut left = vec![9.0f32; FRAMES];
    let mut right = vec![9.0f32; FRAMES];
    unsafe { shell::process(instance, input.as_ptr(), input.as_ptr(), left.as_mut_ptr(), right.as_mut_ptr(), FRAMES as u32) }
}

fn param(step: usize) -> OpEvent {
    OpEvent { kind: EVENT_PARAM_PLAIN, index: 0, value: 0.1 + 0.1 * step as f32, ..OpEvent::default() }
}

#[test]
fn process_events_runs_flushed_and_the_host_mode_comes_back() {
    let instance = mk();
    for (step, (name, word)) in host_mode::hosts().into_iter().enumerate() {
        let (after, (status, left)) = as_host(word, || block(instance, &[param(step)]));
        assert_eq!(status, 1, "{name}");
        assert!(left.iter().all(|s| *s == OUT), "{name}");
        assert_flushed(take_process(), word, name);
        assert_flushed(take_set_param(), word, name);
        assert_eq!(after, word, "{name}: the host's mode was not restored after process_events");
    }
    unsafe { destroy(instance) };
}

#[test]
fn the_simple_process_runs_flushed_and_restores() {
    let instance = mk();
    for (name, word) in host_mode::hosts() {
        let (after, status) = as_host(word, || simple(instance));
        assert_eq!(status, 1, "{name}");
        assert_flushed(take_process(), word, name);
        assert_eq!(after, word, "{name}: the host's mode was not restored after process");
    }
    unsafe { destroy(instance) };
}

#[test]
fn a_panic_in_process_still_restores_the_host_mode() {
    let instance = mk();
    for (name, word) in host_mode::hosts() {
        panic_next_process(true);
        let (after, (status, left)) = as_host(word, || block(instance, &[]));
        let (after_simple, simple_status) = as_host(word, || simple(instance));
        panic_next_process(false);
        assert_eq!((status, simple_status), (0, 0), "{name}: the panic was not reported");
        assert!(left.iter().all(|s| *s == 0.0), "{name}: a panicking block is silent");
        assert_flushed(take_process(), word, name);
        assert_eq!(after, word, "{name}: not restored after a panic in process_events");
        assert_eq!(after_simple, word, "{name}: not restored after a panic in process");
    }
    unsafe { destroy(instance) };
}

#[test]
fn reset_and_midi_out_restore_the_host_mode() {
    let instance = mk();
    for (name, word) in host_mode::hosts() {
        let (after, _) = as_host(word, || unsafe { reset(instance) });
        assert_flushed(take_reset(), word, name);
        assert_eq!(after, word, "{name}: the host's mode was not restored after reset");
        let mut out = [OpMidiEvent::default(); 4];
        let (after, _) = as_host(word, || unsafe { midi_out(instance, out.as_mut_ptr(), out.len() as u32) });
        assert_eq!(after, word, "{name}: the host's mode was not restored after midi_out");
    }
    unsafe { destroy(instance) };
}

#[test]
fn the_rust_host_session_runs_flushed_too() {
    let session = HostSession::new(Box::new(FpProbe::default()), 48_000.0, FRAMES, 2).unwrap();
    for (name, word) in host_mode::hosts() {
        let mut left = vec![0.5f32; FRAMES];
        let mut right = vec![0.5f32; FRAMES];
        let (after, ok) = as_host(word, || session.process_stereo(&mut left, &mut right));
        assert!(ok, "{name}");
        assert_flushed(take_process(), word, name);
        assert_eq!(after, word, "{name}");
    }
}

#[test]
fn a_plugin_that_opts_out_sees_the_host_mode_untouched() {
    opt_out(true);
    let instance = mk();
    opt_out(false);
    for (step, (name, word)) in host_mode::hosts().into_iter().enumerate() {
        let (after, _) = as_host(word, || block(instance, &[param(step)]));
        assert_eq!(take_process().map(|seen| seen.control), Some(word), "{name}: process_events");
        assert_eq!(take_set_param().map(|seen| seen.control), Some(word), "{name}: set_param");
        assert_eq!(after, word, "{name}");
        let (after, _) = as_host(word, || simple(instance));
        assert_eq!(take_process().map(|seen| seen.control), Some(word), "{name}: process");
        assert_eq!(after, word, "{name}");
        let (after, _) = as_host(word, || unsafe { reset(instance) });
        assert_eq!(take_reset().map(|seen| seen.control), Some(word), "{name}: reset");
        assert_eq!(after, word, "{name}");
    }
    unsafe { destroy(instance) };
}
