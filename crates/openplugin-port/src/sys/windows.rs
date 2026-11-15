//! Windows: `LoadLibraryExW`, and VST3 modules as `.vst3` DLLs. Only Windows
//! work edits this file; see `sys/mod.rs`.

#![allow(non_snake_case, dead_code, clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::{CString, c_char, c_int, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(file: *const u16, reserved: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn GetLastError() -> u32;
    fn FormatMessageW(flags: u32, source: *const c_void, id: u32, language: u32, buffer: *mut u16, size: u32, args: *mut c_void) -> u32;
    fn GetCurrentProcess() -> *mut c_void;
    fn TerminateProcess(process: *mut c_void, code: u32) -> c_int;
}

/// Look for the DLLs a plug-in depends on in the plug-in's own folder first,
/// the way a host loads it. Needs an absolute path.
const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x0000_0008;
const FORMAT_MESSAGE_IGNORE_INSERTS: u32 = 0x0000_0200;
const FORMAT_MESSAGE_FROM_SYSTEM: u32 = 0x0000_1000;

/// The system's text for the calling thread's last error.
fn last_error() -> String {
    let code = unsafe { GetLastError() };
    let mut buf = [0u16; 512];
    let n = unsafe {
        FormatMessageW(
            FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
            std::ptr::null(),
            code,
            0,
            buf.as_mut_ptr(),
            buf.len() as u32,
            std::ptr::null_mut(),
        )
    };
    let text = String::from_utf16_lossy(&buf[..(n as usize).min(buf.len())]);
    let text = text.trim();
    if text.is_empty() { format!("error {code}") } else { format!("{text} (error {code})") }
}

/// `LoadLibraryExW` a DLL; the system's error text on failure.
pub fn open_library(path: &Path) -> Result<*mut c_void, String> {
    let path = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let h = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) };
    if h.is_null() {
        return Err(format!("cannot load {}: {}", path.display(), last_error()));
    }
    Ok(h)
}

pub fn symbol(handle: *mut c_void, name: &str) -> *mut c_void {
    let c = CString::new(name).unwrap_or_default();
    unsafe { GetProcAddress(handle, c.as_ptr()) }
}

/// Leave the process at once, without running any teardown: no `DllMain`
/// detach calls, no atexit handlers, no Rust exit path.
pub fn exit_now(code: c_int) -> ! {
    unsafe {
        TerminateProcess(GetCurrentProcess(), code as u32);
    }
    // TerminateProcess on the current process does not return; if it ever
    // failed, still leave.
    std::process::exit(code)
}

// ---------- VST3 module loading ----------

/// The VST3 module entry point, `InitDll()`. It takes no argument; the
/// callers pass the module handle, which the x64, x86 and ARM64 Windows
/// calling conventions let a function without parameters ignore (the argument
/// travels in a register, and the caller cleans up).
pub const MODULE_ENTRY: &str = "InitDll";

/// What `load_bundle` returns: the module handle.
pub type ModuleRef = *const c_void;

/// The VST3 bundle layout's architecture folder on this machine.
fn vst3_arch_dir() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64-win"
    } else if cfg!(target_arch = "aarch64") {
        "arm64-win"
    } else if cfg!(target_arch = "x86") {
        "x86-win"
    } else {
        "unknown-win"
    }
}

/// Load the VST3 module at `path`: `<name>.vst3\Contents\<arch>-win\<name>.vst3`,
/// or a single-file `.vst3` DLL (the older layout). Returns the module handle.
pub fn load_bundle(path: &Path) -> Result<ModuleRef, String> {
    let file = if path.is_dir() {
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let p = path.join("Contents").join(vst3_arch_dir()).join(format!("{stem}.vst3"));
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
