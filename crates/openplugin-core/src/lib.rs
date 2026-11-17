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
pub mod denormal;
pub mod editor;
pub mod expression;
pub mod foreign_state;
pub mod identity;
pub mod layout;
pub mod midi;
pub mod mpe;
pub mod notify;
pub mod plane;
pub mod preset;
pub mod ramp;
pub mod seqlock;
pub mod smooth;
pub mod spsc;
pub mod state;
pub mod worker;

pub use denormal::DenormalGuard;
pub use automation::{
    AutomationInterpolation, AutomationLane, AutomationMode, CurveInterpolation, LaneMode,
};
pub use layout::{
    BusMeters, IoConfig, NotePorts, ParamFlags, PortKind, SpeakerLayout, Tail, STEREO_LAYOUTS,
    BUS_METER_CHANNELS, MAX_AUDIO_CHANNELS,
};
pub use editor::{
    Editor, EditorFactory, EditorHost, EditorState, Frame, Gesture, GestureQueue, ParamMenu,
    ParamMenuAction, ValueMirror,
};
pub use expression::{Expression, ExpressionSet, NoteRef, VoiceInfo};
pub use identity::{AuIds, ParamIdMap, ParamPins, PinError, PinFormat, Vst3Ids};
pub use mpe::{MpeReceiver, MpeZones};
pub use notify::HostNotifier;
pub use plane::{ParamCursor, ParamPlane};
pub use preset::{Preset, PresetBank};
pub use ramp::SampleRamp;
pub use seqlock::Seqlock;
pub use smooth::OnePole;
pub use state::{
    decode_params, decode_session, encode_params, encode_session, restore_params, AbCompare,
    Session, SESSION_MAGIC, SESSION_VERSION, SNAPSHOT_MAGIC, SNAPSHOT_VERSION,
    SNAPSHOT_VERSION_KEYED,
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
    /// Travel is the linear proportion raised to the skew: `t = ((v - min) / (max - min)) ^ skew`,
    /// so a skew below 1 gives the low end more travel. The skewed ranges many plugin SDKs offer;
    /// a port keeps the old curve, and with it the meaning of saved normalized automation.
    Skew(Skew),
}

/// The exponent of [`Taper::Skew`], kept as `f32` bits so a taper stays `Eq` and `Hash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Skew(u32);

impl Skew {
    pub const fn new(skew: f32) -> Self {
        Self(skew.to_bits())
    }

    pub fn get(self) -> f32 {
        f32::from_bits(self.0)
    }
}

/// The formats in which a parameter shows hosts its normalized 0..1 travel instead of plain units
/// ([`ParamInfo::with_normalized_in`]). The plugin, its editor and `set_param` still see plain values;
/// only the host-facing value, range and default change. It is the value space of a plugin from an
/// SDK that published normalized values, so automation saved with that plugin replays on a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize)]
pub struct HostSpace(u8);

impl HostSpace {
    /// Plain units in every format (the default).
    pub const PLAIN: Self = Self(0);
    /// CLAP `param_info`, values, events and text.
    pub const CLAP: Self = Self(1);
    /// The VST3, Audio Unit and AAX shells (`op_shell_*` and the editor bridge). VST3 and AAX
    /// automation is normalized anyway; the Audio Unit's values and ranges become 0..1.
    pub const SHELLS: Self = Self(4);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0 && other.0 != 0
    }

    pub const fn is_plain(&self) -> bool {
        self.0 == 0
    }
}

/// A parameter's own value text: how a plain value prints and how typed text reads back
/// ([`ParamInfo::with_text`]). `parse` returns a plain value; the result is clamped afterwards.
#[derive(Clone, Copy)]
pub struct ParamText {
    pub format: fn(f32) -> String,
    pub parse: fn(&str) -> Option<f32>,
}

impl std::fmt::Debug for ParamText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ParamText")
    }
}

impl PartialEq for ParamText {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::fn_addr_eq(self.format, other.format) && std::ptr::fn_addr_eq(self.parse, other.parse)
    }
}

/// Separates the names in a [`ParamInfo::group`] path.
pub const GROUP_SEPARATOR: char = '/';
/// Longest [`ParamInfo::group`] path, in bytes.
pub const GROUP_MAX_BYTES: usize = 255;

/// Why [`check_group`] refused a group path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupError {
    /// A name is empty: the path starts or ends with `/`, or has `//`.
    EmptyName,
    /// A name starts or ends with a space (`"Filter / Envelope"`).
    UntrimmedName,
    /// A control character (below U+0020, or U+007F) is in the path.
    ControlCharacter,
    /// The path is longer than [`GROUP_MAX_BYTES`].
    TooLong,
}

/// Checks a group path: `""` (the top level), or names separated by `/`,
/// none empty, none starting or ending with a space, no control characters,
/// at most [`GROUP_MAX_BYTES`] bytes in all. `const`, so a parameter table
/// can be checked at build time (see [`groups_valid`]).
pub const fn check_group(path: &str) -> Result<(), GroupError> {
    let bytes = path.as_bytes();
    if bytes.is_empty() {
        return Ok(());
    }
    if bytes.len() > GROUP_MAX_BYTES {
        return Err(GroupError::TooLong);
    }
    let mut start = 0;
    let mut i = 0;
    while i <= bytes.len() {
        if i == bytes.len() || bytes[i] == b'/' {
            if i == start {
                return Err(GroupError::EmptyName);
            }
            if bytes[start] == b' ' || bytes[i - 1] == b' ' {
                return Err(GroupError::UntrimmedName);
            }
            start = i + 1;
        } else if bytes[i] < 0x20 || bytes[i] == 0x7f {
            return Err(GroupError::ControlCharacter);
        }
        i += 1;
    }
    Ok(())
}

/// Whether every parameter's group passes [`check_group`]. For a build-time
/// check of a parameter table:
///
/// ```
/// # use openplugin_core::{groups_valid, ParamInfo, Unit};
/// static PARAMS: [ParamInfo; 2] = [
///     ParamInfo::new("cutoff", "Cutoff", 20.0, 20_000.0, 1_000.0, Unit::Hz).with_group("Filter"),
///     ParamInfo::new("attack", "Attack", 0.0, 1.0, 0.01, Unit::Seconds).with_group("Filter/Envelope"),
/// ];
/// const _: () = assert!(groups_valid(&PARAMS));
/// ```
pub const fn groups_valid(params: &[ParamInfo]) -> bool {
    let mut i = 0;
    while i < params.len() {
        if check_group(params[i].group).is_err() {
            return false;
        }
        i += 1;
    }
    true
}

/// Every group path a parameter table uses, each once, parents included and
/// listed before their children: `"Filter/Envelope"` alone gives `"Filter"`,
/// then `"Filter/Envelope"`. Paths come in the order their first parameter
/// is declared. Empty when no parameter has a group. For format entries that
/// declare groups up front (LV2 `pg:Group`, VST3 units); main thread.
pub fn group_paths(params: &[ParamInfo]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for param in params {
        let group = param.host_group();
        if group.is_empty() {
            continue;
        }
        for (at, _) in group.match_indices(GROUP_SEPARATOR).chain(std::iter::once((group.len(), ""))) {
            let path = &group[..at];
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

/// The parent of a group path (`"Filter"` for `"Filter/Envelope"`), and `""`
/// for a top-level group. The last name is [`group_name`].
pub fn group_parent(path: &str) -> &str {
    path.rfind(GROUP_SEPARATOR).map_or("", |at| &path[..at])
}

/// The last name of a group path (`"Envelope"` for `"Filter/Envelope"`).
pub fn group_name(path: &str) -> &str {
    path.rfind(GROUP_SEPARATOR).map_or(path, |at| &path[at + 1..])
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
    /// The group a host lists the parameter under: names separated by `/`,
    /// outermost first (`"Filter/Envelope"`). Empty is the top level. Set it
    /// with [`ParamInfo::with_group`]; [`check_group`] says what a valid path
    /// is, and a host is shown an invalid one as the top level.
    #[serde(skip_serializing_if = "str::is_empty")]
    pub group: &'static str,
    /// The identifiers hosts store for this parameter, where they are set
    /// explicitly ([`identity`]). Empty leaves every one derived: the
    /// parameter's position for CLAP, VST3 and Audio Unit, and [`Self::id`]
    /// for AAX. Set them with [`ParamInfo::with_host_id`] and the per-format
    /// forms.
    #[serde(skip_serializing_if = "ParamPins::is_empty")]
    pub pins: ParamPins,
    /// Formats that show hosts normalized values ([`ParamInfo::with_normalized_in`]).
    #[serde(skip_serializing_if = "HostSpace::is_plain")]
    pub normalized_in: HostSpace,
    /// The parameter's own value text ([`ParamInfo::with_text`]); `None` prints by [`Unit`].
    #[serde(skip)]
    pub text: Option<ParamText>,
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
            group: "",
            pins: ParamPins::NONE,
            normalized_in: HostSpace::PLAIN,
            text: None,
        }
    }

    pub const fn with_taper(mut self, taper: Taper) -> Self {
        self.taper = taper;
        self
    }

    /// Show hosts normalized 0..1 values in `formats` ([`HostSpace`]).
    pub const fn with_normalized_in(mut self, formats: HostSpace) -> Self {
        self.normalized_in = formats;
        self
    }

    /// Print and read values with the plugin's own functions instead of by [`Unit`].
    pub const fn with_text(mut self, format: fn(f32) -> String, parse: fn(&str) -> Option<f32>) -> Self {
        self.text = Some(ParamText { format, parse });
        self
    }

    /// The value a host in `format` sees for the plain value `plain`.
    pub fn to_host(&self, format: HostSpace, plain: f32) -> f32 {
        if self.normalized_in.contains(format) { self.to_normalized(plain) } else { plain }
    }

    /// The plain value for a value a host in `format` sent, clamped.
    pub fn from_host(&self, format: HostSpace, value: f32) -> f32 {
        if self.normalized_in.contains(format) { self.from_normalized(value) } else { self.clamp(value) }
    }

    /// Minimum, maximum and default as a host in `format` sees them.
    pub fn host_range(&self, format: HostSpace) -> (f32, f32, f32) {
        if self.normalized_in.contains(format) {
            (0.0, 1.0, self.to_normalized(self.default))
        } else {
            (self.min, self.max, self.default)
        }
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

    /// List the parameter under a group: names separated by `/`, outermost
    /// first, as in `"Filter/Envelope"`. `""` is the top level. A path that
    /// [`check_group`] refuses is shown to hosts as the top level; assert
    /// [`groups_valid`] on the parameter table to catch one at build time.
    pub const fn with_group(mut self, group: &'static str) -> Self {
        self.group = group;
        self
    }

    /// Pin the number hosts store for this parameter in CLAP (`param_id`),
    /// VST3 (`ParamID`) and Audio Unit (`AudioUnitParameterID`) to `id`, in
    /// place of the parameter's position in the table. Automation and saved
    /// projects then follow the parameter when the table is reordered or
    /// grows. Once released, the number never changes. Ids must be unique in
    /// the table; a parameter without a pin keeps its position as its id. See
    /// [`identity`] for the ids a format reserves, and
    /// [`identity::check_pins`] to test a table.
    ///
    /// A table with any pin is also saved by [`Self::id`] instead of by
    /// position ([`identity::state_is_keyed`]).
    pub const fn with_host_id(mut self, id: u32) -> Self {
        self.pins.clap = Some(id);
        self.pins.vst3 = Some(id);
        self.pins.au = Some(id);
        self
    }

    /// Pin the CLAP `param_id` alone, for a port whose formats used different numbers.
    pub const fn with_clap_id(mut self, id: u32) -> Self {
        self.pins.clap = Some(id);
        self
    }

    /// Pin the VST3 `ParamID` alone. At most `0x7FFF_FFFF`.
    pub const fn with_vst3_id(mut self, id: u32) -> Self {
        self.pins.vst3 = Some(id);
        self
    }

    /// Pin the `AudioUnitParameterID` alone.
    pub const fn with_au_id(mut self, id: u32) -> Self {
        self.pins.au = Some(id);
        self
    }

    /// Pin the AAX parameter id (1 to 31 printable ASCII characters) in place
    /// of [`Self::id`]. Pro Tools stores automation by it.
    pub const fn with_aax_id(mut self, id: &'static str) -> Self {
        self.pins.aax = Some(id);
        self
    }

    /// The group path every format shows: [`Self::group`] when [`check_group`]
    /// accepts it, otherwise `""` (the top level).
    pub const fn host_group(&self) -> &'static str {
        match check_group(self.group) {
            Ok(()) => self.group,
            Err(_) => "",
        }
    }

    /// The names of [`Self::host_group`], outermost first; none at the top level.
    pub fn group_names(&self) -> impl Iterator<Item = &'static str> {
        let group = self.host_group();
        group.split(GROUP_SEPARATOR).filter(move |_| !group.is_empty())
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
            Taper::Skew(s) if s.get() > 0.0 && s.get().is_finite() => {
                let p = ((v - self.min) / span).clamp(0.0, 1.0);
                if p > 0.0 { (p.ln() * s.get()).exp().clamp(0.0, 1.0) } else { 0.0 }
            }
            _ => ((v - self.min) / span).clamp(0.0, 1.0),
        }
    }

    /// Maps normalized 0..1 travel back to raw parameter value honoring the taper.
    pub fn from_normalized(&self, t: f32) -> f32 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        let v = match self.taper {
            Taper::Log if self.min > 0.0 => self.min * (self.max / self.min).powf(t),
            Taper::Skew(s) if s.get() > 0.0 && s.get().is_finite() => {
                let p = if t > 0.0 { (t.ln() / s.get()).exp() } else { 0.0 };
                self.min + (self.max - self.min) * p
            }
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
        if let Some(own) = self.text {
            return (own.parse)(t).filter(|v| v.is_finite()).map(|v| self.clamp(v));
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
        if let Some(own) = self.text {
            return (own.format)(v);
        }
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

/// What the host says about the track the plugin sits on. Delivered through
/// [`Plugin::track_info_changed`] and [`Editor::track_info`] on the main
/// thread, never on the audio thread. A field the host does not know is
/// `None` / 0.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackInfo {
    /// The track's name, if the host gave one and it is not empty.
    pub name: Option<String>,
    /// The track's colour as `0xRRGGBBAA`, if the host gave one.
    pub colour: Option<u32>,
    /// A combination of the `TrackInfo::IS_*` bits.
    pub kind: u32,
}

impl TrackInfo {
    /// The plugin is on a return (send) track: start with wet 100%.
    pub const IS_RETURN: u32 = 1;
    /// The plugin is on a bus (group) track.
    pub const IS_BUS: u32 = 2;
    /// The plugin is on the master track.
    pub const IS_MASTER: u32 = 4;

    /// The track is a return track.
    pub fn is_return(&self) -> bool {
        self.kind & Self::IS_RETURN != 0
    }

    /// The track is a bus.
    pub fn is_bus(&self) -> bool {
        self.kind & Self::IS_BUS != 0
    }

    /// The track is the master.
    pub fn is_master(&self) -> bool {
        self.kind & Self::IS_MASTER != 0
    }

    /// The colour as `0x00RRGGBB`, the editor's pixel format (alpha dropped).
    pub fn rgb(&self) -> Option<u32> {
        self.colour.map(|c| c >> 8)
    }
}

/// Lowest sample rate a format bridge accepts, in hertz.
///
/// Every DSP block in `openplugin-dsp` computes its coefficients for the real
/// rate down to this floor (`openplugin_dsp::effective_rate`), and
/// `openplugin-dsp/tests/low_rates.rs` checks their time constants at 1 kHz.
/// `clap-validator`'s varying-sample-rates test runs at 1234.5678 Hz, so the
/// old 8 kHz floor failed validation.
pub const MIN_SAMPLE_RATE: f32 = 1_000.0;

/// Highest sample rate a format bridge accepts, in hertz: 16 x 48 kHz.
///
/// That is the top rate `clap-validator` tries and higher than any PCM host rate.
pub const MAX_SAMPLE_RATE: f32 = 768_000.0;

/// Whether `sample_rate` is a finite rate in [`MIN_SAMPLE_RATE`]..=[`MAX_SAMPLE_RATE`].
///
/// CLAP and LV2 hand the rate to the bridge as an `f64`; [`Plugin::accepts_sample_rate`]
/// is the plugin's own `f32` predicate, which defaults to this range. A format
/// bridge checks this range first, so a plugin can narrow the decision but not widen it.
pub fn sample_rate_supported(sample_rate: f64) -> bool {
    sample_rate.is_finite()
        && sample_rate >= f64::from(MIN_SAMPLE_RATE)
        && sample_rate <= f64::from(MAX_SAMPLE_RATE)
}

/// Core Plugin Trait.
///
/// Guaranteed contracts:
/// - Realtime safety: `process` never allocates heap memory or panics.
/// - Unaltered channels: if `io` provides more channels than configured in `prepare`,
///   extra channels are untouched.
///
/// A new method must also be forwarded by `openplugin_dsp::SurroundAdapter`,
/// or every wrapped plugin silently answers with the default.
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

    /// False rejects the rate at activate. The default accepts
    /// [`MIN_SAMPLE_RATE`] through [`MAX_SAMPLE_RATE`], the range every format
    /// bridge enforces. A plugin may narrow it.
    fn accepts_sample_rate(&self, sample_rate: f32) -> bool {
        sample_rate_supported(f64::from(sample_rate))
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

    /// The per-note expressions the plugin reacts to ([`expression`]). The
    /// default is none, and [`Self::note_expression`] is then never called
    /// by CLAP or VST3; the VST3 controller offers `INoteExpressionController`
    /// only for a plugin that lists some. Read once when an instance is made.
    fn note_expressions(&self) -> ExpressionSet {
        ExpressionSet::NONE
    }

    /// MPE over MIDI ([`mpe`]). `Some(zones)` turns the plugin's MIDI input
    /// into per-note events through an [`MpeReceiver`] starting with these
    /// zones (an MPE Configuration Message from the controller changes them):
    /// notes arrive through [`Self::note_on_ref`] and [`Self::note_off_ref`]
    /// with their channel, and pitch bend, channel pressure and CC 74 as
    /// [`Expression::Tuning`], [`Expression::Pressure`] and
    /// [`Expression::Brightness`] through [`Self::note_expression`] (whatever
    /// [`Self::note_expressions`] says). The default `None` is plain MIDI, as
    /// before. Read once when an instance is made.
    fn mpe_input(&self) -> Option<MpeZones> {
        None
    }

    /// A note-on with the host's note id where it has one ([`NoteRef`]: id,
    /// port, channel and key; the port, channel and key are 0 or more). The
    /// default calls [`Self::note_on_channel`] with the channel (clamped to
    /// 0..=15) and the key as the entries passed it before note ids existed,
    /// so a plugin that does not override it sees what it saw before.
    fn note_on_ref(&mut self, offset: usize, note: NoteRef, velocity: u8) {
        let channel = note.channel.clamp(0, 15) as u8;
        self.note_on_channel(offset, channel, note.key as u8, velocity);
    }

    /// A note-off for a note started by [`Self::note_on_ref`], with its
    /// release velocity (0..=127). `note` may hold wildcards (`-1`), as a
    /// CLAP host may send. The default calls [`Self::note_off`] with the key
    /// as the entries did before (a wildcard key arrives there as 255).
    fn note_off_ref(&mut self, offset: usize, note: NoteRef, velocity: u8) {
        let _ = velocity;
        self.note_off(offset, note.key as u8);
    }

    /// A per-note expression for the notes `note` names
    /// ([`NoteRef::matches`]; wildcards reach several), in the units of
    /// [`Expression::range`], already clamped and never NaN. The value is a
    /// statement, not a step: it replaces the last one of that expression.
    /// Called only for expressions in [`Self::note_expressions`] (or the MPE
    /// three, with [`Self::mpe_input`]). `openplugin_dsp::voice_alloc` routes
    /// it to the matching voices.
    fn note_expression(&mut self, _offset: usize, _note: NoteRef, _expression: Expression, _value: f32) {}

    /// Modulation of parameter `index` by `amount` in plain units, added to
    /// the parameter's value for the notes `note` names (CLAP
    /// `CLAP_EVENT_PARAM_MOD`). Called only for a parameter with
    /// [`ParamFlags::MODULATABLE`] (then `note` is [`NoteRef::ALL`]: the whole
    /// plugin) or [`ParamFlags::POLY_MODULATION`] (any `note`). A new amount
    /// replaces the last one for those notes; `amount` is finite and at most
    /// the parameter's range either way. Modulation is not saved and does not
    /// move the parameter's value.
    fn param_modulation(&mut self, _offset: usize, _index: usize, _note: NoteRef, _amount: f32) {}

    /// The plugin's voices, for a host that asks (CLAP `voice-info`). The
    /// default `None` offers nothing, as before. Called on the main thread,
    /// each time the host asks, like [`Self::latency`]; `openplugin_dsp`'s
    /// `VoiceAllocator::voice_info` gives the answer for its voices.
    fn voice_info(&self) -> Option<VoiceInfo> {
        None
    }

    /// One voice that has finished since the last call, as the [`NoteRef`]
    /// its note-on carried (note id, port, channel, key); `None` ends the
    /// drain. The CLAP entry calls it after every block (audio thread, so it
    /// must not allocate) and sends each as `CLAP_EVENT_NOTE_END`, which
    /// tells the host it can stop modulating that note. A voice ends when its
    /// release is over, or when it is taken for another note.
    /// `openplugin_dsp`'s `VoiceAllocator::pop_ended` keeps that list for its
    /// voices. The default `None` sends nothing, as before.
    fn pull_note_end(&mut self) -> Option<NoteRef> {
        None
    }

    /// Whether the format entries run the plugin with denormals flushed. The
    /// default `true` sets flush-to-zero (FZ on Arm, FTZ and DAZ on x86) for
    /// the duration of every audio-thread call into the plugin (CLAP
    /// `process`, `params.flush` and `reset`; LV2 `run`; the shell ABI's
    /// `process`, `process_events`, `reset` and `midi_out`, which the VST3,
    /// Audio Unit and AAX shells call) and restores the host's own mode
    /// afterwards, also after a panic ([`DenormalGuard`]). Return `false`
    /// only for a plugin whose arithmetic needs subnormal numbers, which is
    /// very unusual: the entries then leave the host's mode as it is, and
    /// the plugin's state can decay through the slow subnormal range on x86.
    /// Read once, when an instance is made.
    fn flush_denormals(&self) -> bool {
        true
    }

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

    /// The host told the plugin about its track: name, colour and kind
    /// ([`TrackInfo`]). Called on the message thread, never inside `process`
    /// (the bridges take the plugin between blocks, as for a state call), once
    /// when the host first knows and again whenever it changes. The default
    /// ignores it. A plugin typically keeps the value for its editor or
    /// starts a return track with wet 100%.
    fn track_info_changed(&mut self, _info: &TrackInfo) {}

    /// The handle through which the plugin tells its host that its latency or
    /// its parameter info changed ([`notify`]). Called once on the message
    /// thread right after the plugin is made, before any audio. The default
    /// drops it: the format entries still compare [`Self::latency`] after
    /// every block, so a plugin that only changes its latency inside
    /// `process` need not keep it. Keep it to report a change made outside
    /// `process`, or a change of parameter names or value text.
    fn set_host_notifier(&mut self, _notifier: HostNotifier) {}

    /// What this engine shares with its editor ([`editor`]). Called once on
    /// the message thread right after the plugin is made, before any audio.
    /// The default shares nothing, and the editor shows host values alone.
    fn editor_state(&self) -> Option<EditorState> {
        None
    }

    /// The CLAP plugin id, when it is not [`PluginInfo::id`]. A CLAP host
    /// finds the plugin in a project by it, so once released it never
    /// changes. `None` uses [`PluginInfo::id`], which also seeds the derived
    /// VST3 and AAX ids; pin this to keep a released CLAP id while those stay
    /// derived from another `PluginInfo::id`, or the reverse. An empty id or
    /// one with a NUL is ignored.
    fn clap_id(&self) -> Option<&'static str> {
        None
    }

    /// Fixed VST3 class ids: the audio processor's and the edit
    /// controller's. A VST3 host finds the plugin in a project by the
    /// processor id, so once released they never change. `None` derives both
    /// from [`PluginInfo::id`]. Ids that fail [`Vst3Ids::is_valid`] are
    /// ignored.
    fn vst3_ids(&self) -> Option<Vst3Ids> {
        None
    }

    /// Fixed Audio Unit subtype and manufacturer codes. `None` leaves them
    /// to the build (`--au-subtype`, `--au-manufacturer`). The codes reach a
    /// host through the bundle's `Info.plist`, which the build writes: a
    /// build that is given no codes takes these, and one that is given
    /// different codes stops. Codes that fail [`AuIds::is_valid`] are
    /// ignored.
    fn au_ids(&self) -> Option<AuIds> {
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

    /// Whether the plugin's whole state lives in its parameters.
    ///
    /// Return `true` only when [`Plugin::save_state`] always returns `None`
    /// and [`Plugin::load_state`] is never needed: nothing outside the
    /// parameter values has to be saved or restored. The default is `false`,
    /// because the SDK cannot tell from outside whether a plugin overrides
    /// the two state methods, and a wrong guess would silently drop a
    /// plugin's chunk.
    ///
    /// The shells (VST3, Audio Unit, AAX) save and load state from a host
    /// thread while the audio thread may be running. A plugin that answers
    /// `false` has its state methods called on the plugin object, so a block
    /// that starts during a save or load is passed through unprocessed (a
    /// dry block). A plugin that answers `true` is never touched by a save
    /// or load: the SDK reads and writes the parameter values it already
    /// shares with the audio thread, so no block is skipped. The engine
    /// picks up loaded values on its next block.
    ///
    /// With `true`, a chunk found in a saved session is ignored; the
    /// parameter values are still restored. The answer must not change
    /// during the plugin's life: it is read once, when the instance is built.
    fn state_is_parameters_only(&self) -> bool {
        false
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
    #[test]
    fn skew_taper_follows_proportion_to_the_power() {
        let q = ParamInfo::new("q", "Q", 0.1, 10.0, 0.707, Unit::None).with_taper(Taper::Skew(Skew::new(0.5)));
        let v = 2.5f32;
        assert!((q.to_normalized(v) - ((v - 0.1) / 9.9).sqrt()).abs() < 1e-6);
        for k in 0..=20 {
            let t = k as f32 / 20.0;
            assert!((q.to_normalized(q.from_normalized(t)) - t).abs() < 1e-5);
        }
        assert_eq!(q.to_normalized(0.1), 0.0);
        assert_eq!(q.from_normalized(1.0), 10.0);
        // an unusable skew is linear travel
        let bad = ParamInfo::new("b", "B", 0.0, 1.0, 0.0, Unit::None).with_taper(Taper::Skew(Skew::new(0.0)));
        assert_eq!(bad.to_normalized(0.25), 0.25);
    }

    #[test]
    fn host_space_is_plain_unless_asked() {
        let p = ParamInfo::new("f", "F", 20.0, 1000.0, 100.0, Unit::Hz).with_taper(Taper::Log);
        assert_eq!(p.host_range(HostSpace::CLAP), (20.0, 1000.0, 100.0));
        assert_eq!(p.to_host(HostSpace::SHELLS, 500.0), 500.0);
        let n = p.clone().with_normalized_in(HostSpace::CLAP);
        assert_eq!(n.host_range(HostSpace::SHELLS), (20.0, 1000.0, 100.0), "only the formats named");
        let (lo, hi, def) = n.host_range(HostSpace::CLAP);
        assert_eq!((lo, hi), (0.0, 1.0));
        assert!((def - n.to_normalized(100.0)).abs() < 1e-7);
        assert!((n.from_host(HostSpace::CLAP, n.to_host(HostSpace::CLAP, 333.0)) - 333.0).abs() < 1e-2);
        let both = p.with_normalized_in(HostSpace::CLAP.union(HostSpace::SHELLS));
        assert!(both.normalized_in.contains(HostSpace::SHELLS) && both.normalized_in.contains(HostSpace::CLAP));
        assert!(!HostSpace::PLAIN.contains(HostSpace::PLAIN));
    }

    fn pct_text(v: f32) -> String { format!("{} %", (v * 100.0).round()) }
    fn pct_parse(t: &str) -> Option<f32> { t.trim_end_matches('%').trim().parse::<f32>().ok().map(|v| v / 100.0) }

    #[test]
    fn own_text_prints_and_reads_clamped_values() {
        let p = ParamInfo::new("h", "H", 0.0, 1.0, 0.0, Unit::None).with_text(pct_text, pct_parse);
        assert_eq!(p.format(0.4), "40 %");
        assert_eq!(p.format(7.0), "100 %", "printed after clamping");
        assert_eq!(p.parse("40 %"), Some(0.4));
        assert_eq!(p.parse("250 %"), Some(1.0), "read values are clamped");
        assert_eq!(p.parse("loud"), None);
        assert_eq!(p.clone(), p, "comparable");
    }

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
    fn group_paths_are_checked() {
        assert_eq!(check_group(""), Ok(()));
        assert_eq!(check_group("Filter"), Ok(()));
        assert_eq!(check_group("Filter/Envelope"), Ok(()));
        assert_eq!(check_group("Osc 1/Pitch Env"), Ok(()));
        assert_eq!(check_group("Fülle/Hüllkurve"), Ok(()));
        for bad in ["/", "/Filter", "Filter/", "Filter//Envelope"] {
            assert_eq!(check_group(bad), Err(GroupError::EmptyName), "{bad:?}");
        }
        for bad in [" Filter", "Filter ", "Filter / Envelope", " "] {
            assert_eq!(check_group(bad), Err(GroupError::UntrimmedName), "{bad:?}");
        }
        for bad in ["Fil\nter", "Filter\0", "Tab\there", "Del\x7f"] {
            assert_eq!(check_group(bad), Err(GroupError::ControlCharacter), "{bad:?}");
        }
        let long = "a".repeat(GROUP_MAX_BYTES);
        assert_eq!(check_group(&long), Ok(()));
        assert_eq!(check_group(&format!("{long}a")), Err(GroupError::TooLong));
    }

    #[test]
    fn a_parameter_without_a_group_is_top_level_and_an_invalid_group_shows_as_top_level() {
        let plain = ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db);
        assert_eq!(plain.group, "");
        assert_eq!(plain.host_group(), "");
        assert_eq!(plain.group_names().count(), 0);
        let nested = plain.clone().with_group("Filter/Envelope");
        assert_eq!(nested.host_group(), "Filter/Envelope");
        assert_eq!(nested.group_names().collect::<Vec<_>>(), ["Filter", "Envelope"]);
        let bad = plain.clone().with_group("Filter//Envelope");
        assert_eq!(bad.group, "Filter//Envelope");
        assert_eq!(bad.host_group(), "");
        assert_eq!(bad.group_names().count(), 0);
        assert!(groups_valid(&[plain.clone(), nested.clone()]));
        assert!(!groups_valid(&[plain, bad]));
    }

    #[test]
    fn group_paths_list_parents_first_in_declared_order() {
        let p = |group| ParamInfo::new("p", "P", 0.0, 1.0, 0.0, Unit::None).with_group(group);
        let params = [p("Osc/Pitch/Env"), p(""), p("Filter"), p("Osc"), p("Osc/Amp"), p("Bad//Path"), p("Filter/Env")];
        assert_eq!(group_paths(&params), ["Osc", "Osc/Pitch", "Osc/Pitch/Env", "Filter", "Osc/Amp", "Filter/Env"]);
        assert!(group_paths(&[p(""), p("x//y")]).is_empty());
        assert_eq!(group_parent("Osc/Pitch/Env"), "Osc/Pitch");
        assert_eq!(group_parent("Osc"), "");
        assert_eq!(group_name("Osc/Pitch/Env"), "Env");
        assert_eq!(group_name("Osc"), "Osc");
    }

    #[test]
    fn an_ungrouped_parameter_serializes_as_before() {
        let plain = ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db);
        let text = serde_json::to_string(&plain).unwrap();
        assert!(!text.contains("group"), "{text}");
        let grouped = serde_json::to_string(&plain.with_group("Out")).unwrap();
        assert!(grouped.contains("\"group\":\"Out\""), "{grouped}");
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
