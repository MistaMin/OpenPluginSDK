//! OpenPluginSDK Target Bridges.
//!
//! Provides the target export wrappers:
//! - [`clap`]: CLAP factory that instantiates a [`PluginForExport`] and processes audio
//! - [`lv2`]: LV2 descriptor. Ports 0 and 1 in, 2 and 3 out, parameters on the shared plane
//! - [`vst3`]: Steinberg VST3 layouts. A host loads `native/vst3`, not these layouts
//! - [`au`]: Audio Unit layouts. A host loads `native/au`
//! - [`aax`]: AAX layouts. A host loads `native/aax`
//! - [`shell`]: C ABI shared by those three native adapters
//! - [`standalone`]: Standalone application runner

pub mod aax;
pub mod au;
pub mod clap;
pub mod clap_abi;
pub mod lv2;
pub mod shell;
pub mod standalone;
pub mod vst3;

pub use aax::AaxAdapter;
pub use au::AuAdapter;
pub use clap::ClapAdapter;
pub use lv2::Lv2Harness;
pub use openplugin_core::Plugin as PluginForExport;
pub use standalone::StandaloneRunner;
pub use vst3::Vst3Adapter;
