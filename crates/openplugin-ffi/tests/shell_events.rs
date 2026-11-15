//! Events, MIDI out, transport, sidechain, layouts, offline and kind through
//! the shell ABI, plus hostile input. Plugins record into a shared log, so the
//! tests serialise on a lock.

use std::sync::Mutex;

use openplugin_core::layout::{IoConfig, NotePorts, SpeakerLayout};
use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Transport, Unit};
use openplugin_ffi::shell::{self, *};

static LOCK: Mutex<()> = Mutex::new(());
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn log(line: String) {
    LOG.lock().unwrap().push(line);
}

fn take_log() -> Vec<String> {
    std::mem::take(&mut *LOG.lock().unwrap())
}

static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", 0.0, 2.0, 1.0, Unit::None)];

static INFO: PluginInfo = PluginInfo {
    id: "com.example.shell-events",
    name: "Shell Events",
    vendor: "Test",
    url: "https://example.com",
    email: "a@b.c",
    version: "0.1.0",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

static LAYOUTS: [IoConfig; 3] = [
    IoConfig {
        input: SpeakerLayout::Stereo,
        output: SpeakerLayout::Stereo,
        sidechain: Some(SpeakerLayout::Stereo),
    },
    IoConfig {
        input: SpeakerLayout::Mono,
        output: SpeakerLayout::Mono,
        sidechain: None,
    },
    IoConfig {
        input: SpeakerLayout::Surround51,
        output: SpeakerLayout::Surround51,
        sidechain: None,
    },
];

struct Probe {
    gain: f32,
    queue: [Option<(u32, [u8; 3])>; 8],
}

impl Default for Probe {
    fn default() -> Self {
        Self { gain: 1.0, queue: [None; 8] }
    }
}

impl Plugin for Probe {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {
        self.gain = 1.0;
    }
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id == "gain" {
            self.gain = value;
            true
        } else {
            false
        }
    }
    fn param(&self, _: &str) -> Option<f32> {
        Some(self.gain)
    }
    fn supported_layouts(&self) -> &'static [IoConfig] {
        &LAYOUTS
    }
    fn note_ports(&self) -> NotePorts {
        NotePorts { input: true, output: true }
    }
    fn set_offline(&mut self, offline: bool) {
        log(format!("offline {offline}"));
    }
    fn set_transport(&mut self, t: &Transport) {
        log(format!(
            "transport playing={} tempo={} sig={}/{} qn={} pos={}",
            t.playing, t.tempo, t.numerator, t.denominator, t.quarter_notes, t.sample_pos
        ));
    }
    fn note_on_channel(&mut self, offset: usize, channel: u8, note: u8, velocity: u8) {
        log(format!("on {offset} {channel} {note} {velocity}"));
        if let Some(slot) = self.queue.iter_mut().find(|s| s.is_none()) {
            *slot = Some((offset as u32, [0x90 | channel, note, velocity]));
        }
    }
    fn note_off(&mut self, offset: usize, note: u8) {
        log(format!("off {offset} {note}"));
    }
    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        log(format!("midi {offset} {} {} {}", msg[0], msg[1], msg[2]));
    }
    fn pull_midi(&mut self) -> Option<(u32, [u8; 3])> {
        self.queue.iter_mut().find_map(|s| s.take())
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        log(format!("block {frames}"));
        for channel in io.iter_mut() {
            for s in channel[..frames].iter_mut() {
                *s *= self.gain;
            }
        }
    }
    fn process_buses(&mut self, io: &mut [Vec<f32>], sidechain: Option<&[Vec<f32>]>, frames: usize) {
        self.process(io, frames);
        if let Some(side) = sidechain {
            for channel in io.iter_mut() {
                for (i, s) in channel[..frames].iter_mut().enumerate() {
                    *s += 0.25 * side[0][i];
                }
            }
        }
    }
}

#[derive(Default)]
struct Inst;
static INST_INFO: PluginInfo = PluginInfo { is_instrument: true, ..INFO };
impl Plugin for Inst {
    fn info(&self) -> &'static PluginInfo {
        &INST_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn note_ports(&self) -> NotePorts {
        NotePorts { input: true, output: false }
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for channel in io.iter_mut() {
            channel[..frames].fill(0.5);
        }
    }
}

static NOTE_LAYOUTS: [IoConfig; 1] = [IoConfig {
    input: SpeakerLayout::Absent,
    output: SpeakerLayout::Absent,
    sidechain: None,
}];
#[derive(Default)]
struct NoteFx;
impl Plugin for NoteFx {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn supported_layouts(&self) -> &'static [IoConfig] {
        &NOTE_LAYOUTS
    }
    fn note_ports(&self) -> NotePorts {
        NotePorts { input: true, output: true }
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
}

fn ev(kind: u32, offset: u32) -> OpEvent {
    OpEvent { offset, kind, ..OpEvent::default() }
}
fn note_on(offset: u32, channel: u8, note: u8, velocity: u8) -> OpEvent {
    OpEvent { channel, note, velocity, ..ev(EVENT_NOTE_ON, offset) }
}
fn note_off(offset: u32, note: u8) -> OpEvent {
    OpEvent { note, ..ev(EVENT_NOTE_OFF, offset) }
}
fn raw(offset: u32, a: u8, b: u8, c: u8) -> OpEvent {
    OpEvent { midi: [a, b, c, 0], ..ev(EVENT_MIDI, offset) }
}

struct Block<'a> {
    inputs: Vec<&'a [f32]>,
    outputs: Vec<&'a mut [f32]>,
    side: Vec<&'a [f32]>,
    events: &'a [OpEvent],
    transport: Option<OpTransport>,
}

fn run(instance: *mut ShellInstance, frames: u32, b: &mut Block) -> u32 {
    let ins: Vec<*const f32> = b.inputs.iter().map(|s| s.as_ptr()).collect();
    let outs: Vec<*mut f32> = b.outputs.iter_mut().map(|s| s.as_mut_ptr()).collect();
    let side: Vec<*const f32> = b.side.iter().map(|s| s.as_ptr()).collect();
    let args = OpProcess {
        struct_size: std::mem::size_of::<OpProcess>() as u32,
        frames,
        inputs: if ins.is_empty() { std::ptr::null() } else { ins.as_ptr() },
        input_channels: ins.len() as u32,
        output_channels: outs.len() as u32,
        outputs: if outs.is_empty() { std::ptr::null() } else { outs.as_ptr() },
        sidechain: if side.is_empty() { std::ptr::null() } else { side.as_ptr() },
        sidechain_channels: side.len() as u32,
        event_count: b.events.len() as u32,
        events: if b.events.is_empty() { std::ptr::null() } else { b.events.as_ptr() },
        transport: b.transport.as_ref().map_or(std::ptr::null(), |t| t as *const _),
    };
    unsafe { process_events(instance, &args) }
}

fn mk() -> *mut ShellInstance {
    let p = create::<Probe>(48_000.0, 256, 2);
    assert!(!p.is_null());
    p
}

fn drain(instance: *mut ShellInstance, capacity: usize) -> Vec<OpMidiEvent> {
    let mut out = vec![OpMidiEvent::default(); capacity];
    let n = unsafe { midi_out(instance, out.as_mut_ptr(), capacity as u32) } as usize;
    out.truncate(n);
    out
}

#[test]
fn abi_version_and_kinds() {
    let _g = lock();
    assert_eq!(ABI, 7);
    assert_eq!(kind::<Probe>(), KIND_EFFECT);
    assert_eq!(kind::<Inst>(), KIND_INSTRUMENT);
    assert_eq!(kind::<NoteFx>(), KIND_NOTE_EFFECT);
    assert_eq!(note_ports::<Probe>(), NOTE_PORT_INPUT | NOTE_PORT_OUTPUT);
    assert_eq!(note_ports::<Inst>(), NOTE_PORT_INPUT);
}

#[test]
fn notes_midi_in_sorted_and_split_midi_out_pulled_in_pages() {
    let _g = lock();
    take_log();
    let i = mk();
    let input = [0.5f32; 64];
    let (mut l, mut r) = ([0.0f32; 64], [0.0f32; 64]);
    // Out of order on purpose; the two at offset 10 keep their given order.
    let events = [
        note_off(40, 61),
        note_on(10, 3, 60, 100),
        raw(5, 0xB0, 7, 99),
        raw(10, 0x91, 62, 80),
        note_on(63, 0, 1, 1),
    ];
    let mut b = Block { inputs: vec![&input, &input], outputs: vec![&mut l, &mut r], side: vec![], events: &events, transport: None };
    assert_eq!(run(i, 64, &mut b), 1);
    assert_eq!(l[17], 0.5);
    assert_eq!(
        take_log(),
        [
            "block 5", "midi 0 176 7 99", "block 5", "on 0 3 60 100", "on 0 1 62 80", "block 30",
            "off 0 61", "block 23", "on 0 0 1 1", "block 1",
        ]
    );
    // Offsets are block offsets, read in two pages.
    let first = drain(i, 2);
    assert_eq!(first.len(), 2);
    assert_eq!((first[0].offset, first[0].data), (10, [0x93, 60, 100, 0]));
    assert_eq!((first[1].offset, first[1].data), (10, [0x91, 62, 80, 0]));
    let rest = drain(i, 8);
    assert_eq!(rest.len(), 1);
    assert_eq!((rest[0].offset, rest[0].data), (63, [0x90, 1, 1, 0]));
    assert!(drain(i, 8).is_empty());
    // The next block clears what nobody read.
    let mut b = Block { inputs: vec![&input, &input], outputs: vec![&mut l, &mut r], side: vec![], events: &[], transport: None };
    assert_eq!(run(i, 64, &mut b), 1);
    assert!(drain(i, 8).is_empty());
    assert_eq!(unsafe { midi_dropped(i) }, 0);
    unsafe { destroy(i) };
}

#[test]
fn parameter_event_changes_the_gain_at_its_offset() {
    let _g = lock();
    take_log();
    let i = mk();
    let input = [1.0f32; 32];
    let (mut l, mut r) = ([0.0f32; 32], [0.0f32; 32]);
    let plain = OpEvent { index: 0, value: 2.0, ..ev(EVENT_PARAM_PLAIN, 8) };
    let norm = OpEvent { index: 0, value: 0.5, ..ev(EVENT_PARAM_NORMALIZED, 20) };
    let events = [norm, plain];
    let mut b = Block { inputs: vec![&input, &input], outputs: vec![&mut l, &mut r], side: vec![], events: &events, transport: None };
    assert_eq!(run(i, 32, &mut b), 1);
    assert_eq!((l[7], l[8], l[19], l[20], r[31]), (1.0, 2.0, 2.0, 1.0, 1.0));
    unsafe { destroy(i) };
}

#[test]
fn transport_reaches_the_plugin_and_null_leaves_it() {
    let _g = lock();
    take_log();
    let i = mk();
    let input = [0.0f32; 16];
    let (mut l, mut r) = ([0.0f32; 16], [0.0f32; 16]);
    let t = OpTransport {
        flags: TRANSPORT_PLAYING | TRANSPORT_HAS_TEMPO | TRANSPORT_HAS_SIGNATURE | TRANSPORT_HAS_BEATS,
        numerator: 7,
        denominator: 8,
        tempo: 133.5,
        sample_pos: 4800,
        quarter_notes: 2.5,
        ..OpTransport::default()
    };
    let mut b = Block { inputs: vec![&input, &input], outputs: vec![&mut l, &mut r], side: vec![], events: &[], transport: Some(t) };
    assert_eq!(run(i, 16, &mut b), 1);
    let mut b = Block { inputs: vec![&input, &input], outputs: vec![&mut l, &mut r], side: vec![], events: &[], transport: None };
    assert_eq!(run(i, 16, &mut b), 1);
    let lines = take_log();
    assert_eq!(lines[0], "transport playing=true tempo=133.5 sig=7/8 qn=2.5 pos=4800");
    assert_eq!(lines.iter().filter(|l| l.starts_with("transport")).count(), 1);
    unsafe { destroy(i) };
}

#[test]
fn sidechain_reaches_process_buses_and_is_optional() {
    let _g = lock();
    take_log();
    let i = mk();
    let input = [0.5f32; 16];
    let side = [1.0f32; 16];
    let (mut l, mut r) = ([0.0f32; 16], [0.0f32; 16]);
    let mut b = Block { inputs: vec![&input, &input], outputs: vec![&mut l, &mut r], side: vec![&side], events: &[], transport: None };
    assert_eq!(run(i, 16, &mut b), 1);
    assert_eq!((l[3], r[3]), (0.75, 0.75));
    let mut b = Block { inputs: vec![&input, &input], outputs: vec![&mut l, &mut r], side: vec![], events: &[], transport: None };
    assert_eq!(run(i, 16, &mut b), 1);
    assert_eq!(l[3], 0.5);
    unsafe { destroy(i) };
}

#[test]
fn layouts_are_described_in_canonical_order_and_selectable() {
    let _g = lock();
    assert_eq!(layout_count::<Probe>(), 3);
    let mut layout = unsafe { std::mem::zeroed::<OpLayout>() };
    assert_eq!(unsafe { write_layout::<Probe>(0, &mut layout) }, 1);
    assert_eq!((layout.input.kind, layout.input.channels), (BUS_STEREO, 2));
    assert_eq!(&layout.input.roles[..3], &[0, 1, ROLE_NONE]);
    assert_eq!((layout.sidechain.kind, layout.sidechain.channels), (BUS_STEREO, 2));
    assert_eq!(unsafe { write_layout::<Probe>(1, &mut layout) }, 1);
    assert_eq!((layout.output.kind, layout.output.channels, layout.sidechain.kind), (BUS_MONO, 1, BUS_ABSENT));
    assert_eq!(layout.output.roles[0], ROLE_NONE);
    assert_eq!(unsafe { write_layout::<Probe>(2, &mut layout) }, 1);
    assert_eq!((layout.output.kind, layout.output.channels), (BUS_SURROUND, 6));
    // 5.1 in the canonical CLAP order: FL FR FC LFE BL BR.
    assert_eq!(&layout.output.roles[..7], &[0, 1, 2, 3, 4, 5, ROLE_NONE]);
    assert!(layout.name.starts_with(b"5.1"));
    assert_eq!(unsafe { write_layout::<Probe>(3, &mut layout) }, 0);
    assert_eq!(unsafe { write_layout::<Probe>(0, std::ptr::null_mut()) }, 0);

    let i = mk();
    assert_eq!(unsafe { layout_selected(i) }, 0);
    assert_eq!(unsafe { set_layout(i, 3) }, 0);
    assert_eq!(unsafe { set_layout(i, 2) }, 1);
    assert_eq!(unsafe { layout_selected(i) }, 2);
    // Six channels through the block; each keeps its own samples.
    let chans: Vec<Vec<f32>> = (0..6).map(|c| vec![c as f32 + 1.0; 16]).collect();
    let mut outs: Vec<Vec<f32>> = (0..6).map(|_| vec![0.0; 16]).collect();
    let mut b = Block {
        inputs: chans.iter().map(|c| c.as_slice()).collect(),
        outputs: outs.iter_mut().map(|c| c.as_mut_slice()).collect(),
        side: vec![],
        events: &[],
        transport: None,
    };
    assert_eq!(run(i, 16, &mut b), 1);
    assert_eq!((outs[0][0], outs[5][15]), (1.0, 6.0));
    unsafe { destroy(i) };
}

#[test]
fn offline_flag_reaches_the_plugin_on_the_next_block() {
    let _g = lock();
    take_log();
    let i = mk();
    let input = [0.0f32; 8];
    let (mut l, mut r) = ([0.0f32; 8], [0.0f32; 8]);
    unsafe { set_offline(i, 1) };
    assert!(take_log().is_empty());
    let mut b = Block { inputs: vec![&input, &input], outputs: vec![&mut l, &mut r], side: vec![], events: &[], transport: None };
    assert_eq!(run(i, 8, &mut b), 1);
    assert_eq!(take_log()[0], "offline true");
    unsafe { set_offline(i, 0) };
    assert_eq!(run(i, 8, &mut b), 1);
    assert_eq!(take_log()[0], "offline false");
    unsafe { destroy(i) };
}

#[test]
fn instrument_renders_with_no_input() {
    let _g = lock();
    let i = create::<Inst>(48_000.0, 64, 2);
    let (mut l, mut r) = ([0.0f32; 16], [0.0f32; 16]);
    let mut b = Block { inputs: vec![], outputs: vec![&mut l, &mut r], side: vec![], events: &[], transport: None };
    assert_eq!(run(i, 16, &mut b), 1);
    assert_eq!((l[0], r[15]), (0.5, 0.5));
    unsafe { destroy(i) };
}

#[test]
fn the_old_process_entry_still_works() {
    let _g = lock();
    let i = mk();
    let mut buf = [0.5f32; 16];
    let ok = unsafe { shell::process(i, buf.as_ptr(), buf.as_ptr(), buf.as_mut_ptr(), buf.as_mut_ptr(), 16) };
    assert_eq!(ok, 1);
    assert_eq!(buf[0], 0.5);
    unsafe { destroy(i) };
}

#[test]
fn hostile_input_is_refused_without_touching_anything() {
    let _g = lock();
    take_log();
    let i = mk();
    let input = [0.5f32; 32];
    let (mut l, mut r) = ([9.0f32; 32], [9.0f32; 32]);

    // Null instance and null args.
    let mut b = Block { inputs: vec![&input, &input], outputs: vec![&mut l, &mut r], side: vec![], events: &[], transport: None };
    assert_eq!(run(std::ptr::null_mut(), 32, &mut b), 0);
    assert_eq!(unsafe { process_events(i, std::ptr::null()) }, 0);

    // Too many events, block longer than prepared, small struct: refused, output untouched.
    let many = vec![note_on(0, 0, 60, 100); MAX_EVENTS + 1];
    let mut b = Block { inputs: vec![&input, &input], outputs: vec![&mut l, &mut r], side: vec![], events: &many, transport: None };
    assert_eq!(run(i, 32, &mut b), 0);
    let mut b = Block { inputs: vec![&input, &input], outputs: vec![&mut l, &mut r], side: vec![], events: &[], transport: None };
    assert_eq!(run(i, 257, &mut b), 0);
    let ins = [input.as_ptr(), input.as_ptr()];
    let outs = [l.as_mut_ptr(), r.as_mut_ptr()];
    let mut args = OpProcess {
        struct_size: 8,
        frames: 32,
        inputs: ins.as_ptr(),
        input_channels: 2,
        output_channels: 2,
        outputs: outs.as_ptr(),
        sidechain: std::ptr::null(),
        sidechain_channels: 0,
        event_count: 0,
        events: std::ptr::null(),
        transport: std::ptr::null(),
    };
    assert_eq!(unsafe { process_events(i, &args) }, 0);
    args.struct_size = std::mem::size_of::<OpProcess>() as u32;
    // Null arrays with counts, too many channels.
    for tweak in 0..5 {
        let mut a = args;
        match tweak {
            0 => a.inputs = std::ptr::null(),
            1 => a.outputs = std::ptr::null(),
            2 => a.event_count = 3,
            3 => a.input_channels = 65,
            _ => a.sidechain_channels = 2,
        }
        assert_eq!(unsafe { process_events(i, &a) }, 0, "tweak {tweak}");
    }
    assert_eq!((l[0], r[31]), (9.0, 9.0));
    assert!(take_log().is_empty());

    // Bad individual events are skipped; the block still runs.
    let bad = [
        note_on(32, 0, 60, 100),
        note_on(u32::MAX, 0, 60, 100),
        note_on(1, 16, 60, 100),
        note_on(1, 0, 128, 100),
        note_off(1, 200),
        raw(1, 0x10, 1, 2),
        raw(1, 0x90, 0x90, 2),
        ev(99, 1),
        OpEvent { index: 0, value: f32::NAN, ..ev(EVENT_PARAM_PLAIN, 1) },
        OpEvent { index: 77, value: 1.0, ..ev(EVENT_PARAM_PLAIN, 1) },
        OpEvent { index: 77, value: 1.0, ..ev(EVENT_PARAM_NORMALIZED, 1) },
    ];
    let mut b = Block { inputs: vec![&input, &input], outputs: vec![&mut l, &mut r], side: vec![], events: &bad, transport: None };
    assert_eq!(run(i, 32, &mut b), 1);
    // Offset 1 events with a valid shape but a bad index still cut the block.
    let lines = take_log();
    assert!(lines.iter().all(|l| l.starts_with("block")), "{lines:?}");
    assert_eq!(l[0], 0.5);

    // Null channel pointers read as silence and are not written.
    let ins = [std::ptr::null(), input.as_ptr()];
    let outs = [std::ptr::null_mut(), r.as_mut_ptr()];
    let a = OpProcess { inputs: ins.as_ptr(), outputs: outs.as_ptr(), ..args };
    assert_eq!(unsafe { process_events(i, &a) }, 1);

    // MIDI pull with nothing valid.
    assert_eq!(unsafe { midi_out(i, std::ptr::null_mut(), 8) }, 0);
    let mut one = OpMidiEvent::default();
    assert_eq!(unsafe { midi_out(i, &mut one, 0) }, 0);
    assert_eq!(unsafe { midi_out(std::ptr::null_mut(), &mut one, 1) }, 0);
    assert_eq!(unsafe { midi_dropped(std::ptr::null()) }, 0);
    unsafe { set_offline(std::ptr::null_mut(), 1) };
    assert_eq!(unsafe { set_layout(std::ptr::null_mut(), 0) }, 0);
    unsafe { destroy(i) };
}

#[test]
fn many_notes_in_one_block_come_out_in_order() {
    let _g = lock();
    take_log();
    let i = mk();
    let input = [0.0f32; 256];
    let (mut l, mut r) = ([0.0f32; 256], [0.0f32; 256]);
    let events: Vec<OpEvent> = (0..200).map(|k| note_on(k, 0, 60, 100)).collect();
    let mut b = Block { inputs: vec![&input, &input], outputs: vec![&mut l, &mut r], side: vec![], events: &events, transport: None };
    assert_eq!(run(i, 256, &mut b), 1);
    let got = drain(i, 1024);
    assert_eq!(got.len(), 200);
    assert!(got.windows(2).all(|w| w[0].offset <= w[1].offset));
    assert_eq!(unsafe { midi_dropped(i) }, 0);
    unsafe { destroy(i) };
}
