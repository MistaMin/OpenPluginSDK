//! The few system calls the probes need, declared by hand: `dlopen`, a little
//! CoreFoundation, and the AudioToolbox calls that host one Audio Unit. Kept
//! to what is used, so the crate needs no binding generator.

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
pub const RTLD_LOCAL: c_int = 4;

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

// ---------- CoreFoundation ----------

pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFURLRef = *const c_void;
pub type CFBundleRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFStringCreateWithCString(alloc: CFAllocatorRef, c: *const c_char, encoding: u32) -> CFStringRef;
    pub fn CFStringGetCString(s: CFStringRef, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    pub fn CFStringGetLength(s: CFStringRef) -> isize;
    pub fn CFArrayGetCount(array: CFTypeRef) -> isize;
    pub fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
    pub fn CFURLCreateFromFileSystemRepresentation(alloc: CFAllocatorRef, buffer: *const u8, len: isize, is_dir: u8) -> CFURLRef;
    pub fn CFBundleCreate(alloc: CFAllocatorRef, url: CFURLRef) -> CFBundleRef;
    pub fn CFBundleLoadExecutable(bundle: CFBundleRef) -> u8;
    pub fn CFBundleGetFunctionPointerForName(bundle: CFBundleRef, name: CFStringRef) -> *mut c_void;
}

/// A CFString as Rust text.
pub fn cf_to_string(s: CFStringRef) -> String {
    if s.is_null() {
        return String::new();
    }
    let len = unsafe { CFStringGetLength(s) }.max(0) as usize;
    let cap = len * 4 + 1;
    let mut buf = vec![0u8; cap];
    let ok = unsafe { CFStringGetCString(s, buf.as_mut_ptr().cast(), cap as isize, K_CF_STRING_ENCODING_UTF8) };
    if ok == 0 {
        return String::new();
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// A new CFString; release it with `CFRelease`.
pub fn cf_string(text: &str) -> CFStringRef {
    let c = CString::new(text).unwrap_or_default();
    unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), K_CF_STRING_ENCODING_UTF8) }
}

/// Load the bundle at `path` and return it loaded.
pub fn load_bundle(path: &Path) -> Result<CFBundleRef, String> {
    let bytes = path.to_string_lossy().into_owned().into_bytes();
    let url = unsafe { CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, 1) };
    if url.is_null() {
        return Err("not a usable path".into());
    }
    let bundle = unsafe { CFBundleCreate(std::ptr::null(), url) };
    unsafe { CFRelease(url) };
    if bundle.is_null() {
        return Err("CFBundleCreate failed: not a bundle".into());
    }
    if unsafe { CFBundleLoadExecutable(bundle) } == 0 {
        return Err("the bundle's executable would not load".into());
    }
    Ok(bundle)
}

pub fn bundle_function(bundle: CFBundleRef, name: &str) -> *mut c_void {
    let s = cf_string(name);
    let f = unsafe { CFBundleGetFunctionPointerForName(bundle, s) };
    unsafe { CFRelease(s) };
    f
}

// ---------- AudioToolbox ----------

pub type OSStatus = i32;
pub type AudioComponent = *mut c_void;
pub type AudioComponentInstance = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AudioComponentDescription {
    pub component_type: u32,
    pub component_sub_type: u32,
    pub component_manufacturer: u32,
    pub component_flags: u32,
    pub component_flags_mask: u32,
}

pub type AudioComponentFactoryFunction = unsafe extern "C" fn(desc: *const AudioComponentDescription) -> *mut c_void;

#[repr(C)]
pub struct AudioUnitParameterInfo {
    pub name: [c_char; 52],
    pub unit_name: CFStringRef,
    pub clump_id: u32,
    pub cf_name_string: CFStringRef,
    pub unit: u32,
    pub min_value: f32,
    pub max_value: f32,
    pub default_value: f32,
    pub flags: u32,
}

#[repr(C)]
pub struct AudioUnitParameterStringFromValue {
    pub param_id: u32,
    pub value: *const f32,
    pub out_string: CFStringRef,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AUChannelInfo {
    pub in_channels: i16,
    pub out_channels: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AudioStreamBasicDescription {
    pub sample_rate: f64,
    pub format_id: u32,
    pub format_flags: u32,
    pub bytes_per_packet: u32,
    pub frames_per_packet: u32,
    pub bytes_per_frame: u32,
    pub channels_per_frame: u32,
    pub bits_per_channel: u32,
    pub reserved: u32,
}

pub const AU_PROP_PARAMETER_LIST: u32 = 3;
pub const AU_PROP_PARAMETER_INFO: u32 = 4;
pub const AU_PROP_STREAM_FORMAT: u32 = 8;
pub const AU_PROP_ELEMENT_COUNT: u32 = 11;
pub const AU_PROP_LATENCY: u32 = 12;
pub const AU_PROP_SUPPORTED_NUM_CHANNELS: u32 = 13;
pub const AU_PROP_PARAMETER_VALUE_STRINGS: u32 = 16;
pub const AU_PROP_TAIL_TIME: u32 = 20;
pub const AU_PROP_PARAMETER_STRING_FROM_VALUE: u32 = 33;
pub const AU_SCOPE_GLOBAL: u32 = 0;
pub const AU_SCOPE_INPUT: u32 = 1;
pub const AU_SCOPE_OUTPUT: u32 = 2;

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    pub fn AudioComponentRegister(desc: *const AudioComponentDescription, name: CFStringRef, version: u32, factory: AudioComponentFactoryFunction) -> AudioComponent;
    pub fn AudioComponentFindNext(previous: AudioComponent, desc: *const AudioComponentDescription) -> AudioComponent;
    pub fn AudioComponentInstanceNew(component: AudioComponent, out: *mut AudioComponentInstance) -> OSStatus;
    pub fn AudioComponentInstanceDispose(instance: AudioComponentInstance) -> OSStatus;
    pub fn AudioComponentCopyName(component: AudioComponent, out: *mut CFStringRef) -> OSStatus;
    pub fn AudioUnitInitialize(unit: AudioComponentInstance) -> OSStatus;
    pub fn AudioUnitGetPropertyInfo(unit: AudioComponentInstance, prop: u32, scope: u32, element: u32, size: *mut u32, writable: *mut u8) -> OSStatus;
    pub fn AudioUnitGetProperty(unit: AudioComponentInstance, prop: u32, scope: u32, element: u32, data: *mut c_void, size: *mut u32) -> OSStatus;
    pub fn AudioUnitSetProperty(unit: AudioComponentInstance, prop: u32, scope: u32, element: u32, data: *const c_void, size: u32) -> OSStatus;
}

/// Four characters as the big-endian code Audio Unit descriptions use.
pub fn fourcc(text: &str) -> Option<u32> {
    let b = text.as_bytes();
    if b.len() != 4 {
        return None;
    }
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

// ---------- AudioToolbox: rendering ----------

pub const AU_PROP_MAXIMUM_FRAMES_PER_SLICE: u32 = 14;
pub const AU_PROP_SET_RENDER_CALLBACK: u32 = 23;
pub const AU_PARAMETER_EVENT_IMMEDIATE: u32 = 1;
pub const AU_FORMAT_LINEAR_PCM: u32 = 0x6C70_636D;
/// Native-endian packed float, non-interleaved.
pub const AU_FORMAT_FLOAT_PLANAR: u32 = 1 | 8 | 32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AudioBuffer {
    pub number_channels: u32,
    pub data_byte_size: u32,
    pub data: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SMPTETime {
    pub subframes: i16,
    pub subframe_divisor: i16,
    pub counter: u32,
    pub kind: u32,
    pub flags: u32,
    pub hours: i16,
    pub minutes: i16,
    pub seconds: i16,
    pub frames: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AudioTimeStamp {
    pub sample_time: f64,
    pub host_time: u64,
    pub rate_scalar: f64,
    pub word_clock_time: u64,
    pub smpte_time: SMPTETime,
    pub flags: u32,
    pub reserved: u32,
}

/// `AudioUnitParameterEvent` with the immediate variant of its union.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AudioUnitParameterEvent {
    pub scope: u32,
    pub element: u32,
    pub parameter: u32,
    pub event_type: u32,
    pub buffer_offset: u32,
    pub value: f32,
    pub unused: [u32; 2],
}

#[repr(C)]
pub struct AURenderCallbackStruct {
    pub input_proc: unsafe extern "C" fn(*mut c_void, *mut u32, *const AudioTimeStamp, u32, u32, *mut c_void) -> OSStatus,
    pub input_proc_ref_con: *mut c_void,
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    pub fn AudioUnitRender(unit: AudioComponentInstance, flags: *mut u32, time: *const AudioTimeStamp, bus: u32, frames: u32, data: *mut c_void) -> OSStatus;
    pub fn AudioUnitScheduleParameters(unit: AudioComponentInstance, events: *const AudioUnitParameterEvent, count: u32) -> OSStatus;
}

/// A code back to four characters; non-printing bytes become `?`.
pub fn fourcc_text(code: u32) -> String {
    code.to_be_bytes().iter().map(|b| if (0x20..0x7f).contains(b) { *b as char } else { '?' }).collect()
}

// ---------- state capture ----------

pub type CFDataRef = *const c_void;
pub const AU_PROP_CLASS_INFO: u32 = 0;
pub const AU_PROP_FACTORY_PRESETS: u32 = 24;
pub const AU_PROP_PRESENT_PRESET: u32 = 36;
/// `kCFPropertyListBinaryFormat_v1_0`.
pub const K_CF_PROPERTY_LIST_BINARY_FORMAT: isize = 200;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AUPreset {
    pub preset_number: i32,
    pub preset_name: CFStringRef,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub fn CFPropertyListCreateData(alloc: CFAllocatorRef, plist: CFTypeRef, format: isize, options: usize, error: *mut CFTypeRef) -> CFDataRef;
    pub fn CFDataGetLength(data: CFDataRef) -> isize;
    pub fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    pub fn AudioUnitGetParameter(unit: AudioComponentInstance, param: u32, scope: u32, element: u32, value: *mut f32) -> OSStatus;
    pub fn AudioUnitSetParameter(unit: AudioComponentInstance, param: u32, scope: u32, element: u32, value: f32, buffer_offset: u32) -> OSStatus;
}
