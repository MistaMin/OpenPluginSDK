//! Telling a CLAP host about changes (roadmap 0.6 task 7), with a fake host
//! that counts every call (`common::clap_host`) and the test plugin in
//! `common::shift`, whose latency follows a parameter.
//!
//! The CLAP rules: `latency.changed` may run only inside `activate`, so a
//! plugin that is active asks for `request_restart` (thread-safe) and reports
//! the new latency when the host activates it again. `params.rescan` runs on
//! the main thread, so a change seen on the audio thread asks for
//! `request_callback` and the rescan happens in `on_main_thread`.

mod common;

use std::sync::Arc;

use common::clap_host::{mark_main_thread, Audio, ChangeHost, Events};
use common::shift::{Calls, Shift, AFTER_LATENCY};
use openplugin_targets::clap_abi::*;
use openplugin_targets::ClapAdapter;

const LOOKAHEAD: u32 = 0;
const NAMES: u32 = 1;

unsafe fn latency_of(plugin: *const clap_plugin) -> u32 {
    unsafe {
        let ext = (*plugin).get_extension.unwrap()(plugin, EXT_LATENCY.as_ptr() as *const _) as *const clap_plugin_latency;
        (*ext).get.unwrap()(plugin)
    }
}

unsafe fn param_name(plugin: *const clap_plugin, index: u32) -> String {
    unsafe {
        let ext = (*plugin).get_extension.unwrap()(plugin, EXT_PARAMS.as_ptr() as *const _) as *const clap_plugin_params;
        let mut info = std::mem::zeroed::<clap_param_info>();
        assert!((*ext).get_info.unwrap()(plugin, index, &mut info));
        std::ffi::CStr::from_ptr(info.name.as_ptr()).to_string_lossy().into_owned()
    }
}

/// What a host does with `request_restart`: deactivate, then activate again.
unsafe fn restart(host: &ChangeHost, plugin: *const clap_plugin) {
    unsafe {
        (*plugin).deactivate.unwrap()(plugin);
        host.in_activate.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!((*plugin).activate.unwrap()(plugin, 48_000.0, 1, 64));
        host.in_activate.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Run `blocks` blocks on another thread, the first one carrying `events`.
fn play(plugin: *const clap_plugin, events: Option<Box<Events>>, blocks: usize) {
    let plugin = plugin as usize;
    std::thread::spawn(move || {
        let plugin = plugin as *const clap_plugin;
        let mut audio = Audio::new(64);
        for block in 0..blocks {
            let with = if block == 0 { events.as_deref() } else { None };
            assert_eq!(unsafe { audio.run(plugin, with) }, PROCESS_CONTINUE);
        }
    })
    .join()
    .unwrap();
}

#[test]
fn a_latency_parameter_asks_for_one_restart_and_activate_reports_the_new_latency() {
    mark_main_thread();
    let host = ChangeHost::new(true);
    let ch = host.clap_host();
    let calls = Arc::new(Calls::default());
    let mut adapter = unsafe { ClapAdapter::with_host(Shift::new(calls.clone()), &ch) };
    let p = adapter.as_plugin();
    host.in_activate.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(adapter.activate(48_000.0, 1, 64));
    host.in_activate.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(unsafe { latency_of(p) }, 0);
    assert_eq!(host.count(&host.latency_changed), 0, "an unchanged latency is not reported");

    // The audio thread applies lookahead = 64: the plugin tells its notifier there.
    play(p, Some(Events::new(&[(LOOKAHEAD, 64.0)])), 8);
    assert_eq!(calls.latency.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(host.count(&host.restarts), 1, "eight blocks, one restart request");
    assert_eq!(host.count(&host.latency_changed), 0, "never outside activate");
    assert_eq!(host.count(&host.callbacks), 0, "a latency change needs no main-thread callback");

    // The host restarts the plugin: activate reports the change once, on the main thread.
    unsafe { restart(&host, p) };
    assert_eq!(host.count(&host.latency_changed), 1);
    assert_eq!(unsafe { latency_of(p) }, 64, "the host reads the new latency back");
    play(p, None, 4);
    assert_eq!(host.count(&host.restarts), 1, "a reported latency asks nothing more");
    assert!(!host.off_main.load(std::sync::atomic::Ordering::SeqCst));
    assert!(!host.changed_outside_activate.load(std::sync::atomic::Ordering::SeqCst));

    // A second change is a second request.
    play(p, Some(Events::new(&[(LOOKAHEAD, 32.0)])), 2);
    assert_eq!(host.count(&host.restarts), 2);
    unsafe { restart(&host, p) };
    assert_eq!(host.count(&host.latency_changed), 2);
    assert_eq!(unsafe { latency_of(p) }, 32);
}

#[test]
fn a_latency_that_moves_by_itself_is_seen_after_the_block() {
    mark_main_thread();
    let host = ChangeHost::new(true);
    let ch = host.clap_host();
    let calls = Arc::new(Calls::default());
    let mut adapter = unsafe { ClapAdapter::with_host(Shift::new(calls.clone()).after_blocks(3), &ch) };
    let p = adapter.as_plugin();
    assert!(adapter.activate(48_000.0, 1, 64));
    play(p, None, 2);
    assert_eq!(host.count(&host.restarts), 0);
    play(p, None, 6);
    assert_eq!(calls.latency.load(std::sync::atomic::Ordering::SeqCst), 0, "the plugin never called its notifier");
    assert_eq!(host.count(&host.restarts), 1);
    unsafe { restart(&host, p) };
    assert_eq!(host.count(&host.latency_changed), 1);
    assert_eq!(unsafe { latency_of(p) }, AFTER_LATENCY as u32);
}

#[test]
fn new_parameter_names_are_rescanned_once_on_the_main_thread() {
    mark_main_thread();
    let host = ChangeHost::new(true);
    let ch = host.clap_host();
    let calls = Arc::new(Calls::default());
    let mut adapter = unsafe { ClapAdapter::with_host(Shift::new(calls.clone()), &ch) };
    let p = adapter.as_plugin();
    assert!(adapter.activate(48_000.0, 1, 64));
    assert_eq!(unsafe { param_name(p, 0) }, "Lookahead");

    play(p, Some(Events::new(&[(NAMES, 1.0)])), 6);
    assert_eq!(calls.param_info.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(host.count(&host.callbacks), 1, "six blocks, one callback request");
    assert_eq!(host.count(&host.rescans), 0, "the audio thread never rescans");

    // The host's main-thread callback.
    unsafe { (*p).on_main_thread.unwrap()(p) };
    assert_eq!(host.count(&host.rescans), 1);
    assert_eq!(
        host.last_rescan_flags.load(std::sync::atomic::Ordering::SeqCst),
        CLAP_PARAM_RESCAN_VALUES | CLAP_PARAM_RESCAN_TEXT | CLAP_PARAM_RESCAN_INFO
    );
    assert_eq!(unsafe { param_name(p, 0) }, "Look Ahead (renamed)", "the host reads the new name back");
    assert!(!host.off_main.load(std::sync::atomic::Ordering::SeqCst));
    // Told once: another callback, or more blocks, rescans nothing.
    unsafe { (*p).on_main_thread.unwrap()(p) };
    play(p, None, 3);
    assert_eq!(host.count(&host.rescans), 1);
    assert_eq!(host.count(&host.callbacks), 1);
    // Back to the first names: a second request and a second rescan.
    play(p, Some(Events::new(&[(NAMES, 0.0)])), 1);
    assert_eq!(host.count(&host.callbacks), 2);
    unsafe { (*p).on_main_thread.unwrap()(p) };
    assert_eq!(host.count(&host.rescans), 2);
    assert_eq!(unsafe { param_name(p, 0) }, "Lookahead");
    // Both at once: one restart, one callback.
    play(p, Some(Events::new(&[(LOOKAHEAD, 10.0), (NAMES, 1.0)])), 2);
    assert_eq!(host.count(&host.restarts), 1);
    assert_eq!(host.count(&host.callbacks), 3);
}

#[test]
fn without_request_callback_the_next_main_thread_call_rescans() {
    mark_main_thread();
    let host = ChangeHost::new(false);
    let ch = host.clap_host();
    let mut adapter = unsafe { ClapAdapter::with_host(Shift::default(), &ch) };
    let p = adapter.as_plugin();
    assert!(adapter.activate(48_000.0, 1, 64));
    play(p, Some(Events::new(&[(NAMES, 1.0)])), 2);
    assert_eq!(host.count(&host.rescans), 0);
    unsafe { (*p).deactivate.unwrap()(p) };
    assert_eq!(host.count(&host.rescans), 1);
    assert!(!host.off_main.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn a_change_while_inactive_is_reported_by_activate() {
    mark_main_thread();
    let host = ChangeHost::new(true);
    let ch = host.clap_host();
    let mut adapter = unsafe { ClapAdapter::with_host(Shift::default(), &ch) };
    let p = adapter.as_plugin();
    assert!(adapter.activate(48_000.0, 1, 64));
    // The host deactivates, then flushes a parameter on the main thread while inactive.
    unsafe {
        (*p).deactivate.unwrap()(p);
        let ext = (*p).get_extension.unwrap()(p, EXT_PARAMS.as_ptr() as *const _) as *const clap_plugin_params;
        let events = Events::new(&[(LOOKAHEAD, 48.0)]);
        (*ext).flush.unwrap()(p, events.list(), std::ptr::null());
    }
    assert_eq!(host.count(&host.restarts), 0, "an inactive plugin needs no restart");
    host.in_activate.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(adapter.activate(48_000.0, 1, 64));
    host.in_activate.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(host.count(&host.latency_changed), 1);
    assert_eq!(unsafe { latency_of(p) }, 48);
    assert!(!host.changed_outside_activate.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn a_steady_plugin_tells_the_host_nothing() {
    // A plugin that never keeps the notifier still has its latency watched.
    mark_main_thread();
    let host = ChangeHost::new(true);
    let ch = host.clap_host();
    let mut adapter = unsafe { ClapAdapter::with_host(Shift::default(), &ch) };
    assert!(adapter.activate(48_000.0, 1, 64));
    play(adapter.as_plugin(), None, 4);
    assert_eq!(host.count(&host.restarts), 0);
    assert_eq!(host.count(&host.callbacks), 0);
    assert_eq!(host.count(&host.rescans), 0);
    assert_eq!(host.count(&host.latency_changed), 0);
}
