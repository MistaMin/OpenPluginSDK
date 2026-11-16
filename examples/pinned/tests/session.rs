//! The identity pinning proof through CLAP (porting toolkit task 2).
//!
//! A fake host saves a project with one build of the plugin and reopens it
//! with another whose parameter table has a different order (and one more
//! parameter). What a CLAP host stores, and all the project holds, is:
//!
//! - the plugin id from the descriptor;
//! - the state the plugin wrote (`clap.state`);
//! - an automation lane: points addressed by `clap_param_info.id`.
//!
//! With pinned ids the project reopens: the same plugin id, the saved values on
//! the same parameters, and the lane played back gives the same audio sample
//! for sample. The negative control is the same pair of builds without pins,
//! where a parameter's id is its position: the state is refused and the lane
//! moves other parameters, so the audio differs.

use std::ffi::{c_void, CStr};

use example_pinned::{PinnedA, PinnedB, PlainA, PlainB, CLAP_ID, DRIVE_ID, INVERT_ID, LEVEL_ID, OFFSET_ID};
use openplugin::core::Plugin;
use openplugin::targets::clap_abi::*;
use openplugin::targets::ClapAdapter;

const FRAMES: usize = 64;

/// One automation point: the parameter as the host stored it, the sample and the value.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Point {
    param_id: u32,
    time: u32,
    value: f64,
}

/// What the host keeps of a plugin in its project file.
#[derive(Debug, Clone, PartialEq)]
struct Project {
    plugin_id: String,
    state: Vec<u8>,
    lane: Vec<Point>,
}

struct Host<P: Plugin + 'static> {
    _adapter: ClapAdapter<P>,
    plugin: *const clap_plugin,
}

struct EventList(Vec<clap_event_param_value>);

unsafe extern "C" fn list_size(list: *const clap_input_events) -> u32 {
    let list = unsafe { &*((*list).ctx as *const EventList) };
    list.0.len() as u32
}

unsafe extern "C" fn list_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let list = unsafe { &*((*list).ctx as *const EventList) };
    &list.0[index as usize].header
}

unsafe extern "C" fn drop_output(_: *const clap_output_events, _: *const clap_event_header) -> bool {
    true
}

unsafe extern "C" fn write_all(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
    let out = unsafe { &mut *((*stream).ctx as *mut Vec<u8>) };
    out.extend_from_slice(unsafe { std::slice::from_raw_parts(buffer as *const u8, size as usize) });
    size as i64
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

unsafe extern "C" fn read_some(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
    let reader = unsafe { &mut *((*stream).ctx as *mut Reader) };
    let count = (reader.bytes.len() - reader.at).min(size as usize);
    unsafe { std::ptr::copy_nonoverlapping(reader.bytes[reader.at..].as_ptr(), buffer as *mut u8, count) };
    reader.at += count;
    count as i64
}

impl<P: Plugin + Default + 'static> Host<P> {
    fn open() -> Self {
        let adapter = ClapAdapter::new(P::default());
        let plugin = adapter.as_plugin();
        unsafe {
            assert!((*plugin).init.unwrap()(plugin));
            assert!((*plugin).activate.unwrap()(plugin, 48_000.0, 1, FRAMES as u32));
            assert!((*plugin).start_processing.unwrap()(plugin));
        }
        Self { _adapter: adapter, plugin }
    }

    fn ext<T>(&self, id: &CStr) -> &T {
        let ext = unsafe { (*self.plugin).get_extension.unwrap()(self.plugin, id.as_ptr()) } as *const T;
        assert!(!ext.is_null(), "CLAP extension {id:?}");
        unsafe { &*ext }
    }

    fn plugin_id(&self) -> String {
        unsafe { CStr::from_ptr((*(*self.plugin).desc).id) }.to_str().unwrap().to_string()
    }

    /// `(id, name)` of every parameter, in the order the plugin lists them.
    fn params(&self) -> Vec<(u32, String)> {
        let params = self.ext::<clap_plugin_params>(c"clap.params");
        let count = unsafe { params.count.unwrap()(self.plugin) };
        (0..count)
            .map(|index| {
                let mut info = unsafe { std::mem::zeroed::<clap_param_info>() };
                assert!(unsafe { params.get_info.unwrap()(self.plugin, index, &mut info) });
                (info.id, unsafe { CStr::from_ptr(info.name.as_ptr()) }.to_str().unwrap().to_string())
            })
            .collect()
    }

    fn id_of(&self, name: &str) -> u32 {
        self.params().into_iter().find(|(_, n)| n == name).unwrap_or_else(|| panic!("no parameter named {name}")).0
    }

    fn value(&self, param_id: u32) -> Option<f64> {
        let params = self.ext::<clap_plugin_params>(c"clap.params");
        let mut value = f64::NAN;
        unsafe { params.get_value.unwrap()(self.plugin, param_id, &mut value) }.then_some(value)
    }

    fn text(&self, param_id: u32, value: f64) -> Option<String> {
        let params = self.ext::<clap_plugin_params>(c"clap.params");
        let mut buffer = [0 as std::ffi::c_char; 64];
        unsafe { params.value_to_text.unwrap()(self.plugin, param_id, value, buffer.as_mut_ptr(), 64) }
            .then(|| unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_str().unwrap().to_string())
    }

    /// One block of a constant `input` on both channels, with `points` as parameter events.
    fn render(&self, input: f32, points: &[Point]) -> Vec<f32> {
        let mut points = points.to_vec();
        points.sort_by_key(|point| point.time);
        let list = EventList(
            points
                .iter()
                .map(|point| clap_event_param_value {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_param_value>() as u32,
                        time: point.time,
                        space_id: 0,
                        type_: EVENT_PARAM_VALUE,
                        flags: 0,
                    },
                    param_id: point.param_id,
                    cookie: std::ptr::null_mut(),
                    note_id: -1,
                    port_index: -1,
                    channel: -1,
                    key: -1,
                    value: point.value,
                })
                .collect(),
        );
        let in_events = clap_input_events { ctx: &list as *const EventList as *mut c_void, size: Some(list_size), get: Some(list_get) };
        let out_events = clap_output_events { ctx: std::ptr::null_mut(), try_push: Some(drop_output) };
        let mut inputs = [vec![input; FRAMES], vec![input; FRAMES]];
        let mut outputs = [vec![9.0f32; FRAMES], vec![9.0f32; FRAMES]];
        let mut in_ptrs: Vec<*mut f32> = inputs.iter_mut().map(|c| c.as_mut_ptr()).collect();
        let mut out_ptrs: Vec<*mut f32> = outputs.iter_mut().map(|c| c.as_mut_ptr()).collect();
        let audio_in = clap_audio_buffer { data32: in_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 };
        let mut audio_out = clap_audio_buffer { data32: out_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 };
        let process = clap_process {
            steady_time: 0,
            frames_count: FRAMES as u32,
            transport: std::ptr::null(),
            audio_inputs: &audio_in,
            audio_outputs: &mut audio_out,
            audio_inputs_count: 1,
            audio_outputs_count: 1,
            in_events: &in_events,
            out_events: &out_events,
        };
        assert_ne!(unsafe { (*self.plugin).process.unwrap()(self.plugin, &process) }, PROCESS_ERROR);
        assert_eq!(outputs[0], outputs[1]);
        outputs[0].clone()
    }

    /// Sets parameters the way a host does while it records: events at the block start.
    fn set(&self, values: &[(u32, f64)]) {
        let points: Vec<Point> = values.iter().map(|&(param_id, value)| Point { param_id, time: 0, value }).collect();
        self.render(0.0, &points);
    }

    fn save_state(&self) -> Vec<u8> {
        let state = self.ext::<clap_plugin_state>(c"clap.state");
        let mut bytes = Vec::new();
        let stream = clap_ostream { ctx: &mut bytes as *mut Vec<u8> as *mut c_void, write: Some(write_all) };
        assert!(unsafe { state.save.unwrap()(self.plugin, &stream) });
        bytes
    }

    fn load_state(&self, bytes: &[u8]) -> bool {
        let state = self.ext::<clap_plugin_state>(c"clap.state");
        let mut reader = Reader { bytes, at: 0 };
        let stream = clap_istream { ctx: &mut reader as *mut Reader as *mut c_void, read: Some(read_some) };
        unsafe { state.load.unwrap()(self.plugin, &stream) }
    }

    fn save_project(&self, lane: &[Point]) -> Project {
        Project { plugin_id: self.plugin_id(), state: self.save_state(), lane: lane.to_vec() }
    }
}

/// The lane of the proof: `level` and `offset` move in the middle of the block.
fn lane(level: u32, offset: u32) -> Vec<Point> {
    vec![
        Point { param_id: level, time: 16, value: 1.5 },
        Point { param_id: offset, time: 32, value: -0.5 },
        Point { param_id: level, time: 48, value: 0.25 },
    ]
}

/// The audio the lane must give after the saved values (level 0.5, offset
/// 0.25, inverted) with an input of 0.5, from the plugin's own formula.
fn expected() -> Vec<f32> {
    (0..FRAMES)
        .map(|n| {
            let level = if n < 16 { 0.5 } else if n < 48 { 1.5 } else { 0.25 };
            let offset = if n < 32 { 0.25 } else { -0.5 };
            -(0.5f32 * level + offset)
        })
        .collect()
}

#[test]
fn the_pinned_builds_show_the_host_their_pins_in_either_order() {
    let (a, b) = (Host::<PinnedA>::open(), Host::<PinnedB>::open());
    assert_eq!(a.plugin_id(), CLAP_ID);
    assert_eq!(b.plugin_id(), CLAP_ID);
    assert_eq!(a.params(), [(LEVEL_ID, "Level".to_string()), (OFFSET_ID, "Offset".to_string()), (INVERT_ID, "Invert".to_string())]);
    assert_eq!(
        b.params(),
        [(INVERT_ID, "Invert".to_string()), (DRIVE_ID, "Drive".to_string()), (OFFSET_ID, "Offset".to_string()), (LEVEL_ID, "Level".to_string())]
    );
    // Value, text and the unknown id, each by the host's id.
    for host_values in [(a.value(LEVEL_ID), a.value(OFFSET_ID), a.value(INVERT_ID)), (b.value(LEVEL_ID), b.value(OFFSET_ID), b.value(INVERT_ID))] {
        assert_eq!(host_values, (Some(1.0), Some(0.0), Some(0.0)));
    }
    assert_eq!(a.text(INVERT_ID, 1.0).as_deref(), Some("On"));
    assert_eq!(b.text(INVERT_ID, 1.0).as_deref(), Some("On"));
    for stale in [0, 1, 2, 3, DRIVE_ID + 1, u32::MAX] {
        assert_eq!(a.value(stale), None, "id {stale} is not a parameter of the pinned build");
        assert_eq!(b.value(stale), None);
        assert_eq!(a.text(stale, 0.0), None);
    }
    assert_eq!(a.value(DRIVE_ID), None, "the first build has no drive");
    assert_eq!(b.value(DRIVE_ID), Some(0.0));
}

#[test]
fn the_builds_without_pins_keep_the_position_as_the_id() {
    let (a, b) = (Host::<PlainA>::open(), Host::<PlainB>::open());
    assert_eq!(a.plugin_id(), "com.opengrid.example.plain");
    assert_eq!(a.params().iter().map(|p| p.0).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(b.params().iter().map(|p| p.0).collect::<Vec<_>>(), [0, 1, 2, 3]);
    // The positional state, byte for byte what it always was: magic, version 1, count, values.
    let mut want = b"OPST".to_vec();
    want.extend_from_slice(&[1, 0, 3, 0]);
    for value in [1.0f32, 0.0, 0.0] {
        want.extend_from_slice(&value.to_le_bytes());
    }
    assert_eq!(a.save_state(), want);
}

#[test]
fn a_project_saved_with_one_pinned_build_reopens_in_the_reordered_one() {
    let first = Host::<PinnedA>::open();
    first.set(&[(LEVEL_ID, 0.5), (OFFSET_ID, 0.25), (INVERT_ID, 1.0)]);
    let project = first.save_project(&lane(first.id_of("Level"), first.id_of("Offset")));
    let heard = first.render(0.5, &project.lane);
    assert_eq!(heard, expected(), "the first build plays its own lane");

    // The second build: another order, one more parameter.
    let second = Host::<PinnedB>::open();
    assert_eq!(second.plugin_id(), project.plugin_id, "the host finds the plugin by the id in the project");
    assert!(project.lane.iter().all(|point| second.params().iter().any(|p| p.0 == point.param_id)), "every lane still has its parameter");
    assert!(second.load_state(&project.state));
    assert_eq!(
        (second.value(LEVEL_ID), second.value(OFFSET_ID), second.value(INVERT_ID), second.value(DRIVE_ID)),
        (Some(0.5), Some(0.25), Some(1.0), Some(0.0)),
        "each saved value is on its own parameter, and the new parameter has its default"
    );
    assert_eq!(second.render(0.5, &project.lane), heard, "the lane sounds the same, sample for sample");

    // And back: a project of the second build, drive included, in the first.
    second.set(&[(LEVEL_ID, 2.0), (OFFSET_ID, -0.25), (INVERT_ID, 0.0), (DRIVE_ID, 0.5)]);
    let newer = second.save_project(&lane(second.id_of("Level"), second.id_of("Offset")));
    let back = Host::<PinnedA>::open();
    assert!(back.load_state(&newer.state));
    assert_eq!((back.value(LEVEL_ID), back.value(OFFSET_ID), back.value(INVERT_ID)), (Some(2.0), Some(-0.25), Some(0.0)));
    // The first build has no drive: its lane points are heard without it.
    let without_drive: Vec<f32> = second.render(0.5, &newer.lane).iter().map(|v| v / 1.5).collect();
    assert_eq!(back.render(0.5, &newer.lane), without_drive);
}

#[test]
fn a_point_for_a_parameter_the_build_does_not_have_moves_nothing() {
    let host = Host::<PinnedA>::open();
    host.set(&[(LEVEL_ID, 0.5)]);
    let quiet = host.render(0.5, &[]);
    // The drive of the second build, a position, and CLAP's invalid id.
    for stale in [DRIVE_ID, 0, 2, u32::MAX] {
        assert_eq!(host.render(0.5, &[Point { param_id: stale, time: 0, value: 1.0 }]), quiet, "id {stale}");
    }
    assert_eq!((host.value(LEVEL_ID), host.value(OFFSET_ID), host.value(INVERT_ID)), (Some(0.5), Some(0.0), Some(0.0)));
}

#[test]
fn without_pins_the_same_project_does_not_reopen() {
    let first = Host::<PlainA>::open();
    let (level, offset, invert) = (first.id_of("Level"), first.id_of("Offset"), first.id_of("Invert"));
    first.set(&[(level, 0.5), (offset, 0.25), (invert, 1.0)]);
    let project = first.save_project(&lane(level, offset));
    let heard = first.render(0.5, &project.lane);
    assert_eq!(heard, expected(), "the unpinned build plays its own lane as the pinned one does");

    let second = Host::<PlainB>::open();
    // The positional state of three parameters is not a state of four.
    assert!(!second.load_state(&project.state), "the reordered build must refuse the positional state");
    // The lane's ids are positions: 0 was level and is now invert, 1 was offset and is now drive.
    assert_eq!((second.id_of("Invert"), second.id_of("Drive")), (level, offset));
    assert_ne!(second.render(0.5, &project.lane), heard, "the lane moves other parameters, so the audio differs");

    // Even with the values put right by hand, the lane still lands on the wrong parameters.
    second.set(&[(second.id_of("Level"), 0.5), (second.id_of("Offset"), 0.25), (second.id_of("Invert"), 1.0), (second.id_of("Drive"), 0.0)]);
    assert_ne!(second.render(0.5, &project.lane), heard);
    // Addressed by name instead, the same lane is right: only the ids were wrong.
    second.set(&[(second.id_of("Level"), 0.5), (second.id_of("Offset"), 0.25), (second.id_of("Invert"), 1.0), (second.id_of("Drive"), 0.0)]);
    assert_eq!(second.render(0.5, &lane(second.id_of("Level"), second.id_of("Offset"))), heard);
}

#[test]
fn the_pinned_state_names_its_values_and_refuses_foreign_or_corrupt_blobs() {
    let host = Host::<PinnedA>::open();
    host.set(&[(LEVEL_ID, 0.5), (OFFSET_ID, 0.25), (INVERT_ID, 1.0)]);
    let state = host.save_state();
    assert_eq!(&state[..8], b"OPST\x02\x00\x03\x00", "version 2, three values");
    for name in ["level", "offset", "invert"] {
        assert!(state.windows(name.len()).any(|w| w == name.as_bytes()), "the state names {name}");
    }
    let fresh = Host::<PinnedB>::open();
    for cut in [0, 4, 8, 11, state.len() - 1] {
        assert!(!fresh.load_state(&state[..cut]), "a state cut at {cut} is refused");
    }
    let mut nan = state.clone();
    let at = nan.len() - 4;
    nan[at..].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(!fresh.load_state(&nan));
    // A keyed state of some other plugin names none of these parameters.
    let mut foreign = b"OPST\x02\x00\x01\x00".to_vec();
    foreign.extend_from_slice(&4u16.to_le_bytes());
    foreign.extend_from_slice(b"size");
    foreign.extend_from_slice(&0.5f32.to_le_bytes());
    assert!(!fresh.load_state(&foreign));
    // Nothing moved.
    assert_eq!((fresh.value(LEVEL_ID), fresh.value(OFFSET_ID), fresh.value(INVERT_ID), fresh.value(DRIVE_ID)), (Some(1.0), Some(0.0), Some(0.0), Some(0.0)));
    assert!(fresh.load_state(&state));
    assert_eq!(fresh.value(INVERT_ID), Some(1.0));
}

#[test]
fn every_pin_of_the_example_is_usable() {
    assert_eq!(openplugin::core::identity::check_pins(&PinnedA::default()), []);
    assert_eq!(openplugin::core::identity::check_pins(&PinnedB::default()), []);
    assert_eq!(openplugin::core::identity::check_pins(&PlainA::default()), []);
}
