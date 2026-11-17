//! The bridged plugin through the SDK's conformance kit, and what the C++ side
//! must do when something goes wrong.

use example_cpp_bridge::CppBridge;
use openplugin::prelude::*;
use openplugin_testing::{Options, conformance};

openplugin_testing::install_allocator!();

#[test]
fn passes_the_conformance_kit() {
    let options = if cfg!(debug_assertions) { Options::quick() } else { Options::default() };
    let report = conformance(CppBridge::default, options);
    report.assert_pass();
    // The allocation check must really have run, not been skipped.
    assert_eq!(report.status(openplugin_testing::Check::NoAlloc), openplugin_testing::Status::Pass, "{report}");
}

fn block(frames: usize, value: f32) -> Vec<Vec<f32>> {
    vec![vec![value; frames]; 2]
}

#[test]
fn dc_settles_to_the_gain() {
    let mut p = CppBridge::default();
    p.prepare(48_000.0, 512, 2);
    assert!(p.is_ready());
    assert!(p.set_param("gain", 6.0));
    p.reset();
    let mut io = block(512, 0.25);
    for _ in 0..40 {
        io = block(512, 0.25);
        p.process(&mut openplugin::core::ChannelsMut::new(&mut io), 512);
    }
    let want = 0.25 * 10f32.powf(6.0 / 20.0);
    assert!((io[1][511] - want).abs() < 1e-3, "{} vs {want}", io[1][511]);
}

#[test]
fn a_cpp_exception_in_prepare_becomes_an_error_and_a_passthrough() {
    let mut p = CppBridge::default();
    p.prepare(48_000.0, 512, 0); // C++ throws std::invalid_argument
    assert!(!p.is_ready());
    assert!(p.last_error().unwrap().contains("channel count"), "{:?}", p.last_error());
    let mut io = block(64, 0.5);
    p.process(&mut openplugin::core::ChannelsMut::new(&mut io), 64);
    assert!(io[0].iter().all(|&s| s == 0.5), "pass-through leaves audio alone");
    // A later good prepare recovers.
    p.prepare(48_000.0, 512, 2);
    assert!(p.is_ready() && p.last_error().is_none());
}

#[test]
fn state_round_trips_as_bytes_and_bad_bytes_are_refused() {
    let mut a = CppBridge::default();
    a.prepare(44_100.0, 256, 2);
    a.set_param("cutoff", 1234.0);
    a.set_param("gain", -7.5);
    let bytes = a.save_state().expect("chunk");
    assert_eq!(bytes.len(), 16);
    let mut b = CppBridge::default();
    assert!(b.load_state(&bytes));
    assert_eq!(b.param("cutoff"), Some(1234.0));
    assert_eq!(b.param("gain"), Some(-7.5));
    for bad in [&bytes[..15], &b"nope-not-a-chunk"[..], &[][..]] {
        assert!(!b.load_state(bad));
    }
    let mut nan = bytes.clone();
    nan[8..12].copy_from_slice(&f32::NAN.to_ne_bytes());
    assert!(!b.load_state(&nan));
    assert_eq!(b.param("cutoff"), Some(1234.0), "a refused chunk changes nothing");
}
