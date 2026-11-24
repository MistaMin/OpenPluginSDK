//! Per-note expression and polyphonic modulation.
//!
//! A host addresses one sounding note in two ways: by the note id it gave at
//! note-on (CLAP `note_id`, VST3 `noteId`), or by the port, channel and key the
//! note was played on. [`NoteRef`] carries all four, with `-1` as "any", the
//! CLAP wildcard rule (`clap/events.h`, "Note on, off, end and choke events"):
//! a field of `-1` matches every voice, any other value must be equal.
//!
//! [`Expression`] names the seven per-note expressions the formats share (the
//! CLAP note expression ids `CLAP_NOTE_EXPRESSION_VOLUME` to `_PRESSURE`, which
//! are also the VST3 predefined note expression types plus pressure), with the
//! CLAP value ranges: volume is a linear gain `0..=4` (1 is unity), pan `0`
//! left to `1` right, tuning in semitones `-120..=120`, the rest `0..=1`.
//!
//! A plugin opts in with [`Plugin::note_expressions`](crate::Plugin::note_expressions)
//! (and [`Plugin::mpe_input`](crate::Plugin::mpe_input) for MPE over MIDI, see
//! [`crate::mpe`]) and receives [`Plugin::note_expression`](crate::Plugin::note_expression).
//! A parameter opts in to modulation with [`ParamFlags::MODULATABLE`](crate::ParamFlags::MODULATABLE)
//! or, per note, [`ParamFlags::POLY_MODULATION`](crate::ParamFlags::POLY_MODULATION) and
//! receives [`Plugin::param_modulation`](crate::Plugin::param_modulation).
//! `openplugin_dsp::voice_alloc` routes both to the matching voices.
//!
//! Nothing here allocates except [`Expression::format`].

/// Which note an event is for. `-1` in any field is a wildcard (any value).
///
/// At note-on the port, channel and key are concrete (0 or more) and the id is
/// the host's note id, or `-1` when the host gives none (MIDI, the Audio Unit,
/// AAX, LV2). An expression or modulation names the notes it is for the same
/// way; see [`NoteRef::matches`].
///
/// ```
/// use openplugin_core::NoteRef;
/// let played = NoteRef::new(17, 0, 3, 60);
/// assert!(NoteRef::ALL.matches(played));
/// assert!(NoteRef::midi(3, 60).matches(played));
/// assert!(!NoteRef::key(61).matches(played));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NoteRef {
    /// Host note id, 0 or more, or -1.
    pub id: i32,
    /// Note port, 0 or more, or -1.
    pub port: i16,
    /// MIDI channel 0..=15, or -1.
    pub channel: i16,
    /// MIDI key 0..=127, or -1.
    pub key: i16,
}

impl NoteRef {
    /// Every note (all four fields are wildcards): a monophonic or global event.
    pub const ALL: Self = Self { id: -1, port: -1, channel: -1, key: -1 };

    /// A note from all four fields, unchecked: `-1` is a wildcard, and any
    /// other negative value matches nothing but itself.
    pub const fn new(id: i32, port: i16, channel: i16, key: i16) -> Self {
        Self { id, port, channel, key }
    }

    /// A note known by its key alone (a plain `note_on`): any port, any channel, no id.
    pub const fn key(key: u8) -> Self {
        Self { id: -1, port: -1, channel: -1, key: key as i16 }
    }

    /// A MIDI note: port 0, `channel`, `key`, no id.
    pub const fn midi(channel: u8, key: u8) -> Self {
        Self { id: -1, port: 0, channel: channel as i16, key: key as i16 }
    }

    /// The same note with the host's note id.
    pub const fn with_id(mut self, id: i32) -> Self {
        self.id = if id < 0 { -1 } else { id };
        self
    }

    /// Every note sounding on `channel` (port 0).
    pub const fn on_channel(channel: u8) -> Self {
        Self { id: -1, port: 0, channel: channel as i16, key: -1 }
    }

    /// The fields a host sent, checked: `None` when a field is below -1, the
    /// channel above 15 or the key above 127 (no note can have them). A note id
    /// below -1 is refused too, so a hostile id is never taken as a wildcard.
    pub const fn checked(id: i32, port: i16, channel: i16, key: i16) -> Option<Self> {
        if id < -1 || port < -1 || channel < -1 || channel > 15 || key < -1 || key > 127 {
            None
        } else {
            Some(Self { id, port, channel, key })
        }
    }

    /// Whether this target names the voice playing `voice` (the [`NoteRef`]
    /// of its note-on).
    ///
    /// - A target with an id (0 or more) and a voice with an id match when the
    ///   ids are equal, whatever the other fields say.
    /// - Otherwise (no id in the target, the fallback for a host that sends
    ///   none) port, channel and key decide: a `-1` in the target matches any
    ///   value, as does a `-1` in the voice (a voice started without a channel
    ///   matches every channel). A target with an id but a voice without one
    ///   matches only when the target also names the key, so an id-only event
    ///   never reaches voices that were not started with ids.
    pub const fn matches(self, voice: NoteRef) -> bool {
        if self.id >= 0 {
            if voice.id >= 0 {
                return self.id == voice.id;
            }
            if self.key < 0 {
                return false;
            }
        }
        field(self.port, voice.port) && field(self.channel, voice.channel) && field(self.key, voice.key)
    }

    /// No id, port, channel or key: matches every note.
    pub const fn is_all(self) -> bool {
        self.id < 0 && self.port < 0 && self.channel < 0 && self.key < 0
    }
}

const fn field(target: i16, voice: i16) -> bool {
    target < 0 || voice < 0 || target == voice
}

/// One per-note expression. The discriminants are the CLAP note expression
/// ids (`clap/events.h`), also used by the shell ABI (`OP_EXPRESSION_*`).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Expression {
    /// Linear gain, `0..=4`; 1 is unity (CLAP: `plain = 20 * log(x)` dB, VST3 `kVolumeTypeID`).
    Volume = 0,
    /// `0` left, `0.5` centre, `1` right.
    Pan = 1,
    /// Relative tuning in equal-tempered semitones, `-120..=120`.
    Tuning = 2,
    /// `0..=1`.
    Vibrato = 3,
    /// `0..=1`.
    Expression = 4,
    /// `0..=1`; MPE's timbre (CC 74).
    Brightness = 5,
    /// `0..=1`; MPE's channel pressure.
    Pressure = 6,
}

impl Expression {
    /// Every expression, in id order.
    pub const ALL: [Expression; 7] = [
        Expression::Volume,
        Expression::Pan,
        Expression::Tuning,
        Expression::Vibrato,
        Expression::Expression,
        Expression::Brightness,
        Expression::Pressure,
    ];
    /// Number of expressions ([`Self::ALL`]`.len()`).
    pub const COUNT: usize = 7;

    /// The expression with CLAP id `id`; `None` for an id the SDK does not know.
    pub const fn from_id(id: i64) -> Option<Self> {
        match id {
            0 => Some(Self::Volume),
            1 => Some(Self::Pan),
            2 => Some(Self::Tuning),
            3 => Some(Self::Vibrato),
            4 => Some(Self::Expression),
            5 => Some(Self::Brightness),
            6 => Some(Self::Pressure),
            _ => None,
        }
    }

    /// The CLAP id, also the index into per-voice tables.
    pub const fn id(self) -> usize {
        self as usize
    }

    /// Lowest and highest value.
    pub const fn range(self) -> (f32, f32) {
        match self {
            Self::Volume => (0.0, 4.0),
            Self::Tuning => (-120.0, 120.0),
            _ => (0.0, 1.0),
        }
    }

    /// The value of a note no expression has touched: unity gain, centre pan,
    /// no detune, zero for the rest.
    pub const fn neutral(self) -> f32 {
        match self {
            Self::Volume => 1.0,
            Self::Pan => 0.5,
            _ => 0.0,
        }
    }

    /// `value` clamped to [`Self::range`]; `None` for NaN or an infinity, so
    /// a hostile value never reaches a voice.
    pub fn sanitize(self, value: f64) -> Option<f32> {
        if !value.is_finite() {
            return None;
        }
        let (lo, hi) = self.range();
        Some((value as f32).clamp(lo, hi))
    }

    /// Display name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Volume => "Volume",
            Self::Pan => "Pan",
            Self::Tuning => "Tuning",
            Self::Vibrato => "Vibrato",
            Self::Expression => "Expression",
            Self::Brightness => "Brightness",
            Self::Pressure => "Pressure",
        }
    }

    /// Short display name, at most 4 bytes.
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::Volume => "Vol",
            Self::Pan => "Pan",
            Self::Tuning => "Tune",
            Self::Vibrato => "Vib",
            Self::Expression => "Expr",
            Self::Brightness => "Brt",
            Self::Pressure => "Prs",
        }
    }

    /// Unit label of the text [`Self::format`] writes.
    pub const fn unit(self) -> &'static str {
        match self {
            Self::Volume => "dB",
            Self::Pan => "",
            Self::Tuning => "st",
            _ => "%",
        }
    }

    /// Centred on a middle value (pan, tuning) rather than starting at zero.
    pub const fn is_bipolar(self) -> bool {
        matches!(self, Self::Pan | Self::Tuning)
    }

    /// Display text: volume in dB (`-inf dB` at 0), pan as `C`, `L50` or
    /// `R100` (percent towards that side), tuning as `+1.00 st`, the rest as a
    /// percentage. Out-of-range values are clamped; NaN shows the neutral value.
    pub fn format(self, value: f32) -> String {
        let (lo, hi) = self.range();
        let v = if value.is_finite() { value.clamp(lo, hi) } else { self.neutral() };
        match self {
            Self::Volume => {
                if v <= 1.0e-5 {
                    "-inf dB".to_string()
                } else {
                    format!("{:.1} dB", 20.0 * v.log10())
                }
            }
            Self::Pan => {
                let side = ((v - 0.5) * 200.0).round();
                if side == 0.0 {
                    "C".to_string()
                } else if side < 0.0 {
                    format!("L{:.0}", -side)
                } else {
                    format!("R{side:.0}")
                }
            }
            Self::Tuning => format!("{v:+.2} st"),
            _ => format!("{:.0}%", v * 100.0),
        }
    }

    /// Reads text in the form [`Self::format`] writes, clamped to the range.
    /// Volume takes dB (`-6`, `-6 dB`, `-inf`); pan `C`, `L50`, `R100` or a
    /// number from 0 to 1; tuning semitones (`7`, `+7 st`) or cents (`50 ct`);
    /// the rest a percentage with or without `%`. `None` when the text is not
    /// a value.
    pub fn parse(self, text: &str) -> Option<f32> {
        let t = text.trim();
        if t.is_empty() || t.len() > 64 {
            return None;
        }
        let lower = t.to_ascii_lowercase();
        let (lo, hi) = self.range();
        let value = match self {
            Self::Volume => {
                if lower.starts_with("-inf") {
                    0.0
                } else {
                    let db = number(lower.trim_end_matches("db").trim())?;
                    10f32.powf(db / 20.0)
                }
            }
            Self::Pan => {
                if lower == "c" || lower == "center" || lower == "centre" {
                    0.5
                } else if let Some(rest) = lower.strip_prefix('l') {
                    0.5 - number(rest.trim().trim_end_matches('%').trim())? / 200.0
                } else if let Some(rest) = lower.strip_prefix('r') {
                    0.5 + number(rest.trim().trim_end_matches('%').trim())? / 200.0
                } else {
                    number(&lower)?
                }
            }
            Self::Tuning => {
                if let Some(cents) = lower.strip_suffix("ct") {
                    number(cents.trim())? / 100.0
                } else {
                    number(lower.trim_end_matches("st").trim())?
                }
            }
            _ => number(lower.trim_end_matches('%').trim())? / 100.0,
        };
        value.is_finite().then(|| value.clamp(lo, hi))
    }
}

fn number(text: &str) -> Option<f32> {
    let value: f32 = text.parse().ok()?;
    value.is_finite().then_some(value)
}

/// A set of [`Expression`]s, one bit per CLAP id.
///
/// ```
/// use openplugin_core::{Expression, ExpressionSet};
/// let set = ExpressionSet::NONE.with(Expression::Tuning).with(Expression::Volume);
/// assert!(set.contains(Expression::Tuning) && !set.contains(Expression::Pan));
/// assert_eq!(set.len(), 2);
/// assert_eq!(ExpressionSet::MPE.len(), 3);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ExpressionSet(u8);

impl ExpressionSet {
    /// No expression (the default).
    pub const NONE: Self = Self(0);
    /// All seven expressions.
    pub const ALL: Self = Self(0x7F);
    /// What MPE carries: tuning (pitch bend), pressure and brightness (CC 74).
    pub const MPE: Self = Self(
        (1 << Expression::Tuning as u8) | (1 << Expression::Pressure as u8) | (1 << Expression::Brightness as u8),
    );

    /// The set with `expression` added.
    pub const fn with(self, expression: Expression) -> Self {
        Self(self.0 | (1 << expression as u8))
    }

    /// Whether `expression` is in the set.
    pub const fn contains(self, expression: Expression) -> bool {
        self.0 & (1 << expression as u8) != 0
    }

    /// Whether the set has no member.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Bit `id` set for each expression (the shell ABI's `op_shell_expressions`).
    pub const fn bits(self) -> u32 {
        self.0 as u32
    }

    /// The set of these bits; bits past the known expressions are dropped.
    pub const fn from_bits(bits: u32) -> Self {
        Self((bits & 0x7F) as u8)
    }

    /// The members, in id order.
    pub fn iter(self) -> impl Iterator<Item = Expression> {
        Expression::ALL.into_iter().filter(move |e| self.contains(*e))
    }

    /// Number of members, 0..=7.
    pub fn len(self) -> usize {
        self.0.count_ones() as usize
    }
}

/// What a plugin tells a host about its voices (CLAP `voice-info`). A host
/// reads it on the main thread from [`Plugin::voice_info`](crate::Plugin::voice_info),
/// each time it asks, and uses it to decide how many voices to model (for
/// polyphonic modulation, for instance). Finished voices are reported one by
/// one through [`Plugin::pull_note_end`](crate::Plugin::pull_note_end).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct VoiceInfo {
    /// The voices the plugin plays now (a polyphony setting, for instance).
    pub count: u32,
    /// The most voices it could ever play; at least `count`.
    pub capacity: u32,
    /// Two held notes on the same key and channel with different note ids
    /// sound as two voices (CLAP `CLAP_VOICE_INFO_SUPPORTS_OVERLAPPING_NOTES`).
    pub overlapping_notes: bool,
}

impl VoiceInfo {
    /// `count` voices out of `capacity`, without overlapping notes.
    pub const fn new(count: u32, capacity: u32) -> Self {
        Self { count, capacity, overlapping_notes: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_target_matches_only_its_voice_and_the_fallback_uses_the_tuple() {
        let a = NoteRef::new(7, 0, 0, 60);
        let b = NoteRef::new(8, 0, 0, 60);
        assert!(NoteRef::new(7, -1, -1, -1).matches(a));
        assert!(!NoteRef::new(7, -1, -1, -1).matches(b));
        // The id decides even if the key differs.
        assert!(NoteRef::new(7, 0, 0, 61).matches(a));
        // No id: port, channel, key with wildcards.
        assert!(NoteRef::new(-1, 0, 0, 60).matches(a) && NoteRef::new(-1, 0, 0, 60).matches(b));
        assert!(NoteRef::on_channel(0).matches(a));
        assert!(!NoteRef::on_channel(1).matches(a));
        assert!(NoteRef::ALL.matches(a));
        // An id-only target never reaches a voice started without an id.
        assert!(!NoteRef::new(7, -1, -1, -1).matches(NoteRef::key(60)));
        assert!(NoteRef::new(7, -1, -1, 60).matches(NoteRef::key(60)));
        // A voice without a channel matches any channel.
        assert!(NoteRef::midi(3, 60).matches(NoteRef::key(60)));
    }

    #[test]
    fn hostile_tuples_are_refused() {
        assert!(NoteRef::checked(-2, 0, 0, 60).is_none());
        assert!(NoteRef::checked(1, -5, 0, 60).is_none());
        assert!(NoteRef::checked(1, 0, 16, 60).is_none());
        assert!(NoteRef::checked(1, 0, 0, 128).is_none());
        assert_eq!(NoteRef::checked(-1, -1, -1, -1), Some(NoteRef::ALL));
    }

    #[test]
    fn values_are_clamped_and_nan_is_dropped() {
        assert_eq!(Expression::Tuning.sanitize(500.0), Some(120.0));
        assert_eq!(Expression::Volume.sanitize(-1.0), Some(0.0));
        assert_eq!(Expression::Pressure.sanitize(f64::NAN), None);
        assert_eq!(Expression::Pan.sanitize(f64::INFINITY), None);
        for e in Expression::ALL {
            assert_eq!(Expression::from_id(e.id() as i64), Some(e));
            let (lo, hi) = e.range();
            assert!(lo <= e.neutral() && e.neutral() <= hi);
        }
        assert_eq!(Expression::from_id(7), None);
        assert_eq!(Expression::from_id(-1), None);
    }

    #[test]
    fn text_round_trips() {
        assert_eq!(Expression::Volume.format(1.0), "0.0 dB");
        assert_eq!(Expression::Volume.format(0.0), "-inf dB");
        assert!((Expression::Volume.parse("-6 dB").unwrap() - 0.501).abs() < 1e-3);
        assert_eq!(Expression::Volume.parse("-inf"), Some(0.0));
        assert_eq!(Expression::Pan.format(0.5), "C");
        assert_eq!(Expression::Pan.format(0.25), "L50");
        assert_eq!(Expression::Pan.parse("R100"), Some(1.0));
        assert_eq!(Expression::Pan.parse("L50"), Some(0.25));
        assert_eq!(Expression::Tuning.format(7.0), "+7.00 st");
        assert_eq!(Expression::Tuning.parse("+7.00 st"), Some(7.0));
        assert_eq!(Expression::Tuning.parse("50 ct"), Some(0.5));
        assert_eq!(Expression::Tuning.parse("999"), Some(120.0));
        assert_eq!(Expression::Pressure.format(0.5), "50%");
        assert_eq!(Expression::Pressure.parse("50%"), Some(0.5));
        assert_eq!(Expression::Brightness.parse("nonsense"), None);
        assert_eq!(Expression::Brightness.parse("NaN"), None);
        assert_eq!(Expression::Tuning.format(f32::NAN), "+0.00 st");
    }

    #[test]
    fn sets_hold_bits() {
        let set = ExpressionSet::NONE.with(Expression::Tuning).with(Expression::Pressure);
        assert!(set.contains(Expression::Tuning) && !set.contains(Expression::Volume));
        assert_eq!(set.len(), 2);
        assert_eq!(ExpressionSet::from_bits(0xFFFF_FFFF), ExpressionSet::ALL);
        assert_eq!(ExpressionSet::ALL.iter().count(), 7);
        assert!(ExpressionSet::MPE.contains(Expression::Brightness));
    }
}
