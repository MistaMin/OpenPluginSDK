//! Notes, MIDI out, transport, sidechain and freewheel through the exported
//! LV2 descriptor, as a host drives them: ports connected, atoms in host
//! memory, `run`.

mod common;

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use common::plugins::{Duck, Synth, EVENTS, OFFLINE, TRANSPORT};
use common::*;
use openplugin_core::Plugin;
use openplugin_targets::lv2::{self, Lv2Descriptor, PortMap};

/// One plugin is installed per process, so the tests take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

struct Rig {
    desc: &'static Lv2Descriptor,
    instance: *mut c_void,
}

impl Rig {
    fn new(create: fn() -> Box<dyn Plugin>, features: *const *const lv2::Lv2Feature) -> Self {
        lv2::set_creator(create);
        let desc = unsafe { &*lv2::descriptor(0) };
        let instance = (desc.instantiate)(desc, 48_000.0, std::ptr::null(), features);
        assert!(!instance.is_null());
        Self { desc, instance }
    }

    fn connect(&self, port: u32, data: *mut c_void) {
        (self.desc.connect_port)(self.instance, port, data);
    }

    fn run(&self, frames: u32) {
        (self.desc.run)(self.instance, frames);
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        (self.desc.cleanup)(self.instance);
    }
}

fn synth() -> Box<dyn Plugin> {
    Box::new(Synth::default())
}

fn duck() -> Box<dyn Plugin> {
    Box::new(Duck::default())
}

fn note(status: u8, note: u8, velocity: u8) -> [u8; 3] {
    [status, note, velocity]
}

#[test]
fn port_maps() {
    let _g = serial();
    let s = PortMap::of(&Synth::default());
    assert_eq!((s.first_param(), s.freewheel(), s.atom_in(), s.atom_out(), s.sidechain), (2, 3, Some(4), Some(5), 0));
    let d = PortMap::of(&Duck::default());
    assert_eq!((d.first_param(), d.freewheel(), d.atom_in(), d.atom_out()), (4, 5, Some(6), Some(7)));
    assert_eq!((d.sidechain, d.sidechain_first()), (2, 8));
}

#[test]
fn instrument_plays_notes_at_their_offsets_and_sends_midi_out() {
    let _g = serial();
    let mut host = Host::new();
    let rig = Rig::new(synth, host.features());
    let mut left = vec![9.0f32; 512];
    let mut right = vec![9.0f32; 512];
    let mut input = Seq::new(&mut host);
    input.midi(&mut host, 100, &note(0x95, 60, 100));
    input.midi(&mut host, 150, &[0xB0, 7, 99]);
    input.midi(&mut host, 300, &note(0x85, 60, 0));
    let mut events_in = input.buf();
    let mut events_out = OutSeq::new(8 + 24 * 8);
    rig.connect(0, left.as_mut_ptr().cast());
    rig.connect(1, right.as_mut_ptr().cast());
    rig.connect(4, events_in.ptr());
    rig.connect(5, events_out.buf.ptr());
    rig.run(512);
    assert_eq!(left[99], 0.0);
    assert_eq!(left[100], 0.5);
    assert_eq!(right[299], 0.5);
    assert_eq!(right[300], 0.0);
    assert_eq!(left[511], 0.0);
    // MIDI out: the same notes an octave up, at the frames the plugin gave.
    let midi = host.urid(MIDI_EVENT);
    assert_eq!(events_out.kind(), host.urid(SEQUENCE));
    assert_eq!(events_out.size(), 8 + 3 * 24);
    let events = events_out.events();
    assert_eq!(
        events,
        vec![
            (100, midi, vec![0x95, 72, 100]),
            (150, midi, vec![0xB0, 7, 99]),
            (300, midi, vec![0x80, 72, 0]),
        ]
    );
    assert!(events_out.sentinel_intact());
    // A block with no input writes an empty sequence, not stale events.
    let mut quiet = Seq::new(&mut host).buf();
    rig.connect(4, quiet.ptr());
    rig.run(64);
    assert_eq!(events_out.size(), 8);
    assert!(events_out.events().is_empty());
}

#[test]
fn midi_out_stops_at_the_hosts_capacity() {
    let _g = serial();
    let mut host = Host::new();
    let rig = Rig::new(synth, host.features());
    let mut input = Seq::new(&mut host);
    for i in 0..5 {
        input.midi(&mut host, i * 10, &note(0x90, 40 + i as u8, 90));
    }
    let mut events_in = input.buf();
    let mut left = vec![0.0f32; 128];
    rig.connect(0, left.as_mut_ptr().cast());
    rig.connect(4, events_in.ptr());
    for (capacity, expected) in [(8 + 24 * 2, 2usize), (8 + 24 * 2 + 23, 2), (8, 0), (4, 0), (0, 0), (8 + 24 * 5, 5)] {
        let mut out = OutSeq::new(capacity);
        rig.connect(5, out.buf.ptr());
        rig.run(128);
        assert!(out.sentinel_intact(), "capacity {capacity} overrun");
        if capacity >= 8 {
            assert_eq!(out.events().len(), expected, "capacity {capacity}");
            assert_eq!(out.size() as usize, 8 + 24 * expected);
        }
    }
    // The plugin's queue was drained every time: nothing carries over.
    let mut out = OutSeq::new(8 + 24 * 8);
    let mut quiet = Seq::new(&mut host).buf();
    rig.connect(4, quiet.ptr());
    rig.connect(5, out.buf.ptr());
    rig.run(128);
    assert!(out.events().is_empty());
    // A misaligned output buffer is left alone.
    let mut out = OutSeq::new(8 + 24 * 8);
    rig.connect(4, events_in.ptr());
    rig.connect(5, out.buf.misaligned());
    rig.run(128);
    assert!(out.sentinel_intact());
    assert_eq!(out.size() as usize, 8 + 24 * 8, "a misaligned port is not written");
}

fn position(host: &mut Host, otype: &str, props: &[(&str, Value)]) -> Vec<u8> {
    object(host, otype, props)
}

#[test]
fn time_position_reaches_the_plugin_as_transport() {
    let _g = serial();
    let mut host = Host::new();
    let rig = Rig::new(duck, host.features());
    let mut input = Seq::new(&mut host);
    let body = position(
        &mut host,
        POSITION,
        &[
            (SPEED, Value::Float(1.0)),
            (BPM, Value::Float(120.0)),
            (BAR, Value::Long(3)),
            (BAR_BEAT, Value::Float(1.5)),
            (BEATS_PER_BAR, Value::Float(6.0)),
            (BEAT_UNIT, Value::Int(8)),
            (FRAME, Value::Long(96_000)),
        ],
    );
    let object = host.urid(OBJECT);
    input.atom(0, object, &body);
    let mut events_in = input.buf();
    let mut left = vec![0.0f32; 256];
    rig.connect(0, left.as_mut_ptr().cast());
    rig.connect(6, events_in.ptr());
    *TRANSPORT.lock().unwrap() = None;
    rig.run(256);
    let t = TRANSPORT.lock().unwrap().expect("set_transport was called");
    assert!(t.playing);
    assert_eq!(t.tempo, 120.0);
    assert_eq!((t.numerator, t.denominator), (6, 8));
    assert_eq!(t.bar_number, 3);
    assert_eq!(t.sample_pos, 96_000);
    assert!((t.seconds - 2.0).abs() < 1e-9);
    // Six eighths per bar is three quarters; bar 3 starts at 9, and 1.5 eighths in is 0.75.
    assert_eq!(t.bar_start, 9.0);
    assert!((t.quarter_notes - 9.75).abs() < 1e-9);

    // No new position: the transport carries on from where it was.
    let mut quiet = Seq::new(&mut host).buf();
    rig.connect(6, quiet.ptr());
    rig.run(256);
    let t = TRANSPORT.lock().unwrap().unwrap();
    assert_eq!(t.sample_pos, 96_256);
    assert!((t.quarter_notes - (9.75 + 256.0 / 48_000.0 * 2.0)).abs() < 1e-6);

    // A stop (Double speed 0, Blank object, Int/Double fields) is read too.
    let mut stop = Seq::new(&mut host);
    let body = position(&mut host, POSITION, &[(SPEED, Value::Double(0.0)), (BPM, Value::Double(90.0)), (BAR, Value::Int(0))]);
    let blank = host.urid(BLANK);
    stop.atom(10, blank, &body);
    let mut stop = stop.buf();
    rig.connect(6, stop.ptr());
    rig.run(32);
    let t = TRANSPORT.lock().unwrap().unwrap();
    assert!(!t.playing);
    assert_eq!(t.tempo, 90.0);
    assert_eq!(t.bar_number, 0);
}

#[test]
fn sidechain_reaches_process_buses() {
    let _g = serial();
    let host = Host::new();
    let rig = Rig::new(duck, host.features());
    let mut in_l = vec![0.25f32; 100];
    let mut in_r = vec![0.5f32; 100];
    let mut out_l = vec![0.0f32; 100];
    let mut out_r = vec![0.0f32; 100];
    let mut side_l = vec![0.125f32; 100];
    let mut side_r = vec![0.0625f32; 100];
    rig.connect(0, in_l.as_mut_ptr().cast());
    rig.connect(1, in_r.as_mut_ptr().cast());
    rig.connect(2, out_l.as_mut_ptr().cast());
    rig.connect(3, out_r.as_mut_ptr().cast());
    rig.run(100);
    assert_eq!((out_l[7], out_r[7]), (0.25, 0.5), "no sidechain connected: it reads as silence");
    rig.connect(8, side_l.as_mut_ptr().cast());
    rig.connect(9, side_r.as_mut_ptr().cast());
    rig.run(100);
    assert_eq!((out_l[7], out_r[99]), (0.375, 0.5625));
    // Unplugged again: back to silence.
    rig.connect(8, std::ptr::null_mut());
    rig.run(100);
    assert_eq!(out_l[7], 0.25);
}

#[test]
fn freewheel_port_sets_offline() {
    let _g = serial();
    let host = Host::new();
    let rig = Rig::new(duck, host.features());
    let freewheel = std::cell::Cell::new(0.0f32);
    rig.connect(5, freewheel.as_ptr().cast());
    OFFLINE.store(false, Ordering::SeqCst);
    rig.run(16);
    assert!(!OFFLINE.load(Ordering::SeqCst));
    freewheel.set(1.0);
    rig.run(16);
    assert!(OFFLINE.load(Ordering::SeqCst));
    freewheel.set(f32::NAN);
    rig.run(16);
    assert!(!OFFLINE.load(Ordering::SeqCst));
    freewheel.set(1.0);
    rig.run(16);
    assert!(OFFLINE.load(Ordering::SeqCst));
    freewheel.set(0.0);
    rig.run(16);
    assert!(!OFFLINE.load(Ordering::SeqCst));
}

#[test]
fn without_urid_map_atoms_are_ignored_and_audio_runs() {
    let _g = serial();
    let mut host = Host::new();
    let rig = Rig::new(synth, std::ptr::null());
    let mut input = Seq::new(&mut host);
    input.midi(&mut host, 0, &note(0x90, 60, 100));
    let mut events_in = input.buf();
    let mut out = OutSeq::new(8 + 24 * 4);
    let mut left = vec![1.0f32; 64];
    rig.connect(0, left.as_mut_ptr().cast());
    rig.connect(4, events_in.ptr());
    rig.connect(5, out.buf.ptr());
    EVENTS.store(0, Ordering::SeqCst);
    rig.run(64);
    assert_eq!(EVENTS.load(Ordering::SeqCst), 0);
    assert_eq!(left[10], 0.0);
    assert_eq!(out.kind(), 0, "the output sequence was not touched");
    assert!(out.sentinel_intact());
}

#[test]
fn ttl_lists_the_new_ports() {
    let _g = serial();
    let synth = lv2::plugin_ttl(&Synth::default(), false);
    assert!(synth.contains("lv2:InstrumentPlugin"));
    assert!(synth.contains("lv2:requiredFeature urid:map"));
    assert!(!synth.contains("\"in_l\""), "an instrument has no main input");
    assert!(synth.contains("lv2:index 0 ; lv2:symbol \"out_l\""));
    assert!(synth.contains("lv2:index 2 ; lv2:symbol \"level\""));
    assert!(synth.contains("lv2:designation lv2:freeWheeling"));
    assert!(synth.contains("lv2:index 4 ; lv2:symbol \"events_in\""));
    assert!(synth.contains("lv2:index 5 ; lv2:symbol \"events_out\""));
    assert!(synth.contains("atom:supports midi:MidiEvent , time:Position"));
    assert!(!synth.contains("isSideChain"));
    let duck = lv2::plugin_ttl(&Duck::default(), false);
    assert_eq!(duck.matches("lv2:isSideChain").count(), 2);
    assert!(duck.contains("lv2:index 8 ; lv2:symbol \"sidechain_1\""));
    assert!(duck.contains("lv2:index 9 ; lv2:symbol \"sidechain_2\""));
    assert!(duck.contains("lv2:index 4 ; lv2:symbol \"level\""));
    assert!(duck.contains("lv2:index 0 ; lv2:symbol \"in_l\""));
}

/// Write each test plugin as a bundle and run `lv2_validate` over it when the
/// tool is installed (CI installs it).
#[test]
fn generated_ttl_passes_lv2_validate_when_installed() {
    let _g = serial();
    let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("lv2-ports-bundles");
    let _ = std::fs::create_dir_all(&root);
    for (name, plugin) in [("synth", Box::new(Synth::default()) as Box<dyn Plugin>), ("duck", Box::new(Duck::default()))] {
        let dir = root.join(format!("{name}.lv2"));
        std::fs::create_dir_all(&dir).unwrap();
        lv2::write_bundle(plugin.as_ref(), &dir, name, false).unwrap();
        let Ok(output) = std::process::Command::new("lv2_validate")
            .current_dir(&dir)
            .args(["manifest.ttl", &format!("{name}.ttl")])
            .output()
        else {
            eprintln!("lv2_validate is not installed; skipped");
            return;
        };
        let log = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(log.contains("Found 0 errors"), "{name}: {log}");
        assert!(!log.lines().any(|l| l.starts_with("error")), "{name}: {log}");
    }
}
