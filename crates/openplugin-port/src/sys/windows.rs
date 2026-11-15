//! Windows: a placeholder until the Windows port lands. It compiles, and every
//! call that would load a plug-in says it is not supported yet. Only Windows
//! work edits this file; see `sys/mod.rs`.

#![allow(dead_code, clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::{c_int, c_void};
use std::path::Path;

const WHY: &str = "loading plug-ins is not supported on Windows yet";

pub fn open_library(_path: &Path) -> Result<*mut c_void, String> {
    Err(WHY.into())
}

pub fn symbol(_handle: *mut c_void, _name: &str) -> *mut c_void {
    std::ptr::null_mut()
}

/// Leave the process at once. The placeholder still runs Rust's exit path.
pub fn exit_now(code: c_int) -> ! {
    std::process::exit(code)
}

/// The VST3 module entry point, `InitDll()`.
pub const MODULE_ENTRY: &str = "InitDll";

/// What `load_bundle` returns: the module handle.
pub type ModuleRef = *const c_void;

pub fn load_bundle(_path: &Path) -> Result<ModuleRef, String> {
    Err(WHY.into())
}

pub fn bundle_function(_module: ModuleRef, _name: &str) -> *mut c_void {
    std::ptr::null_mut()
}
