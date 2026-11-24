//! macOS: `dlopen`, a little CoreFoundation, and the AudioToolbox calls that
//! host one Audio Unit. Only macOS work edits this file; see `sys/mod.rs`.

#![allow(non_camel_case_types, non_snake_case, dead_code, clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;

unsafe extern "C" {
    /// POSIX `dlopen`: load a shared library; null on failure. Raw FFI; prefer [`open_library`].
    pub fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    /// POSIX `dlsym`: an exported symbol's address, or null. Raw FFI; prefer [`symbol`].
    pub fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    /// POSIX `dlerror`: the last `dl*` error as C text, or null.
    pub fn dlerror() -> *const c_char;
    /// POSIX `_exit`: end the process with no teardown. Prefer [`exit_now`].
    pub fn _exit(code: c_int) -> !;
}

/// `RTLD_NOW`: resolve every symbol at load.
pub const RTLD_NOW: c_int = 2;
/// `RTLD_LOCAL` (4 on macOS): keep the library's symbols out of the global namespace.
pub const RTLD_LOCAL: c_int = 4;

/// The VST3 module entry point, `bundleEntry(CFBundleRef)`.
pub const MODULE_ENTRY: &str = "bundleEntry";

/// What `load_bundle` returns and the module entry point takes: the CFBundle.
pub type ModuleRef = CFBundleRef;

/// Leave the process at once, without running any teardown.
pub fn exit_now(code: c_int) -> ! {
    unsafe { _exit(code) }
}

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

/// The address of `name` in a library from [`open_library`], or null when it is not exported
/// (or `name` holds a NUL byte).
pub fn symbol(handle: *mut c_void, name: &str) -> *mut c_void {
    let c = CString::new(name).unwrap_or_default();
    unsafe { dlsym(handle, c.as_ptr()) }
}

// ---------- CoreFoundation ----------

/// CoreFoundation `CFTypeRef`: any CF object.
pub type CFTypeRef = *const c_void;
/// CoreFoundation `CFStringRef`.
pub type CFStringRef = *const c_void;
/// CoreFoundation `CFURLRef`.
pub type CFURLRef = *const c_void;
/// CoreFoundation `CFBundleRef`.
pub type CFBundleRef = *const c_void;
/// CoreFoundation `CFAllocatorRef`; null is the default allocator.
pub type CFAllocatorRef = *const c_void;
/// `kCFStringEncodingUTF8`.
pub const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    /// CoreFoundation `CFRelease`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn CFRelease(cf: CFTypeRef);
    /// CoreFoundation `CFStringCreateWithCString`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn CFStringCreateWithCString(alloc: CFAllocatorRef, c: *const c_char, encoding: u32) -> CFStringRef;
    /// CoreFoundation `CFStringGetCString`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn CFStringGetCString(s: CFStringRef, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    /// CoreFoundation `CFStringGetLength`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn CFStringGetLength(s: CFStringRef) -> isize;
    /// CoreFoundation `CFArrayGetCount`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn CFArrayGetCount(array: CFTypeRef) -> isize;
    /// CoreFoundation `CFArrayGetValueAtIndex`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
    /// CoreFoundation `CFURLCreateFromFileSystemRepresentation`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn CFURLCreateFromFileSystemRepresentation(alloc: CFAllocatorRef, buffer: *const u8, len: isize, is_dir: u8) -> CFURLRef;
    /// CoreFoundation `CFBundleCreate`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn CFBundleCreate(alloc: CFAllocatorRef, url: CFURLRef) -> CFBundleRef;
    /// CoreFoundation `CFBundleLoadExecutable`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn CFBundleLoadExecutable(bundle: CFBundleRef) -> u8;
    /// CoreFoundation `CFBundleGetFunctionPointerForName`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
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

/// The address of `name` in a loaded bundle from [`load_bundle`], or null when it is not exported.
pub fn bundle_function(bundle: CFBundleRef, name: &str) -> *mut c_void {
    let s = cf_string(name);
    let f = unsafe { CFBundleGetFunctionPointerForName(bundle, s) };
    unsafe { CFRelease(s) };
    f
}

// ---------- AudioToolbox ----------

/// `OSStatus`: 0 is success, anything else an error code.
pub type OSStatus = i32;
/// AudioToolbox `AudioComponent`.
pub type AudioComponent = *mut c_void;
/// AudioToolbox `AudioComponentInstance` (an `AudioUnit`).
pub type AudioComponentInstance = *mut c_void;

/// AudioToolbox `AudioComponentDescription`. Same layout as the C struct.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AudioComponentDescription {
    /// `mComponentType`.
    pub component_type: u32,
    /// `mComponentSubType`.
    pub component_sub_type: u32,
    /// `mComponentManufacturer`.
    pub component_manufacturer: u32,
    /// `mComponentFlags`.
    pub component_flags: u32,
    /// `mComponentFlagsMask`.
    pub component_flags_mask: u32,
}

/// AudioToolbox `AudioComponentFactoryFunction`: what a bundle's `factoryFunction` symbol is.
pub type AudioComponentFactoryFunction = unsafe extern "C" fn(desc: *const AudioComponentDescription) -> *mut c_void;

/// AudioToolbox `AudioUnitParameterInfo`. Same layout as the C struct.
#[repr(C)]
pub struct AudioUnitParameterInfo {
    /// `name`.
    pub name: [c_char; 52],
    /// `unitName`.
    pub unit_name: CFStringRef,
    /// `clumpID`.
    pub clump_id: u32,
    /// `cfNameString`.
    pub cf_name_string: CFStringRef,
    /// `unit`.
    pub unit: u32,
    /// `minValue`.
    pub min_value: f32,
    /// `maxValue`.
    pub max_value: f32,
    /// `defaultValue`.
    pub default_value: f32,
    /// `flags`.
    pub flags: u32,
}

/// AudioToolbox `AudioUnitParameterStringFromValue`. Same layout as the C struct.
#[repr(C)]
pub struct AudioUnitParameterStringFromValue {
    /// `inParamID`.
    pub param_id: u32,
    /// `inValue`.
    pub value: *const f32,
    /// `outString`.
    pub out_string: CFStringRef,
}

/// AudioToolbox `AUChannelInfo`; -1 means any count. Same layout as the C struct.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AUChannelInfo {
    /// `inChannels`.
    pub in_channels: i16,
    /// `outChannels`.
    pub out_channels: i16,
}

/// CoreAudio `AudioStreamBasicDescription`. Same layout as the C struct.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AudioStreamBasicDescription {
    /// `mSampleRate`.
    pub sample_rate: f64,
    /// `mFormatID`.
    pub format_id: u32,
    /// `mFormatFlags`.
    pub format_flags: u32,
    /// `mBytesPerPacket`.
    pub bytes_per_packet: u32,
    /// `mFramesPerPacket`.
    pub frames_per_packet: u32,
    /// `mBytesPerFrame`.
    pub bytes_per_frame: u32,
    /// `mChannelsPerFrame`.
    pub channels_per_frame: u32,
    /// `mBitsPerChannel`.
    pub bits_per_channel: u32,
    /// `mReserved`.
    pub reserved: u32,
}

/// `kAudioUnitProperty_ParameterList`.
pub const AU_PROP_PARAMETER_LIST: u32 = 3;
/// `kAudioUnitProperty_ParameterInfo`.
pub const AU_PROP_PARAMETER_INFO: u32 = 4;
/// `kAudioUnitProperty_StreamFormat`.
pub const AU_PROP_STREAM_FORMAT: u32 = 8;
/// `kAudioUnitProperty_ElementCount`.
pub const AU_PROP_ELEMENT_COUNT: u32 = 11;
/// `kAudioUnitProperty_Latency` (seconds, as `f64`).
pub const AU_PROP_LATENCY: u32 = 12;
/// `kAudioUnitProperty_SupportedNumChannels`.
pub const AU_PROP_SUPPORTED_NUM_CHANNELS: u32 = 13;
/// `kAudioUnitProperty_ParameterValueStrings`.
pub const AU_PROP_PARAMETER_VALUE_STRINGS: u32 = 16;
/// `kAudioUnitProperty_TailTime` (seconds, as `f64`).
pub const AU_PROP_TAIL_TIME: u32 = 20;
/// `kAudioUnitProperty_ParameterStringFromValue`.
pub const AU_PROP_PARAMETER_STRING_FROM_VALUE: u32 = 33;
/// `kAudioUnitScope_Global`.
pub const AU_SCOPE_GLOBAL: u32 = 0;
/// `kAudioUnitScope_Input`.
pub const AU_SCOPE_INPUT: u32 = 1;
/// `kAudioUnitScope_Output`.
pub const AU_SCOPE_OUTPUT: u32 = 2;

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    /// AudioToolbox `AudioComponentRegister`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn AudioComponentRegister(desc: *const AudioComponentDescription, name: CFStringRef, version: u32, factory: AudioComponentFactoryFunction) -> AudioComponent;
    /// AudioToolbox `AudioComponentFindNext`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn AudioComponentFindNext(previous: AudioComponent, desc: *const AudioComponentDescription) -> AudioComponent;
    /// AudioToolbox `AudioComponentInstanceNew`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn AudioComponentInstanceNew(component: AudioComponent, out: *mut AudioComponentInstance) -> OSStatus;
    /// AudioToolbox `AudioComponentInstanceDispose`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn AudioComponentInstanceDispose(instance: AudioComponentInstance) -> OSStatus;
    /// AudioToolbox `AudioComponentCopyName`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn AudioComponentCopyName(component: AudioComponent, out: *mut CFStringRef) -> OSStatus;
    /// AudioToolbox `AudioUnitInitialize`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn AudioUnitInitialize(unit: AudioComponentInstance) -> OSStatus;
    /// AudioToolbox `AudioUnitGetPropertyInfo`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn AudioUnitGetPropertyInfo(unit: AudioComponentInstance, prop: u32, scope: u32, element: u32, size: *mut u32, writable: *mut u8) -> OSStatus;
    /// AudioToolbox `AudioUnitGetProperty`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn AudioUnitGetProperty(unit: AudioComponentInstance, prop: u32, scope: u32, element: u32, data: *mut c_void, size: *mut u32) -> OSStatus;
    /// AudioToolbox `AudioUnitSetProperty`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
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

/// `kAudioUnitProperty_MaximumFramesPerSlice`.
pub const AU_PROP_MAXIMUM_FRAMES_PER_SLICE: u32 = 14;
/// `kAudioUnitProperty_SetRenderCallback`.
pub const AU_PROP_SET_RENDER_CALLBACK: u32 = 23;
/// `kParameterEvent_Immediate`.
pub const AU_PARAMETER_EVENT_IMMEDIATE: u32 = 1;
/// `kAudioFormatLinearPCM` (`'lpcm'`).
pub const AU_FORMAT_LINEAR_PCM: u32 = 0x6C70_636D;
/// Native-endian packed float, non-interleaved.
pub const AU_FORMAT_FLOAT_PLANAR: u32 = 1 | 8 | 32;

/// CoreAudio `AudioBuffer`. Same layout as the C struct.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AudioBuffer {
    /// `mNumberChannels`.
    pub number_channels: u32,
    /// `mDataByteSize`.
    pub data_byte_size: u32,
    /// `mData`.
    pub data: *mut c_void,
}

/// CoreAudio `SMPTETime`. Same layout as the C struct.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SMPTETime {
    /// `mSubframes`.
    pub subframes: i16,
    /// `mSubframeDivisor`.
    pub subframe_divisor: i16,
    /// `mCounter`.
    pub counter: u32,
    /// `mKind`.
    pub kind: u32,
    /// `mFlags`.
    pub flags: u32,
    /// `mHours`.
    pub hours: i16,
    /// `mMinutes`.
    pub minutes: i16,
    /// `mSeconds`.
    pub seconds: i16,
    /// `mFrames`.
    pub frames: i16,
}

/// CoreAudio `AudioTimeStamp`. Same layout as the C struct.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AudioTimeStamp {
    /// `mSampleTime`.
    pub sample_time: f64,
    /// `mHostTime`.
    pub host_time: u64,
    /// `mRateScalar`.
    pub rate_scalar: f64,
    /// `mWordClockTime`.
    pub word_clock_time: u64,
    /// `mSMPTETime`.
    pub smpte_time: SMPTETime,
    /// `mFlags`.
    pub flags: u32,
    /// `mReserved`.
    pub reserved: u32,
}

/// `AudioUnitParameterEvent` with the immediate variant of its union.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AudioUnitParameterEvent {
    /// `scope`.
    pub scope: u32,
    /// `element`.
    pub element: u32,
    /// `parameter`.
    pub parameter: u32,
    /// `eventType`.
    pub event_type: u32,
    /// `eventValues.immediate.bufferOffset`.
    pub buffer_offset: u32,
    /// `eventValues.immediate.value`.
    pub value: f32,
    /// The rest of the `eventValues` union (padding).
    pub unused: [u32; 2],
}

/// AudioToolbox `AURenderCallbackStruct`. Same layout as the C struct.
#[repr(C)]
pub struct AURenderCallbackStruct {
    /// `inputProc`.
    pub input_proc: unsafe extern "C" fn(*mut c_void, *mut u32, *const AudioTimeStamp, u32, u32, *mut c_void) -> OSStatus,
    /// `inputProcRefCon`.
    pub input_proc_ref_con: *mut c_void,
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    /// AudioToolbox `AudioUnitRender`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn AudioUnitRender(unit: AudioComponentInstance, flags: *mut u32, time: *const AudioTimeStamp, bus: u32, frames: u32, data: *mut c_void) -> OSStatus;
    /// AudioToolbox `AudioUnitScheduleParameters`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn AudioUnitScheduleParameters(unit: AudioComponentInstance, events: *const AudioUnitParameterEvent, count: u32) -> OSStatus;
}

/// A code back to four characters; non-printing bytes become `?`.
pub fn fourcc_text(code: u32) -> String {
    code.to_be_bytes().iter().map(|b| if (0x20..0x7f).contains(b) { *b as char } else { '?' }).collect()
}

// ---------- state capture ----------

/// CoreFoundation `CFDataRef`.
pub type CFDataRef = *const c_void;
/// `kAudioUnitProperty_ClassInfo`: the unit's state as a property list.
pub const AU_PROP_CLASS_INFO: u32 = 0;
/// `kAudioUnitProperty_FactoryPresets`.
pub const AU_PROP_FACTORY_PRESETS: u32 = 24;
/// `kAudioUnitProperty_PresentPreset`.
pub const AU_PROP_PRESENT_PRESET: u32 = 36;
/// `kCFPropertyListBinaryFormat_v1_0`.
pub const K_CF_PROPERTY_LIST_BINARY_FORMAT: isize = 200;

/// AudioToolbox `AUPreset`. Same layout as the C struct.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AUPreset {
    /// `presetNumber`.
    pub preset_number: i32,
    /// `presetName`.
    pub preset_name: CFStringRef,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    /// CoreFoundation `CFPropertyListCreateData`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn CFPropertyListCreateData(alloc: CFAllocatorRef, plist: CFTypeRef, format: isize, options: usize, error: *mut CFTypeRef) -> CFDataRef;
    /// CoreFoundation `CFPropertyListCreateWithData`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn CFPropertyListCreateWithData(alloc: CFAllocatorRef, data: CFDataRef, options: usize, format: *mut isize, error: *mut CFTypeRef) -> CFTypeRef;
    /// CoreFoundation `CFDataCreate`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn CFDataCreate(alloc: CFAllocatorRef, bytes: *const u8, length: isize) -> CFDataRef;
    /// CoreFoundation `CFDataGetLength`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn CFDataGetLength(data: CFDataRef) -> isize;
    /// CoreFoundation `CFDataGetBytePtr`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    /// AudioToolbox `AudioUnitGetParameter`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn AudioUnitGetParameter(unit: AudioComponentInstance, param: u32, scope: u32, element: u32, value: *mut f32) -> OSStatus;
    /// AudioToolbox `AudioUnitSetParameter`. Raw FFI: the caller upholds the C contract (see Apple's documentation).
    pub fn AudioUnitSetParameter(unit: AudioComponentInstance, param: u32, scope: u32, element: u32, value: f32, buffer_offset: u32) -> OSStatus;
}
