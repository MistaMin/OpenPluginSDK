//! The version-aware import pattern for `Plugin::load_state`.
//!
//! A port lists the formats it can read as [`StateFormat`] values. Each has a
//! `detect` function that looks at the front of the bytes (a magic, a version
//! field, a root element) and says which version of the format it is, and a
//! `decode` function that reads that version into an [`ImportedState`]: the
//! old plugin's parameter ids with the values as stored, plus any opaque
//! chunk. [`import`] tries the formats in order. A [`ParamMap`] then maps the
//! old ids and values to the port's [`ParamInfo`] ids and units.
//!
//! ```
//! use openplugin_core::foreign_state::import::*;
//! use openplugin_core::foreign_state::bytes::Reader;
//! use openplugin_core::{ParamInfo, Unit};
//!
//! // The old plugin wrote: "OLD1", then (u32 id, f32 plain value) pairs to the end.
//! fn detect(data: &[u8]) -> Option<u32> {
//!     data.starts_with(b"OLD1").then_some(1)
//! }
//! fn decode(data: &[u8], _version: u32) -> openplugin_core::foreign_state::Result<ImportedState> {
//!     let mut r = Reader::little(data);
//!     r.skip(4)?;
//!     let mut state = ImportedState::new("old-plugin", 1);
//!     while !r.is_empty() {
//!         let id = r.u32()?;
//!         state.push_plain(ForeignId::Index(id), f64::from(r.f32()?));
//!     }
//!     Ok(state)
//! }
//!
//! static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];
//! let formats = [StateFormat { name: "old-plugin", detect, decode }];
//!
//! let mut blob = b"OLD1".to_vec();
//! blob.extend_from_slice(&7u32.to_le_bytes());
//! blob.extend_from_slice(&(-6.0f32).to_le_bytes());
//!
//! let mut map = ParamMap::new();
//! map.map(ForeignId::Index(7), "gain");
//! let state = import(&formats, &blob).expect("recognised").expect("valid");
//! let applied = map.apply(&PARAMS, &state);
//! assert_eq!(applied.values, vec![(0, -6.0)]);
//! assert!(applied.unmapped.is_empty());
//! ```

use super::Result;
use crate::ParamInfo;

/// How the old plugin names a parameter in its saved state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ForeignId {
    /// A numeric id (the scanner manifest's `params[].id`).
    Index(u32),
    /// A string key (an XML attribute, a plist key, a record key).
    Name(String),
}

impl From<u32> for ForeignId {
    fn from(v: u32) -> Self {
        ForeignId::Index(v)
    }
}

impl From<&str> for ForeignId {
    fn from(v: &str) -> Self {
        ForeignId::Name(v.to_string())
    }
}

/// What a stored value means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValueKind {
    /// In the old plugin's own units.
    Plain,
    /// 0 to 1 across the old parameter's range.
    Normalized,
}

/// One parameter value read from an old state. The module example builds
/// these through [`ImportedState::push_plain`].
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedParam {
    /// The old plugin's id for the parameter.
    pub id: ForeignId,
    /// The value as stored; not checked (it may be NaN until [`ParamMap::apply`] drops it).
    pub value: f64,
    /// Whether `value` is plain or normalised.
    pub kind: ValueKind,
}

/// What a decoder found in an old state.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedState {
    /// The format's name, as given by [`StateFormat::name`].
    pub format: &'static str,
    /// The format version `detect` named.
    pub version: u32,
    /// The parameter values found, in the order they were read.
    pub params: Vec<ImportedParam>,
    /// Anything the port must interpret itself: samples, a patch name, a table.
    pub chunk: Vec<u8>,
}

impl ImportedState {
    /// An empty state of `format`, `version`. Does not allocate until a push.
    pub fn new(format: &'static str, version: u32) -> Self {
        Self { format, version, params: Vec::new(), chunk: Vec::new() }
    }

    /// Add a value in the old plugin's own units.
    pub fn push_plain(&mut self, id: ForeignId, value: f64) {
        self.params.push(ImportedParam { id, value, kind: ValueKind::Plain });
    }

    /// Add a value stored as 0 to 1 across the old parameter's range.
    pub fn push_normalized(&mut self, id: ForeignId, value: f64) {
        self.params.push(ImportedParam { id, value, kind: ValueKind::Normalized });
    }
}

/// One format the port reads. See the module example.
#[derive(Clone, Copy)]
pub struct StateFormat {
    /// A name for the format, copied into [`ImportedState::format`] by convention.
    pub name: &'static str,
    /// `Some(version)` if the bytes are this format. Must be cheap and must
    /// not fail: look at a magic, not at the whole document.
    pub detect: fn(&[u8]) -> Option<u32>,
    /// Read the bytes as the version `detect` named.
    pub decode: fn(&[u8], u32) -> Result<ImportedState>,
}

/// Try each format's `detect` in order. `None` when none recognises the
/// bytes (use the port's own format then); otherwise the first recognising
/// format's decode result, which may be an error (a recognised but damaged
/// state: refuse it, do not fall back).
pub fn import(formats: &[StateFormat], data: &[u8]) -> Option<Result<ImportedState>> {
    formats.iter().find_map(|f| (f.detect)(data).map(|version| (f.decode)(data, version)))
}

struct MapEntry {
    from: ForeignId,
    to: &'static str,
    convert: Option<fn(f64) -> f64>,
}

/// Old parameter ids and values to the port's.
#[derive(Default)]
pub struct ParamMap {
    entries: Vec<MapEntry>,
}

/// The result of [`ParamMap::apply`].
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    /// Parameter index in the port's table and its value, clamped to the range.
    pub values: Vec<(usize, f32)>,
    /// Old ids with no entry in the map, or whose target is not in the table,
    /// or whose value was not finite.
    pub unmapped: Vec<ForeignId>,
}

impl ParamMap {
    /// An empty map. Allocates as entries are added; build it on the main thread.
    pub fn new() -> Self {
        Self::default()
    }

    /// Old id `from` is the port's parameter `to` ([`ParamInfo::id`]), same units.
    pub fn map(&mut self, from: impl Into<ForeignId>, to: &'static str) -> &mut Self {
        self.entries.push(MapEntry { from: from.into(), to, convert: None });
        self
    }

    /// As [`Self::map`], with `convert` applied to the value before it is
    /// clamped: a plain value in the old units to the port's, or, for a
    /// normalised value, 0 to 1 to the port's units.
    pub fn map_with(&mut self, from: impl Into<ForeignId>, to: &'static str, convert: fn(f64) -> f64) -> &mut Self {
        self.entries.push(MapEntry { from: from.into(), to, convert: Some(convert) });
        self
    }

    /// Resolve an imported state against the port's parameter table. The
    /// state is not applied: the caller sets the parameters (all or none).
    pub fn apply(&self, table: &[ParamInfo], state: &ImportedState) -> Applied {
        let mut values = Vec::new();
        let mut unmapped = Vec::new();
        for p in &state.params {
            let target = self.entries.iter().find(|e| e.from == p.id).and_then(|e| table.iter().position(|t| t.id == e.to).map(|i| (i, e)));
            let Some((index, entry)) = target else {
                unmapped.push(p.id.clone());
                continue;
            };
            let v = entry.convert.map_or(p.value, |f| f(p.value));
            if !v.is_finite() {
                unmapped.push(p.id.clone());
                continue;
            }
            let info = &table[index];
            let value = match (p.kind, entry.convert) {
                // A normalised value with no conversion follows the port's own taper.
                (ValueKind::Normalized, None) => info.from_normalized(v as f32),
                _ => info.clamp(v as f32),
            };
            values.push((index, value));
        }
        Applied { values, unmapped }
    }
}
