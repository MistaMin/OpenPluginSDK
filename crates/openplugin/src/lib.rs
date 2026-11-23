//! # OpenPluginSDK
//!
//! A zero-allocation, panic-free digital audio plugin development framework with GoodLookin DRI
//! (Design Rust Interface), targeting **CLAP**, **LV2**, **VST3**, **Audio Unit (AU)**, and **Avid AAX**.
//!
//! ## Modules
//! - [`core`]: Plugin trait, parameter descriptors, normalization, and transport.
//! - [`dsp`]: Standard library of textbook, realtime-safe DSP filters and dynamics.
//! - [`dri`]: Procedural hardware GUI engine (GoodLookin DRI).
//! - [`targets`]: The format entries: `export_clap!`, `export_lv2!` and `export_shell!`.
//! - [`host`]: Rust host session. A host writes the parameter plane and renders through scratch.
//! - [`prelude`]: what nearly every plugin imports.

pub use openplugin_core as core;
pub use openplugin_dri as dri;
pub use openplugin_dsp as dsp;
pub use openplugin_host as host;
pub use openplugin_targets as targets;
#[cfg(feature = "testing")]
pub use openplugin_testing as testing;

/// The names nearly every plugin uses: the plugin and editor contracts, the descriptors,
/// the channel adapters its tests call `process` through, and the export macros. DSP blocks and drawing come from [`crate::dsp`] and [`crate::dri`].
pub mod prelude {
    pub use openplugin_core::{
        Category, Channels, ChannelsMut, Editor, EditorFactory, EditorHost, EditorState, Frame, HostSpace, IoConfig,
        MeterFrame, NotePorts, ParamFlags, ParamInfo, Plugin, PluginInfo, SpeakerLayout, Skew, Tail, Taper,
        Transport, Unit,
    };
    pub use openplugin_targets::{export_clap, export_lv2, export_shell};
}
