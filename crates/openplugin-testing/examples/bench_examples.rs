//! Benchmark runner for the SDK's example plugins.
//!
//! ```text
//! cargo run --release -p openplugin-testing --example bench_examples -- gain
//! cargo run --release -p openplugin-testing --example bench_examples -- gain --out before.json
//! cargo run --release -p openplugin-testing --example bench_examples -- gain --baseline before.json
//! ```
//!
//! The first argument is `gain`, `editor`, `design` or `console_eq`; the rest are the flags of
//! [`openplugin_testing::bench::cli`]. Exit code 1 means a case is slower than the baseline.

#![allow(dead_code, unused_imports)]

use std::process::ExitCode;

#[path = "../../../examples/console_eq/src/main.rs"]
mod console_eq;
#[path = "../../../examples/gain/src/main.rs"]
mod gain;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let which = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();
    match which.as_str() {
        "gain" => openplugin_testing::bench::cli(gain::GainPlugin::default, rest),
        "editor" => openplugin_testing::bench::cli(example_editor::EditorGain::default, rest),
        "design" => openplugin_testing::bench::cli(example_design_editor::DesignDemo::default, rest),
        "console_eq" => openplugin_testing::bench::cli(console_eq::ConsoleEqPlugin::default, rest),
        _ => {
            eprintln!("usage: bench_examples gain|editor|design|console_eq [--json] [--out FILE] [--baseline FILE] [--tolerance 0.25] [--quick] [--filter TEXT] [--iterations N]");
            ExitCode::from(2)
        }
    }
}
