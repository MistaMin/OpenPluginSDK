//! # OpenPluginSDK
//!
//! A zero-allocation, panic-free digital audio plugin development framework with GoodLookin DRI
//! (Direct Rendering Interface), targeting **CLAP**, **VST3**, **Audio Unit (AU)**, and **Avid AAX**.
//!
//! ## Modules
//! - [`core`]: Plugin trait, parameter descriptors, normalization, and transport.
//! - [`dsp`]: Standard library of textbook, realtime-safe DSP filters and dynamics.
//! - [`dri`]: Procedural hardware GUI engine (GoodLookin DRI).
//! - [`targets`]: Host target bridges (CLAP, VST3, AU, AAX, LV2, Standalone).
//! - [`ffi`]: Rust host session. A host writes the parameter plane and renders through scratch.

pub use openplugin_core as core;
pub use openplugin_dri as dri;
pub use openplugin_dsp as dsp;
pub use openplugin_ffi as ffi;
pub use openplugin_targets as targets;

pub mod prelude {
    pub use openplugin_core::{
        decode_params, encode_params, restore_params, AbCompare, Category, MeterFrame, OnePole,
        ParamCursor, ParamInfo, ParamPlane, Plugin, PluginInfo, SampleRamp, Seqlock, Taper,
        Transport, Unit,
    };
    pub use openplugin_dri::{
        draw_faceplate, draw_faceplate_code, draw_fader, draw_knob, draw_knob_code, draw_pan_knob,
        draw_slider, draw_vu_meter, fader_db_to_pos, fader_pos_to_db, Color, DrawList,
        FaceplateStyle, KnobStyle, MeterStyle,
    };
    pub use openplugin_dsp::{
        f32_to_i16, f32_to_i32, hann, hz_from_midi, i16_to_f32, i32_to_f32, lerp, midi_from_hz,
        ms_from_samples, ms_per_beat, plan_rate, samples_from_ms, samples_from_ticks,
        samples_per_beat, saturate_sample, Adaa2Taylor, Biquad, Compressor, DelayLine, FilterType,
        GainComp, LinearPhaseOversampler, MeterBank, MeterReadings, Oscillator, OversampleFactor,
        Oversampler2x, Polyphony, ResidualAdaa1, SampleVoice, SaturationMode, VuMeter, Wave,
        WindowRms,
    };
    pub use openplugin_targets::{
        AaxAdapter, AuAdapter, ClapAdapter, Lv2Harness, StandaloneRunner, Vst3Adapter,
    };
}
