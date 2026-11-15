//! A tiny LV2 host for the tests: a `urid:map`, atom sequences and objects
//! built byte by byte, and host-sized output sequences.
#![allow(dead_code)]

use std::ffi::{c_char, c_void, CStr, CString};

pub mod clap_host;
pub mod plugins;
pub mod shift;
pub mod worker;

use openplugin_targets::lv2::Lv2Feature;

pub const SEQUENCE: &str = "http://lv2plug.in/ns/ext/atom#Sequence";
pub const OBJECT: &str = "http://lv2plug.in/ns/ext/atom#Object";
pub const BLANK: &str = "http://lv2plug.in/ns/ext/atom#Blank";
pub const FLOAT: &str = "http://lv2plug.in/ns/ext/atom#Float";
pub const DOUBLE: &str = "http://lv2plug.in/ns/ext/atom#Double";
pub const INT: &str = "http://lv2plug.in/ns/ext/atom#Int";
pub const LONG: &str = "http://lv2plug.in/ns/ext/atom#Long";
pub const MIDI_EVENT: &str = "http://lv2plug.in/ns/ext/midi#MidiEvent";
pub const POSITION: &str = "http://lv2plug.in/ns/ext/time#Position";
pub const SPEED: &str = "http://lv2plug.in/ns/ext/time#speed";
pub const BPM: &str = "http://lv2plug.in/ns/ext/time#beatsPerMinute";
pub const BAR: &str = "http://lv2plug.in/ns/ext/time#bar";
pub const BAR_BEAT: &str = "http://lv2plug.in/ns/ext/time#barBeat";
pub const BEATS_PER_BAR: &str = "http://lv2plug.in/ns/ext/time#beatsPerBar";
pub const BEAT_UNIT: &str = "http://lv2plug.in/ns/ext/time#beatUnit";
pub const FRAME: &str = "http://lv2plug.in/ns/ext/time#frame";

#[repr(C)]
struct UridMapFeature {
    handle: *mut c_void,
    map: unsafe extern "C" fn(*mut c_void, *const c_char) -> u32,
}

unsafe extern "C" fn map_uri(handle: *mut c_void, uri: *const c_char) -> u32 {
    let table = unsafe { &mut *(handle as *mut Vec<String>) };
    let uri = unsafe { CStr::from_ptr(uri) }.to_string_lossy().into_owned();
    match table.iter().position(|known| *known == uri) {
        Some(index) => index as u32 + 1,
        None => {
            table.push(uri);
            table.len() as u32
        }
    }
}

/// The host side of `urid:map`, with the feature array an instance is given.
pub struct Host {
    #[allow(clippy::box_collection)] // the Vec header must keep its address for the map handle
    table: Box<Vec<String>>,
    _map: Box<UridMapFeature>,
    _uri: CString,
    _feature: Box<Lv2Feature>,
    features: Box<[*const Lv2Feature; 2]>,
}

impl Host {
    pub fn new() -> Self {
        let mut table: Box<Vec<String>> = Box::default();
        let map = Box::new(UridMapFeature { handle: (&mut *table) as *mut Vec<String> as *mut c_void, map: map_uri });
        let uri = CString::new("http://lv2plug.in/ns/ext/urid#map").unwrap();
        let feature = Box::new(Lv2Feature { uri: uri.as_ptr(), data: (&*map) as *const UridMapFeature as *mut c_void });
        let features = Box::new([&*feature as *const Lv2Feature, std::ptr::null()]);
        Self { table, _map: map, _uri: uri, _feature: feature, features }
    }

    pub fn features(&self) -> *const *const Lv2Feature {
        self.features.as_ptr()
    }

    pub fn urid(&mut self, uri: &str) -> u32 {
        let c = CString::new(uri).unwrap();
        unsafe { map_uri((&mut *self.table) as *mut Vec<String> as *mut c_void, c.as_ptr()) }
    }
}

/// An 8-byte-aligned atom buffer.
pub struct AtomBuf {
    words: Vec<u64>,
}

impl AtomBuf {
    pub fn from_bytes(bytes: &[u8], capacity_bytes: usize) -> Self {
        let mut words = vec![0u64; capacity_bytes.max(bytes.len()).div_ceil(8) + 1];
        let dst = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, words.len() * 8) };
        dst[..bytes.len()].copy_from_slice(bytes);
        Self { words }
    }

    pub fn ptr(&mut self) -> *mut c_void {
        self.words.as_mut_ptr() as *mut c_void
    }

    /// A pointer 4 bytes past the aligned start (not 8-aligned).
    pub fn misaligned(&mut self) -> *mut c_void {
        unsafe { (self.words.as_mut_ptr() as *mut u8).add(4) as *mut c_void }
    }

    pub fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.words.as_ptr() as *const u8, self.words.len() * 8) }
    }

    pub fn bytes_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.words.as_mut_ptr() as *mut u8, self.words.len() * 8) }
    }
}

fn pad8(n: usize) -> usize {
    (n + 7) & !7
}

/// Builds the bytes of an input sequence (after the 8-byte atom header).
pub struct Seq {
    pub unit: u32,
    pub body: Vec<u8>,
    sequence: u32,
}

impl Seq {
    pub fn new(host: &mut Host) -> Self {
        Self { unit: 0, body: Vec::new(), sequence: host.urid(SEQUENCE) }
    }

    pub fn event(&mut self, frame: i64, kind: u32, size_field: u32, data: &[u8]) -> &mut Self {
        self.body.extend_from_slice(&frame.to_ne_bytes());
        self.body.extend_from_slice(&size_field.to_ne_bytes());
        self.body.extend_from_slice(&kind.to_ne_bytes());
        self.body.extend_from_slice(data);
        self.body.resize(pad8(self.body.len()), 0);
        self
    }

    pub fn atom(&mut self, frame: i64, kind: u32, data: &[u8]) -> &mut Self {
        self.event(frame, kind, data.len() as u32, data)
    }

    pub fn midi(&mut self, host: &mut Host, frame: i64, data: &[u8]) -> &mut Self {
        let kind = host.urid(MIDI_EVENT);
        self.atom(frame, kind, data)
    }

    /// The whole atom: header, unit, padding, events. `size_override`
    /// replaces the true body size in the header.
    pub fn bytes(&self, size_override: Option<u32>) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&size_override.unwrap_or(8 + self.body.len() as u32).to_ne_bytes());
        out.extend_from_slice(&self.sequence.to_ne_bytes());
        out.extend_from_slice(&self.unit.to_ne_bytes());
        out.extend_from_slice(&0u32.to_ne_bytes());
        out.extend_from_slice(&self.body);
        out
    }

    pub fn buf(&self) -> AtomBuf {
        AtomBuf::from_bytes(&self.bytes(None), 0)
    }
}

/// Property value kinds for [`object`].
pub enum Value {
    Float(f32),
    Double(f64),
    Int(i32),
    Long(i64),
}

/// The body of an object atom (id, type, properties), for use as an event.
pub fn object(host: &mut Host, otype: &str, props: &[(&str, Value)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u32.to_ne_bytes());
    out.extend_from_slice(&host.urid(otype).to_ne_bytes());
    for (key, value) in props {
        let (kind, bytes): (&str, Vec<u8>) = match value {
            Value::Float(v) => (FLOAT, v.to_ne_bytes().to_vec()),
            Value::Double(v) => (DOUBLE, v.to_ne_bytes().to_vec()),
            Value::Int(v) => (INT, v.to_ne_bytes().to_vec()),
            Value::Long(v) => (LONG, v.to_ne_bytes().to_vec()),
        };
        out.extend_from_slice(&host.urid(key).to_ne_bytes());
        out.extend_from_slice(&0u32.to_ne_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_ne_bytes());
        out.extend_from_slice(&host.urid(kind).to_ne_bytes());
        out.extend_from_slice(&bytes);
        out.resize(pad8(out.len()), 0);
    }
    out
}

/// A host output sequence: `size` is the capacity in body bytes, the rest of
/// the buffer is a sentinel byte that must survive.
pub struct OutSeq {
    pub buf: AtomBuf,
    pub capacity: usize,
}

pub const SENTINEL: u8 = 0xAB;

impl OutSeq {
    pub fn new(capacity: usize) -> Self {
        let mut buf = AtomBuf::from_bytes(&[], 8 + capacity + 64);
        let bytes = buf.bytes_mut();
        bytes.fill(SENTINEL);
        bytes[0..4].copy_from_slice(&(capacity as u32).to_ne_bytes());
        bytes[4..8].copy_from_slice(&0u32.to_ne_bytes());
        Self { buf, capacity }
    }

    /// What a host does before each `run`: the size field is the capacity again.
    pub fn reset(&mut self) {
        let capacity = self.capacity as u32;
        let bytes = self.buf.bytes_mut();
        bytes[0..4].copy_from_slice(&capacity.to_ne_bytes());
        bytes[4..8].copy_from_slice(&0u32.to_ne_bytes());
    }

    pub fn size(&self) -> u32 {
        u32::from_ne_bytes(self.buf.bytes()[0..4].try_into().unwrap())
    }

    pub fn kind(&self) -> u32 {
        u32::from_ne_bytes(self.buf.bytes()[4..8].try_into().unwrap())
    }

    /// The events written: (frame, atom type, data).
    pub fn events(&self) -> Vec<(i64, u32, Vec<u8>)> {
        let bytes = self.buf.bytes();
        let end = 8 + self.size() as usize;
        let mut pos = 16;
        let mut out = Vec::new();
        while pos + 16 <= end {
            let frame = i64::from_ne_bytes(bytes[pos..pos + 8].try_into().unwrap());
            let size = u32::from_ne_bytes(bytes[pos + 8..pos + 12].try_into().unwrap()) as usize;
            let kind = u32::from_ne_bytes(bytes[pos + 12..pos + 16].try_into().unwrap());
            out.push((frame, kind, bytes[pos + 16..pos + 16 + size].to_vec()));
            pos += 16 + pad8(size);
        }
        out
    }

    /// Nothing past the capacity the host gave was touched.
    pub fn sentinel_intact(&self) -> bool {
        self.buf.bytes()[8 + self.capacity..].iter().all(|b| *b == SENTINEL)
    }
}
