//! OpenPluginSDK Core Interfaces.
//!
//! Provides the core realtime-safe `Plugin` trait, metadata descriptors (`PluginInfo`, `ParamInfo`),
//! units, tapers, transport, and MIDI handling.
//!
//! # Guarantees
//! - Zero allocation in the audio processing thread (`process()`).
//! - No panics: hostile parameter values and malformed transport packets are sanitized.
//! - Parameter normalization and formatting with linear and logarithmic tapers.

#![deny(unsafe_code)]

/// Plugin category for host menus and classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Category {
    Eq,
    Dynamics,
    PitchShift,
    Reverb,
    Delay,
    Modulation,
    Harmonic,
    Dither,
    Instrument,
    Utility,
    Other,
}

/// Display unit of a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Unit {
    None,
    Db,
    Hz,
    Ms,
    Percent,
    Ratio,
    Semitones,
    Cents,
    Seconds,
    Toggle,
    Choice,
}

/// Taper describing how normalized 0..1 travel maps onto parameter values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Taper {
    Linear,
    Log,
}

/// Static description of a single plugin parameter.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ParamInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub unit: Unit,
    pub taper: Taper,
    /// Choice labels for `Unit::Choice` (value is integer index).
    pub choices: &'static [&'static str],
}

impl ParamInfo {
    pub const fn new(id: &'static str, name: &'static str, min: f32, max: f32, default: f32, unit: Unit) -> Self {
        Self {
            id,
            name,
            min,
            max,
            default,
            unit,
            taper: Taper::Linear,
            choices: &[],
        }
    }

    pub const fn with_taper(mut self, taper: Taper) -> Self {
        self.taper = taper;
        self
    }

    pub const fn with_choices(mut self, choices: &'static [&'static str]) -> Self {
        self.choices = choices;
        self.unit = Unit::Choice;
        self
    }

    /// Clamps value safely into range; NaN falls back to default.
    pub fn clamp(&self, value: f32) -> f32 {
        let v = if value.is_nan() { self.default } else { value };
        let v = v.clamp(self.min, self.max);
        match self.unit {
            Unit::Toggle | Unit::Choice => v.round().clamp(self.min, self.max),
            _ => v,
        }
    }

    /// Maps a raw value to normalized 0..1 travel honoring the taper.
    pub fn to_normalized(&self, value: f32) -> f32 {
        let v = self.clamp(value);
        let span = self.max - self.min;
        if span <= 0.0 {
            return 0.0;
        }
        match self.taper {
            Taper::Log if self.min > 0.0 => ((v / self.min).ln() / (self.max / self.min).ln()).clamp(0.0, 1.0),
            _ => ((v - self.min) / span).clamp(0.0, 1.0),
        }
    }

    /// Maps normalized 0..1 travel back to raw parameter value honoring the taper.
    pub fn from_normalized(&self, t: f32) -> f32 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        let v = match self.taper {
            Taper::Log if self.min > 0.0 => self.min * (self.max / self.min).powf(t),
            _ => self.min + (self.max - self.min) * t,
        };
        self.clamp(v)
    }

    /// Formats a parameter value for display with appropriate units.
    pub fn format(&self, value: f32) -> String {
        let v = self.clamp(value);
        match self.unit {
            Unit::Choice => self
                .choices
                .get(v as usize)
                .map(|s| (*s).to_string())
                .unwrap_or_else(|| format!("{v}")),
            Unit::Toggle => (if v >= 0.5 { "On" } else { "Off" }).to_string(),
            Unit::Db => {
                if v <= -95.9 {
                    "-inf dB".to_string()
                } else {
                    format!("{v:.1} dB")
                }
            }
            Unit::Hz => {
                if v >= 1000.0 {
                    format!("{:.2} kHz", v / 1000.0)
                } else {
                    format!("{v:.1} Hz")
                }
            }
            Unit::Ms => format!("{v:.1} ms"),
            Unit::Percent => format!("{v:.0}%"),
            Unit::Ratio => format!("{v:.1}:1"),
            Unit::Semitones => format!("{v:.0} st"),
            Unit::Cents => format!("{v:.0} ct"),
            Unit::Seconds => format!("{v:.2} s"),
            Unit::None => format!("{v:.2}"),
        }
    }
}

/// Static descriptor of a plugin.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PluginInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub vendor: &'static str,
    pub url: &'static str,
    pub email: &'static str,
    pub version: &'static str,
    pub category: Category,
    pub params: &'static [ParamInfo],
    pub is_instrument: bool,
    pub has_editor: bool,
}

impl PluginInfo {
    pub fn param(&self, id: &str) -> Option<&'static ParamInfo> {
        self.params.iter().find(|p| p.id == id)
    }
}

/// Host transport state for synchronization.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Transport {
    pub playing: bool,
    pub recording: bool,
    pub tempo: f64,
    pub numerator: u32,
    pub denominator: u32,
    pub sample_pos: i64,
    pub seconds: f64,
    pub quarter_notes: f64,
    pub bar_start: f64,
    pub bar_number: i64,
}

/// Core Plugin Trait.
///
/// Guaranteed contracts:
/// - Realtime safety: `process` never allocates heap memory or panics.
/// - Unaltered channels: if `io` provides more channels than configured in `prepare`,
///   extra channels are untouched.
pub trait Plugin: Send + 'static {
    fn info(&self) -> &'static PluginInfo;

    /// Configures audio stream parameters before processing begins.
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize);

    /// Informs the plugin if the host is rendering offline (e.g. bounce/export) rather than realtime playback.
    /// Plugins can use this to engage higher-order oversampling or non-realtime precision modes.
    fn set_offline(&mut self, _offline: bool) {}

    /// Resets internal filter history, delay lines, and voice states.
    fn reset(&mut self);

    /// Sets parameter value by ID. Returns false if ID is unrecognized.
    fn set_param(&mut self, id: &str, value: f32) -> bool;

    /// Retrieves current parameter value.
    fn param(&self, id: &str) -> Option<f32>;

    /// In-place planar audio processing.
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize);

    /// Latency in samples introduced by DSP buffers (for Delay Compensation).
    fn latency(&self) -> usize {
        0
    }

    /// Tail duration in samples after input ceases (e.g. reverb decay).
    fn tail_samples(&self) -> usize {
        0
    }

    /// Metering: current gain reduction in dB (e.g. for compressor VU meters).
    fn gain_reduction_db(&self) -> f32 {
        0.0
    }

    /// Updates transport info prior to each block.
    fn set_transport(&mut self, _transport: &Transport) {}

    /// MIDI Note-On event at block sample offset.
    fn note_on(&mut self, _offset: usize, _note: u8, _velocity: u8) {}

    /// MIDI Note-Off event at block sample offset.
    fn note_off(&mut self, _offset: usize, _note: u8) {}

    /// Raw MIDI 1.0 message (CC, pitch bend, etc.).
    fn midi(&mut self, _offset: usize, _msg: [u8; 3]) {}

    /// Panic / All Notes Off.
    fn all_notes_off(&mut self) {}

    /// Serializes plugin state into an opaque byte buffer.
    fn save_state(&mut self) -> Option<Vec<u8>> {
        None
    }

    /// Deserializes and restores state from an opaque byte buffer.
    fn load_state(&mut self, _data: &[u8]) -> bool {
        false
    }
}
