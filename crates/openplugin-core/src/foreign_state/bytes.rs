//! A bounds-checked cursor over binary data.

use super::{ErrorKind, ForeignError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}

/// The width of a length prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LenWidth {
    U8,
    U16,
    U32,
    U64,
}

impl LenWidth {
    pub fn bytes(self) -> usize {
        match self {
            LenWidth::U8 => 1,
            LenWidth::U16 => 2,
            LenWidth::U32 => 4,
            LenWidth::U64 => 8,
        }
    }
}

/// Reads fields from a byte slice. Every read checks the bytes are there and
/// returns an error, not a panic, when they are not. A declared length is
/// compared with what remains before anything is allocated.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    endian: Endian,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], endian: Endian) -> Self {
        Self { data, pos: 0, endian }
    }

    pub fn little(data: &'a [u8]) -> Self {
        Self::new(data, Endian::Little)
    }

    pub fn big(data: &'a [u8]) -> Self {
        Self::new(data, Endian::Big)
    }

    pub fn endian(&self) -> Endian {
        self.endian
    }

    pub fn set_endian(&mut self, endian: Endian) {
        self.endian = endian;
    }

    /// Bytes consumed so far.
    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub fn rest(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    fn err(&self, kind: ErrorKind, what: &'static str) -> ForeignError {
        ForeignError::new(kind, self.pos, what)
    }

    /// The next `n` bytes, or an error when fewer remain.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(self.err(ErrorKind::Truncated, "fewer bytes remain than the field needs"));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }

    /// Look at the next `n` bytes without consuming them.
    pub fn peek(&self, n: usize) -> Option<&'a [u8]> {
        self.data.get(self.pos..self.pos.checked_add(n)?)
    }

    /// Consume `magic` if it is next; `false` and no movement otherwise.
    pub fn eat(&mut self, magic: &[u8]) -> bool {
        if self.peek(magic.len()) == Some(magic) {
            self.pos += magic.len();
            true
        } else {
            false
        }
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let s = self.bytes(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    pub fn u16(&mut self) -> Result<u16> {
        let a = self.array::<2>()?;
        Ok(match self.endian {
            Endian::Little => u16::from_le_bytes(a),
            Endian::Big => u16::from_be_bytes(a),
        })
    }

    pub fn u32(&mut self) -> Result<u32> {
        let a = self.array::<4>()?;
        Ok(match self.endian {
            Endian::Little => u32::from_le_bytes(a),
            Endian::Big => u32::from_be_bytes(a),
        })
    }

    pub fn u64(&mut self) -> Result<u64> {
        let a = self.array::<8>()?;
        Ok(match self.endian {
            Endian::Little => u64::from_le_bytes(a),
            Endian::Big => u64::from_be_bytes(a),
        })
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn i64(&mut self) -> Result<i64> {
        Ok(self.u64()? as i64)
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.u64()?))
    }

    /// An unsigned integer of `width` bytes (1 to 8) in the reader's byte order.
    pub fn uint(&mut self, width: usize) -> Result<u64> {
        if width == 0 || width > 8 {
            return Err(self.err(ErrorKind::Invalid, "integer width must be 1 to 8 bytes"));
        }
        let s = self.bytes(width)?;
        let mut v = 0u64;
        match self.endian {
            Endian::Big => s.iter().for_each(|b| v = (v << 8) | u64::from(*b)),
            Endian::Little => s.iter().rev().for_each(|b| v = (v << 8) | u64::from(*b)),
        }
        Ok(v)
    }

    /// A length prefix of the given width.
    pub fn len_prefix(&mut self, width: LenWidth) -> Result<u64> {
        self.uint(width.bytes())
    }

    /// A length prefix followed by that many bytes. A length larger than the
    /// bytes that remain is refused before anything is read.
    pub fn prefixed_bytes(&mut self, width: LenWidth) -> Result<&'a [u8]> {
        let at = self.pos;
        let n = self.len_prefix(width)?;
        if n > self.remaining() as u64 {
            self.pos = at;
            return Err(ForeignError::new(ErrorKind::BadLength, at, "declared length exceeds the remaining data"));
        }
        self.bytes(n as usize)
    }

    /// A length-prefixed UTF-8 string.
    pub fn prefixed_str(&mut self, width: LenWidth) -> Result<&'a str> {
        let at = self.pos;
        let b = self.prefixed_bytes(width)?;
        std::str::from_utf8(b).map_err(|_| ForeignError::new(ErrorKind::Utf8, at, "string is not valid UTF-8"))
    }

    /// `n` bytes of UTF-8.
    pub fn str_fixed(&mut self, n: usize) -> Result<&'a str> {
        let at = self.pos;
        let b = self.bytes(n)?;
        std::str::from_utf8(b).map_err(|_| ForeignError::new(ErrorKind::Utf8, at, "string is not valid UTF-8"))
    }

    /// UTF-8 up to a zero byte, which is consumed.
    pub fn cstr(&mut self) -> Result<&'a str> {
        let at = self.pos;
        let rest = self.rest();
        let end = rest.iter().position(|b| *b == 0).ok_or_else(|| self.err(ErrorKind::Truncated, "no terminating zero"))?;
        self.pos += end + 1;
        std::str::from_utf8(&rest[..end]).map_err(|_| ForeignError::new(ErrorKind::Utf8, at, "string is not valid UTF-8"))
    }

    /// `units` UTF-16 code units in the reader's byte order.
    pub fn utf16(&mut self, units: usize) -> Result<String> {
        let at = self.pos;
        let need = units.checked_mul(2).ok_or_else(|| self.err(ErrorKind::BadLength, "length overflows"))?;
        let b = self.bytes(need)?;
        let v: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| match self.endian {
                Endian::Little => u16::from_le_bytes([c[0], c[1]]),
                Endian::Big => u16::from_be_bytes([c[0], c[1]]),
            })
            .collect();
        String::from_utf16(&v).map_err(|_| ForeignError::new(ErrorKind::Utf8, at, "string is not valid UTF-16"))
    }

    /// A reader over the next `n` bytes (same byte order); this one moves past them.
    pub fn sub(&mut self, n: usize) -> Result<Reader<'a>> {
        let s = self.bytes(n)?;
        Ok(Reader { data: s, pos: 0, endian: self.endian })
    }

    /// `count` values of `elem_size` bytes each must fit in what remains.
    /// Call before allocating a vector of `count` items.
    pub fn check_count(&self, count: u64, elem_size: usize) -> Result<usize> {
        match count.checked_mul(elem_size.max(1) as u64) {
            Some(n) if n <= self.remaining() as u64 => Ok(count as usize),
            _ => Err(self.err(ErrorKind::BadLength, "declared count exceeds the remaining data")),
        }
    }
}
