//! Length-prefixed key/value records.
//!
//! Many plugins store a bag of named values as a run of records: a key
//! string, then a value, each with its length in front. [`Layout`] describes
//! the widths and byte order; [`read_pairs`] reads the run into [`Pairs`]
//! whose values are raw bytes that the typed getters decode.

use super::bytes::{Endian, LenWidth, Reader};
use super::{ErrorKind, ForeignError, Limits, Result};

/// How a run of records is laid out.
///
/// ```
/// use openplugin_core::foreign_state::bytes::{Endian, LenWidth};
/// use openplugin_core::foreign_state::kv::{read_pairs, Layout};
/// use openplugin_core::foreign_state::Limits;
/// const OLD: Layout = Layout { endian: Endian::Little, key_len: LenWidth::U8, value_len: LenWidth::U16, count: None };
/// let mut data = vec![4, b'g', b'a', b'i', b'n', 4, 0];
/// data.extend_from_slice(&0.5f32.to_le_bytes());
/// let pairs = read_pairs(&data, &OLD, &Limits::default()).unwrap();
/// assert_eq!(pairs.f32("gain"), Some(0.5));
/// assert_eq!(pairs.f32("mix"), None);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    /// Byte order of the length prefixes, the count and the typed getters.
    pub endian: Endian,
    /// Width of the key's length prefix (the key is UTF-8).
    pub key_len: LenWidth,
    /// Width of the value's length prefix (in bytes).
    pub value_len: LenWidth,
    /// When set, the run starts with a record count of this width; otherwise
    /// records run to the end of the data.
    pub count: Option<LenWidth>,
}

/// The records of one run, in file order.
#[derive(Debug, Clone, PartialEq)]
pub struct Pairs {
    /// `(key, raw value)` per record, in file order; a key may repeat.
    pub entries: Vec<(String, Vec<u8>)>,
    endian: Endian,
}

/// Read a run of records. Counts and lengths are checked against the bytes
/// that remain before anything is allocated; `limits` bounds the record
/// count and the total size.
pub fn read_pairs(data: &[u8], layout: &Layout, limits: &Limits) -> Result<Pairs> {
    if data.len() > limits.max_len {
        return Err(ForeignError::new(ErrorKind::TooLarge, 0, "input larger than the limit"));
    }
    let mut r = Reader::new(data, layout.endian);
    let min_record = layout.key_len.bytes() + layout.value_len.bytes();
    let declared = match layout.count {
        Some(w) => {
            let n = r.len_prefix(w)?;
            r.check_count(n, min_record)?;
            Some(n as usize)
        }
        None => None,
    };
    let mut entries = Vec::new();
    let mut total = 0usize;
    while declared.map_or(!r.is_empty(), |n| entries.len() < n) {
        if entries.len() >= limits.max_nodes {
            return Err(ForeignError::new(ErrorKind::TooLarge, r.position(), "too many records"));
        }
        let key = r.prefixed_str(layout.key_len)?.to_string();
        let value = r.prefixed_bytes(layout.value_len)?;
        total = total.saturating_add(key.len() + value.len());
        if total > limits.max_total {
            return Err(ForeignError::new(ErrorKind::TooLarge, r.position(), "records larger than the limit"));
        }
        entries.push((key, value.to_vec()));
    }
    if declared.is_some() && !r.is_empty() {
        return Err(ForeignError::new(ErrorKind::Invalid, r.position(), "bytes after the last record"));
    }
    Ok(Pairs { entries, endian: layout.endian })
}

impl Pairs {
    /// The first value stored under `key`.
    pub fn get(&self, key: &str) -> Option<&[u8]> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_slice())
    }

    /// The first value under `key` as UTF-8; `None` when missing or not UTF-8.
    ///
    /// The number getters below read the value's first bytes in the layout's
    /// byte order: `None` when the key is missing or the value is too short.
    /// A longer value is not refused; its extra bytes are ignored.
    pub fn str(&self, key: &str) -> Option<&str> {
        std::str::from_utf8(self.get(key)?).ok()
    }

    fn reader(&self, key: &str) -> Option<Reader<'_>> {
        Some(Reader::new(self.get(key)?, self.endian))
    }

    /// The first value under `key` as an IEEE single (it may be NaN).
    pub fn f32(&self, key: &str) -> Option<f32> {
        self.reader(key)?.f32().ok()
    }

    /// The first value under `key` as an IEEE double (it may be NaN).
    pub fn f64(&self, key: &str) -> Option<f64> {
        self.reader(key)?.f64().ok()
    }

    /// The first value under `key` as a 32-bit signed integer.
    pub fn i32(&self, key: &str) -> Option<i32> {
        self.reader(key)?.i32().ok()
    }

    /// The first value under `key` as a 32-bit unsigned integer.
    pub fn u32(&self, key: &str) -> Option<u32> {
        self.reader(key)?.u32().ok()
    }

    /// The first value under `key` as a 64-bit signed integer.
    pub fn i64(&self, key: &str) -> Option<i64> {
        self.reader(key)?.i64().ok()
    }
}
