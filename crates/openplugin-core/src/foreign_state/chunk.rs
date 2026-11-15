//! Chunks: a block that starts with a magic and says how long it is.
//!
//! [`ChunkLayout`] describes one framework-agnostic shape: magic bytes, an
//! optional version number, a size, then the payload. [`TaggedChunks`] walks
//! the other common shape, a sequence of four-character tag, size, payload.

use super::bytes::{Endian, LenWidth, Reader};
use super::{ErrorKind, ForeignError, Result};

/// `magic`, then an optional `version_bytes`-wide version, then a `size`
/// field, then the payload.
#[derive(Debug, Clone, Copy)]
pub struct ChunkLayout {
    pub magic: &'static [u8],
    pub endian: Endian,
    /// Width in bytes of the version field after the magic; 0 for none (at most 8).
    pub version_bytes: usize,
    pub size: LenWidth,
    /// Whether the size counts the header (magic, version and size field) too.
    pub size_includes_header: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk<'a> {
    pub version: Option<u64>,
    pub payload: &'a [u8],
}

/// Whether `data` starts with `magic`.
pub fn has_magic(data: &[u8], magic: &[u8]) -> bool {
    data.len() >= magic.len() && &data[..magic.len()] == magic
}

/// Read one chunk from the front of `data`. Returns the chunk and the bytes
/// after it. The declared size is checked against the data before the payload
/// is touched.
pub fn read_chunk<'a>(data: &'a [u8], layout: &ChunkLayout) -> Result<(Chunk<'a>, &'a [u8])> {
    let mut r = Reader::new(data, layout.endian);
    if !r.eat(layout.magic) {
        return Err(ForeignError::new(ErrorKind::Invalid, 0, "magic does not match"));
    }
    let version = if layout.version_bytes > 0 { Some(r.uint(layout.version_bytes)?) } else { None };
    let size = r.len_prefix(layout.size)?;
    let header = r.position() as u64;
    let payload_len = if layout.size_includes_header {
        size.checked_sub(header).ok_or_else(|| ForeignError::new(ErrorKind::BadLength, r.position(), "size is smaller than the header"))?
    } else {
        size
    };
    if payload_len > r.remaining() as u64 {
        return Err(ForeignError::new(ErrorKind::BadLength, r.position(), "declared size exceeds the remaining data"));
    }
    let payload = r.bytes(payload_len as usize)?;
    Ok((Chunk { version, payload }, r.rest()))
}

/// Read `data` as exactly one chunk with nothing after it.
pub fn read_exact<'a>(data: &'a [u8], layout: &ChunkLayout) -> Result<Chunk<'a>> {
    let (chunk, rest) = read_chunk(data, layout)?;
    if !rest.is_empty() {
        return Err(ForeignError::new(ErrorKind::Invalid, data.len() - rest.len(), "bytes after the chunk"));
    }
    Ok(chunk)
}

/// A sequence of `tag(4) size(4) payload` chunks (RIFF-like). Iterating yields
/// `Err` once and then stops if a size runs past the data.
#[derive(Debug, Clone)]
pub struct TaggedChunks<'a> {
    r: Reader<'a>,
    pad_even: bool,
    failed: bool,
}

impl<'a> TaggedChunks<'a> {
    /// `size` is a 32-bit field in `endian` order; with `pad_even`, a payload of
    /// odd length is followed by one pad byte not counted in the size.
    pub fn new(data: &'a [u8], endian: Endian, pad_even: bool) -> Self {
        Self { r: Reader::new(data, endian), pad_even, failed: false }
    }
}

impl<'a> Iterator for TaggedChunks<'a> {
    type Item = Result<([u8; 4], &'a [u8])>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.r.is_empty() {
            return None;
        }
        let res = (|| {
            let t = self.r.bytes(4)?;
            let tag = [t[0], t[1], t[2], t[3]];
            let payload = self.r.prefixed_bytes(LenWidth::U32)?;
            if self.pad_even && payload.len() % 2 == 1 && !self.r.is_empty() {
                self.r.skip(1)?;
            }
            Ok((tag, payload))
        })();
        if res.is_err() {
            self.failed = true;
        }
        Some(res)
    }
}
