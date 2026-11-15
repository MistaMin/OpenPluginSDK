//! OpenPluginSDK DSP Library.
//!
//! Realtime-safe DSP building blocks:
//! - [`Biquad`]: Robert Bristow-Johnson cookbook audio filters.
//! - [`Svf`], [`MatchedBiquad`], [`Butterworth`], [`Crossover`] (Linkwitz-Riley), [`OnePoleFilter`], [`DcBlocker`], [`Allpass1`], [`Allpass2`]: TPT, analog-matched and classic filters (see `docs/DSP_COOKBOOK.md`).
//! - [`Compressor`]: Log-domain dynamics processor with soft knee.
//! - [`Dynamics`], [`GainComputer`], [`EnvelopeFollower`], [`Ballistics`]: linked compressor / expander / gate with soft knee, hysteresis, range, hold, a sidechain hook (Giannoulis, Massberg, Reiss 2012).
//! - [`Limiter`], [`TruePeak`]: lookahead brickwall limiter with a 4x/8x true-peak ceiling (ITU-R BS.1770-4 Annex 2) and a reported latency.
//! - [`VuMeter`]: ANSI standard 300 ms ballistic level detector.
//! - [`MeterBank`]: stereo peak, RMS, PPM, and VU, plus envelope gain reduction.
//! - [`LinearPhaseOversampler`]: 1x/2x/4x linear-phase resampling, 64 host samples at 2x and 4x.
//! - [`BlockConvolver`]: overlap-save playback of a real impulse of at most 1024 taps.
//! - [`LongConvolver`]: partitioned convolution of impulses of seconds, zero latency, mono, stereo and true stereo ([`ConvolverConfig`], [`ConvolutionIr`]).
//! - [`GainComp`]: slow stereo RMS loudness match. The default cap is ±12 dB. `with_limit` opens it to ±60 dB.
//! - [`SoftBypass`]: dry path delayed by the reported latency, with a short crossfade.
//! - [`Delta`]: what the processing adds or removes (processed minus the aligned dry), with a glide.
//! - [`Loudness`]: BS.1770-4 / EBU R 128 momentary, short-term and gated integrated LUFS ([`KWeighting`]).
//! - [`LoudnessMatch`]: automatic gain that holds the output at the input's K-weighted loudness.
//! - [`Downmix`], [`Upmix`]: surround to stereo or mono by channel role (BS.775 / A/52), and the passive stereo upmix.
//! - [`Dither`]: RPDF/TPDF dither to 8..=24 bits with noise shaping and auto-blank.
//! - [`saturate_sample`]: Analog tape/tube non-linear saturation models.
//! - [`BandlimitedOsc`], [`WavetableOsc`], [`Adsr`], [`PinkNoise`], [`VoiceAllocator`], [`SincKernel`]: synthesis (PolyBLEP/BLAMP oscillators with hard sync, mip-mapped wavetables, envelopes, noise, voice stealing and glide, a windowed-sinc resampler).

pub mod adaa;
pub mod adsr;
pub mod biquad;
pub mod blosc;
pub mod bypass;
pub mod convolve;
pub mod crossover;
pub mod delta;
pub mod downmix;
pub mod dither;
pub mod delay;
pub mod dynamics;
pub mod envelope;
pub mod limiter;
pub mod filters;
pub mod gain_comp;
pub mod gain_computer;
pub mod linear_phase;
pub mod loudness;
pub mod matched;
pub mod meter;
pub mod musical;
pub mod noise;
pub mod oscillator;
pub mod oversampling;
pub mod partitioned;
pub mod pcm;
pub mod pitch;
pub mod rate;
pub mod resample;
pub mod saturation;
pub mod svf;
pub mod voice;
pub mod voice_alloc;
pub mod true_peak;
pub mod wavetable;
pub mod window;

pub use adaa::{Adaa2Taylor, ResidualAdaa1};
pub use adsr::{Adsr, Ahdsr, Curve, Gate, RetriggerMode, Stage};
pub use biquad::{Biquad, FilterType};
pub use blosc::{blamp_residual, blep_residual, BandlimitedOsc, BlWave, BlepOrder};
pub use bypass::SoftBypass;
pub use convolve::BlockConvolver;
pub use crossover::{Crossover, LinkwitzRiley, LrOrder, MAX_BANDS};
pub use delta::Delta;
pub use delay::DelayLine;
pub use downmix::{
    roles_of, Downmix, DownmixTarget, MixSettings, Normalize, SurroundFill, Upmix, UpmixSettings,
};
pub use dynamics::Compressor;
pub use envelope::{Ballistics, Detection, Domain, EnvelopeFollower, Topology};
pub use gain_computer::{Dynamics, DynamicsMode, DynamicsParams, GainComputer, Link, Smoothing};
pub use limiter::Limiter;
pub use true_peak::TruePeak;
pub use filters::{
    Allpass1, Allpass2, Butterworth, ButterworthKind, DcBlocker, OnePoleFilter, OnePoleMode,
    MAX_BUTTERWORTH_ORDER,
};
pub use gain_comp::GainComp;
pub use linear_phase::{LinearPhaseOversampler, OversampleError};
pub use loudness::{KWeighting, Loudness, LoudnessMatch};
pub use matched::{MatchedBiquad, MatchedKind};
pub use meter::{correlation, BlockLevel, MeterBank, MeterReadings, VuMeter};
pub use musical::{
    ms_from_samples, ms_per_beat, samples_from_ms, samples_from_ticks, samples_per_beat,
};
pub use noise::{PinkNoise, WhiteNoise, PINK_LOW_HZ, PINK_RMS};
pub use oscillator::{Oscillator, Wave};
pub use oversampling::{OversampleFactor, Oversampler2x};
pub use dither::{Dither, DitherKind, NoiseShaping};
pub use partitioned::{ConvolutionIr, ConvolveError, ConvolverConfig, Layout, LongConvolver};
pub use pcm::{f32_to_i16, f32_to_i24, f32_to_i32, i16_to_f32, i24_to_f32, i32_to_f32, lerp};
pub use pitch::{hz_from_midi, midi_from_hz};
pub use rate::{effective_rate, is_studio_rate, plan_rate, RateError, RatePlan, STUDIO_RATES_HZ};
pub use resample::{playback_ratio, Quality, SincConfig, SincKernel, SincPlayer};
pub use saturation::{saturate_sample, SaturationMode};
pub use svf::{Svf, SvfMode};
pub use voice::{Polyphony, SampleVoice};
pub use voice_alloc::{
    Allocation, GlideMode, NoteOff, StealPolicy, Voice, VoiceAllocator, VoiceMode, VoicePhase,
};
pub use wavetable::{WavetableError, WavetableOsc, WavetableSet};
pub use window::{hann, WindowRms};
