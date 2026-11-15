//! Reading the state a plugin written with another SDK saved.
//!
//! A team that ports a shipped plugin has to open its customers' sessions and
//! presets: the host hands the port the bytes the old plugin wrote, through
//! [`crate::Plugin::load_state`]. What is in those bytes is the old plugin's
//! own choice, but it is nearly always one of a few containers. This module
//! reads those containers, from the data formats alone:
//!
//! * [`bytes`]: a bounds-checked cursor over little- or big-endian binary data,
//!   with length-prefixed strings and blobs.
//! * [`kv`]: a sequence of length-prefixed key/value records.
//! * [`chunk`]: a block with a magic, an optional version and a size header,
//!   and the 4-character-tag-plus-size chunk sequence.
//! * [`xml`]: a small, strict XML reader (elements, attributes, text, the five
//!   entities and character references, CDATA; no DTD).
//! * [`plist`]: Apple property lists, both the XML form and `bplist00`.
//! * [`import`]: the pattern that ties these together, a list of recognised
//!   formats tried in order, and the map from the old parameter ids and values
//!   to the port's own.
//!
//! Every reader treats its input as hostile. A reader returns a
//! [`ForeignError`] and never panics; it checks every declared length against
//! the bytes that are really there before it allocates, and [`Limits`] bound
//! nesting, node count and the total size of what is built, so a few hundred
//! bytes cannot ask for gigabytes.
//!
//! # The load-state pattern
//!
//! `Plugin::load_state` receives the bytes a host stored. A port saves its own
//! format from now on, but for sessions made before the port it also accepts
//! the old plugin's. The recipe (a worked example is in
//! `crates/openplugin-port/tests/state_import.rs`):
//!
//! 1. Capture states from the old plugin with `openplugin-port capture` and
//!    look at the blobs. Find what tells the old format apart: a magic, a
//!    version number, a root element name, a plist key.
//! 2. Write one function per old format version that returns `Some(version)`
//!    when the bytes are that format ([`import::StateFormat::detect`]) and one
//!    that decodes them into an [`import::ImportedState`]
//!    ([`import::StateFormat::decode`]), using the readers here. Decode into
//!    the old plugin's parameter ids and its values as stored (plain or
//!    normalised); do not convert yet.
//! 3. Build an [`import::ParamMap`] from the old ids (the scanner manifest's
//!    `params[].id` and `name`) to your [`crate::ParamInfo::id`]s, with a
//!    conversion function where the units differ.
//! 4. In `load_state`, try [`import::import`] first. If a format is recognised,
//!    decode, map, validate, and only then change the plugin (a refused state
//!    must change nothing). If none is, fall back to your own format.
//!
//! The `Plugin` trait is unchanged; the old plugin's chunk is just another
//! input to `load_state`.

pub mod bytes;
pub mod chunk;
pub mod import;
pub mod kv;
pub mod plist;
pub mod xml;

use std::fmt;

/// Bounds on what a reader will build from untrusted input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Deepest nesting of elements, arrays or dictionaries. At most 512 is honoured.
    pub max_depth: usize,
    /// Most nodes (elements, text runs, plist values, key/value records).
    pub max_nodes: usize,
    /// Largest input, and largest single string or blob, in bytes.
    pub max_len: usize,
    /// Most bytes of strings and blobs built in one document, counting a value
    /// once per place it is used.
    pub max_total: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_depth: 64, max_nodes: 1 << 20, max_len: 64 << 20, max_total: 128 << 20 }
    }
}

impl Limits {
    pub(crate) fn depth(&self) -> usize {
        self.max_depth.min(512)
    }
}

/// Why a reader refused its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The data ends before a field does.
    Truncated,
    /// A declared length or count is larger than the data that follows it.
    BadLength,
    /// Nesting is deeper than [`Limits::max_depth`].
    TooDeep,
    /// A [`Limits`] bound other than depth was reached.
    TooLarge,
    /// Text that is not valid UTF-8 (or UTF-16).
    Utf8,
    /// The data is not the format: a wrong magic, a bad marker, a mismatched tag.
    Invalid,
}

/// A reader's refusal: what, roughly where, and a fixed description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForeignError {
    pub kind: ErrorKind,
    /// Byte offset in the input where the problem was found.
    pub offset: usize,
    pub what: &'static str,
}

impl ForeignError {
    pub(crate) fn new(kind: ErrorKind, offset: usize, what: &'static str) -> Self {
        Self { kind, offset, what }
    }
}

impl fmt::Display for ForeignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} at byte {}: {}", self.kind, self.offset, self.what)
    }
}

impl std::error::Error for ForeignError {}

pub type Result<T> = std::result::Result<T, ForeignError>;
