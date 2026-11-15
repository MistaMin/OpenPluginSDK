//! Hostile atoms through the exported LV2 descriptor.
//!
//! Truncated, oversized and mistyped atoms in the input sequence, odd
//! output capacities, and random bytes under a valid header. None of them may
//! crash, deliver an event, or write outside the host's output capacity; a
//! valid event afterwards still plays.

mod common;

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use common::plugins::{Synth, EVENTS, TRANSPORT};
use common::*;
use openplugin_core::Plugin;
use openplugin_targets::lv2;

const FRAMES: u32 = 256;

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Rig {
    desc: &'static lv2::Lv2Descriptor,
    instance: *mut c_void,
    left: Vec<f32>,
    out: OutSeq,
}

impl Rig {
    fn new(host: &Host) -> Self {
        lv2::set_creator(|| Box::new(Synth::default()) as Box<dyn Plugin>);
        let desc = unsafe { &*lv2::descriptor(0) };
        let instance = (desc.instantiate)(desc, 48_000.0, std::ptr::null(), host.features());
        assert!(!instance.is_null());
        let mut rig = Self { desc, instance, left: vec![0.0; FRAMES as usize], out: OutSeq::new(8 + 24 * 4) };
        (desc.connect_port)(instance, 0, rig.left.as_mut_ptr().cast());
        (desc.connect_port)(instance, 5, rig.out.buf.ptr());
        rig
    }

    /// Run one block with this input and report how many events the plugin saw.
    fn delivered(&mut self, input: *mut c_void) -> usize {
        (self.desc.connect_port)(self.instance, 4, input);
        EVENTS.store(0, Ordering::SeqCst);
        (self.desc.run)(self.instance, FRAMES);
        assert!(self.out.sentinel_intact(), "the output sequence was overrun");
        assert!(self.out.size() <= 8 + 24 * 4);
        EVENTS.load(Ordering::SeqCst)
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        (self.desc.cleanup)(self.instance);
    }
}

fn note_on() -> [u8; 3] {
    [0x90, 60, 100]
}

#[test]
fn hostile_atoms_are_ignored() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut host = Host::new();
    let mut rig = Rig::new(&host);
    let int = host.urid(INT);
    let midi = host.urid(MIDI_EVENT);

    // The baseline: one good event.
    let mut good = Seq::new(&mut host);
    good.midi(&mut host, 10, &note_on());
    assert_eq!(rig.delivered(good.buf().ptr()), 1);

    // Unconnected port.
    assert_eq!(rig.delivered(std::ptr::null_mut()), 0);

    // The atom is not a sequence.
    let mut bytes = good.bytes(None);
    bytes[4..8].copy_from_slice(&int.to_ne_bytes());
    assert_eq!(rig.delivered(AtomBuf::from_bytes(&bytes, 0).ptr()), 0);

    // Sizes too small to hold the sequence header, and absurdly large.
    for size in [0u32, 1, 4, 7, 0x0100_0001, 0x7FFF_FFFF, u32::MAX] {
        let buf = good.bytes(Some(size));
        assert_eq!(rig.delivered(AtomBuf::from_bytes(&buf, 0).ptr()), 0, "sequence size {size}");
    }

    // Time stamps in beats (any unit but frames) are not read.
    let mut beats = Seq::new(&mut host);
    beats.unit = int;
    beats.midi(&mut host, 10, &note_on());
    assert_eq!(rig.delivered(beats.buf().ptr()), 0);

    // Truncated: the sequence ends inside the event header and inside its body.
    for cut in 1..19 {
        let buf = good.bytes(Some(8 + cut));
        assert_eq!(rig.delivered(AtomBuf::from_bytes(&buf, 0).ptr()), 0, "cut at {cut}");
    }

    // An event that claims more bytes than the sequence has.
    for size in [1u32 << 20, 0x7FFF_FFFF, 0xFFFF_FFF8, u32::MAX] {
        let mut seq = Seq::new(&mut host);
        seq.event(10, midi, size, &note_on());
        assert_eq!(rig.delivered(seq.buf().ptr()), 0, "event size {size}");
    }

    // Wrong types and malformed MIDI: an Int, empty, a lone status, data bytes first.
    for (kind, data) in [
        (int, &note_on()[..]),
        (midi, &[][..]),
        (midi, &[0x90][..]),
        (midi, &[0x90, 60][..]),
        (midi, &[0x40, 0x40, 0x40][..]),
        (midi, &[0x90, 0xFF, 0xFF][..]),
        (0, &note_on()[..]),
        (u32::MAX, &note_on()[..]),
    ] {
        let mut seq = Seq::new(&mut host);
        seq.atom(10, kind, data);
        assert_eq!(rig.delivered(seq.buf().ptr()), 0, "kind {kind} data {data:?}");
    }

    // Frames outside the block are skipped.
    for frame in [-1i64, i64::MIN, FRAMES as i64, FRAMES as i64 + 1, i64::MAX] {
        let mut seq = Seq::new(&mut host);
        seq.midi(&mut host, frame, &note_on());
        assert_eq!(rig.delivered(seq.buf().ptr()), 0, "frame {frame}");
    }

    // Out of order times still deliver both, in the order given.
    let mut late = Seq::new(&mut host);
    late.midi(&mut host, 100, &note_on());
    late.midi(&mut host, 20, &note_on());
    assert_eq!(rig.delivered(late.buf().ptr()), 2);

    // A buffer that is not 8-byte aligned is not read.
    let mut aligned = good.buf();
    assert_eq!(rig.delivered(aligned.misaligned()), 0);

    // A good event after a bad one in the same sequence is not lost; a bad
    // event ends the walk instead.
    let mut mixed = Seq::new(&mut host);
    mixed.midi(&mut host, 5, &note_on());
    mixed.atom(6, int, &[1, 2, 3, 4]);
    mixed.midi(&mut host, 7, &note_on());
    assert_eq!(rig.delivered(mixed.buf().ptr()), 2);

    // Time position objects that are wrong or damaged leave the transport alone.
    *TRANSPORT.lock().unwrap() = None;
    let object = host.urid(OBJECT);
    let good_props = [(SPEED, Value::Float(1.0)), (BPM, Value::Float(100.0))];
    let not_position = common::object(&mut host, INT, &good_props);
    let mut bytes = common::object(&mut host, POSITION, &good_props);
    // Cut at every length: a cut on a property boundary is a shorter valid
    // object, so these only have to be survived.
    for n in 0..bytes.len() {
        let mut seq = Seq::new(&mut host);
        seq.atom(0, object, &bytes[..n]);
        rig.delivered(seq.buf().ptr());
    }
    *TRANSPORT.lock().unwrap() = None;
    // A fresh instance has no running transport. Then: a property that claims
    // a huge value size, and an object of another type.
    drop(rig);
    let mut rig = Rig::new(&host);
    bytes[16..20].copy_from_slice(&u32::MAX.to_ne_bytes());
    for body in [bytes, not_position] {
        let mut seq = Seq::new(&mut host);
        seq.atom(0, object, &body);
        rig.delivered(seq.buf().ptr());
    }
    let seen = *TRANSPORT.lock().unwrap();
    assert!(seen.is_none_or(|t| !t.playing && t.tempo == 0.0), "damaged positions changed the transport: {seen:?}");

    // Non-finite and nonsense numbers are dropped field by field.
    let junk = common::object(
        &mut host,
        POSITION,
        &[
            (SPEED, Value::Float(f32::NAN)),
            (BPM, Value::Float(f32::INFINITY)),
            (BAR, Value::Long(i64::MIN)),
            (BEATS_PER_BAR, Value::Float(-3.0)),
            (BEAT_UNIT, Value::Int(0)),
            (FRAME, Value::Long(-5)),
            (BAR_BEAT, Value::Double(f64::NAN)),
        ],
    );
    let mut seq = Seq::new(&mut host);
    seq.atom(0, object, &junk);
    rig.delivered(seq.buf().ptr());
    let seen = *TRANSPORT.lock().unwrap();
    if let Some(t) = seen {
        assert!(!t.playing && t.tempo == 0.0 && t.sample_pos == 0 && t.bar_number == 0, "{t:?}");
        assert!(t.quarter_notes.is_finite() && t.bar_start.is_finite() && t.seconds.is_finite());
    }

    // Output capacities that cannot hold a sequence: nothing is written.
    for capacity in [0usize, 1, 7] {
        let mut out = OutSeq::new(capacity);
        (rig.desc.connect_port)(rig.instance, 5, out.buf.ptr());
        (rig.desc.connect_port)(rig.instance, 4, good.buf().ptr());
        (rig.desc.run)(rig.instance, FRAMES);
        assert!(out.sentinel_intact());
        assert_eq!(out.size() as usize, capacity);
    }
    (rig.desc.connect_port)(rig.instance, 5, rig.out.buf.ptr());

    // And a good event still plays.
    assert_eq!(rig.delivered(good.buf().ptr()), 1);
    assert_eq!(rig.left[9], 0.0);
    assert_eq!(rig.left[10], 0.5);
}

#[test]
fn random_bytes_under_a_valid_header_are_survived() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut host = Host::new();
    let rig = Rig::new(&host);
    let sequence = host.urid(SEQUENCE);
    let midi = host.urid(MIDI_EVENT);
    let object = host.urid(OBJECT);
    let position = host.urid(POSITION);
    let mut state = 0x5EED_1234u32;
    let mut next = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state >> 8
    };
    for round in 0..4_000 {
        let length = (next() % 200) as usize;
        let mut body: Vec<u8> = (0..length).map(|_| next() as u8).collect();
        // Make a fair share of the words real types, so the fuzz gets inside events and objects.
        for at in (0..length.saturating_sub(4)).step_by(4) {
            match next() % 8 {
                0 => body[at..at + 4].copy_from_slice(&midi.to_ne_bytes()),
                1 => body[at..at + 4].copy_from_slice(&object.to_ne_bytes()),
                2 => body[at..at + 4].copy_from_slice(&position.to_ne_bytes()),
                3 => body[at..at + 4].copy_from_slice(&(next() % 16).to_ne_bytes()),
                _ => {}
            }
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&((8 + length) as u32).to_ne_bytes());
        bytes.extend_from_slice(&sequence.to_ne_bytes());
        bytes.extend_from_slice(&[0u8; 8]);
        bytes.extend_from_slice(&body);
        let mut buf = AtomBuf::from_bytes(&bytes, 0);
        let frames = [1u32, 7, 64, FRAMES][round % 4];
        (rig.desc.connect_port)(rig.instance, 4, buf.ptr());
        (rig.desc.run)(rig.instance, frames);
        assert!(rig.out.sentinel_intact(), "round {round}");
        assert!(rig.out.size() <= 8 + 24 * 4, "round {round}");
    }
}
