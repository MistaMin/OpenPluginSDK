//! Conformance kit and benchmark harness for OpenPluginSDK plugins.
//!
//! Use it as a dev-dependency. In a test file of your plugin crate:
//!
//! ```
//! # use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
//! # static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];
//! # static INFO: PluginInfo = PluginInfo::new("com.example.gain", "Gain", "Example", "1.0.0", Category::Utility, &PARAMS);
//! # #[derive(Default)]
//! # struct MyPlugin { db: f32 }
//! # impl Plugin for MyPlugin {
//! #     fn info(&self) -> &'static PluginInfo { &INFO }
//! #     fn prepare(&mut self, _: f32, _: usize, _: usize) {}
//! #     fn reset(&mut self) {}
//! #     fn set_param(&mut self, id: &str, v: f32) -> bool { if id != "gain" { return false; } self.db = PARAMS[0].clamp(v); true }
//! #     fn param(&self, id: &str) -> Option<f32> { (id == "gain").then_some(self.db) }
//! #     fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
//! #         let g = 10f32.powf(self.db / 20.0);
//! #         for c in io { for s in &mut c[..frames] { *s *= g; } }
//! #     }
//! # }
//! openplugin_testing::install_allocator!();
//!
//! // In `tests/conformance.rs` of your plugin crate, mark this `#[test]`.
//! fn conforms() {
//!     openplugin_testing::conformance(MyPlugin::default, Default::default()).assert_pass();
//! }
//! # // The full run above takes a while in a debug build; the doc test runs a short one.
//! # let options = openplugin_testing::Options { sample_rates: vec![48_000.0], block_sizes: vec![64], ..openplugin_testing::Options::quick() };
//! # openplugin_testing::conformance(MyPlugin::default, options).assert_pass();
//! ```
//!
//! [`conformance`] runs your plugin through what hosts do and returns a
//! [`Report`]. [`bench::bench`] measures its CPU cost. `docs/TESTING_PLUGINS.md`
//! explains both and how to read the reports.
//!
//! Locks on the audio path cannot be detected reliably in user space, so that
//! check is always reported as skipped rather than faked.

#![deny(unsafe_code)]
#![deny(missing_docs)]

pub mod alloc;
pub mod bench;
mod conformance;
mod json;
mod report;
pub mod rng;

pub use conformance::{Options, HOST_BLOCK_SIZES, HOST_SAMPLE_RATES, LATENCY_SETTLE_MS};
pub use report::{Check, CheckResult, Failure, Report, Status};

use openplugin_core::Plugin;

/// Run `make`'s plugin through sample rates, block sizes, hostile input,
/// parameter fuzz, reset, state, layouts, offline and transport changes,
/// notes and tails, and report every check. `make` builds a fresh plugin each time the suite
/// needs one.
///
/// Never panics on a plugin's panic: the panic is caught and reported as a
/// [`Check::NoPanic`] failure. Allocation is checked only when
/// [`install_allocator!`] is in the test binary; otherwise [`Check::NoAlloc`] is skipped.
/// A full run takes seconds in a release build; use [`Options::quick`] for debug builds.
///
/// ```
/// # use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
/// # static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];
/// # static INFO: PluginInfo = PluginInfo::new("com.example.gain", "Gain", "Example", "1.0.0", Category::Utility, &PARAMS);
/// # #[derive(Default)]
/// # struct MyPlugin { db: f32 }
/// # impl Plugin for MyPlugin {
/// #     fn info(&self) -> &'static PluginInfo { &INFO }
/// #     fn prepare(&mut self, _: f32, _: usize, _: usize) {}
/// #     fn reset(&mut self) {}
/// #     fn set_param(&mut self, id: &str, v: f32) -> bool { if id != "gain" { return false; } self.db = PARAMS[0].clamp(v); true }
/// #     fn param(&self, id: &str) -> Option<f32> { (id == "gain").then_some(self.db) }
/// #     fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
/// #         let g = 10f32.powf(self.db / 20.0);
/// #         for c in io { for s in &mut c[..frames] { *s *= g; } }
/// #     }
/// # }
/// openplugin_testing::install_allocator!();
///
/// use openplugin_testing::{conformance, Options};
///
/// let options = Options { sample_rates: vec![44_100.0], block_sizes: vec![1, 333], ..Options::quick() };
/// conformance(MyPlugin::default, options).assert_pass();
/// ```
pub fn conformance<P: Plugin>(make: impl Fn() -> P, options: Options) -> Report {
    conformance::run(make, options)
}
