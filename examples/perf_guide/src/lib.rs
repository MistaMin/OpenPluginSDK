//! Measured examples for `docs/PERFORMANCE.md` and `docs/STABILITY.md`.
//!
//! Every optimisation in the performance guide is a pair of small kernels here, a "before" and an
//! "after", wrapped as plugins so the SDK's own bench harness
//! ([`openplugin_testing::bench`]) times them the way it times a real plugin. The `perf_guide`
//! binary runs every pair and prints the table that the guide quotes:
//!
//! ```text
//! cargo run --release -p example-perf-guide
//! ```
//!
//! The stability numbers come from [`stability`], printed by the same binary with `stability`.
//! The tests in `tests/` check that each "after" computes the same thing as its "before" (or
//! within the stated error), and run the conformance kit on the kernels that make a claim about it.

#![allow(clippy::needless_range_loop)]

use openplugin_core::{Category, Plugin, PluginInfo};

pub mod approx;
pub mod kernels;
pub mod stability;
pub mod stable_filter;

/// A piece of audio work with no parameters. [`Rig`] turns it into a [`Plugin`].
pub trait Kernel: Send + 'static {
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize);
    fn reset(&mut self) {}
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize);
}

impl Kernel for Box<dyn Kernel> {
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        (**self).prepare(sample_rate, max_block, channels);
    }
    fn reset(&mut self) {
        (**self).reset();
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        (**self).process(io, frames);
    }
}

static INFO: PluginInfo = PluginInfo::new(
    "com.opengrid.example.perf-guide",
    "Perf guide kernel",
    "OpenGrid",
    "1.0.0-beta",
    Category::Utility,
    &[],
)
.with_url("https://github.com/MistaMin/OpenPluginSDK")
.with_email("contact@mistamin.com");

/// Wraps a [`Kernel`] as a parameterless [`Plugin`], so the bench harness and the conformance kit
/// accept it.
pub struct Rig<K: Kernel>(pub K);

impl<K: Kernel> Plugin for Rig<K> {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.0.prepare(sample_rate, max_block, channels);
    }
    fn reset(&mut self) {
        self.0.reset();
    }
    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        false
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        self.0.process(io, frames);
    }
}
