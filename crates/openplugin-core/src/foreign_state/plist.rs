//! Apple property lists: the XML form and the `bplist00` binary form.
//!
//! The binary reader follows the documented layout of `bplist00`: the 8-byte
//! magic, the objects, an offset table with one entry per object, and a
//! 32-byte trailer at the end that gives the table's entry width, the width of
//! an object reference, the object count, the root object and the table's
//! offset. Object markers: `0x00` null, `0x08`/`0x09` false/true, `0x1n`
//! integer of 2^n bytes, `0x22`/`0x23` real of 4/8 bytes, `0x33` date (8-byte
//! real, seconds since 2001-01-01), `0x4n` data, `0x5n` ASCII string, `0x6n`
//! UTF-16BE string, `0x8n` UID of n+1 bytes, `0xAn` array, `0xCn` set, `0xDn`
//! dictionary. A length nibble of `0xF` means the length follows as an
//! integer object.
//!
//! A document is checked before it is built: the trailer's sizes, the offset
//! table and every offset are verified, each count is compared with the bytes
//! that remain, and references are followed with a depth limit and a node and
//! byte budget, so a document that points at the same large object many times
//! (or at itself) is refused rather than expanded.

use super::bytes::{Endian, Reader};
use super::xml::{self, Element};
use super::{ErrorKind, ForeignError, Limits, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i128),
    Real(f64),
    /// Seconds since 2001-01-01 00:00:00 UTC.
    Date(f64),
    String(String),
    Data(Vec<u8>),
    /// Binary property lists only: a keyed-archiver object reference.
    Uid(u64),
    Array(Vec<Value>),
    /// Entries in file order.
    Dict(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(d) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_data(&self) -> Option<&[u8]> {
        match self {
            Value::Data(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<i128> {
        match self {
            Value::Integer(i) => Some(*i),
            _ => None,
        }
    }

    /// Integers and reals as `f64`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Integer(i) => Some(*i as f64),
            Value::Real(r) => Some(*r),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }
}

/// Whether `data` looks like a binary property list.
pub fn is_binary(data: &[u8]) -> bool {
    data.starts_with(b"bplist00")
}

/// Parse either form, chosen by the magic, with the default [`Limits`].
pub fn parse(data: &[u8]) -> Result<Value> {
    parse_with(data, &Limits::default())
}

pub fn parse_with(data: &[u8], limits: &Limits) -> Result<Value> {
    if is_binary(data) { parse_binary(data, limits) } else { parse_xml(data, limits) }
}

// ---------- binary ----------

struct Bin<'a> {
    data: &'a [u8],
    table: usize,
    off_size: usize,
    ref_size: usize,
    count: usize,
    nodes: usize,
    bytes: usize,
    limits: Limits,
}

pub fn parse_binary(data: &[u8], limits: &Limits) -> Result<Value> {
    let bad = |what: &'static str| ForeignError::new(ErrorKind::Invalid, 0, what);
    if data.len() > limits.max_len {
        return Err(ForeignError::new(ErrorKind::TooLarge, 0, "input larger than the limit"));
    }
    if data.len() < 8 + 1 + 32 || !is_binary(data) {
        return Err(bad("not a bplist00 document"));
    }
    let trailer = data.len() - 32;
    let mut t = Reader::big(&data[trailer..]);
    t.skip(6)?;
    let off_size = t.u8()? as usize;
    let ref_size = t.u8()? as usize;
    let count = t.u64()?;
    let top = t.u64()?;
    let table = t.u64()?;
    if !(1..=8).contains(&off_size) || !(1..=8).contains(&ref_size) {
        return Err(ForeignError::new(ErrorKind::Invalid, trailer + 6, "entry or reference size out of range"));
    }
    if count == 0 || top >= count {
        return Err(ForeignError::new(ErrorKind::Invalid, trailer + 8, "object count or root out of range"));
    }
    // The offset table must lie between the objects and the trailer.
    let table_len = count.checked_mul(off_size as u64).ok_or_else(|| bad("offset table size overflows"))?;
    let table_end = table.checked_add(table_len).ok_or_else(|| bad("offset table size overflows"))?;
    if table < 9 || table_end > trailer as u64 {
        return Err(ForeignError::new(ErrorKind::BadLength, trailer + 24, "offset table lies outside the document"));
    }
    // Each object takes at least one byte, so the count is bounded by the data.
    if count > (table - 8) {
        return Err(ForeignError::new(ErrorKind::BadLength, trailer + 8, "more objects than bytes"));
    }
    if ref_size < 8 && count > (1u64 << (8 * ref_size)) {
        return Err(ForeignError::new(ErrorKind::Invalid, trailer + 7, "reference size too small for the object count"));
    }
    let mut b = Bin { data: &data[..table as usize], table: table as usize, off_size, ref_size, count: count as usize, nodes: 0, bytes: 0, limits: *limits };
    b.object(top as usize, 0, data)
}

impl<'a> Bin<'a> {
    fn err(&self, kind: ErrorKind, at: usize, what: &'static str) -> ForeignError {
        ForeignError::new(kind, at, what)
    }

    fn offset_of(&self, index: usize, whole: &[u8]) -> Result<usize> {
        let at = self.table + index * self.off_size;
        let mut r = Reader::big(&whole[at..at + self.off_size]);
        let off = r.uint(self.off_size)?;
        if off < 8 || off >= self.table as u64 {
            return Err(self.err(ErrorKind::Invalid, at, "object offset outside the object area"));
        }
        Ok(off as usize)
    }

    fn spend(&mut self, at: usize, bytes: usize) -> Result<()> {
        self.nodes += 1;
        self.bytes = self.bytes.saturating_add(bytes);
        if self.nodes > self.limits.max_nodes {
            return Err(self.err(ErrorKind::TooLarge, at, "too many objects"));
        }
        if bytes > self.limits.max_len || self.bytes > self.limits.max_total {
            return Err(self.err(ErrorKind::TooLarge, at, "strings and data larger than the limit"));
        }
        Ok(())
    }

    /// The length that goes with a marker: the nibble, or a following integer object.
    fn length(&self, r: &mut Reader<'_>, nibble: u8, at: usize) -> Result<usize> {
        if nibble != 0xF {
            return Ok(nibble as usize);
        }
        let m = r.u8()?;
        if m >> 4 != 1 {
            return Err(self.err(ErrorKind::Invalid, at, "length is not an integer object"));
        }
        let width = 1usize << (m & 0xF);
        if width > 8 {
            return Err(self.err(ErrorKind::Invalid, at, "length integer too wide"));
        }
        let n = r.uint(width)?;
        usize::try_from(n).map_err(|_| self.err(ErrorKind::BadLength, at, "length too large"))
    }

    fn object(&mut self, index: usize, depth: usize, whole: &[u8]) -> Result<Value> {
        let at = self.offset_of(index, whole)?;
        if depth > self.limits.depth() {
            return Err(self.err(ErrorKind::TooDeep, at, "containers nested too deeply"));
        }
        let mut r = Reader::big(&self.data[at..]);
        let marker = r.u8()?;
        let (hi, lo) = (marker >> 4, marker & 0xF);
        self.spend(at, 0)?;
        match (hi, lo) {
            (0x0, 0x0) => Ok(Value::Null),
            (0x0, 0x8) => Ok(Value::Bool(false)),
            (0x0, 0x9) => Ok(Value::Bool(true)),
            (0x1, n) => {
                let width = 1usize << n;
                match width {
                    1 | 2 | 4 => Ok(Value::Integer(i128::from(r.uint(width)?))),
                    8 => Ok(Value::Integer(i128::from(r.i64()?))),
                    16 => {
                        let hi = r.u64()?;
                        let lo = r.u64()?;
                        Ok(Value::Integer(((u128::from(hi) << 64) | u128::from(lo)) as i128))
                    }
                    _ => Err(self.err(ErrorKind::Invalid, at, "unsupported integer width")),
                }
            }
            (0x2, 2) => Ok(Value::Real(f64::from(r.f32()?))),
            (0x2, 3) => Ok(Value::Real(r.f64()?)),
            (0x3, 3) => Ok(Value::Date(r.f64()?)),
            (0x4, n) => {
                let len = self.length(&mut r, n, at)?;
                if len > r.remaining() {
                    return Err(self.err(ErrorKind::BadLength, at, "data length exceeds the document"));
                }
                self.spend(at, len)?;
                Ok(Value::Data(r.bytes(len)?.to_vec()))
            }
            (0x5, n) => {
                let len = self.length(&mut r, n, at)?;
                if len > r.remaining() {
                    return Err(self.err(ErrorKind::BadLength, at, "string length exceeds the document"));
                }
                self.spend(at, len)?;
                let s = std::str::from_utf8(r.bytes(len)?).map_err(|_| self.err(ErrorKind::Utf8, at, "string is not valid text"))?;
                Ok(Value::String(s.to_string()))
            }
            (0x6, n) => {
                let units = self.length(&mut r, n, at)?;
                if units.checked_mul(2).is_none_or(|b| b > r.remaining()) {
                    return Err(self.err(ErrorKind::BadLength, at, "string length exceeds the document"));
                }
                self.spend(at, units * 2)?;
                Ok(Value::String(r.utf16(units)?))
            }
            (0x8, n) => {
                let width = n as usize + 1;
                if width > 8 {
                    return Err(self.err(ErrorKind::Invalid, at, "UID wider than 8 bytes"));
                }
                Ok(Value::Uid(r.uint(width)?))
            }
            (0xA, n) | (0xC, n) => {
                let count = self.length(&mut r, n, at)?;
                let refs = self.refs(&mut r, count, 1, at)?;
                let mut out = Vec::with_capacity(count);
                for child in refs {
                    out.push(self.object(child, depth + 1, whole)?);
                }
                Ok(Value::Array(out))
            }
            (0xD, n) => {
                let count = self.length(&mut r, n, at)?;
                let refs = self.refs(&mut r, count, 2, at)?;
                let mut out = Vec::with_capacity(count);
                for i in 0..count {
                    let Value::String(key) = self.object(refs[i], depth + 1, whole)? else {
                        return Err(self.err(ErrorKind::Invalid, at, "dictionary key is not a string"));
                    };
                    out.push((key, self.object(refs[count + i], depth + 1, whole)?));
                }
                Ok(Value::Dict(out))
            }
            _ => Err(self.err(ErrorKind::Invalid, at, "unknown object marker")),
        }
    }

    /// `count * per` object references, checked to fit in the document and to name real objects.
    fn refs(&mut self, r: &mut Reader<'_>, count: usize, per: usize, at: usize) -> Result<Vec<usize>> {
        let total = count.checked_mul(per).ok_or_else(|| self.err(ErrorKind::BadLength, at, "count overflows"))?;
        r.check_count(total as u64, self.ref_size)?;
        self.spend(at, total.saturating_mul(self.ref_size))?;
        let mut v = Vec::with_capacity(total);
        for _ in 0..total {
            let n = r.uint(self.ref_size)?;
            if n >= self.count as u64 {
                return Err(self.err(ErrorKind::Invalid, at, "object reference out of range"));
            }
            v.push(n as usize);
        }
        Ok(v)
    }
}

// ---------- XML ----------

pub fn parse_xml(data: &[u8], limits: &Limits) -> Result<Value> {
    let root = xml::parse_with(data, limits)?;
    if root.name != "plist" {
        return Err(ForeignError::new(ErrorKind::Invalid, 0, "root element is not <plist>"));
    }
    let mut it = root.elements();
    let (Some(first), None) = (it.next(), it.next()) else {
        return Err(ForeignError::new(ErrorKind::Invalid, 0, "<plist> must hold exactly one value"));
    };
    let mut budget = Budget { nodes: 0, bytes: 0, limits: *limits };
    xml_value(first, 0, &mut budget)
}

struct Budget {
    nodes: usize,
    bytes: usize,
    limits: Limits,
}

fn bad(what: &'static str) -> ForeignError {
    ForeignError::new(ErrorKind::Invalid, 0, what)
}

fn xml_value(e: &Element, depth: usize, b: &mut Budget) -> Result<Value> {
    if depth > b.limits.depth() {
        return Err(ForeignError::new(ErrorKind::TooDeep, 0, "containers nested too deeply"));
    }
    b.nodes += 1;
    if b.nodes > b.limits.max_nodes {
        return Err(ForeignError::new(ErrorKind::TooLarge, 0, "too many values"));
    }
    match e.name.as_str() {
        "dict" => {
            let mut out = Vec::new();
            let mut it = e.elements();
            while let Some(k) = it.next() {
                if k.name != "key" {
                    return Err(bad("expected <key> in <dict>"));
                }
                let v = it.next().ok_or_else(|| bad("<key> without a value"))?;
                out.push((k.text(), xml_value(v, depth + 1, b)?));
            }
            Ok(Value::Dict(out))
        }
        "array" => e.elements().map(|c| xml_value(c, depth + 1, b)).collect::<Result<Vec<_>>>().map(Value::Array),
        "string" => {
            let s = e.text();
            b.bytes = b.bytes.saturating_add(s.len());
            Ok(Value::String(s))
        }
        "integer" => parse_int(e.text().trim()).map(Value::Integer).ok_or_else(|| bad("bad <integer>")),
        "real" => {
            let t = e.text();
            let t = t.trim();
            let r = match t.to_ascii_lowercase().as_str() {
                "nan" => Some(f64::NAN),
                "inf" | "+inf" | "infinity" => Some(f64::INFINITY),
                "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
                _ => t.parse::<f64>().ok().filter(|_| !t.is_empty() && !t.starts_with(['i', 'n', 'I', 'N'])),
            };
            r.map(Value::Real).ok_or_else(|| bad("bad <real>"))
        }
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        "data" => {
            let d = base64(&e.text()).ok_or_else(|| bad("bad base64 in <data>"))?;
            b.bytes = b.bytes.saturating_add(d.len());
            Ok(Value::Data(d))
        }
        "date" => parse_date(e.text().trim()).map(Value::Date).ok_or_else(|| bad("bad <date>")),
        _ => Err(bad("unknown property list element")),
    }
    .and_then(|v| if b.bytes > b.limits.max_total { Err(ForeignError::new(ErrorKind::TooLarge, 0, "strings and data larger than the limit")) } else { Ok(v) })
}

fn parse_int(t: &str) -> Option<i128> {
    let (neg, digits) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let v = match digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
        Some(h) => i128::from_str_radix(h, 16).ok()?,
        None => digits.parse::<i128>().ok()?,
    };
    if digits.is_empty() || digits.starts_with(['-', '+']) {
        return None;
    }
    if neg { v.checked_neg() } else { Some(v) }
}

/// Base64 (standard alphabet), ignoring white space. `None` on a bad character or length.
fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0;
    let mut pad = 0;
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => {
                pad += 1;
                continue;
            }
            b' ' | b'\t' | b'\n' | b'\r' => continue,
            _ => return None,
        };
        if pad > 0 {
            return None;
        }
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    // Leftover bits must be zero padding bits and the pad count must fit.
    let ok = match bits {
        0 => pad == 0,
        2 => pad == 1 && acc == 0,
        4 => pad == 2 && acc == 0,
        _ => false,
    };
    ok.then_some(out)
}

/// `YYYY-MM-DDTHH:MM:SSZ` as seconds since 2001-01-01 UTC.
fn parse_date(t: &str) -> Option<f64> {
    let b = t.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| -> Option<i64> { t.get(r).filter(|s| s.bytes().all(|c| c.is_ascii_digit())).and_then(|s| s.parse().ok()) };
    let (y, mo, d, h, mi, s) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    // Days from civil date (proleptic Gregorian), relative to 2001-01-01.
    let (y2, m2) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2.rem_euclid(400);
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let epoch_2001 = 11_323; // days from 1970-01-01 to 2001-01-01
    Some(((days - epoch_2001) * 86_400 + h * 3600 + mi * 60 + s) as f64)
}

/// The byte order `bplist00` numbers use.
pub const BINARY_ENDIAN: Endian = Endian::Big;
