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
    fn SetErrorMode(mode: u32) -> u32;
}

/// Look for the DLLs a plug-in depends on in the plug-in's own folder first,
/// the way a host loads it. Needs an absolute path.
const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x0000_0008;
const FORMAT_MESSAGE_IGNORE_INSERTS: u32 = 0x0000_0200;
const FORMAT_MESSAGE_FROM_SYSTEM: u32 = 0x0000_1000;
/// No system dialog for a DLL that cannot be found or opened, and no Windows
/// Error Reporting window when the plug-in crashes: the probe dies with its
/// exception code at once, instead of waiting in a dialog (in a desktop
/// session) until the parent's timeout kills it and reports a hang.
const SEM_FAILCRITICALERRORS: u32 = 0x0001;
const SEM_NOGPFAULTERRORBOX: u32 = 0x0002;
const SEM_NOOPENFILEERRORBOX: u32 = 0x8000;

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

/// `LoadLibraryExW` a DLL; the system's error text on failure. Only the probe
/// child loads plug-ins, so the process-wide error mode set here is the child's.
pub fn open_library(path: &Path) -> Result<*mut c_void, String> {
    unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX) };
    let path = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let h = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) };
    if h.is_null() {
        return Err(format!("cannot load {}: {}", path.display(), last_error()));
    }
    Ok(h)
}

/// The address of `name` in a library from [`open_library`], or null when it is not exported
/// (or `name` holds a NUL byte).
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

/// The address of `name` in a module from [`load_bundle`], or null when it is not exported.
pub fn bundle_function(module: ModuleRef, name: &str) -> *mut c_void {
    symbol(module as *mut c_void, name)
}

// ---------- VST3 as COM ----------

/// `kNoInterface` and `kNotImplemented` in a VST3 module built for Windows. The
/// VST3 SDK is COM compatible there (`COM_COMPATIBLE` in `funknown.h`), so its
/// result codes are COM's `E_NOINTERFACE` and `E_NOTIMPL`, not the small numbers
/// the other systems use. `kResultOk` (0) and `kResultFalse` (1) are the same everywhere.
pub const VST3_NO_INTERFACE: i32 = 0x8000_4002_u32 as i32;
/// `kNotImplemented` in a Windows VST3 module: COM's `E_NOTIMPL`.
pub const VST3_NOT_IMPLEMENTED: i32 = 0x8000_4001_u32 as i32;

/// An interface id written as the four 32-bit words of `DECLARE_CLASS_IID`, laid
/// out as the SDK lays it out on Windows (`INLINE_UID` with `COM_COMPATIBLE`): a
/// COM `GUID`, whose first 32-bit field and next two 16-bit fields are in the
/// machine's little-endian byte order; the last eight bytes are as written.
pub const fn vst3_iid(a: u32, b: u32, c: u32, d: u32) -> [u8; 16] {
    let (a, c, d) = (a.to_le_bytes(), c.to_be_bytes(), d.to_be_bytes());
    let (b1, b2) = (((b >> 16) as u16).to_le_bytes(), (b as u16).to_le_bytes());
    [a[0], a[1], a[2], a[3], b1[0], b1[1], b2[0], b2[1], c[0], c[1], c[2], c[3], d[0], d[1], d[2], d[3]]
}

/// A class id from a Windows module (a COM `GUID`) as the 16 bytes its FUID string
/// shows, which are the bytes the same class has on the other systems: the first
/// 32-bit field and the next two 16-bit fields turned round.
pub const fn vst3_fuid_bytes(id: &[u8; 16]) -> [u8; 16] {
    [id[3], id[2], id[1], id[0], id[5], id[4], id[7], id[6], id[8], id[9], id[10], id[11], id[12], id[13], id[14], id[15]]
}

#[cfg(test)]
mod tests {
    /// Turning an id round twice gives it back, and an id laid out by `vst3_iid` reads
    /// as the words it was written with.
    #[test]
    fn a_class_id_reads_the_same_as_on_the_other_systems() {
        let com = super::vst3_iid(0x0011_2233, 0x4455_6677, 0x8899_AABB, 0xCCDD_EEFF);
        let read = super::vst3_fuid_bytes(&com);
        assert_eq!(read, [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        assert_eq!(super::vst3_fuid_bytes(&read), com);
    }

    /// FUnknown is COM's IUnknown, `00000000-0000-0000-C000-000000000046`; the
    /// second id has no byte that repeats, so every position is checked.
    #[test]
    fn a_vst3_iid_is_a_com_guid() {
        assert_eq!(super::vst3_iid(0x0000_0000, 0x0000_0000, 0xC000_0000, 0x0000_0046), [0, 0, 0, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
        assert_eq!(
            super::vst3_iid(0x0011_2233, 0x4455_6677, 0x8899_AABB, 0xCCDD_EEFF),
            [0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]
        );
    }
}
