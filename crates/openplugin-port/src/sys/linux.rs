//! Linux: `dlopen`, and VST3 modules as `.so` files. Only Linux work edits this
//! file; see `sys/mod.rs`.

#![allow(non_camel_case_types, non_snake_case, dead_code, clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;

unsafe extern "C" {
    pub fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    pub fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    pub fn dlerror() -> *const c_char;
    pub fn _exit(code: c_int) -> !;
}

pub const RTLD_NOW: c_int = 2;
// Not the macOS value: on Linux 4 is RTLD_NOLOAD, and RTLD_LOCAL is 0.
pub const RTLD_LOCAL: c_int = 0;

/// `dlopen` a file; the error text from `dlerror` on failure.
pub fn open_library(path: &Path) -> Result<*mut c_void, String> {
    let c = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    let h = unsafe { dlopen(c.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
    if h.is_null() {
        let e = unsafe { dlerror() };
        let text = if e.is_null() { "dlopen failed".to_string() } else { unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned() };
        return Err(text);
    }
    Ok(h)
}

pub fn symbol(handle: *mut c_void, name: &str) -> *mut c_void {
    let c = CString::new(name).unwrap_or_default();
    unsafe { dlsym(handle, c.as_ptr()) }
}

/// Leave the process at once, without running any teardown.
pub fn exit_now(code: c_int) -> ! {
    unsafe { _exit(code) }
}

// ---------- VST3 module loading ----------

/// The VST3 module entry point, `ModuleEntry(void *library_handle)`.
pub const MODULE_ENTRY: &str = "ModuleEntry";

/// What `load_bundle` returns and the module entry point takes: the `dlopen` handle.
pub type ModuleRef = *const c_void;

/// The VST3 bundle layout's architecture folder on this machine.
fn vst3_arch_dir() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64-linux"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64-linux"
    } else if cfg!(target_arch = "x86") {
        "i386-linux"
    } else if cfg!(target_arch = "arm") {
        "armv7l-linux"
    } else {
        "unknown-linux"
    }
}

/// Load the VST3 module at `path`: `<name>.vst3/Contents/<arch>-linux/<name>.so`,
/// or a bare `.so` file. Returns the `dlopen` handle.
pub fn load_bundle(path: &Path) -> Result<ModuleRef, String> {
    let file = if path.is_dir() {
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let p = path.join("Contents").join(vst3_arch_dir()).join(format!("{stem}.so"));
        if !p.is_file() {
            return Err(format!("{} has no {}", path.display(), p.strip_prefix(path).unwrap_or(&p).display()));
        }
        p
    } else {
        path.to_path_buf()
    };
    open_library(&file).map(|h| h as ModuleRef)
}

pub fn bundle_function(module: ModuleRef, name: &str) -> *mut c_void {
    symbol(module as *mut c_void, name)
}
