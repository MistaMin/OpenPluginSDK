//! The SDK's own example plugins through the conformance kit.

#![allow(dead_code, unused_imports)]

use openplugin_testing::{conformance, Options};

openplugin_testing::install_allocator!();

// The example binaries have no library target, so their source is included.
#[path = "../../../examples/gain/src/main.rs"]
mod gain;
#[path = "../../../examples/console_eq/src/main.rs"]
mod console_eq;

fn options() -> Options {
    if cfg!(debug_assertions) { Options::quick() } else { Options::default() }
}

#[test]
fn example_gain_conforms() {
    let report = conformance(gain::GainPlugin::default, options());
    println!("{report}");
    report.assert_pass();
}

#[test]
fn example_editor_conforms() {
    let report = conformance(example_editor::EditorGain::default, options());
    println!("{report}");
    report.assert_pass();
}

#[test]
fn example_design_editor_conforms() {
    let report = conformance(example_design_editor::DesignDemo::default, options());
    println!("{report}");
    report.assert_pass();
}

/// A real finding: `openplugin_dsp::Biquad` keeps a NaN in its state for good, so one NaN at
/// the input silences the EQ until the host resets it. Everything else passes. When the filter
/// is fixed this test fails; change it to `assert_pass`.
#[test]
fn example_console_eq_conforms_except_nan_recovery() {
    let report = conformance(console_eq::ConsoleEqPlugin::default, options());
    println!("{report}");
    assert_eq!(report.failed_checks(), vec![openplugin_testing::Check::HostileInput], "{report}");
}
