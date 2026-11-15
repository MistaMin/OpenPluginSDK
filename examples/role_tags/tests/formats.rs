//! The channel order proof (0.6 task 8) through the Rust entries: CLAP and LV2.
//!
//! Each fake host writes, on every channel of its own channel order, the tag of
//! the speaker that channel is in that format (`in_tag`), and reads back the
//! plugin's tag of the same speaker (`out_tag`) on the same channel. The plugin
//! (`RoleTags`) checks every input sample against the role it believes the
//! channel has, by `SpeakerLayout::role_at`. So a pass means every role arrived
//! at the right index in and out.
//!
//! Every layout the plugin declares is either run or listed as skipped, with
//! the reason: CLAP has no ids for the top side and wide channels; the LV2
//! entry has fixed stereo ports. The VST3, Audio Unit and AAX halves of the
//! proof are the native role proofs (`native/vst3/roles_host.cpp`,
//! `native/au/roles_host.cpp`, `native/aax/host_roles.cpp`).

use std::ffi::{c_void, CStr};
use std::sync::Mutex;

use example_role_tags::{in_tag, out_tag, RoleTags, LAYOUTS, MISMATCH, MONO_CODE};
use openplugin::core::{IoConfig, Plugin, SpeakerLayout};
use openplugin::targets::clap_abi::*;
use openplugin::targets::lv2::{self, PortMap};
use openplugin::targets::ClapAdapter;

const FRAMES: usize = 64;

/// The LV2 creator is process-wide; the tests take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// What one output channel says about the role the plugin gave it.
fn check_output(format: &str, layout: &str, channel: usize, code: u8, samples: &[f32]) {
    for (n, &v) in samples.iter().enumerate() {
        if v >= MISMATCH - 1.0 {
            panic!(
                "{format} {layout}: host channel {channel} (code {code}) sample {n}: the plugin expected a different input tag, got {}",
                v - MISMATCH
            );
        }
        assert_eq!(v, out_tag(code), "{format} {layout}: host channel {channel} (code {code}) sample {n} holds the output tag of another role");
    }
}

// ---- CLAP ------------------------------------------------------------------

/// CLAP names a speaker by its `CLAP_SURROUND_*` id, and the SDK's role ids 0
/// to 17 are those ids. A host must never see an id above `CLAP_SURROUND_TBR`.
const CLAP_SURROUND_TBR: u8 = 17;

/// The adapter owns the plugin and frees it when dropped (no `destroy`).
struct Clap {
    _adapter: ClapAdapter<RoleTags>,
    plugin: *const clap_plugin,
}

impl Clap {
    fn new() -> Self {
        let adapter = ClapAdapter::new(RoleTags::default());
        let plugin = adapter.as_plugin();
        assert!(unsafe { (*plugin).init.unwrap()(plugin) });
        Self { _adapter: adapter, plugin }
    }

    fn ext<T>(&self, id: &CStr) -> *const T {
        let ext = unsafe { (*self.plugin).get_extension.unwrap()(self.plugin, id.as_ptr()) } as *const T;
        assert!(!ext.is_null(), "CLAP extension {id:?}");
        ext
    }

    /// The configs offered: (id, input channels, output channels).
    fn configs(&self) -> Vec<(u32, u32, u32)> {
        let configs = self.ext::<clap_plugin_audio_ports_config>(c"clap.audio-ports-config");
        let count = unsafe { (*configs).count.unwrap()(self.plugin) };
        (0..count)
            .map(|i| unsafe {
                let mut config = std::mem::zeroed::<clap_audio_ports_config>();
                assert!((*configs).get.unwrap()(self.plugin, i, &mut config));
                (config.id, config.main_input_channel_count, config.main_output_channel_count)
            })
            .collect()
    }

    fn select(&self, id: u32) -> bool {
        let configs = self.ext::<clap_plugin_audio_ports_config>(c"clap.audio-ports-config");
        unsafe { (*configs).select.unwrap()(self.plugin, id) }
    }

    /// The speaker code of every channel of main port 0, as a CLAP host reads
    /// it: the port type, and for a surround port the channel map.
    fn speakers(&self, is_input: bool) -> Vec<u8> {
        let ports = self.ext::<clap_plugin_audio_ports>(c"clap.audio-ports");
        let mut info = unsafe { std::mem::zeroed::<clap_audio_port_info>() };
        assert!(unsafe { (*ports).get.unwrap()(self.plugin, 0, is_input, &mut info) });
        let kind = unsafe { CStr::from_ptr(info.port_type) }.to_bytes();
        let n = info.channel_count as usize;
        match kind {
            b"mono" => {
                assert_eq!(n, 1);
                vec![MONO_CODE]
            }
            // CLAP's stereo port is left, right.
            b"stereo" => {
                assert_eq!(n, 2);
                vec![0, 1]
            }
            b"surround" => {
                let surround = self.ext::<clap_plugin_surround>(c"clap.surround/4");
                let mut map = [0xFFu8; 32];
                let stored = unsafe { (*surround).get_channel_map.unwrap()(self.plugin, is_input, 0, map.as_mut_ptr(), 32) };
                assert_eq!(stored as usize, n, "the channel map names every channel");
                assert!(map[..n].iter().all(|&id| id <= CLAP_SURROUND_TBR), "only CLAP surround ids");
                map[..n].to_vec()
            }
            other => panic!("unexpected port type {:?}", String::from_utf8_lossy(other)),
        }
    }

    /// One block: `input[j]` on host channel j, separate output buffers.
    fn run(&self, input: &[Vec<f32>]) -> Vec<Vec<f32>> {
        let mut inputs: Vec<Vec<f32>> = input.to_vec();
        let mut outputs = vec![vec![7.0f32; FRAMES]; input.len()];
        let mut in_ptrs: Vec<*mut f32> = inputs.iter_mut().map(|c| c.as_mut_ptr()).collect();
        let mut out_ptrs: Vec<*mut f32> = outputs.iter_mut().map(|c| c.as_mut_ptr()).collect();
        let audio_in = clap_audio_buffer {
            data32: in_ptrs.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: input.len() as u32,
            latency: 0,
            constant_mask: 0,
        };
        let mut audio_out = clap_audio_buffer {
            data32: out_ptrs.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: input.len() as u32,
            latency: 0,
            constant_mask: 0,
        };
        unsafe extern "C" fn no_events(_: *const clap_input_events) -> u32 {
            0
        }
        unsafe extern "C" fn no_event(_: *const clap_input_events, _: u32) -> *const clap_event_header {
            std::ptr::null()
        }
        unsafe extern "C" fn push(_: *const clap_output_events, _: *const clap_event_header) -> bool {
            true
        }
        let in_events = clap_input_events { ctx: std::ptr::null_mut::<c_void>(), size: Some(no_events), get: Some(no_event) };
        let out_events = clap_output_events { ctx: std::ptr::null_mut::<c_void>(), try_push: Some(push) };
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
        let status = unsafe { (*self.plugin).process.unwrap()(self.plugin, &process) };
        assert_ne!(status, PROCESS_ERROR);
        outputs
    }

    fn start(&self) {
        unsafe {
            assert!((*self.plugin).activate.unwrap()(self.plugin, 48_000.0, 1, FRAMES as u32));
            assert!((*self.plugin).start_processing.unwrap()(self.plugin));
        }
    }

    fn stop(&self) {
        unsafe {
            (*self.plugin).stop_processing.unwrap()(self.plugin);
            (*self.plugin).deactivate.unwrap()(self.plugin);
        }
    }
}

fn tags(codes: &[u8]) -> Vec<Vec<f32>> {
    codes.iter().map(|&c| vec![in_tag(c); FRAMES]).collect()
}

#[test]
fn clap_every_layout_it_can_express_carries_every_role() {
    let _g = serial();
    let clap = Clap::new();
    let configs = clap.configs();
    let mut ran = Vec::new();
    let mut skipped = Vec::new();
    for (index, layout) in LAYOUTS.iter().enumerate() {
        let name = layout.output.label();
        let offered = configs.iter().find(|c| c.0 == index as u32);
        // Mono and stereo have no surround map but are plain CLAP port types.
        let expressible = matches!(layout.output, SpeakerLayout::Mono | SpeakerLayout::Stereo) || layout.output.clap_expressible();
        if !expressible {
            assert!(offered.is_none(), "CLAP offers {name}, which it cannot express");
            assert!(!clap.select(index as u32), "CLAP selects {name}");
            skipped.push(name);
            continue;
        }
        let channels = layout.output.channels().unwrap() as u32;
        assert_eq!(offered.map(|c| (c.1, c.2)), Some((channels, channels)), "CLAP offers {name}");
        assert!(clap.select(index as u32), "select {name}");
        let input = clap.speakers(true);
        let output = clap.speakers(false);
        assert_eq!(input, output);
        clap.start();
        let out = clap.run(&tags(&input));
        for (j, samples) in out.iter().enumerate() {
            check_output("CLAP", name, j, output[j], samples);
        }
        clap.stop();
        ran.push(name);
    }
    println!("CLAP role proof: every role at its index in {}", ran.join(", "));
    println!("CLAP role proof: skipped (no CLAP_SURROUND id for the top side or wide channels): {}", skipped.join(", "));
    assert_eq!(skipped, ["5.1.2", "7.1.2", "9.1.6"]);
    assert_eq!(ran.len(), 10);
}

/// The negative control: a host that ignores the channel map and writes the
/// tags in another order is caught on every channel it moved.
#[test]
fn clap_a_host_that_ignores_the_channel_map_is_caught() {
    let _g = serial();
    let clap = Clap::new();
    let index = LAYOUTS.iter().position(|l| l.output == SpeakerLayout::Surround714).unwrap();
    assert!(clap.select(index as u32));
    let mut speakers = clap.speakers(true);
    speakers.rotate_left(1);
    clap.start();
    let out = clap.run(&tags(&speakers));
    assert!(out.iter().all(|c| c.iter().all(|&v| v >= MISMATCH - 1.0)), "every moved channel is flagged");
    clap.stop();
}

// ---- LV2 -------------------------------------------------------------------

/// The audio ports of the plugin's TTL, as (index, symbol), in file order.
fn lv2_audio_ports() -> Vec<(u32, String)> {
    let ttl = lv2::plugin_ttl(&RoleTags::default(), false);
    let mut ports = Vec::new();
    for block in ttl.split("lv2:AudioPort").skip(1) {
        let index = block.split("lv2:index ").nth(1).and_then(|s| s.split([' ', ';']).next()).and_then(|s| s.parse().ok());
        let symbol = block.split("lv2:symbol \"").nth(1).and_then(|s| s.split('"').next());
        ports.push((index.expect("port index"), symbol.expect("port symbol").to_string()));
    }
    ports
}

#[test]
fn lv2_carries_both_stereo_roles_and_offers_nothing_else() {
    let _g = serial();
    // An LV2 host knows a port's speaker from the description: the stereo
    // symbols are left and right. Any other audio port would need a speaker.
    let ports = lv2_audio_ports();
    let speaker = |symbol: &str| match symbol {
        "in_l" | "out_l" => 0u8,
        "in_r" | "out_r" => 1,
        other => panic!("LV2 audio port {other} has no speaker this host knows"),
    };
    let inputs: Vec<(u32, u8)> = ports.iter().filter(|p| p.1.starts_with("in_")).map(|p| (p.0, speaker(&p.1))).collect();
    let outputs: Vec<(u32, u8)> = ports.iter().filter(|p| p.1.starts_with("out_")).map(|p| (p.0, speaker(&p.1))).collect();
    assert_eq!((inputs.len(), outputs.len()), (2, 2), "the LV2 entry has fixed stereo ports: {ports:?}");

    let desc = unsafe { &*example_role_tags::lv2_descriptor(0) };
    let mut host = Lv2Host::new();
    let instance = (desc.instantiate)(desc, 48_000.0, std::ptr::null(), host.features());
    assert!(!instance.is_null());
    let mut audio_in: Vec<Vec<f32>> = inputs.iter().map(|&(_, code)| vec![in_tag(code); FRAMES]).collect();
    let mut audio_out = vec![vec![7.0f32; FRAMES]; outputs.len()];
    for (k, &(port, _)) in inputs.iter().enumerate() {
        (desc.connect_port)(instance, port, audio_in[k].as_mut_ptr().cast());
    }
    for (k, &(port, _)) in outputs.iter().enumerate() {
        (desc.connect_port)(instance, port, audio_out[k].as_mut_ptr().cast());
    }
    let map = PortMap::of(&RoleTags::default());
    let mut spare = 0.0f32;
    let mut freewheel = 0.0f32;
    (desc.connect_port)(instance, map.first_param(), (&mut spare as *mut f32).cast());
    (desc.connect_port)(instance, map.freewheel(), (&mut freewheel as *mut f32).cast());
    (desc.activate)(instance);
    (desc.run)(instance, FRAMES as u32);
    for (k, &(port, code)) in outputs.iter().enumerate() {
        check_output("LV2", "stereo", port as usize, code, &audio_out[k]);
    }
    // The negative control: left and right swapped on the way in are caught.
    audio_in.swap(0, 1);
    (desc.connect_port)(instance, inputs[0].0, audio_in[0].as_mut_ptr().cast());
    (desc.connect_port)(instance, inputs[1].0, audio_in[1].as_mut_ptr().cast());
    (desc.run)(instance, FRAMES as u32);
    assert!(audio_out.iter().all(|c| c.iter().all(|&v| v >= MISMATCH - 1.0)), "swapped stereo is flagged");
    (desc.deactivate)(instance);
    (desc.cleanup)(instance);

    let layout = lv2::layout_of(&RoleTags::default());
    assert_eq!(layout, IoConfig::STEREO);
    let skipped: Vec<&str> = LAYOUTS.iter().filter(|l| **l != layout).map(|l| l.output.label()).collect();
    println!("LV2 role proof: every role at its index in Stereo");
    println!("LV2 role proof: skipped (the LV2 entry has fixed stereo ports): {}", skipped.join(", "));
    assert_eq!(skipped.len(), LAYOUTS.len() - 1);
}

/// The plugin flags a wrong tag on its own, whatever the format: the control
/// that keeps every proof above from passing vacuously.
#[test]
fn the_plugin_flags_a_wrong_tag() {
    let mut plugin = RoleTags::default();
    let layout = LAYOUTS[12];
    assert!(plugin.set_layout(layout));
    let roles = layout.input.roles();
    let mut io: Vec<Vec<f32>> = roles.iter().map(|&r| vec![in_tag(r); 8]).collect();
    io[3][5] = in_tag(roles[4]);
    plugin.process(&mut io, 8);
    for (i, channel) in io.iter().enumerate() {
        for (n, &v) in channel.iter().enumerate() {
            if (i, n) == (3, 5) {
                assert_eq!(v, MISMATCH + in_tag(roles[4]));
            } else {
                assert_eq!(v, out_tag(roles[i]));
            }
        }
    }
    // Mono has no role and its own code.
    assert!(plugin.set_layout(LAYOUTS[0]));
    let mut mono = vec![vec![in_tag(MONO_CODE); 4]];
    plugin.process(&mut mono, 4);
    assert!(mono[0].iter().all(|&v| v == out_tag(MONO_CODE)));
}

// ---- a tiny LV2 host: urid:map only -----------------------------------------

#[repr(C)]
struct UridMap {
    handle: *mut c_void,
    map: unsafe extern "C" fn(*mut c_void, *const std::ffi::c_char) -> u32,
}

unsafe extern "C" fn map_uri(handle: *mut c_void, uri: *const std::ffi::c_char) -> u32 {
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

struct Lv2Host {
    #[allow(clippy::box_collection)] // the Vec header must keep its address for the map handle
    _table: Box<Vec<String>>,
    _map: Box<UridMap>,
    _feature: Box<lv2::Lv2Feature>,
    features: Box<[*const lv2::Lv2Feature; 2]>,
}

impl Lv2Host {
    fn new() -> Self {
        let mut table: Box<Vec<String>> = Box::default();
        let map = Box::new(UridMap { handle: (&mut *table) as *mut Vec<String> as *mut c_void, map: map_uri });
        let feature = Box::new(lv2::Lv2Feature { uri: c"http://lv2plug.in/ns/ext/urid#map".as_ptr(), data: (&*map) as *const UridMap as *mut c_void });
        let features = Box::new([&*feature as *const lv2::Lv2Feature, std::ptr::null()]);
        Self { _table: table, _map: map, _feature: feature, features }
    }

    fn features(&mut self) -> *const *const lv2::Lv2Feature {
        self.features.as_ptr()
    }
}
