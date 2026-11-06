//! # OpenPluginSDK
//!
//! A zero-allocation, panic-free digital audio plugin development framework with GoodLookin DRI
//! (Direct Rendering Interface), targeting **CLAP**, **VST3**, **Audio Unit (AU)**, and **Avid AAX**.
//!
//! ## Modules
//! - [`core`]: Plugin trait, parameter descriptors, normalization, and transport.
//! - [`dsp`]: Standard library of textbook, realtime-safe DSP filters and dynamics.
//! - [`dri`]: Procedural hardware GUI engine (GoodLookin DRI).
//! - [`targets`]: Host target bridges (CLAP, VST3, AU, AAX, Standalone).

pub use openplugin_core as core;
pub use openplugin_dsp as dsp;
pub use openplugin_dri as dri;
pub use openplugin_targets as targets;

pub mod prelude {
    pub use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Taper, Transport, Unit};
    pub use openplugin_dsp::{Adaa2Taylor, Biquad, Compressor, FilterType, OversampleFactor, Oversampler2x, ResidualAdaa1, SaturationMode, VuMeter, saturate_sample};
    pub use openplugin_dri::{Color, DrawList, FaceplateStyle, KnobStyle, MeterStyle, draw_faceplate, draw_fader, draw_knob, draw_pan_knob, draw_slider, draw_vu_meter, fader_db_to_pos, fader_pos_to_db};
    pub use openplugin_targets::{AaxAdapter, AuAdapter, ClapAdapter, StandaloneRunner, Vst3Adapter};
}
