//! OpenPluginSDK DSP Library.
//!
//! Realtime-safe DSP building blocks:
//! - [`Biquad`]: Robert Bristow-Johnson cookbook audio filters.
//! - [`Compressor`]: Log-domain dynamics processor with soft knee.
//! - [`VuMeter`]: ANSI standard 300 ms ballistic level detector.
//! - [`saturate_sample`]: Analog tape/tube non-linear saturation models.

pub mod biquad;
pub mod dynamics;
pub mod meter;
pub mod saturation;

pub use biquad::{Biquad, FilterType};
pub use dynamics::Compressor;
pub use meter::VuMeter;
pub use saturation::{SaturationMode, saturate_sample};
