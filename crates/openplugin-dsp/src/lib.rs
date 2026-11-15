//! OpenPluginSDK DSP Library.
//!
//! Realtime-safe DSP building blocks:
//! - [`Biquad`]: Robert Bristow-Johnson cookbook audio filters.
//! - [`Svf`], [`MatchedBiquad`], [`Butterworth`], [`Crossover`] (Linkwitz-Riley), [`OnePoleFilter`], [`DcBlocker`], [`Allpass1`], [`Allpass2`]: TPT, analog-matched and classic filters (see `docs/DSP_COOKBOOK.md`).
//! - [`Compressor`]: Log-domain dynamics processor with soft knee.
//! - [`Dynamics`], [`GainComputer`], [`EnvelopeFollower`], [`Ballistics`]: linked compressor / expander / gate with soft knee, hysteresis, range, hold, a sidechain hook (Giannoulis, Massberg, Reiss 2012).
//! - [`Limiter`], [`TruePeak`]: lookahead brickwall limiter with a true-peak ceiling (ITU-R BS.1770-4 Annex 2) held for full-band material too, and a reported latency; a 4x/8x true-peak detector.
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
//! - [`SurroundAdapter`], [`LinkedDetector`], [`LfeMode`]: a stereo plugin on surround buses, one engine per channel pair, every parameter linked, an optional shared detector, and a fold to stereo.
//! - [`Dither`]: RPDF/TPDF dither to 8..=24 bits with noise shaping and auto-blank.
//! - [`neural::Model`]: small Dense/GRU/LSTM/Conv1d inference for amp and pedal models, loaded from JSON.
//! - [`SpectralRepair`]: spectral gate and noise reduction from a learned noise profile (spectral subtraction, Wiener, MMSE-STSA).
//! - [`Stft`], [`SpectrumAnalyzer`], [`WindowKind`]: STFT frames with overlap-add, windows with ENBW, and a lock-free spectrum feed to editors.
//! - [`ModDelay`], [`Interpolation`]: fractional, per-sample modulated delay (Hermite, Lagrange, Thiran allpass).
//! - [`Lfo`]: sine, triangle, saw, square, sample-and-hold and smoothed random, free-running or locked to the host [`openplugin_core::Transport`].
//! - [`Chorus`], [`Flanger`], [`Phaser`]: modulation effect building blocks.
//! - [`utility`]: mid/side, width, pan laws, balance, polarity, smoothed dB gain, [`DenormalGuard`].
//! - [`TruePeakMeter`], [`Ppm`] (EBU, DIN, Nordic, BBC), [`KMeter`], [`VuIec`], [`EbuMeter`], and the lock-free [`scope_feed`] for correlation and goniometer.
//! - [`FdnReverb`], [`PlateReverb`], [`EarlyReflections`]: Jot-Chaigne feedback delay network, Dattorro plate, and tapped-delay early reflections.
//! - [`design_kaiser`], [`design_least_squares`], [`linear_phase_eq`], [`minimum_phase_eq`], [`MinPhaseHalfband`]: FIR design off the audio thread (windowed sinc with Kaiser's formulas, least squares, linear and minimum phase from a magnitude curve, minimum-phase half-band oversampling).
//! - [`TransistorLadder`], [`DiodeLadder`], [`Korg35`], the [`wdf`] building blocks with [`DiodeClipper`], and [`TapeHysteresis`]: virtual analog (zero-delay-feedback filters, wave digital filters, Jiles-Atherton tape).
//! - [`saturate_sample`]: Analog tape/tube non-linear saturation models.
//! - [`PluckedString`], [`WaveguideString`], [`WaveguideBore`], [`ModalBank`]: physical models (Karplus-Strong with the Jaffe-Smith extensions, digital waveguides, modal resonators with bar, plate and membrane presets).
//! - [`TimeStretch`], [`PitchShifter`] (phase vocoder with identity phase locking and onset phase reset), [`DelayPitchShifter`] (low-latency modulated-delay taps), [`YinDetector`], [`MpmDetector`] (pitch detection with confidence).
//! - [`BandlimitedOsc`], [`WavetableOsc`], [`Adsr`], [`PinkNoise`], [`VoiceAllocator`], [`SincKernel`]: synthesis (PolyBLEP/BLAMP oscillators with hard sync, mip-mapped wavetables, envelopes, noise, voice stealing and glide, a windowed-sinc resampler).

pub mod adaa;
pub mod adsr;
pub mod biquad;
pub mod blosc;
pub mod bypass;
pub mod chorus;
pub mod convolve;
pub mod clipper;
pub mod crossover;
pub mod deesser;
pub mod dynamic_eq;
pub mod multiband;
pub mod transient;
pub mod delay_pitch;
pub mod delta;
pub mod downmix;
pub mod dither;
pub mod delay;
pub mod dynamics;
pub mod envelope;
pub mod limiter;
pub mod filters;
pub mod fir_design;
pub mod fir_phase;
pub mod flanger;
pub mod halfband_min;
pub mod interp;
mod intersample;
pub mod early;
pub mod fdn;
pub mod gain_comp;
pub mod gain_computer;
pub mod korg35;
pub mod ladder;
pub mod lfo;
pub mod linear_phase;
pub mod kmeter;
pub mod loudness;
pub mod matched;
pub mod meter;
pub mod moddelay;
pub mod neural;
pub mod musical;
pub mod noise;
pub mod oscillator;
pub mod oversampling;
pub mod partitioned;
pub mod phaser;
pub mod pcm;
pub mod pitch;
pub mod modal;
mod phys_common;
pub mod pluck;
pub mod waveguide;
pub mod pitch_detect;
pub mod ppm;
pub mod true_peak;
pub mod rate;
pub mod scope;
pub mod plate;
mod reverb_common;
pub mod resample;
pub mod saturation;
pub mod surround_adapter;
pub mod simd;
pub mod svf;
pub mod tape;
pub mod utility;
pub mod voice;
pub mod true_peak_meter;
pub mod spectral_repair;
pub mod spectrum;
pub mod stft;
pub mod vocoder;
pub mod voice_alloc;
pub mod wavetable;
pub mod wdf;
pub mod window;
pub mod windows;

pub use adaa::{Adaa2Taylor, ResidualAdaa1};
pub use adsr::{Adsr, Ahdsr, Curve, Gate, RetriggerMode, Stage};
pub use biquad::{Biquad, FilterType};
pub use blosc::{blamp_residual, blep_residual, BandlimitedOsc, BlWave, BlepOrder};
pub use bypass::SoftBypass;
pub use chorus::{Chorus, MAX_CHORUS_MS, MAX_CHORUS_VOICES};
pub use convolve::BlockConvolver;
pub use clipper::TruePeakClipper;
pub use deesser::{DeEssDetect, DeEssMode, DeEsser};
pub use dynamic_eq::{DynEqKind, DynEqMode, DynamicEqBand, DynamicEqParams};
pub use multiband::MultibandCompressor;
pub use transient::TransientShaper;
pub use crossover::{Crossover, LinkwitzRiley, LrOrder, MAX_BANDS};
pub use delay_pitch::{DelayPitchShifter, MAX_DELAY_PITCH_TAPS};
pub use delta::Delta;
pub use delay::DelayLine;
pub use downmix::{
    roles_of, Downmix, DownmixTarget, MixSettings, Normalize, SurroundFill, Upmix, UpmixSettings,
};
pub use dynamics::Compressor;
pub use surround_adapter::{LfeMode, LinkedDetector, StereoOnSurround, SurroundAdapter};
pub use envelope::{Ballistics, Detection, Domain, EnvelopeFollower, Topology};
pub use gain_computer::{Dynamics, DynamicsMode, DynamicsParams, GainComputer, Link, Smoothing};
pub use limiter::Limiter;
pub use true_peak::TruePeak;
pub use filters::{
    Allpass1, Allpass2, Butterworth, ButterworthKind, DcBlocker, OnePoleFilter, OnePoleMode,
    MAX_BUTTERWORTH_ORDER,
};
pub use fir_design::{
    design_kaiser, design_least_squares, design_windowed, fir_group_delay, fir_magnitude_db,
    fir_response, kaiser_attenuation_db, kaiser_beta, kaiser_taps, ls_grid, FirError, FirKind,
    LsBand, LsFit, LsPoint,
};
pub use fir_phase::{
    linear_phase_eq, linear_phase_from_magnitude, linear_phase_latency, minimum_phase_eq,
    minimum_phase_from_linear, minimum_phase_from_magnitude, mixed_phase_eq,
    mixed_phase_from_magnitude, partial_energy, sample_curve, EqPoint,
};
pub use halfband_min::MinPhaseHalfband;
pub use flanger::{Flanger, MAX_FLANGER_FEEDBACK, MAX_FLANGER_MS};
pub use early::{EarlyPattern, EarlyReflections, EarlyTap, MAX_EARLY_TAPS};
pub use fdn::{FdnMatrix, FdnReverb};
pub use gain_comp::GainComp;
pub use interp::{group_delay, phase_delay, Interpolation};
pub use korg35::{Korg35, Korg35Mode};
pub use ladder::{DiodeLadder, TransistorLadder, DIODE_K_CRIT};
pub use lfo::{Feel, Lfo, LfoRate, LfoShape, NoteDivision};
pub use linear_phase::{LinearPhaseOversampler, OversampleError};
pub use matched::{MatchedBiquad, MatchedKind};
pub use moddelay::{Glide, ModDelay};
pub use kmeter::{KMeter, KScale, KZone};
pub use loudness::{bs1770_weight, EbuMeter, EbuScale, KWeighting, Loudness, LoudnessMatch, Target};
pub use ppm::{Ppm, PpmType};
pub use scope::{scope_feed, ScopeFrame, ScopeReader, ScopeWriter, SCOPE_POINTS};
pub use true_peak_meter::TruePeakMeter;
pub use utility::{
    balance_gains, db_to_gain, gain_to_db, ms_decode, ms_encode, polarity, stereo_width, DenormalGuard, Gain,
    PanLaw, Panner,
};
pub use meter::{correlation, BlockLevel, MeterBank, MeterReadings, VuIec, VuMeter};
pub use musical::{
    ms_from_samples, ms_per_beat, samples_from_ms, samples_from_ticks, samples_per_beat,
};
pub use noise::{PinkNoise, WhiteNoise, PINK_LOW_HZ, PINK_RMS};
pub use oscillator::{Oscillator, Wave};
pub use oversampling::{OversampleFactor, Oversampler2x};
pub use dither::{Dither, DitherKind, NoiseShaping};
pub use phaser::{Phaser, MAX_PHASER_FEEDBACK, MAX_PHASER_STAGES};
pub use partitioned::{ConvolutionIr, ConvolveError, ConvolverConfig, Layout, LongConvolver};
pub use pcm::{f32_to_i16, f32_to_i24, f32_to_i32, i16_to_f32, i24_to_f32, i32_to_f32, lerp};
pub use pitch::{hz_from_midi, midi_from_hz};
pub use modal::{
    mode_ratios, Exciter, ModalBank, ModalPreset, PresetSettings, Strike, MAX_PRESET_MODES,
    MEMBRANE_ZEROS,
};
pub use phys_common::{stiff_partial_hz, MAX_DISPERSION_SECTIONS};
pub use pluck::{PluckedString, Tuning};
pub use waveguide::{BoreEnd, WaveguideBore, WaveguideString};
pub use pitch_detect::{MpmDetector, Pitch, PitchDetector, YinDetector};
pub use plate::PlateReverb;
pub use rate::{effective_rate, is_studio_rate, plan_rate, RateError, RatePlan, STUDIO_RATES_HZ};
pub use resample::{playback_ratio, Quality, SincConfig, SincKernel, SincPlayer};
pub use saturation::{saturate_sample, SaturationMode};
pub use svf::{Svf, SvfMode};
pub use tape::TapeHysteresis;
pub use wdf::{
    wright_omega, Capacitor, DiodeClipper, DiodePair, DiodeParams, Inductor, Inverter, Parallel,
    Resistor, ResistiveVoltageSource, Series, SingleDiode, WdfPort,
};
pub use vocoder::{PhaseLocking, PitchShifter, TimeStretch, VocoderConfig};
pub use voice::{Polyphony, SampleVoice};
pub use voice_alloc::{
    Allocation, GlideMode, NoteOff, StealPolicy, Voice, VoiceAllocator, VoiceMode, VoicePhase,
};
pub use wavetable::{WavetableError, WavetableOsc, WavetableSet};
pub use window::{hann, WindowRms};
pub use spectrum::{
    bin_frequency, log_frequency_bins, spectrum_feed, SpectrumAnalyzer, SpectrumConfig,
    SpectrumReader, SpectrumWriter,
};
pub use spectral_repair::{
    GainRule, ProfileError, RepairConfig, RepairMode, RepairParams, SpectralRepair, MAX_AMOUNT_DB,
    MAX_SMOOTH_BINS,
};
pub use stft::{
    check_ola, check_ola_windows, Complex, Identity, OlaReport, SpectralProcessor, Stft,
    StftConfig, StftError,
};
pub use windows::{bessel_i0, coherent_gain, enbw_bins, WindowKind};
