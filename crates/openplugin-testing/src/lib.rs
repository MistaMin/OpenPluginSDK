//! Conformance kit and benchmark harness for OpenPluginSDK plugins.
//!
//! Use it as a dev-dependency. In a test file of your plugin crate:
//!
//! ```ignore
//! openplugin_testing::install_allocator!();
//!
//! #[test]
//! fn conforms() {
//!     openplugin_testing::conformance(MyPlugin::default, Default::default()).assert_pass();
//! }
//! ```
//!
//! [`conformance`] runs your plugin through what hosts do and returns a
//! [`Report`]. [`bench::bench`] measures its CPU cost. `docs/TESTING_PLUGINS.md`
//! explains both and how to read the reports.
//!
//! Locks on the audio path cannot be detected reliably in user space, so that
//! check is always reported as skipped rather than faked.

#![deny(unsafe_code)]

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
/// notes and tails, and report every check.
pub fn conformance<P: Plugin>(make: impl Fn() -> P, options: Options) -> Report {
    conformance::run(make, options)
}
