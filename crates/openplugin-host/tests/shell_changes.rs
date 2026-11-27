//! Shell ABI 6, `op_shell_take_changes`: what the native VST3, Audio Unit and
//! AAX shells poll on their main thread to learn that the plugin's latency or
//! parameter info changed. Driven through the same functions `export_shell!`
//! exports, with the test plugin of `openplugin-targets/tests/common/shift.rs`
//! (latency = its `lookahead` parameter; `names` swaps the parameter names).

#[path = "../../openplugin-targets/tests/common/shift.rs"]
mod shift;

use std::sync::Arc;

use openplugin_core::HostNotifier;
use openplugin_host::shell::{self, ShellInstance, CHANGE_LATENCY, CHANGE_PARAM_INFO};
use openplugin_host::HostSession;
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
    assert_eq!(openplugin_host::shell::CHANGE_STATE, HostNotifier::STATE);
    let notifier = HostNotifier::new();
    notifier.state_changed();
    assert_eq!(notifier.take(), HostNotifier::STATE);
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
    let ffi = HostSession::new(Box::new(Shift::new(calls.clone()).after_blocks(3)), 48_000.0, FRAMES, 2).unwrap();
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

// ---- ABI 9: the instance's own parameter table, and its largest latency ----

fn name_of(desc: &shell::OpParamDesc) -> String {
    let end = desc.name.iter().position(|b| *b == 0).unwrap_or(desc.name.len());
    String::from_utf8_lossy(&desc.name[..end]).into_owned()
}

fn unit_of(desc: &shell::OpParamDesc) -> String {
    let end = desc.unit.iter().position(|b| *b == 0).unwrap_or(desc.unit.len());
    String::from_utf8_lossy(&desc.unit[..end]).into_owned()
}

fn instance_desc(instance: *const ShellInstance, index: u32) -> shell::OpParamDesc {
    let mut desc = std::mem::MaybeUninit::<shell::OpParamDesc>::zeroed();
    assert_eq!(unsafe { shell::instance_param::<Shift>(instance, index, desc.as_mut_ptr()) }, 1);
    unsafe { desc.assume_init() }
}

fn type_desc(index: u32) -> shell::OpParamDesc {
    let mut desc = std::mem::MaybeUninit::<shell::OpParamDesc>::zeroed();
    assert_eq!(unsafe { shell::write_param::<Shift>(index, desc.as_mut_ptr()) }, 1);
    unsafe { desc.assume_init() }
}

fn instance_text(instance: *const ShellInstance, index: u32, plain: f64) -> String {
    let mut buf = [0u8; 128];
    let length = unsafe { shell::instance_param_format::<Shift>(instance, index, plain, buf.as_mut_ptr(), buf.len()) };
    String::from_utf8(buf[..length].to_vec()).unwrap()
}

/// What a shell does after `OP_CHANGE_PARAM_INFO`: it reads the instance's
/// table and gets the new name, unit and value text; the calls without an
/// instance keep the type's first table (the negative control).
#[test]
fn a_renamed_table_is_read_through_the_instance() {
    let instance = make();
    unsafe {
        assert_eq!(name_of(&instance_desc(instance, LOOKAHEAD)), "Lookahead");
        let before = instance_text(instance, LOOKAHEAD, 64.0);
        assert_eq!(before, shift::PARAMS[0].format(64.0));
        shell::set_plain(instance, NAMES, 1.0);
        block(instance);
        assert_eq!(shell::take_changes(instance), CHANGE_PARAM_INFO);
        let renamed = instance_desc(instance, LOOKAHEAD);
        assert_eq!(name_of(&renamed), "Look Ahead (renamed)");
        assert_eq!(unit_of(&renamed), "ms");
        assert_eq!(name_of(&instance_desc(instance, NAMES)), "Names (renamed)");
        let after = instance_text(instance, LOOKAHEAD, 64.0);
        assert_eq!(after, shift::RENAMED[0].format(64.0));
        assert_ne!(after, before, "the value text follows the renamed table");
        // Typed text is read by the renamed table too.
        let typed = std::ffi::CString::new(after.clone()).unwrap();
        let mut plain = 0.0;
        assert_eq!(shell::instance_param_parse::<Shift>(instance, LOOKAHEAD, typed.as_ptr(), &mut plain), 1);
        assert_eq!(plain, 64.0);
        // Everything but the name and unit is the first table's.
        let first = type_desc(LOOKAHEAD);
        assert_eq!((renamed.min, renamed.max, renamed.steps, renamed.flags, renamed.id), (first.min, first.max, first.steps, first.flags, first.id));
        // Negative control: the calls without an instance still read the first table.
        assert_eq!(name_of(&first), "Lookahead");
        let mut buf = [0u8; 128];
        let length = shell::param_format::<Shift>(LOOKAHEAD, 64.0, buf.as_mut_ptr(), buf.len());
        assert_eq!(std::str::from_utf8(&buf[..length]).unwrap(), before);
        // And back.
        shell::set_plain(instance, NAMES, 0.0);
        block(instance);
        assert_eq!(shell::take_changes(instance), CHANGE_PARAM_INFO);
        assert_eq!(name_of(&instance_desc(instance, LOOKAHEAD)), "Lookahead");
        assert_eq!(instance_text(instance, LOOKAHEAD, 64.0), before);
        shell::destroy(instance);
    }
}

/// A null, foreign or destroyed instance answers from the first table, and an
/// unknown index or a null `out` writes nothing.
#[test]
fn a_pointer_that_is_not_a_live_instance_reads_the_first_table() {
    let instance = make();
    unsafe {
        shell::set_plain(instance, NAMES, 1.0);
        block(instance);
        assert_eq!(name_of(&instance_desc(instance, LOOKAHEAD)), "Look Ahead (renamed)");
        assert_eq!(name_of(&instance_desc(std::ptr::null(), LOOKAHEAD)), "Lookahead");
        let local = 0u64;
        let foreign = &local as *const u64 as *const ShellInstance;
        assert_eq!(name_of(&instance_desc(foreign, LOOKAHEAD)), "Lookahead");
        assert_eq!(instance_text(foreign, LOOKAHEAD, 64.0), shift::PARAMS[0].format(64.0));
        assert_eq!(shell::instance_param::<Shift>(instance, 2, std::ptr::null_mut()), 0);
        let mut desc = std::mem::MaybeUninit::<shell::OpParamDesc>::zeroed();
        assert_eq!(shell::instance_param::<Shift>(instance, 2, desc.as_mut_ptr()), 0);
        shell::destroy(instance);
        assert_eq!(name_of(&instance_desc(instance, LOOKAHEAD)), "Lookahead", "a destroyed instance is not read");
    }
}

/// A rename the plugin makes outside a block (from its editor, say): no block
/// runs, and the poll itself reads the new table before it reports the change.
#[test]
fn a_rename_outside_a_block_is_read_before_it_is_reported() {
    let shared = Arc::new(FlipShared::default());
    let ffi = HostSession::new(Box::new(Flip(shared.clone())), 48_000.0, FRAMES, 2).unwrap();
    assert_eq!(ffi.current_info().params[0].name, "Lookahead");
    shared.renamed.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(ffi.take_changes(), 0, "a table the plugin never announced is not read");
    shared.notifier.lock().unwrap().as_ref().unwrap().param_info_changed();
    assert_eq!(ffi.take_changes(), HostNotifier::PARAM_INFO);
    assert_eq!(ffi.current_info().params[0].name, "Look Ahead (renamed)", "read before it was reported");
    assert_eq!(ffi.take_changes(), 0, "told once");
}

#[derive(Default)]
struct FlipShared {
    renamed: std::sync::atomic::AtomicBool,
    notifier: std::sync::Mutex<Option<HostNotifier>>,
}

/// Renames when the test says so, outside any block.
struct Flip(Arc<FlipShared>);

impl openplugin_core::Plugin for Flip {
    fn info(&self) -> &'static openplugin_core::PluginInfo {
        if self.0.renamed.load(std::sync::atomic::Ordering::SeqCst) { &shift::INFO_RENAMED } else { &shift::INFO }
    }
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        true
    }
    fn param(&self, _id: &str) -> Option<f32> {
        Some(0.0)
    }
    fn process(&mut self, _io: &mut [&mut [f32]], _frames: usize) {}
    fn set_host_notifier(&mut self, notifier: HostNotifier) {
        *self.0.notifier.lock().unwrap() = Some(notifier);
    }
}

/// The largest latency is the plugin's declaration, and the audio thread's
/// read of the latency does not swallow the change the host must be told.
#[test]
fn the_largest_latency_and_a_read_that_is_not_the_hosts() {
    let instance = make();
    unsafe {
        assert_eq!(shell::max_latency(std::ptr::null()), 0);
        assert_eq!(shell::latency_now(std::ptr::null()), 0);
        assert_eq!(shell::max_latency(instance), shift::MAX_LOOKAHEAD as u32);
        shell::set_plain(instance, LOOKAHEAD, 40.0);
        block(instance);
        assert_eq!(shell::latency_now(instance), 40);
        assert_eq!(shell::take_changes(instance), CHANGE_LATENCY, "latency_now is not the host's read");
        assert_eq!(shell::latency(instance), 40);
        assert_eq!(shell::take_changes(instance), 0);
        shell::destroy(instance);
    }
}

/// A plugin that declares no largest latency: its latency is the largest.
#[test]
fn without_a_declaration_the_largest_latency_is_the_latency() {
    let ffi = HostSession::new(Box::new(Shift::default()), 48_000.0, FRAMES, 2).unwrap();
    assert_eq!(ffi.max_latency(), shift::MAX_LOOKAHEAD as u32);
    let quiet = HostSession::new(Box::new(NoMax), 48_000.0, FRAMES, 2).unwrap();
    assert_eq!(quiet.max_latency(), 7);
}

struct NoMax;

impl openplugin_core::Plugin for NoMax {
    fn info(&self) -> &'static openplugin_core::PluginInfo {
        &shift::INFO
    }
    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        true
    }
    fn param(&self, _id: &str) -> Option<f32> {
        Some(0.0)
    }
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
    fn reset(&mut self) {}
    fn process(&mut self, _io: &mut [&mut [f32]], _frames: usize) {}
    fn latency(&self) -> usize {
        7
    }
}
