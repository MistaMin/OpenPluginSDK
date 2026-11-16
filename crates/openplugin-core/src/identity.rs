//! Identity pins: the identifiers a host stores for a plugin and its
//! parameters, set explicitly.
//!
//! A host finds a plugin in a saved project by its class or component ids, and
//! finds an automation lane's parameter by the parameter's numeric id (CLAP
//! `param_id`, VST3 `ParamID`, `AudioUnitParameterID`) or its AAX id string.
//! Without pins the SDK derives all of them: the class ids from
//! [`PluginInfo::id`](crate::PluginInfo), and every parameter id from the
//! parameter's position in the table. A pin replaces a derived id with a fixed
//! one, so a port of a plugin from another SDK is the same plugin to a host,
//! and a parameter table can be reordered or extended without breaking saved
//! projects.
//!
//! - Plugin ids: [`Plugin::clap_id`](crate::Plugin::clap_id),
//!   [`Plugin::vst3_ids`](crate::Plugin::vst3_ids),
//!   [`Plugin::au_ids`](crate::Plugin::au_ids),
//!   [`Plugin::aax_ids`](crate::Plugin::aax_ids),
//!   [`Plugin::lv2_uri`](crate::Plugin::lv2_uri).
//! - Parameter ids: [`ParamInfo::with_host_id`](crate::ParamInfo::with_host_id)
//!   and the per-format forms, kept in [`ParamPins`].
//!
//! An id set that a format cannot carry (two parameters with one id, an id the
//! format reserves) is ignored as a whole for that format, which then uses the
//! derived ids. [`check_pins`] reports such a set, so a plugin's own tests can
//! refuse it.
//!
//! A table with any pin is saved by parameter id instead of by position
//! ([`state_is_keyed`], `OPST` version 2 in [`crate::state`]).

use crate::{ParamInfo, Plugin};

/// The pinned identifiers of one parameter. `None` leaves the format's id
/// derived: the parameter's position in the table for the numeric ids, and
/// [`ParamInfo::id`] for AAX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct ParamPins {
    /// CLAP `clap_param_info.id`.
    pub clap: Option<u32>,
    /// VST3 `ParamID`.
    pub vst3: Option<u32>,
    /// `AudioUnitParameterID`.
    pub au: Option<u32>,
    /// AAX parameter id (`AAX_CParamID`): 1 to 31 printable ASCII characters.
    pub aax: Option<&'static str>,
}

impl ParamPins {
    /// No pin: every id is derived.
    pub const NONE: Self = Self { clap: None, vst3: None, au: None, aax: None };

    /// True when no format is pinned.
    pub const fn is_empty(&self) -> bool {
        self.clap.is_none() && self.vst3.is_none() && self.au.is_none() && self.aax.is_none()
    }

    /// The pin for `format`, if any.
    pub const fn numeric(&self, format: PinFormat) -> Option<u32> {
        match format {
            PinFormat::Clap => self.clap,
            PinFormat::Vst3 => self.vst3,
            PinFormat::Au => self.au,
        }
    }
}

/// The formats whose hosts store a number per parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PinFormat {
    Clap,
    Vst3,
    Au,
}

impl PinFormat {
    pub const ALL: [Self; 3] = [Self::Clap, Self::Vst3, Self::Au];

    /// `CLAP`, `VST3` or `Audio Unit`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Clap => "CLAP",
            Self::Vst3 => "VST3",
            Self::Au => "Audio Unit",
        }
    }
}

/// CLAP's `CLAP_INVALID_ID`, which no parameter may use.
pub const CLAP_INVALID_PARAM_ID: u32 = u32::MAX;
/// The largest VST3 `ParamID` a plugin may use (the SDK keeps the upper half
/// of the range for hosts).
pub const VST3_MAX_PARAM_ID: u32 = 0x7FFF_FFFF;
/// The VST3 shell's own bypass parameter.
pub const VST3_BYPASS_PARAM_ID: u32 = 0x4259_5053;
/// The VST3 shell's hidden MIDI controller parameters start here.
pub const VST3_MIDI_PARAM_BASE: u32 = 0x4D49_4400;
/// How many ids from [`VST3_MIDI_PARAM_BASE`] the VST3 shell uses (16 channels
/// of 128 controllers, channel pressure and pitch bend).
pub const VST3_MIDI_PARAM_COUNT: u32 = 16 * 130;
/// The longest AAX parameter id, in bytes.
pub const AAX_PARAM_ID_MAX: usize = 31;
/// The id of the AAX shell's own bypass parameter (the AAX SDK's
/// `cDefaultMasterBypassID`).
pub const AAX_BYPASS_PARAM_ID: &str = "MasterBypassID";

/// Why a set of pins cannot be used. The index is the parameter's position in
/// the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinError {
    /// Two parameters have one numeric id in `format`.
    Duplicate { format: PinFormat, first: usize, second: usize, id: u32 },
    /// The id is one the format or the SDK's shell keeps for itself.
    Reserved { format: PinFormat, index: usize, id: u32 },
    /// Two parameters have one AAX id.
    AaxDuplicate { first: usize, second: usize },
    /// The AAX pin is empty, longer than [`AAX_PARAM_ID_MAX`], not printable
    /// ASCII, or the bypass parameter's id.
    AaxInvalid { index: usize },
    /// [`Plugin::vst3_ids`] is all zero, or names one id for both classes.
    Vst3Class,
    /// [`Plugin::au_ids`] has a code that is not four printable characters.
    AuCode,
    /// [`Plugin::aax_ids`] fails [`AaxIds::is_valid`](crate::AaxIds::is_valid).
    AaxCode,
    /// [`Plugin::clap_id`] is empty or holds a NUL byte.
    ClapId,
    /// A table with pins is saved by [`ParamInfo::id`], which must then be
    /// unique and not empty; this parameter's is neither.
    StateKey { index: usize },
}

impl std::fmt::Display for PinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate { format, first, second, id } => {
                write!(f, "{}: parameters {first} and {second} both have id {id}", format.name())
            }
            Self::Reserved { format, index, id } => write!(f, "{}: parameter {index} has the reserved id {id}", format.name()),
            Self::AaxDuplicate { first, second } => write!(f, "AAX: parameters {first} and {second} have one id"),
            Self::AaxInvalid { index } => write!(f, "AAX: the id pinned for parameter {index} is not 1 to {AAX_PARAM_ID_MAX} printable characters, or is the bypass id"),
            Self::Vst3Class => f.write_str("VST3: a pinned class id is all zero, or both classes have one id"),
            Self::AuCode => f.write_str("Audio Unit: a pinned code is not four printable characters"),
            Self::AaxCode => f.write_str("AAX: the pinned codes are not four printable characters each, or mono and stereo are one code"),
            Self::ClapId => f.write_str("CLAP: the pinned plugin id is empty or holds a NUL"),
            Self::StateKey { index } => write!(f, "state: the string id of parameter {index} is empty or used twice, so the pinned table cannot be saved by id"),
        }
    }
}

impl std::error::Error for PinError {}

const fn reserved(format: PinFormat, id: u32) -> bool {
    match format {
        PinFormat::Clap => id == CLAP_INVALID_PARAM_ID,
        PinFormat::Vst3 => {
            id > VST3_MAX_PARAM_ID
                || id == VST3_BYPASS_PARAM_ID
                || (id >= VST3_MIDI_PARAM_BASE && id - VST3_MIDI_PARAM_BASE < VST3_MIDI_PARAM_COUNT)
        }
        // kAudioUnitParameterID has no reserved value; all ones is avoided as the "no id" of this module.
        PinFormat::Au => id == u32::MAX,
    }
}

/// True when any parameter of the table has a pin in any format.
pub fn params_pinned(params: &[ParamInfo]) -> bool {
    params.iter().any(|p| !p.pins.is_empty())
}

/// True when the table has a pin for `format`.
fn format_pinned(params: &[ParamInfo], format: PinFormat) -> bool {
    params.iter().any(|p| p.pins.numeric(format).is_some())
}

/// The id of every parameter in `format`, in table order: the pin, or the
/// position for a parameter without one. An error names the first id that
/// cannot be used. A table without pins for `format` is always `Ok` and gives
/// `0..n`: the derived ids are not checked against the reserved ones (they
/// were not before pins existed).
pub fn param_host_ids(params: &[ParamInfo], format: PinFormat) -> Result<Vec<u32>, PinError> {
    let ids: Vec<u32> = params
        .iter()
        .enumerate()
        .map(|(index, p)| p.pins.numeric(format).unwrap_or(index.min(u32::MAX as usize) as u32))
        .collect();
    if !format_pinned(params, format) {
        return Ok(ids);
    }
    if let Some(index) = ids.iter().position(|&id| reserved(format, id)) {
        return Err(PinError::Reserved { format, index, id: ids[index] });
    }
    let mut sorted: Vec<(u32, usize)> = ids.iter().copied().zip(0..).collect();
    sorted.sort_unstable();
    for pair in sorted.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(PinError::Duplicate { format, first: pair[0].1, second: pair[1].1, id: pair[0].0 });
        }
    }
    Ok(ids)
}

fn aax_pin_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= AAX_PARAM_ID_MAX && id.bytes().all(|b| (0x20..0x7f).contains(&b)) && id != AAX_BYPASS_PARAM_ID
}

/// The AAX id of every parameter, in table order: the pin, or
/// [`ParamInfo::id`]. A table without AAX pins is always `Ok` and gives the
/// string ids unchecked, as before pins existed.
pub fn param_aax_ids(params: &[ParamInfo]) -> Result<Vec<&'static str>, PinError> {
    let ids: Vec<&'static str> = params.iter().map(|p| p.pins.aax.unwrap_or(p.id)).collect();
    if params.iter().all(|p| p.pins.aax.is_none()) {
        return Ok(ids);
    }
    for (index, p) in params.iter().enumerate() {
        if p.pins.aax.is_some_and(|id| !aax_pin_ok(id)) {
            return Err(PinError::AaxInvalid { index });
        }
    }
    for (second, id) in ids.iter().enumerate() {
        if let Some(first) = ids[..second].iter().position(|earlier| earlier == id) {
            return Err(PinError::AaxDuplicate { first, second });
        }
    }
    Ok(ids)
}

/// Whether the table's values are saved by [`ParamInfo::id`] (`OPST` version
/// 2) instead of by position (version 1): true when a parameter has a pin and
/// every string id is unique and not empty. A table without pins keeps the
/// positional snapshot, byte for byte.
pub fn state_is_keyed(params: &[ParamInfo]) -> bool {
    params_pinned(params) && state_key_error(params).is_none()
}

fn state_key_error(params: &[ParamInfo]) -> Option<PinError> {
    for (index, p) in params.iter().enumerate() {
        let bad = p.id.is_empty() || p.id.len() > u16::MAX as usize || params[..index].iter().any(|earlier| earlier.id == p.id);
        if bad {
            return Some(PinError::StateKey { index });
        }
    }
    None
}

/// Numeric parameter ids of one format, both ways: the id of a table position
/// and the position of an id. Built once (it allocates); [`Self::index`] and
/// [`Self::id`] allocate nothing and are safe on the audio thread.
///
/// A pin set the format cannot carry ([`param_host_ids`] fails) gives the
/// identity map, as does a table without pins.
#[derive(Debug, Clone)]
pub struct ParamIdMap {
    /// `ids[index]`; empty for the identity map.
    ids: Box<[u32]>,
    /// `(id, index)` by id; empty for the identity map.
    sorted: Box<[(u32, u32)]>,
    count: usize,
}

impl ParamIdMap {
    pub fn new(params: &[ParamInfo], format: PinFormat) -> Self {
        match param_host_ids(params, format) {
            Ok(ids) if format_pinned(params, format) => {
                let mut sorted: Vec<(u32, u32)> = ids.iter().copied().zip(0u32..).collect();
                sorted.sort_unstable();
                Self { ids: ids.into_boxed_slice(), sorted: sorted.into_boxed_slice(), count: params.len() }
            }
            _ => Self::identity(params.len()),
        }
    }

    /// Ids `0..count`: what every format used before pins.
    pub fn identity(count: usize) -> Self {
        Self { ids: Box::new([]), sorted: Box::new([]), count }
    }

    /// True when every id is the position.
    pub fn is_identity(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The id of the parameter at `index`.
    pub fn id(&self, index: usize) -> Option<u32> {
        if index >= self.count {
            None
        } else if self.is_identity() {
            Some(index as u32)
        } else {
            Some(self.ids[index])
        }
    }

    /// The position of the parameter the host calls `id`.
    pub fn index(&self, id: u32) -> Option<usize> {
        if self.is_identity() {
            return ((id as usize) < self.count).then_some(id as usize);
        }
        self.sorted.binary_search_by_key(&id, |entry| entry.0).ok().map(|at| self.sorted[at].1 as usize)
    }
}

/// The two VST3 class ids a host stores: the audio processor's (the id a
/// project names the plugin by) and the edit controller's. Each is the 16
/// bytes of the class id in the order its 32 hex digits are written
/// (`FUID::toString`, the `CID` of `moduleinfo.json`, `openplugin-port scan`'s
/// `class_id`), the same on every system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vst3Ids {
    pub processor: [u8; 16],
    /// `None` keeps the controller id derived from the plugin id, for a port
    /// of a single-component plugin, which had no controller class.
    pub controller: Option<[u8; 16]>,
}

impl Vst3Ids {
    /// Both ids from their 32 hex digits, as a scan prints them. `None` when
    /// either text is not 32 hex digits.
    pub const fn from_hex(processor: &str, controller: &str) -> Option<Self> {
        match (hex16(processor), hex16(controller)) {
            (Some(processor), Some(controller)) => Some(Self { processor, controller: Some(controller) }),
            _ => None,
        }
    }

    /// The processor id alone, from its 32 hex digits.
    pub const fn processor_from_hex(processor: &str) -> Option<Self> {
        match hex16(processor) {
            Some(processor) => Some(Self { processor, controller: None }),
            None => None,
        }
    }

    /// True when no id is all zero and the two differ.
    pub fn is_valid(&self) -> bool {
        self.processor != [0; 16] && self.controller != Some([0; 16]) && self.controller != Some(self.processor)
    }
}

const fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 32 hex digits as 16 bytes.
const fn hex16(text: &str) -> Option<[u8; 16]> {
    let bytes = text.as_bytes();
    if bytes.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    let mut i = 0;
    while i < 16 {
        let (Some(high), Some(low)) = (hex_digit(bytes[2 * i]), hex_digit(bytes[2 * i + 1])) else {
            return None;
        };
        out[i] = high << 4 | low;
        i += 1;
    }
    Some(out)
}

/// The Audio Unit subtype and manufacturer codes, four printable ASCII
/// characters each, such as `*b"ExGn"`. The component type follows the
/// plugin's kind (or the build's `OP_AU_TYPE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuIds {
    pub subtype: [u8; 4],
    pub manufacturer: [u8; 4],
}

impl AuIds {
    /// True when both codes are four printable characters.
    pub fn is_valid(&self) -> bool {
        [self.subtype, self.manufacturer].iter().all(|code| code.iter().all(|b| (0x21..0x7f).contains(b)))
    }
}

/// True when `id` can be a CLAP plugin id: not empty, no NUL.
pub fn clap_id_valid(id: &str) -> bool {
    !id.is_empty() && !id.bytes().any(|b| b == 0)
}

/// Every pin of `plugin` that cannot be used, and would therefore be ignored
/// in favour of the derived id. Empty when all pins are usable (and for a
/// plugin without pins). Call it from the plugin's own tests:
///
/// ```ignore
/// assert_eq!(openplugin::core::identity::check_pins(&MyPlugin::default()), []);
/// ```
pub fn check_pins(plugin: &dyn Plugin) -> Vec<PinError> {
    let params = plugin.info().params;
    let mut errors = Vec::new();
    for format in PinFormat::ALL {
        if let Err(error) = param_host_ids(params, format) {
            errors.push(error);
        }
    }
    if let Err(error) = param_aax_ids(params) {
        errors.push(error);
    }
    if params_pinned(params) {
        errors.extend(state_key_error(params));
    }
    if plugin.clap_id().is_some_and(|id| !clap_id_valid(id)) {
        errors.push(PinError::ClapId);
    }
    if plugin.vst3_ids().is_some_and(|ids| !ids.is_valid()) {
        errors.push(PinError::Vst3Class);
    }
    if plugin.au_ids().is_some_and(|ids| !ids.is_valid()) {
        errors.push(PinError::AuCode);
    }
    if plugin.aax_ids().is_some_and(|ids| !ids.is_valid()) {
        errors.push(PinError::AaxCode);
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Unit;

    fn p(id: &'static str) -> ParamInfo {
        ParamInfo::new(id, id, 0.0, 1.0, 0.0, Unit::None)
    }

    #[test]
    fn a_table_without_pins_has_its_positions_as_ids_in_every_format() {
        let params = [p("a"), p("b"), p("c")];
        assert!(!params_pinned(&params));
        assert!(!state_is_keyed(&params));
        for format in PinFormat::ALL {
            assert_eq!(param_host_ids(&params, format), Ok(vec![0, 1, 2]));
            let map = ParamIdMap::new(&params, format);
            assert!(map.is_identity());
            assert_eq!((map.id(0), map.id(2), map.id(3)), (Some(0), Some(2), None));
            assert_eq!((map.index(0), map.index(2), map.index(3), map.index(u32::MAX)), (Some(0), Some(2), None, None));
        }
        assert_eq!(param_aax_ids(&params), Ok(vec!["a", "b", "c"]));
    }

    #[test]
    fn pins_replace_positions_and_follow_the_parameter_through_a_reorder() {
        let first = [p("gain").with_host_id(100), p("mix").with_host_id(7), p("mode").with_host_id(2_000_000)];
        let second = [first[2].clone(), first[0].clone(), first[1].clone()];
        for format in PinFormat::ALL {
            let (a, b) = (ParamIdMap::new(&first, format), ParamIdMap::new(&second, format));
            assert!(!a.is_identity());
            for (index, param) in first.iter().enumerate() {
                let id = a.id(index).unwrap();
                assert_eq!(Some(id), param.pins.numeric(format));
                assert_eq!(a.index(id), Some(index));
                let moved = b.index(id).unwrap();
                assert_eq!(second[moved].id, param.id);
            }
            assert_eq!(a.index(0), None);
            assert_eq!(a.index(8), None);
        }
        assert!(state_is_keyed(&first));
    }

    #[test]
    fn one_format_can_be_pinned_alone() {
        let params = [p("a").with_vst3_id(40), p("b").with_vst3_id(41)];
        assert!(ParamIdMap::new(&params, PinFormat::Clap).is_identity());
        assert!(ParamIdMap::new(&params, PinFormat::Au).is_identity());
        assert_eq!(param_host_ids(&params, PinFormat::Vst3), Ok(vec![40, 41]));
        let aax = [p("a").with_aax_id("Gain"), p("b")];
        assert_eq!(param_aax_ids(&aax), Ok(vec!["Gain", "b"]));
        assert!(state_is_keyed(&aax));
    }

    #[test]
    fn a_parameter_without_a_pin_in_a_pinned_table_keeps_its_position() {
        let params = [p("a").with_host_id(9), p("b")];
        assert_eq!(param_host_ids(&params, PinFormat::Clap), Ok(vec![9, 1]));
        let clash = [p("a").with_host_id(1), p("b")];
        assert_eq!(
            param_host_ids(&clash, PinFormat::Clap),
            Err(PinError::Duplicate { format: PinFormat::Clap, first: 0, second: 1, id: 1 })
        );
        assert!(ParamIdMap::new(&clash, PinFormat::Clap).is_identity());
    }

    #[test]
    fn reserved_and_duplicate_ids_are_refused_and_fall_back_to_positions() {
        let invalid = [p("a").with_clap_id(CLAP_INVALID_PARAM_ID), p("b")];
        assert_eq!(
            param_host_ids(&invalid, PinFormat::Clap),
            Err(PinError::Reserved { format: PinFormat::Clap, index: 0, id: u32::MAX })
        );
        for id in [VST3_BYPASS_PARAM_ID, VST3_MIDI_PARAM_BASE, VST3_MIDI_PARAM_BASE + VST3_MIDI_PARAM_COUNT - 1, VST3_MAX_PARAM_ID + 1] {
            let params = [p("a").with_vst3_id(id)];
            assert_eq!(param_host_ids(&params, PinFormat::Vst3), Err(PinError::Reserved { format: PinFormat::Vst3, index: 0, id }));
            assert!(ParamIdMap::new(&params, PinFormat::Vst3).is_identity());
        }
        for id in [VST3_MIDI_PARAM_BASE - 1, VST3_MIDI_PARAM_BASE + VST3_MIDI_PARAM_COUNT, VST3_MAX_PARAM_ID] {
            assert!(param_host_ids(&[p("a").with_vst3_id(id)], PinFormat::Vst3).is_ok());
        }
        let twice = [p("a").with_host_id(5), p("b").with_host_id(5)];
        assert!(matches!(param_host_ids(&twice, PinFormat::Au), Err(PinError::Duplicate { first: 0, second: 1, id: 5, .. })));
    }

    #[test]
    fn aax_pins_are_checked() {
        let long = "a".repeat(AAX_PARAM_ID_MAX + 1);
        let long: &'static str = Box::leak(long.into_boxed_str());
        for bad in ["", long, "Gain\n", AAX_BYPASS_PARAM_ID, "Größe"] {
            assert_eq!(param_aax_ids(&[p("a").with_aax_id(bad)]), Err(PinError::AaxInvalid { index: 0 }), "{bad:?}");
        }
        assert_eq!(param_aax_ids(&[p("a").with_aax_id("b"), p("b")]), Err(PinError::AaxDuplicate { first: 0, second: 1 }));
        assert!(param_aax_ids(&[p("a").with_aax_id(&long[..AAX_PARAM_ID_MAX])]).is_ok());
    }

    #[test]
    fn a_pinned_table_with_a_repeated_string_id_is_not_saved_by_id() {
        let params = [p("a").with_host_id(1), p("a").with_host_id(2)];
        assert!(params_pinned(&params));
        assert!(!state_is_keyed(&params));
        assert_eq!(state_key_error(&params), Some(PinError::StateKey { index: 1 }));
        assert!(!state_is_keyed(&[p("").with_host_id(1)]));
    }

    #[test]
    fn vst3_ids_read_the_hex_a_scan_prints() {
        let ids = Vst3Ids::from_hex("34D4F20CA96548A2CCF8B6568B816BBE", "4f46ab32f25718ae0303babe150a0bdf").unwrap();
        assert_eq!(ids.processor[..4], [0x34, 0xD4, 0xF2, 0x0C]);
        assert_eq!(ids.controller.unwrap()[12..], [0x15, 0x0A, 0x0B, 0xDF]);
        assert!(ids.is_valid());
        assert!(Vst3Ids::from_hex("34D4", "4F46").is_none());
        assert!(Vst3Ids::from_hex("34D4F20CA96548A2CCF8B6568B816BBG", "4F46AB32F25718AE0303BABE150A0BDF").is_none());
        let single = Vst3Ids::processor_from_hex("34D4F20CA96548A2CCF8B6568B816BBE").unwrap();
        assert!(single.is_valid() && single.controller.is_none());
        assert!(!Vst3Ids { processor: [0; 16], controller: None }.is_valid());
        assert!(!Vst3Ids { processor: [1; 16], controller: Some([1; 16]) }.is_valid());
        assert!(!Vst3Ids { processor: [1; 16], controller: Some([0; 16]) }.is_valid());
        const PINNED: Option<Vst3Ids> = Vst3Ids::from_hex("000102030405060708090A0B0C0D0E0F", "101112131415161718191A1B1C1D1E1F");
        assert_eq!(PINNED.unwrap().processor[15], 0x0F);
    }

    #[test]
    fn au_codes_must_be_printable() {
        assert!(AuIds { subtype: *b"ExGn", manufacturer: *b"OpGd" }.is_valid());
        assert!(!AuIds { subtype: [0, 1, 2, 3], manufacturer: *b"OpGd" }.is_valid());
        assert!(!AuIds { subtype: *b"Ex n", manufacturer: *b"OpGd" }.is_valid());
    }
}
