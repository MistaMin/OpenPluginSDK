//! Audio Unit descriptor types.
//!
//! A host cannot load these. The loadable shell is `native/au`, over the `op_shell_*` ABI.

use std::ffi::c_void;
use openplugin_core::Plugin;

#[repr(C)]
pub struct AudioComponentDescription {
    pub component_type: u32,
    pub component_sub_type: u32,
    pub component_manufacturer: u32,
    pub component_flags: u32,
    pub component_flags_mask: u32,
}

#[repr(C)]
pub struct AudioComponentPlugInInterface {
    pub open: unsafe extern "C" fn(self_ptr: *mut c_void, instance: *mut c_void) -> i32,
    pub close: unsafe extern "C" fn(instance: *mut c_void) -> i32,
    pub lookup: unsafe extern "C" fn(selector: i16) -> *const c_void,
    pub reserved: *mut c_void,
}

pub struct AuAdapter<P: Plugin> {
    pub inner: P,
}

impl<P: Plugin> AuAdapter<P> {
    pub fn new(inner: P) -> Self {
        Self { inner }
    }
}
