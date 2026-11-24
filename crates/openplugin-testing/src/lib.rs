//! Conformance kit and benchmark harness for OpenPluginSDK plugins.
//!
//! Use it as a dev-dependency. In a test file of your plugin crate:
//!
//! ```
//! # use example_editor::EditorGain as MyPlugin;
//! openplugin_testing::install_allocator!();
//!
//! #[test]
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

pub use conformance::{Options, HOST_BLOCK_SIZES, HOST_SAMPLE_RATES};
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
/// openplugin_testing::install_allocator!();
///
/// use openplugin_testing::{conformance, Options};
///
/// let options = Options { sample_rates: vec![44_100.0], block_sizes: vec![1, 333], ..Options::quick() };
/// conformance(example_editor::EditorGain::default, options).assert_pass();
/// ```
pub fn conformance<P: Plugin>(make: impl Fn() -> P, options: Options) -> Report {
    conformance::run(make, options)
}
