//! CLAP 1.2 layouts used by the bridge.
//!
//! Field order follows the CLAP headers. OpenGridder inlined this same subset in
//! `native/clap/OpenGridderCLAP.mm` so the plugin would not depend on the CLAP SDK.
#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_void};

pub const CLAP_VERSION: clap_version = clap_version { major: 1, minor: 2, revision: 0 };
pub const NAME_SIZE: usize = 256;
pub const PATH_SIZE: usize = 1024;

pub const PROCESS_ERROR: i32 = 0;
pub const PROCESS_CONTINUE: i32 = 1;

pub const AUDIO_PORT_IS_MAIN: u32 = 1 << 0;
pub const PARAM_IS_STEPPED: u32 = 1 << 0;
pub const PARAM_IS_HIDDEN: u32 = 1 << 2;
pub const PARAM_IS_READONLY: u32 = 1 << 3;
pub const PARAM_IS_BYPASS: u32 = 1 << 4;
pub const PARAM_IS_AUTOMATABLE: u32 = 1 << 5;
pub const PARAM_REQUIRES_PROCESS: u32 = 1 << 15;
pub const CLAP_INVALID_ID: u32 = u32::MAX;
pub const NOTE_DIALECT_CLAP: u32 = 1 << 0;
pub const NOTE_DIALECT_MIDI: u32 = 1 << 1;
pub const TRANSPORT_HAS_TEMPO: u32 = 1 << 0;
pub const TRANSPORT_HAS_BEATS: u32 = 1 << 1;
pub const TRANSPORT_HAS_SECONDS: u32 = 1 << 2;
pub const TRANSPORT_HAS_SIGNATURE: u32 = 1 << 3;
pub const TRANSPORT_IS_PLAYING: u32 = 1 << 4;
pub const TRANSPORT_IS_RECORDING: u32 = 1 << 5;
pub const AMBISONIC_ORDERING_ACN: u32 = 1;
pub const AMBISONIC_NORMALIZATION_SN3D: u32 = 1;

pub const EVENT_NOTE_ON: u16 = 0;
pub const EVENT_NOTE_OFF: u16 = 1;
pub const EVENT_PARAM_VALUE: u16 = 5;
pub const EVENT_MIDI: u16 = 10;

pub const EXT_AUDIO_PORTS: &[u8] = b"clap.audio-ports\0";
pub const EXT_AUDIO_PORTS_CONFIG: &[u8] = b"clap.audio-ports-config\0";
pub const EXT_AUDIO_PORTS_CONFIG_INFO: &[u8] = b"clap.audio-ports-config-info/1\0";
pub const EXT_NOTE_PORTS: &[u8] = b"clap.note-ports\0";
pub const EXT_LATENCY: &[u8] = b"clap.latency\0";
pub const EXT_PARAMS: &[u8] = b"clap.params\0";
pub const EXT_STATE: &[u8] = b"clap.state\0";
pub const EXT_TAIL: &[u8] = b"clap.tail\0";
pub const EXT_RENDER: &[u8] = b"clap.render\0";
pub const RENDER_REALTIME: u32 = 0;
pub const RENDER_OFFLINE: u32 = 1;
pub const EXT_SURROUND: &[u8] = b"clap.surround/4\0";
pub const EXT_SURROUND_COMPAT: &[u8] = b"clap.surround.draft/4\0";
pub const EXT_AMBISONIC: &[u8] = b"clap.ambisonic/3\0";
pub const EXT_AMBISONIC_COMPAT: &[u8] = b"clap.ambisonic.draft/3\0";
pub const FACTORY_ID: &[u8] = b"clap.plugin-factory\0";

pub static PORT_MONO: &[u8] = b"mono\0";
pub static PORT_STEREO: &[u8] = b"stereo\0";
pub static PORT_SURROUND: &[u8] = b"surround\0";
pub static PORT_AMBISONIC: &[u8] = b"ambisonic\0";

#[repr(C)]
#[derive(Clone, Copy)]
pub struct clap_version {
    pub major: u32,
    pub minor: u32,
    pub revision: u32,
}

#[repr(C)]
pub struct clap_plugin_descriptor {
    pub clap_version: clap_version,
    pub id: *const c_char,
    pub name: *const c_char,
    pub vendor: *const c_char,
    pub url: *const c_char,
    pub manual_url: *const c_char,
    pub support_url: *const c_char,
    pub version: *const c_char,
    pub description: *const c_char,
    pub features: *const *const c_char,
}

#[repr(C)]
pub struct clap_host {
    pub clap_version: clap_version,
    pub host_data: *mut c_void,
    pub name: *const c_char,
    pub vendor: *const c_char,
    pub url: *const c_char,
    pub version: *const c_char,
    pub get_extension: Option<unsafe extern "C" fn(host: *const clap_host, extension_id: *const c_char) -> *const c_void>,
    pub request_restart: Option<unsafe extern "C" fn(host: *const clap_host)>,
    pub request_process: Option<unsafe extern "C" fn(host: *const clap_host)>,
    pub request_callback: Option<unsafe extern "C" fn(host: *const clap_host)>,
}

#[repr(C)]
pub struct clap_event_header {
    pub size: u32,
    pub time: u32,
    pub space_id: u16,
    pub type_: u16,
    pub flags: u32,
}

#[repr(C)]
pub struct clap_event_param_value {
    pub header: clap_event_header,
    pub param_id: u32,
    pub cookie: *mut c_void,
    pub note_id: i32,
    pub port_index: i16,
    pub channel: i16,
    pub key: i16,
    pub value: f64,
}

#[repr(C)]
pub struct clap_event_note {
    pub header: clap_event_header,
    pub note_id: i32,
    pub port_index: i16,
    pub channel: i16,
    pub key: i16,
    pub velocity: f64,
}

#[repr(C)]
pub struct clap_event_midi {
    pub header: clap_event_header,
    pub port_index: u16,
    pub data: [u8; 3],
}

#[repr(C)]
pub struct clap_input_events {
    pub ctx: *mut c_void,
    pub size: Option<unsafe extern "C" fn(list: *const clap_input_events) -> u32>,
    pub get: Option<unsafe extern "C" fn(list: *const clap_input_events, index: u32) -> *const clap_event_header>,
}

#[repr(C)]
pub struct clap_output_events {
    pub ctx: *mut c_void,
    pub try_push: Option<unsafe extern "C" fn(list: *const clap_output_events, event: *const clap_event_header) -> bool>,
}

#[repr(C)]
pub struct clap_audio_buffer {
    pub data32: *mut *mut f32,
    pub data64: *mut *mut f64,
    pub channel_count: u32,
    pub latency: u32,
    pub constant_mask: u64,
}

#[repr(C)]
pub struct clap_process {
    pub steady_time: i64,
    pub frames_count: u32,
    pub transport: *const clap_event_transport,
    pub audio_inputs: *const clap_audio_buffer,
    pub audio_outputs: *mut clap_audio_buffer,
    pub audio_inputs_count: u32,
    pub audio_outputs_count: u32,
    pub in_events: *const clap_input_events,
    pub out_events: *const clap_output_events,
}

#[repr(C)]
pub struct clap_plugin {
    pub desc: *const clap_plugin_descriptor,
    pub plugin_data: *mut c_void,
    pub init: Option<unsafe extern "C" fn(plugin: *const clap_plugin) -> bool>,
    pub destroy: Option<unsafe extern "C" fn(plugin: *const clap_plugin)>,
    pub activate: Option<unsafe extern "C" fn(plugin: *const clap_plugin, sample_rate: f64, min_frames: u32, max_frames: u32) -> bool>,
    pub deactivate: Option<unsafe extern "C" fn(plugin: *const clap_plugin)>,
    pub start_processing: Option<unsafe extern "C" fn(plugin: *const clap_plugin) -> bool>,
    pub stop_processing: Option<unsafe extern "C" fn(plugin: *const clap_plugin)>,
    pub reset: Option<unsafe extern "C" fn(plugin: *const clap_plugin)>,
    pub process: Option<unsafe extern "C" fn(plugin: *const clap_plugin, process: *const clap_process) -> i32>,
    pub get_extension: Option<unsafe extern "C" fn(plugin: *const clap_plugin, id: *const c_char) -> *const c_void>,
    pub on_main_thread: Option<unsafe extern "C" fn(plugin: *const clap_plugin)>,
}

#[repr(C)]
pub struct clap_audio_port_info {
    pub id: u32,
    pub name: [c_char; NAME_SIZE],
    pub flags: u32,
    pub channel_count: u32,
    pub port_type: *const c_char,
    pub in_place_pair: u32,
}

#[repr(C)]
pub struct clap_plugin_audio_ports {
    pub count: Option<unsafe extern "C" fn(plugin: *const clap_plugin, is_input: bool) -> u32>,
    pub get: Option<unsafe extern "C" fn(plugin: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_audio_port_info) -> bool>,
}

#[repr(C)]
pub struct clap_note_port_info {
    pub id: u32,
    pub supported_dialects: u32,
    pub preferred_dialect: u32,
    pub name: [c_char; NAME_SIZE],
}

#[repr(C)]
pub struct clap_plugin_note_ports {
    pub count: Option<unsafe extern "C" fn(plugin: *const clap_plugin, is_input: bool) -> u32>,
    pub get: Option<unsafe extern "C" fn(plugin: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_note_port_info) -> bool>,
}

#[repr(C)]
pub struct clap_plugin_latency {
    pub get: Option<unsafe extern "C" fn(plugin: *const clap_plugin) -> u32>,
}

#[repr(C)]
pub struct clap_param_info {
    pub id: u32,
    pub flags: u32,
    pub cookie: *mut c_void,
    pub name: [c_char; NAME_SIZE],
    pub module: [c_char; PATH_SIZE],
    pub min_value: f64,
    pub max_value: f64,
    pub default_value: f64,
}

#[repr(C)]
pub struct clap_plugin_params {
    pub count: Option<unsafe extern "C" fn(plugin: *const clap_plugin) -> u32>,
    pub get_info: Option<unsafe extern "C" fn(plugin: *const clap_plugin, index: u32, info: *mut clap_param_info) -> bool>,
    pub get_value: Option<unsafe extern "C" fn(plugin: *const clap_plugin, param_id: u32, out_value: *mut f64) -> bool>,
    pub value_to_text: Option<unsafe extern "C" fn(plugin: *const clap_plugin, param_id: u32, value: f64, out_buffer: *mut c_char, capacity: u32) -> bool>,
    pub text_to_value: Option<unsafe extern "C" fn(plugin: *const clap_plugin, param_id: u32, text: *const c_char, out_value: *mut f64) -> bool>,
    pub flush: Option<unsafe extern "C" fn(plugin: *const clap_plugin, input: *const clap_input_events, output: *const clap_output_events)>,
}

#[repr(C)]
pub struct clap_istream {
    pub ctx: *mut c_void,
    pub read: Option<unsafe extern "C" fn(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64>,
}

#[repr(C)]
pub struct clap_ostream {
    pub ctx: *mut c_void,
    pub write: Option<unsafe extern "C" fn(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64>,
}

#[repr(C)]
pub struct clap_plugin_state {
    pub save: Option<unsafe extern "C" fn(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool>,
    pub load: Option<unsafe extern "C" fn(plugin: *const clap_plugin, stream: *const clap_istream) -> bool>,
}

#[repr(C)]
pub struct clap_plugin_factory {
    pub get_plugin_count: Option<unsafe extern "C" fn(factory: *const clap_plugin_factory) -> u32>,
    pub get_plugin_descriptor: Option<unsafe extern "C" fn(factory: *const clap_plugin_factory, index: u32) -> *const clap_plugin_descriptor>,
    pub create_plugin: Option<unsafe extern "C" fn(factory: *const clap_plugin_factory, host: *const clap_host, plugin_id: *const c_char) -> *const clap_plugin>,
}

#[repr(C)]
pub struct clap_plugin_entry {
    pub clap_version: clap_version,
    pub init: Option<unsafe extern "C" fn(plugin_path: *const c_char) -> bool>,
    pub deinit: Option<unsafe extern "C" fn()>,
    pub get_factory: Option<unsafe extern "C" fn(factory_id: *const c_char) -> *const c_void>,
}

#[repr(C)]
pub struct clap_plugin_tail {
    pub get: Option<unsafe extern "C" fn(plugin: *const clap_plugin) -> u32>,
}

#[repr(C)]
pub struct clap_plugin_render {
    pub has_hard_realtime_requirement: Option<unsafe extern "C" fn(plugin: *const clap_plugin) -> bool>,
    pub set: Option<unsafe extern "C" fn(plugin: *const clap_plugin, mode: u32) -> bool>,
}

#[repr(C)]
pub struct clap_host_latency {
    pub changed: Option<unsafe extern "C" fn(host: *const clap_host)>,
}

#[repr(C)]
pub struct clap_host_tail {
    pub changed: Option<unsafe extern "C" fn(host: *const clap_host)>,
}

#[repr(C)]
pub struct clap_audio_ports_config {
    pub id: u32,
    pub name: [c_char; NAME_SIZE],
    pub input_port_count: u32,
    pub output_port_count: u32,
    pub has_main_input: bool,
    pub main_input_channel_count: u32,
    pub main_input_port_type: *const c_char,
    pub has_main_output: bool,
    pub main_output_channel_count: u32,
    pub main_output_port_type: *const c_char,
}

#[repr(C)]
pub struct clap_plugin_audio_ports_config {
    pub count: Option<unsafe extern "C" fn(plugin: *const clap_plugin) -> u32>,
    pub get: Option<unsafe extern "C" fn(plugin: *const clap_plugin, index: u32, config: *mut clap_audio_ports_config) -> bool>,
    pub select: Option<unsafe extern "C" fn(plugin: *const clap_plugin, config_id: u32) -> bool>,
}

#[repr(C)]
pub struct clap_plugin_audio_ports_config_info {
    pub current_config: Option<unsafe extern "C" fn(plugin: *const clap_plugin) -> u32>,
    pub get: Option<
        unsafe extern "C" fn(
            plugin: *const clap_plugin,
            config_id: u32,
            port_index: u32,
            is_input: bool,
            info: *mut clap_audio_port_info,
        ) -> bool,
    >,
}

#[repr(C)]
pub struct clap_plugin_surround {
    pub is_channel_mask_supported: Option<unsafe extern "C" fn(plugin: *const clap_plugin, channel_mask: u64) -> bool>,
    pub get_channel_map: Option<
        unsafe extern "C" fn(
            plugin: *const clap_plugin,
            is_input: bool,
            port_index: u32,
            channel_map: *mut u8,
            channel_map_capacity: u32,
        ) -> u32,
    >,
}

#[repr(C)]
pub struct clap_ambisonic_config {
    pub ordering: u32,
    pub normalization: u32,
}

#[repr(C)]
pub struct clap_plugin_ambisonic {
    pub is_config_supported: Option<unsafe extern "C" fn(plugin: *const clap_plugin, config: *const clap_ambisonic_config) -> bool>,
    pub get_config: Option<
        unsafe extern "C" fn(
            plugin: *const clap_plugin,
            is_input: bool,
            port_index: u32,
            config: *mut clap_ambisonic_config,
        ) -> bool,
    >,
}

/// Song position at the start of a block, or as a transport event.
/// Field order matches `clap_event_transport` in CLAP 1.2. Beat and second
/// positions are 31-bit fixed point stored in `i64`.
#[repr(C)]
pub struct clap_event_transport {
    pub header: clap_event_header,
    pub flags: u32,
    pub song_pos_beats: i64,
    pub song_pos_seconds: i64,
    pub tempo: f64,
    pub tempo_inc: f64,
    pub loop_start_beats: i64,
    pub loop_end_beats: i64,
    pub loop_start_seconds: i64,
    pub loop_end_seconds: i64,
    pub bar_start: i64,
    pub bar_number: i32,
    pub tsig_num: u16,
    pub tsig_denom: u16,
}
