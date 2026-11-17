//! Deliberate faults in the C++ side. Run with `cargo test -p example-cpp-bridge --features test-faults`.
//!
//! These tests settle what the conformance kit can and cannot see of C++:
//! - the kit counts calls to Rust's global allocator on the audio thread. C++
//!   `new` only reaches it because the test build replaces `operator new`
//!   (`cpp/count_new.cpp`); so `new` in C++ is caught;
//! - a bare C++ `malloc` never touches Rust's allocator, so the kit alone does
//!   NOT see it. `malloc_logger` (below) does.
#![cfg(feature = "test-faults")]
#![allow(unsafe_code)]

use std::cell::Cell;
use std::sync::Mutex;

use example_cpp_bridge::{CppBridge, Fault};
use openplugin::prelude::*;
use openplugin_testing::{Check, Options, Status, conformance};

openplugin_testing::install_allocator!();

/// A plugin that switches a fault on after construction.
struct Faulty<const F: u8>(CppBridge);
impl<const F: u8> Default for Faulty<F> {
    fn default() -> Self {
        let mut p = CppBridge::default();
        p.inject_fault(match F {
            1 => Fault::CppNew,
            2 => Fault::CppMalloc,
            _ => Fault::None,
        });
        Self(p)
    }
}
macro_rules! forward {
    () => {
        fn info(&self) -> &'static PluginInfo { self.0.info() }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) { self.0.prepare(sr, mb, ch) }
        fn reset(&mut self) { self.0.reset() }
        fn set_param(&mut self, id: &str, v: f32) -> bool { self.0.set_param(id, v) }
        fn param(&self, id: &str) -> Option<f32> { self.0.param(id) }
        fn process(&mut self, io: &mut [&mut [f32]], n: usize) { self.0.process(io, n) }
        fn tail(&self) -> Tail { self.0.tail() }
        fn note_ports(&self) -> NotePorts { self.0.note_ports() }
        fn save_state(&mut self) -> Option<Vec<u8>> { self.0.save_state() }
        fn load_state(&mut self, d: &[u8]) -> bool { self.0.load_state(d) }
    };
}
impl<const F: u8> Plugin for Faulty<F> {
    forward!();
}

fn quick() -> Options {
    let mut o = Options::quick();
    o.fuzz_sessions = 2;
    o
}

#[test]
fn cpp_new_on_the_audio_path_fails_the_no_alloc_check() {
    let report = conformance(Faulty::<1>::default, quick());
    assert_eq!(report.status(Check::NoAlloc), Status::Fail, "{report}");
    assert!(report.failed_checks().contains(&Check::NoAlloc));
}

#[test]
fn the_unfaulted_plugin_passes_the_same_check() {
    let report = conformance(Faulty::<0>::default, quick());
    assert_eq!(report.status(Check::NoAlloc), Status::Pass, "{report}");
}

// ---- raw malloc: the kit does not see it, malloc_logger does ----

type MallocLogger = unsafe extern "C" fn(u32, usize, usize, usize, usize, u32);
unsafe extern "C" {
    static mut malloc_logger: Option<MallocLogger>;
}
thread_local! { static MALLOCS: Cell<u64> = const { Cell::new(0) }; static ARMED: Cell<bool> = const { Cell::new(false) }; }
static PROBE: Mutex<()> = Mutex::new(());

unsafe extern "C" fn log_malloc(kind: u32, _a: usize, _b: usize, _c: usize, _d: usize, _e: u32) {
    // stack_logging_type_alloc = 2: set for malloc/calloc/realloc results.
    if kind & 2 != 0 {
        let _ = ARMED.try_with(|a| {
            if a.get() {
                let _ = MALLOCS.try_with(|m| m.set(m.get() + 1));
            }
        });
    }
}

/// Number of malloc-family allocations `f` makes on this thread, process wide
/// allocator included (C++ `malloc`, `new`, anything in libc).
fn count_mallocs(f: impl FnOnce()) -> u64 {
    let _guard = PROBE.lock().unwrap();
    MALLOCS.with(|m| m.set(0));
    unsafe { malloc_logger = Some(log_malloc) };
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    unsafe { malloc_logger = None };
    MALLOCS.with(Cell::get)
}

#[test]
fn raw_cpp_malloc_is_invisible_to_the_kit_but_visible_to_malloc_logger() {
    let report = conformance(Faulty::<2>::default, quick());
    assert_eq!(
        report.status(Check::NoAlloc),
        Status::Pass,
        "if this fails the kit learned to see malloc: update docs/CPP_BRIDGE.md\n{report}"
    );

    let mut clean = Faulty::<0>::default();
    let mut dirty = Faulty::<2>::default();
    clean.prepare(48_000.0, 256, 2);
    dirty.prepare(48_000.0, 256, 2);
    let mut io = vec![vec![0.1f32; 256]; 2];
    let clean_count = count_mallocs(|| clean.process(&mut openplugin::core::ChannelsMut::new(&mut io), 256));
    let dirty_count = count_mallocs(|| dirty.process(&mut openplugin::core::ChannelsMut::new(&mut io), 256));
    assert_eq!(clean_count, 0, "the clean path makes no malloc");
    assert_eq!(dirty_count, 2, "one malloc per channel");
}

// ---- exceptions and panics ----

#[test]
fn a_non_std_cpp_exception_in_prepare_is_an_error() {
    let mut p = CppBridge::default();
    p.inject_fault(Fault::PrepareForeignException);
    p.prepare(48_000.0, 256, 2);
    assert!(!p.is_ready());
    assert!(p.last_error().unwrap().contains("non-std"), "{:?}", p.last_error());
}

#[test]
fn a_std_exception_in_prepare_keeps_its_message() {
    let mut p = CppBridge::default();
    p.inject_fault(Fault::PrepareStdException);
    p.prepare(48_000.0, 256, 2);
    assert_eq!(p.last_error(), Some("injected std::exception in prepare"));
}

#[test]
fn a_non_std_cpp_exception_in_load_state_is_refused() {
    let mut p = CppBridge::default();
    let chunk = p.save_state().unwrap();
    p.inject_fault(Fault::LoadForeignException);
    assert!(!p.load_state(&chunk));
    assert!(p.last_error().unwrap().contains("non-std"));
}

#[test]
fn a_rust_panic_in_process_is_caught_and_the_plugin_passes_audio_through() {
    let mut p = CppBridge::default();
    p.prepare(48_000.0, 256, 2);
    p.inject_fault(Fault::RustPanic);
    let mut io = vec![vec![0.5f32; 64]; 2];
    p.process(&mut openplugin::core::ChannelsMut::new(&mut io), 64);
    assert!(!p.is_ready());
    assert_eq!(p.last_error(), Some("panic in process"));
    p.process(&mut openplugin::core::ChannelsMut::new(&mut io), 64);
    assert!(io[0].iter().all(|&s| s == 0.5));
}
