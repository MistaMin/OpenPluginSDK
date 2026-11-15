//! Shell ABI 6, `op_shell_take_changes`: what the native VST3, Audio Unit and
//! AAX shells poll on their main thread to learn that the plugin's latency or
//! parameter info changed. Driven through the same functions `export_shell!`
//! exports, with the test plugin of `openplugin-targets/tests/common/shift.rs`
//! (latency = its `lookahead` parameter; `names` swaps the parameter names).

#[path = "../../openplugin-targets/tests/common/shift.rs"]
mod shift;

use std::sync::Arc;

use openplugin_core::HostNotifier;
use openplugin_ffi::shell::{self, ShellInstance, CHANGE_LATENCY, CHANGE_PARAM_INFO};
use openplugin_ffi::FfiInstance;
use shift::{Calls, Shift, AFTER_LATENCY};

const LOOKAHEAD: u32 = 0;
const NAMES: u32 = 1;
const FRAMES: usize = 64;

fn make() -> *mut ShellInstance {
    let instance = shell::create::<Shift>(48_000.0, FRAMES as u32, 2);
    assert!(!instance.is_null());
    instance
}

fn block(instance: *mut ShellInstance) {
    let (mut left, mut right) = ([0.1f32; FRAMES], [0.1f32; FRAMES]);
    let (l, r) = (left.as_mut_ptr(), right.as_mut_ptr());
    assert_eq!(unsafe { shell::process(instance, l, r, l, r, FRAMES as u32) }, 1);
}

#[test]
fn the_constants_are_the_notifier_bits() {
    const { assert!(shell::ABI >= 6) };
    assert_eq!(CHANGE_LATENCY, HostNotifier::LATENCY);
    assert_eq!(CHANGE_PARAM_INFO, HostNotifier::PARAM_INFO);
    assert_eq!(unsafe { shell::take_changes(std::ptr::null()) }, 0);
}

#[test]
fn a_latency_change_is_reported_once_and_read_back() {
    let instance = make();
    unsafe {
        assert_eq!(shell::take_changes(instance), 0, "a new instance has nothing to say");
        assert_eq!(shell::latency(instance), 0);
        assert_eq!(shell::set_plain(instance, LOOKAHEAD, 64.0), 1);
        assert_eq!(shell::take_changes(instance), 0, "a stored value changes nothing until a block applies it");
        block(instance);
        assert_eq!(shell::take_changes(instance), CHANGE_LATENCY);
        assert_eq!(shell::take_changes(instance), 0, "told once");
        assert_eq!(shell::latency(instance), 64, "the shell reads the new latency back");
        for _ in 0..4 {
            block(instance);
        }
        assert_eq!(shell::take_changes(instance), 0, "an unchanged latency is quiet");
        // A host that reads the latency before the shell polls already knows it.
        shell::set_plain(instance, LOOKAHEAD, 32.0);
        block(instance);
        assert_eq!(shell::latency(instance), 32);
        assert_eq!(shell::take_changes(instance), 0);
        // There and back before the poll: nothing new for the host.
        shell::set_plain(instance, LOOKAHEAD, 8.0);
        block(instance);
        shell::set_plain(instance, LOOKAHEAD, 32.0);
        block(instance);
        assert_eq!(shell::take_changes(instance), 0);
        shell::destroy(instance);
    }
}

#[test]
fn new_parameter_names_are_reported_once() {
    let instance = make();
    unsafe {
        shell::set_plain(instance, NAMES, 1.0);
        block(instance);
        assert_eq!(shell::take_changes(instance), CHANGE_PARAM_INFO);
        assert_eq!(shell::take_changes(instance), 0);
        // Both at once.
        shell::set_plain(instance, NAMES, 0.0);
        shell::set_plain(instance, LOOKAHEAD, 12.0);
        block(instance);
        assert_eq!(shell::take_changes(instance), CHANGE_LATENCY | CHANGE_PARAM_INFO);
        assert_eq!(shell::take_changes(instance), 0);
        shell::destroy(instance);
    }
}

#[test]
fn a_latency_that_moves_by_itself_is_reported() {
    let calls = Arc::new(Calls::default());
    let ffi = FfiInstance::new(Box::new(Shift::new(calls.clone()).after_blocks(3)), 48_000.0, FRAMES, 2);
    let (mut left, mut right) = (vec![0.0f32; FRAMES], vec![0.0f32; FRAMES]);
    assert!(ffi.process_stereo(&mut left, &mut right));
    assert!(ffi.process_stereo(&mut left, &mut right));
    assert_eq!(ffi.take_changes(), 0);
    assert!(ffi.process_stereo(&mut left, &mut right));
    assert_eq!(ffi.take_changes(), HostNotifier::LATENCY);
    assert_eq!(ffi.latency(), AFTER_LATENCY as u32);
    assert_eq!(calls.latency.load(std::sync::atomic::Ordering::SeqCst), 0, "the plugin never said so itself");
}

/// A VST3 host answers kLatencyChanged by deactivating and activating the
/// plugin, and the shell builds a new engine. The parameters it stores reach
/// the engine at `reset`, so the latency the host reads at once is the right
/// one, and the new engine has nothing more to report.
#[test]
fn a_new_engine_with_its_parameters_reset_in_reports_nothing() {
    let instance = make();
    unsafe {
        shell::set_plain(instance, LOOKAHEAD, 64.0);
        assert_eq!(shell::latency(instance), 0, "not applied yet");
        shell::reset(instance);
        assert_eq!(shell::latency(instance), 64, "reset applies the stored values first");
        block(instance);
        assert_eq!(shell::take_changes(instance), 0, "the host read 64 already");
        shell::destroy(instance);
    }
}

#[test]
fn a_host_that_never_reads_is_told_after_reset_too() {
    let instance = make();
    unsafe {
        shell::set_plain(instance, LOOKAHEAD, 20.0);
        shell::reset(instance);
        assert_eq!(shell::take_changes(instance), CHANGE_LATENCY);
        assert_eq!(shell::latency(instance), 20);
        shell::destroy(instance);
    }
}

#[test]
fn takes_from_another_thread_see_every_change_once() {
    let instance = make();
    let address = instance as usize;
    unsafe {
        let mut told = 0;
        for value in [10.0f32, 20.0, 30.0, 40.0] {
            shell::set_plain(instance, LOOKAHEAD, value);
            block(instance);
            told += std::thread::spawn(move || {
                let instance = address as *const ShellInstance;
                let flags = shell::take_changes(instance);
                let latency = shell::latency(instance);
                (flags == CHANGE_LATENCY && latency == value as u32) as u32
            })
            .join()
            .unwrap();
        }
        assert_eq!(told, 4);
        shell::destroy(instance);
    }
}
