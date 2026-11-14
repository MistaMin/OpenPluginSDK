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

pub mod automation;
pub mod build;
pub mod editor;
pub mod layout;
pub mod midi;
pub mod plane;
pub mod preset;
pub mod ramp;
pub mod seqlock;
pub mod smooth;
pub mod spsc;
pub mod state;

pub use automation::{
    AutomationInterpolation, AutomationLane, AutomationMode, CurveInterpolation, LaneMode,
};
pub use layout::{
    BusMeters, IoConfig, NotePorts, ParamFlags, PortKind, SpeakerLayout, Tail, STEREO_LAYOUTS,
    BUS_METER_CHANNELS, MAX_AUDIO_CHANNELS,
};
pub use editor::{
    Editor, EditorFactory, EditorHost, EditorState, Frame, Gesture, GestureQueue, ValueMirror,
};
pub use plane::{ParamCursor, ParamPlane};
pub use preset::{Preset, PresetBank};
pub use ramp::SampleRamp;
pub use seqlock::Seqlock;
pub use smooth::OnePole;
pub use state::{
    decode_params, decode_session, encode_params, encode_session, restore_params, AbCompare,
    Session, SESSION_MAGIC, SESSION_VERSION, SNAPSHOT_MAGIC, SNAPSHOT_VERSION,
};

/// Levels the audio thread publishes for an editor. Decibels, not linear gain.
/// Reduction is positive attenuation from the dynamics envelope.
///
/// `true_peak_db`, the loudness fields, `correlation`, and `sidechain_db` are
/// whatever the plugin last published. `correlation` runs from -1 to 1.
/// `clip` bit 0 is left and bit 1 is right. Wider buses use [`BusMeters`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterFrame {
    pub input_db: [f32; 2],
    pub output_db: [f32; 2],
    pub reduction_db: f32,
    pub true_peak_db: [f32; 2],
    pub short_loudness_db: f32,
    pub integrated_loudness_db: f32,
    pub correlation: f32,
    pub sidechain_db: [f32; 2],
    pub clip: u8,
}

impl MeterFrame {
    pub const SILENT: Self = Self {
        input_db: [-100.0, -100.0],
        output_db: [-100.0, -100.0],
        reduction_db: 0.0,
        true_peak_db: [-100.0, -100.0],
        short_loudness_db: -100.0,
        integrated_loudness_db: -100.0,
        correlation: 0.0,
        sidechain_db: [-100.0, -100.0],
        clip: 0,
    };
}

impl Default for MeterFrame {
    fn default() -> Self {
        Self::SILENT
    }
}

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
    /// Discrete positions including the endpoints. `0` or `1` means continuous.
    /// A host turns this into `(max - min) / (steps - 1)`.
    pub steps: u32,
    /// Host roles. [`ParamInfo::new`] sets [`ParamFlags::AUTOMATABLE`].
    pub flags: ParamFlags,
}

impl ParamInfo {
    pub const fn new(
        id: &'static str,
        name: &'static str,
        min: f32,
        max: f32,
        default: f32,
        unit: Unit,
    ) -> Self {
        Self {
            id,
            name,
            min,
            max,
            default,
            unit,
            taper: Taper::Linear,
            choices: &[],
            steps: 0,
            flags: ParamFlags::AUTOMATABLE,
        }
    }

    pub const fn with_taper(mut self, taper: Taper) -> Self {
        self.taper = taper;
        self
    }

    /// Snap the value to `steps` positions, including both endpoints.
    pub const fn with_steps(mut self, steps: u32) -> Self {
        self.steps = steps;
        self
    }

    pub const fn with_choices(mut self, choices: &'static [&'static str]) -> Self {
        self.choices = choices;
        self.unit = Unit::Choice;
        self
    }

    /// Replace the flag word.
    pub const fn with_flags(mut self, flags: ParamFlags) -> Self {
        self.flags = flags;
        self
    }

    /// Add one flag. Read-only and not-realtime clear [`ParamFlags::AUTOMATABLE`].
    pub const fn flag(mut self, flag: ParamFlags) -> Self {
        self.flags = self.flags.union(flag);
        if flag.contains(ParamFlags::READ_ONLY) || flag.contains(ParamFlags::NOT_REALTIME) {
            self.flags = self.flags.difference(ParamFlags::AUTOMATABLE);
        }
        self
    }

    /// Clamps value safely into range; NaN falls back to default.
    pub fn clamp(&self, value: f32) -> f32 {
        let v = if value.is_nan() { self.default } else { value };
        let v = v.clamp(self.min, self.max);
        let v = match self.unit {
            Unit::Toggle | Unit::Choice => v.round().clamp(self.min, self.max),
            _ => v,
        };
        self.snap_steps(v)
    }

    fn snap_steps(&self, value: f32) -> f32 {
        if self.steps <= 1 || matches!(self.unit, Unit::Toggle | Unit::Choice) {
            return value;
        }
        let span = self.max - self.min;
        let divisions = (self.steps - 1) as f32;
        if span <= 0.0 || !divisions.is_finite() {
            return value;
        }
        let position = ((value - self.min) / span * divisions).round() / divisions;
        self.min + span * position.clamp(0.0, 1.0)
    }

    /// Maps a raw value to normalized 0..1 travel honoring the taper.
    pub fn to_normalized(&self, value: f32) -> f32 {
        let v = self.clamp(value);
        let span = self.max - self.min;
        if span <= 0.0 {
            return 0.0;
        }
        match self.taper {
            Taper::Log if self.min > 0.0 => {
                ((v / self.min).ln() / (self.max / self.min).ln()).clamp(0.0, 1.0)
            }
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

    /// Reads a value a user typed, or one [`Self::format`] printed, back into
    /// plain units. It accepts a choice name or its index, `On`/`Off` (also
    /// `yes`/`no`, `true`/`false`), `-inf`, and a number with an optional
    /// unit: `kHz`/`Hz`, `ms`/`s`, `dB`, `%`, `:1`, `st`, `ct`. A unit that
    /// scales converts to the parameter's own unit (`1.5 kHz` is 1500 for a
    /// Hz parameter, `250 ms` is 0.25 for a seconds parameter). The result is
    /// clamped like [`Self::clamp`]. `None` when the text is not a value.
    pub fn parse(&self, text: &str) -> Option<f32> {
        let t = text.trim();
        if t.is_empty() {
            return None;
        }
        // `format` names a choice by its value, so the position is the value.
        if let Some(i) = self.choices.iter().position(|c| c.trim().eq_ignore_ascii_case(t)) {
            return Some(self.clamp(i as f32));
        }
        let lower = t.to_ascii_lowercase();
        let switch = match lower.as_str() {
            "on" | "yes" | "true" => Some(1.0),
            "off" | "no" | "false" => Some(0.0),
            _ => None,
        };
        if let Some(v) = switch {
            if matches!(self.unit, Unit::Toggle) || (self.min <= v && v <= self.max) {
                return Some(self.clamp(v));
            }
        }
        if lower.starts_with("-inf") {
            return Some(self.min);
        }
        let end = lower
            .char_indices()
            .find(|&(i, c)| {
                !(c.is_ascii_digit()
                    || c == '.'
                    || ((c == '-' || c == '+') && i == 0)
                    || (c == 'e' && i > 0 && lower[i + 1..].starts_with(|n: char| n.is_ascii_digit() || n == '-' || n == '+')))
            })
            .map(|(i, _)| i)
            .unwrap_or(lower.len());
        let number: f32 = lower[..end].parse().ok()?;
        if !number.is_finite() {
            return None;
        }
        let suffix = lower[end..].trim();
        let value = match (suffix, self.unit) {
            ("khz", Unit::Hz) => number * 1000.0,
            ("hz", Unit::Hz) => number,
            ("ms", Unit::Seconds) => number / 1000.0,
            ("s" | "sec", Unit::Ms) => number * 1000.0,
            ("", _)
            | ("hz", _)
            | ("khz", _)
            | ("ms", _)
            | ("s" | "sec", _)
            | ("db", _)
            | ("%", _)
            | (":1", _)
            | ("st", _)
            | ("ct", _) => number,
            _ => return None,
        };
        Some(self.clamp(value))
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

    /// Index form of [`Self::set_param`]. Hosts call this after reading a [`ParamPlane`].
    fn set_param_index(&mut self, index: usize, value: f32) -> bool {
        match self.info().params.get(index) {
            Some(param) => self.set_param(param.id, value),
            None => false,
        }
    }

    /// Retrieves current parameter value.
    fn param(&self, id: &str) -> Option<f32>;

    /// Editor-facing levels. The audio thread publishes this through a [`Seqlock`].
    fn meters(&self) -> MeterFrame {
        MeterFrame::SILENT
    }

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

    /// Tail as a host property. The default maps [`Self::tail_samples`]: zero
    /// is [`Tail::None`], anything else is [`Tail::Finite`]. Override this to
    /// report [`Tail::Unknown`] or [`Tail::Infinite`].
    fn tail(&self) -> Tail {
        match self.tail_samples() {
            0 => Tail::None,
            samples => Tail::Finite(samples.min(u32::MAX as usize) as u32),
        }
    }

    /// Layouts the host may select while the plugin is deactivated.
    /// The default is stereo in, stereo out, and no sidechain.
    fn supported_layouts(&self) -> &'static [IoConfig] {
        STEREO_LAYOUTS
    }

    /// Apply a layout from [`Self::supported_layouts`]. Called while deactivated.
    fn set_layout(&mut self, layout: IoConfig) -> bool {
        self.supported_layouts().contains(&layout)
    }

    /// `prepare` for a chosen layout. The default passes the wider main bus
    /// and ignores the sidechain width.
    fn prepare_layout(&mut self, sample_rate: f32, max_block: usize, layout: IoConfig) {
        let channels = layout.main_channels().clamp(1, MAX_AUDIO_CHANNELS);
        self.prepare(sample_rate, max_block, channels);
    }

    /// Main bus plus an optional sidechain. The sidechain is detector input.
    /// The default drops it and calls [`Self::process`].
    fn process_buses(&mut self, io: &mut [Vec<f32>], sidechain: Option<&[Vec<f32>]>, frames: usize) {
        let _ = sidechain;
        self.process(io, frames);
    }

    /// Note ports. The default keeps a MIDI input so existing instruments and
    /// MIDI effects still receive notes. An audio effect that does not read
    /// MIDI should return both flags false.
    fn note_ports(&self) -> NotePorts {
        NotePorts {
            input: true,
            output: false,
        }
    }

    /// Same as [`Self::note_on`] with a MIDI channel 0..15.
    fn note_on_channel(&mut self, offset: usize, _channel: u8, note: u8, velocity: u8) {
        self.note_on(offset, note, velocity);
    }

    /// One queued MIDI output event, in block-sample order. `None` ends the drain.
    fn pull_midi(&mut self) -> Option<(u32, [u8; 3])> {
        None
    }

    /// Host bypass, separate from a power parameter inside the processor.
    fn set_host_bypass(&mut self, _bypassed: bool) {}

    /// Soft bypass keeps the reported latency and the dry path delayed by it.
    /// Hard bypass may drop the reported latency on the next activate.
    fn keeps_latency_when_bypassed(&self) -> bool {
        true
    }

    /// Peaks and gain reduction past the stereo [`MeterFrame`].
    fn bus_meters(&self) -> BusMeters {
        BusMeters::SILENT
    }

    /// False rejects the rate at activate. The default accepts 8 kHz through 768 kHz.
    fn accepts_sample_rate(&self, sample_rate: f32) -> bool {
        sample_rate.is_finite() && (8_000.0..=768_000.0).contains(&sample_rate)
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

    /// Factory programs. `0` means the host shows no program list.
    /// [`Self::preset_values`] is plain values in descriptor order and must
    /// not touch DSP state: a message thread can read it during playback.
    fn preset_count(&self) -> usize {
        0
    }

    fn preset_name(&self, _index: usize) -> &'static str {
        ""
    }

    fn preset_values(&self, _index: usize) -> Option<&'static [f32]> {
        None
    }

    /// What this engine shares with its editor ([`editor`]). Called once on
    /// the message thread right after the plugin is made, before any audio.
    /// The default shares nothing, and the editor shows host values alone.
    fn editor_state(&self) -> Option<EditorState> {
        None
    }

    /// Fixed AAX identifiers. Pro Tools finds a plugin in a session by them,
    /// so once released they never change. `None` uses development IDs
    /// hashed from the plugin id, which cannot go in an AAX page table.
    fn aax_ids(&self) -> Option<AaxIds> {
        None
    }

    /// The LV2 URI. Saved LV2 sessions find the plugin by it, so once
    /// released it never changes. `None` uses [`PluginInfo::id`].
    fn lv2_uri(&self) -> Option<&'static str> {
        None
    }

    /// Serializes plugin state into an opaque byte buffer.
    fn save_state(&mut self) -> Option<Vec<u8>> {
        None
    }

    /// Deserializes and restores state from an opaque byte buffer.
    fn load_state(&mut self, _data: &[u8]) -> bool {
        false
    }
}

/// The four AAX codes: manufacturer, product, and the mono and stereo plug-in
/// types. Each is four printable ASCII characters, such as `*b"OpGd"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AaxIds {
    pub manufacturer: [u8; 4],
    pub product: [u8; 4],
    pub mono: [u8; 4],
    pub stereo: [u8; 4],
}

impl AaxIds {
    /// True when every code is four printable characters and the two
    /// plug-in types differ.
    pub fn is_valid(&self) -> bool {
        let printable = |code: &[u8; 4]| code.iter().all(|b| (0x20..0x7f).contains(b));
        [self.manufacturer, self.product, self.mono, self.stereo].iter().all(printable) && self.mono != self.stereo
    }
}

/// Set every momentary parameter back to its default. An offline render calls
/// this so listen, delta, and solo buttons do not stay down.
pub fn release_momentary(plugin: &mut dyn Plugin) {
    let params = plugin.info().params;
    for (index, param) in params.iter().enumerate() {
        if param.flags.contains(ParamFlags::MOMENTARY) {
            let _ = plugin.set_param_index(index, param.default);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_what_format_prints() {
        static MODES: [&str; 3] = ["Room", "Hall", "Plate"];
        let mode = ParamInfo::new("mode", "Mode", 0.0, 2.0, 0.0, Unit::None).with_choices(&MODES);
        let toggle = ParamInfo::new("freeze", "Freeze", 0.0, 1.0, 0.0, Unit::Toggle);
        let freq = ParamInfo::new("freq", "Freq", 20.0, 20_000.0, 1_000.0, Unit::Hz);
        let time = ParamInfo::new("time", "Time", 0.0, 10.0, 1.0, Unit::Seconds);
        let delay = ParamInfo::new("delay", "Delay", 0.0, 2_000.0, 10.0, Unit::Ms);
        let gain = ParamInfo::new("gain", "Gain", -96.0, 24.0, 0.0, Unit::Db);
        for p in [&mode, &toggle, &freq, &time, &delay, &gain] {
            for v in [p.min, p.default, p.max, 0.5 * (p.min + p.max)] {
                let text = p.format(v);
                let back = p.parse(&text).unwrap_or_else(|| panic!("{} did not read {text:?}", p.id));
                let tolerance = (p.max - p.min) * 0.001;
                assert!((back - p.clamp(v)).abs() <= tolerance, "{} {text:?} gave {back}", p.id);
            }
        }
        assert_eq!(mode.parse("plate"), Some(2.0));
        assert_eq!(mode.parse("1"), Some(1.0));
        assert_eq!(toggle.parse("ON"), Some(1.0));
        assert_eq!(freq.parse("1.5 kHz"), Some(1500.0));
        assert_eq!(time.parse("250 ms"), Some(0.25));
        assert_eq!(delay.parse("1.2 s"), Some(1200.0));
        assert_eq!(gain.parse("-inf dB"), Some(-96.0));
        assert_eq!(gain.parse("+3dB"), Some(3.0));
        assert_eq!(gain.parse("1e1"), Some(10.0));
        assert_eq!(gain.parse("loud"), None);
        assert_eq!(gain.parse("3 parsecs"), None);
        assert_eq!(gain.parse(""), None);
    }

    #[test]
    fn aax_ids_must_be_printable_and_distinct() {
        let ok = AaxIds { manufacturer: *b"OpGd", product: *b"Sp9v", mono: *b"Sp9M", stereo: *b"Sp9S" };
        assert!(ok.is_valid());
        assert!(!AaxIds { stereo: *b"Sp9M", ..ok }.is_valid());
        assert!(!AaxIds { product: [0, 1, 2, 3], ..ok }.is_valid());
    }

    #[test]
    fn discrete_steps_snap_and_continuous_values_do_not() {
        let stepped = ParamInfo::new("mode", "Mode", 0.0, 4.0, 0.0, Unit::None).with_steps(5);
        assert_eq!(stepped.clamp(1.2), 1.0);
        assert_eq!(stepped.clamp(1.6), 2.0);
        let smooth = ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db);
        assert!((smooth.clamp(1.2) - 1.2).abs() < 1e-6);
    }
}
