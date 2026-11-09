//! VST3 descriptor types.
//!
//! These are the COM layouts only. A host cannot load them. The working from-scratch
//! VST3 shell is OpenGridder's `native/vst3/OpenGridderVST3.mm`. Porting it means
//! separating that shell from the offload client, the same way the CLAP bridge was.

use std::ffi::c_void;
use openplugin_core::Plugin;

#[repr(C)]
pub struct Vst3Guid {
    pub data: [u8; 16],
}

/// COM vtable representation for VST3 IComponent
#[repr(C)]
pub struct IComponentVtbl {
    pub query_interface: unsafe extern "system" fn(this: *mut c_void, iid: *const Vst3Guid, obj: *mut *mut c_void) -> i32,
    pub add_ref: unsafe extern "system" fn(this: *mut c_void) -> u32,
    pub release: unsafe extern "system" fn(this: *mut c_void) -> u32,
    pub initialize: unsafe extern "system" fn(this: *mut c_void, context: *mut c_void) -> i32,
    pub terminate: unsafe extern "system" fn(this: *mut c_void) -> i32,
    pub get_controller_class_id: unsafe extern "system" fn(this: *mut c_void, class_id: *mut Vst3Guid) -> i32,
    pub set_io_mode: unsafe extern "system" fn(this: *mut c_void, mode: i32) -> i32,
    pub get_bus_count: unsafe extern "system" fn(this: *mut c_void, media_type: i32, dir: i32) -> i32,
    pub get_bus_info: unsafe extern "system" fn(this: *mut c_void, media_type: i32, dir: i32, index: i32, bus: *mut c_void) -> i32,
    pub get_routing_info: unsafe extern "system" fn(this: *mut c_void, in_info: *mut c_void, out_info: *mut c_void) -> i32,
    pub activate_bus: unsafe extern "system" fn(this: *mut c_void, media_type: i32, dir: i32, index: i32, state: u8) -> i32,
    pub set_active: unsafe extern "system" fn(this: *mut c_void, state: u8) -> i32,
    pub set_state: unsafe extern "system" fn(this: *mut c_void, state: *mut c_void) -> i32,
    pub get_state: unsafe extern "system" fn(this: *mut c_void, state: *mut c_void) -> i32,
}

pub struct Vst3Adapter<P: Plugin> {
    pub inner: P,
}

impl<P: Plugin> Vst3Adapter<P> {
    pub fn new(inner: P) -> Self {
        Self { inner }
    }
}
