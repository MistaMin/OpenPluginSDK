//! OpenPluginSDK DSP Library.
//!
//! Realtime-safe DSP building blocks:
//! - [`Biquad`]: Robert Bristow-Johnson cookbook audio filters.
//! - [`Compressor`]: Log-domain dynamics processor with soft knee.
//! - [`VuMeter`]: ANSI standard 300 ms ballistic level detector.
//! - [`MeterBank`]: stereo peak, RMS, PPM, and VU, plus envelope gain reduction.
//! - [`LinearPhaseOversampler`]: 1x/2x/4x linear-phase resampling, 64 host samples at 2x and 4x.
//! - [`BlockConvolver`]: overlap-save playback of a real impulse of at most 1024 taps.
//! - [`GainComp`]: slow stereo RMS loudness match. The default cap is ±12 dB. `with_limit` opens it to ±60 dB.
//! - [`SoftBypass`]: dry path delayed by the reported latency, with a short crossfade.
//! - [`Delta`]: what the processing adds or removes (processed minus the aligned dry), with a glide.
//! - [`Loudness`]: BS.1770-4 / EBU R 128 momentary, short-term and gated integrated LUFS ([`KWeighting`]).
//! - [`LoudnessMatch`]: automatic gain that holds the output at the input's K-weighted loudness.
//! - [`saturate_sample`]: Analog tape/tube non-linear saturation models.

pub mod adaa;
pub mod biquad;
pub mod bypass;
pub mod convolve;
pub mod delta;
pub mod delay;
pub mod dynamics;
pub mod gain_comp;
pub mod linear_phase;
pub mod loudness;
pub mod meter;
pub mod musical;
pub mod oscillator;
pub mod oversampling;
pub mod pcm;
pub mod pitch;
pub mod rate;
pub mod saturation;
pub mod voice;
pub mod window;

pub use adaa::{Adaa2Taylor, ResidualAdaa1};
pub use biquad::{Biquad, FilterType};
pub use bypass::SoftBypass;
pub use convolve::BlockConvolver;
pub use delta::Delta;
pub use delay::DelayLine;
pub use dynamics::Compressor;
pub use gain_comp::GainComp;
pub use linear_phase::{LinearPhaseOversampler, OversampleError};
pub use loudness::{KWeighting, Loudness, LoudnessMatch};
pub use meter::{correlation, BlockLevel, MeterBank, MeterReadings, VuMeter};
pub use musical::{
    ms_from_samples, ms_per_beat, samples_from_ms, samples_from_ticks, samples_per_beat,
};
pub use oscillator::{Oscillator, Wave};
pub use oversampling::{OversampleFactor, Oversampler2x};
pub use pcm::{f32_to_i16, f32_to_i32, i16_to_f32, i32_to_f32, lerp};
pub use pitch::{hz_from_midi, midi_from_hz};
pub use rate::{is_studio_rate, plan_rate, RateError, RatePlan, STUDIO_RATES_HZ};
pub use saturation::{saturate_sample, SaturationMode};
pub use voice::{Polyphony, SampleVoice};
pub use window::{hann, WindowRms};
