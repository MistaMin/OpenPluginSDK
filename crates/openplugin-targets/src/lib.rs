//! OpenPluginSDK Target Bridges.
//!
//! Provides the target export wrappers:
//! - [`clap`]: CLAP factory that instantiates a [`PluginForExport`] and processes audio
//! - [`lv2`]: LV2 descriptor. Ports 0 and 1 in, 2 and 3 out, parameters on the shared plane
//! - [`vst3`]: Steinberg VST3 layouts. The working shell is OpenGridder's
//! - [`au`]: Audio Unit layouts. The working shell is OpenGridder's
//! - [`aax`]: AAX layouts. The working shell is OpenGridder's
//! - [`standalone`]: Standalone application runner

pub mod aax;
pub mod au;
pub mod clap;
pub mod clap_abi;
pub mod lv2;
pub mod standalone;
pub mod vst3;

pub use aax::AaxAdapter;
pub use au::AuAdapter;
pub use clap::ClapAdapter;
pub use lv2::Lv2Harness;
pub use openplugin_core::Plugin as PluginForExport;
pub use standalone::StandaloneRunner;
pub use vst3::Vst3Adapter;
