//! CLAP (CLever Audio Plugin) target bridge.

use std::ffi::{c_char, c_void};
use openplugin_core::Plugin;

#[repr(C)]
pub struct ClapPluginDescriptor {
    pub clap_version: [u32; 3],
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
pub struct ClapPlugin {
    pub desc: *const ClapPluginDescriptor,
    pub plugin_data: *mut c_void,
    pub init: unsafe extern "C" fn(plugin: *const ClapPlugin) -> bool,
    pub destroy: unsafe extern "C" fn(plugin: *const ClapPlugin),
    pub activate: unsafe extern "C" fn(plugin: *const ClapPlugin, sample_rate: f64, min_frames: u32, max_frames: u32) -> bool,
    pub deactivate: unsafe extern "C" fn(plugin: *const ClapPlugin),
    pub start_processing: unsafe extern "C" fn(plugin: *const ClapPlugin) -> bool,
    pub stop_processing: unsafe extern "C" fn(plugin: *const ClapPlugin),
    pub reset: unsafe extern "C" fn(plugin: *const ClapPlugin),
    pub process: unsafe extern "C" fn(plugin: *const ClapPlugin, process: *const c_void) -> i32,
    pub get_extension: unsafe extern "C" fn(plugin: *const ClapPlugin, id: *const c_char) -> *const c_void,
    pub on_main_thread: unsafe extern "C" fn(plugin: *const ClapPlugin),
}

/// Helper wrapper that instantiates and operates a CLAP plugin from a Rust `Plugin` type.
pub struct ClapAdapter<P: Plugin> {
    pub inner: P,
    pub sample_rate: f32,
    pub max_block: usize,
}

impl<P: Plugin> ClapAdapter<P> {
    pub fn new(inner: P) -> Self {
        Self {
            inner,
            sample_rate: 44100.0,
            max_block: 512,
        }
    }
}
