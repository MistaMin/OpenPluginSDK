//! OpenPluginSDK Target Bridges.
//!
//! Provides the target export wrappers:
//! - [`clap`]: CLever Audio Plugin format
//! - [`vst3`]: Steinberg VST3 format
//! - [`au`]: Apple Audio Unit format
//! - [`aax`]: Avid Audio eXtension format (Pro Tools)
//! - [`standalone`]: Standalone application runner

pub mod aax;
pub mod au;
pub mod clap;
pub mod standalone;
pub mod vst3;

pub use aax::AaxAdapter;
pub use au::AuAdapter;
pub use clap::ClapAdapter;
pub use standalone::StandaloneRunner;
pub use vst3::Vst3Adapter;
