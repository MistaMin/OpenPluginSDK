//! VST3 probe, over a hand-written COM binding.
//!
//! The vtables below follow the public interface headers of the VST3 SDK
//! (`pluginterfaces/base/ipluginbase.h`, `funknown.h`, `vst/ivstcomponent.h`,
//! `vst/ivstaudioprocessor.h`, `vst/ivsteditcontroller.h`,
//! `vst/ivsthostapplication.h`), which are MIT licensed. Only the methods the
//! scan calls are given types; the rest are `usize` placeholders that keep the
//! slot order. Runs in the child process.

use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::manifest::*;
use crate::sys;

use render::ProcessData;

type Tuid = [u8; 16];
type TResult = i32;
const OK: TResult = 0;
const NO_INTERFACE: TResult = -1;
const NOT_IMPLEMENTED: TResult = 3;

/// An interface id written as the four 32-bit words of `DECLARE_CLASS_IID`
/// (big-endian byte order, as on macOS).
const fn iid(a: u32, b: u32, c: u32, d: u32) -> Tuid {
    let (a, b, c, d) = (a.to_be_bytes(), b.to_be_bytes(), c.to_be_bytes(), d.to_be_bytes());
    [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3], c[0], c[1], c[2], c[3], d[0], d[1], d[2], d[3]]
}
const IID_FUNKNOWN: Tuid = iid(0x0000_0000, 0x0000_0000, 0xC000_0000, 0x0000_0046);
const IID_PLUGIN_FACTORY2: Tuid = iid(0x0007_B650, 0xF24B_4C0B, 0xA464_EDB9, 0xF00B_2ABB);
const IID_PLUGIN_FACTORY3: Tuid = iid(0x4555_A2AB, 0xC123_4E57, 0x9B12_2910, 0x3687_8931);
const IID_COMPONENT: Tuid = iid(0xE831_FF31, 0xF2D5_4301, 0x928E_BBEE, 0x2569_7802);
const IID_AUDIO_PROCESSOR: Tuid = iid(0x4204_3F99, 0xB7DA_453C, 0xA569_E79D, 0x9AAE_C33D);
const IID_EDIT_CONTROLLER: Tuid = iid(0xDCD7_BBE3, 0x7742_448D, 0xA874_AACC, 0x979C_759E);
const IID_COMPONENT_HANDLER: Tuid = iid(0x93A0_BEA3, 0x0BD0_45DB, 0x8E89_0B0C, 0xC1E4_6AC6);
const IID_MESSAGE: Tuid = iid(0x936F_033B, 0xC6C0_47DB, 0xBB08_82F8, 0x13C1_E613);
const IID_ATTRIBUTE_LIST: Tuid = iid(0x1E5F_0AEB, 0xCC7F_4533, 0xA254_4011, 0x38AD_5EE4);
const IID_CONNECTION_POINT: Tuid = iid(0x70A4_156F, 0x6E6E_4026, 0x9891_48BF, 0xAA60_D8D1);
const IID_HOST_APPLICATION: Tuid = iid(0x58E5_95CC, 0xDB2D_4969, 0x8B6A_AF8C, 0x36A6_64E5);

/// A COM object: a pointer to a pointer to a table of function pointers.
#[repr(C)]
struct Com<V> {
    vtbl: *const V,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Tuid, *mut *mut c_void) -> TResult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct FactoryVtbl {
    base: UnknownVtbl,
    get_factory_info: unsafe extern "system" fn(*mut c_void, *mut PFactoryInfo) -> TResult,
    count_classes: unsafe extern "system" fn(*mut c_void) -> i32,
    get_class_info: unsafe extern "system" fn(*mut c_void, i32, *mut PClassInfo) -> TResult,
    create_instance: unsafe extern "system" fn(*mut c_void, *const Tuid, *const Tuid, *mut *mut c_void) -> TResult,
    // IPluginFactory2
    get_class_info2: unsafe extern "system" fn(*mut c_void, i32, *mut PClassInfo2) -> TResult,
    // IPluginFactory3
    get_class_info_unicode: unsafe extern "system" fn(*mut c_void, i32, *mut PClassInfoW) -> TResult,
    set_host_context: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
}

#[repr(C)]
struct PFactoryInfo {
    vendor: [u8; 64],
    url: [u8; 256],
    email: [u8; 128],
    flags: i32,
}

#[repr(C)]
struct PClassInfo {
    cid: Tuid,
    cardinality: i32,
    category: [u8; 32],
    name: [u8; 64],
}

#[repr(C)]
struct PClassInfo2 {
    cid: Tuid,
    cardinality: i32,
    category: [u8; 32],
    name: [u8; 64],
    class_flags: u32,
    sub_categories: [u8; 128],
    vendor: [u8; 64],
    version: [u8; 64],
    sdk_version: [u8; 64],
}

#[repr(C)]
struct PClassInfoW {
    cid: Tuid,
    cardinality: i32,
    category: [u8; 32],
    name: [u16; 64],
    class_flags: u32,
    sub_categories: [u8; 128],
    vendor: [u16; 64],
    version: [u16; 64],
    sdk_version: [u16; 64],
}

#[repr(C)]
struct ComponentVtbl {
    base: UnknownVtbl,
    initialize: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    terminate: unsafe extern "system" fn(*mut c_void) -> TResult,
    get_controller_class_id: unsafe extern "system" fn(*mut c_void, *mut Tuid) -> TResult,
    set_io_mode: usize,
    get_bus_count: unsafe extern "system" fn(*mut c_void, i32, i32) -> i32,
    get_bus_info: unsafe extern "system" fn(*mut c_void, i32, i32, i32, *mut BusInfo) -> TResult,
    get_routing_info: usize,
    activate_bus: unsafe extern "system" fn(*mut c_void, i32, i32, i32, u8) -> TResult,
    set_active: unsafe extern "system" fn(*mut c_void, u8) -> TResult,
    set_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    get_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
}

#[repr(C)]
struct BusInfo {
    media_type: i32,
    direction: i32,
    channel_count: i32,
    name: [u16; 128],
    bus_type: i32,
    flags: u32,
}

#[repr(C)]
struct ProcessorVtbl {
    base: UnknownVtbl,
    set_bus_arrangements: unsafe extern "system" fn(*mut c_void, *mut u64, i32, *mut u64, i32) -> TResult,
    get_bus_arrangement: unsafe extern "system" fn(*mut c_void, i32, i32, *mut u64) -> TResult,
    can_process_sample_size: usize,
    get_latency_samples: unsafe extern "system" fn(*mut c_void) -> u32,
    setup_processing: unsafe extern "system" fn(*mut c_void, *mut ProcessSetup) -> TResult,
    set_processing: unsafe extern "system" fn(*mut c_void, u8) -> TResult,
    process: unsafe extern "system" fn(*mut c_void, *mut ProcessData) -> TResult,
    get_tail_samples: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct ProcessSetup {
    process_mode: i32,
    symbolic_sample_size: i32,
    max_samples_per_block: i32,
    sample_rate: f64,
}

#[repr(C)]
struct ControllerVtbl {
    base: UnknownVtbl,
    initialize: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    terminate: unsafe extern "system" fn(*mut c_void) -> TResult,
    set_component_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    set_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    get_state: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    get_parameter_count: unsafe extern "system" fn(*mut c_void) -> i32,
    get_parameter_info: unsafe extern "system" fn(*mut c_void, i32, *mut ParameterInfo) -> TResult,
    get_param_string_by_value: unsafe extern "system" fn(*mut c_void, u32, f64, *mut u16) -> TResult,
    get_param_value_by_string: usize,
    normalized_param_to_plain: unsafe extern "system" fn(*mut c_void, u32, f64) -> f64,
    plain_param_to_normalized: unsafe extern "system" fn(*mut c_void, u32, f64) -> f64,
    get_param_normalized: unsafe extern "system" fn(*mut c_void, u32) -> f64,
    set_param_normalized: unsafe extern "system" fn(*mut c_void, u32, f64) -> TResult,
    set_component_handler: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    create_view: usize,
}

#[repr(C)]
struct ParameterInfo {
    id: u32,
    title: [u16; 128],
    short_title: [u16; 128],
    units: [u16; 128],
    step_count: i32,
    default_normalized: f64,
    unit_id: i32,
    flags: i32,
}

// ---------- the host side: just enough for plugins to initialise ----------

#[repr(C)]
struct HostVtbl {
    base: UnknownVtbl,
    get_name: unsafe extern "system" fn(*mut c_void, *mut u16) -> TResult,
    create_instance: unsafe extern "system" fn(*mut c_void, *const Tuid, *const Tuid, *mut *mut c_void) -> TResult,
}

#[repr(C)]
struct HandlerVtbl {
    base: UnknownVtbl,
    begin_edit: unsafe extern "system" fn(*mut c_void, u32) -> TResult,
    perform_edit: unsafe extern "system" fn(*mut c_void, u32, f64) -> TResult,
    end_edit: unsafe extern "system" fn(*mut c_void, u32) -> TResult,
    restart_component: unsafe extern "system" fn(*mut c_void, i32) -> TResult,
}

static REFS: AtomicU32 = AtomicU32::new(1);

unsafe extern "system" fn app_query(this: *mut c_void, wanted: *const Tuid, out: *mut *mut c_void) -> TResult {
    let want = unsafe { *wanted };
    let found = if want == IID_FUNKNOWN || want == IID_HOST_APPLICATION {
        this
    } else if want == IID_COMPONENT_HANDLER {
        host_handler()
    } else {
        std::ptr::null_mut()
    };
    unsafe { *out = found };
    if found.is_null() { NO_INTERFACE } else { OK }
}
unsafe extern "system" fn handler_query(this: *mut c_void, wanted: *const Tuid, out: *mut *mut c_void) -> TResult {
    let want = unsafe { *wanted };
    let found = if want == IID_FUNKNOWN || want == IID_COMPONENT_HANDLER { this } else { std::ptr::null_mut() };
    unsafe { *out = found };
    if found.is_null() { NO_INTERFACE } else { OK }
}
unsafe extern "system" fn h_add(_: *mut c_void) -> u32 {
    REFS.fetch_add(1, Ordering::Relaxed) + 1
}
unsafe extern "system" fn h_release(_: *mut c_void) -> u32 {
    REFS.load(Ordering::Relaxed)
}
unsafe extern "system" fn h_name(_: *mut c_void, out: *mut u16) -> TResult {
    let name: Vec<u16> = "openplugin-port".encode_utf16().collect();
    for (i, c) in name.iter().enumerate() {
        unsafe { *out.add(i) = *c };
    }
    unsafe { *out.add(name.len()) = 0 };
    OK
}
/// `IHostApplication::createInstance`: plugins ask for an `IMessage` (to talk
/// to their controller) or an `IAttributeList`; the class id is the interface id.
unsafe extern "system" fn h_create(_: *mut c_void, cid: *const Tuid, _iid: *const Tuid, out: *mut *mut c_void) -> TResult {
    let want = unsafe { *cid };
    let obj = if want == IID_MESSAGE {
        new_message()
    } else if want == IID_ATTRIBUTE_LIST {
        new_attributes()
    } else {
        std::ptr::null_mut()
    };
    unsafe { *out = obj };
    if obj.is_null() { NOT_IMPLEMENTED } else { OK }
}
unsafe extern "system" fn h_edit(_: *mut c_void, _: u32) -> TResult {
    OK
}
unsafe extern "system" fn h_perform(_: *mut c_void, _: u32, _: f64) -> TResult {
    OK
}
unsafe extern "system" fn h_restart(_: *mut c_void, _: i32) -> TResult {
    OK
}

const APP_BASE: UnknownVtbl = UnknownVtbl { query_interface: app_query, add_ref: h_add, release: h_release };
const HANDLER_BASE: UnknownVtbl = UnknownVtbl { query_interface: handler_query, add_ref: h_add, release: h_release };
static HOST_VTBL: HostVtbl = HostVtbl { base: APP_BASE, get_name: h_name, create_instance: h_create };
static HANDLER_VTBL: HandlerVtbl = HandlerVtbl { base: HANDLER_BASE, begin_edit: h_edit, perform_edit: h_perform, end_edit: h_edit, restart_component: h_restart };

/// The host the plugins see: `IHostApplication` and `IComponentHandler`, one object each.
struct Host {
    app: Com<HostVtbl>,
    handler: Com<HandlerVtbl>,
}
unsafe impl Sync for Host {}
static HOST: Host = Host { app: Com { vtbl: &HOST_VTBL }, handler: Com { vtbl: &HANDLER_VTBL } };

fn host_app() -> *mut c_void {
    &HOST.app as *const _ as *mut c_void
}
fn host_handler() -> *mut c_void {
    &HOST.handler as *const _ as *mut c_void
}

// ---------- IMessage and IAttributeList ----------
//
// A component and its controller usually share state through a message sent
// over `IConnectionPoint`. The messages are made by the host, so the host
// must have them. They are leaked: the process is short-lived.

#[repr(C)]
struct MessageVtbl {
    base: UnknownVtbl,
    get_message_id: unsafe extern "system" fn(*mut c_void) -> *const u8,
    set_message_id: unsafe extern "system" fn(*mut c_void, *const u8),
    get_attributes: unsafe extern "system" fn(*mut c_void) -> *mut c_void,
}

#[repr(C)]
struct AttributesVtbl {
    base: UnknownVtbl,
    set_int: unsafe extern "system" fn(*mut c_void, *const u8, i64) -> TResult,
    get_int: unsafe extern "system" fn(*mut c_void, *const u8, *mut i64) -> TResult,
    set_float: unsafe extern "system" fn(*mut c_void, *const u8, f64) -> TResult,
    get_float: unsafe extern "system" fn(*mut c_void, *const u8, *mut f64) -> TResult,
    set_string: unsafe extern "system" fn(*mut c_void, *const u8, *const u16) -> TResult,
    get_string: unsafe extern "system" fn(*mut c_void, *const u8, *mut u16, u32) -> TResult,
    set_binary: unsafe extern "system" fn(*mut c_void, *const u8, *const c_void, u32) -> TResult,
    get_binary: unsafe extern "system" fn(*mut c_void, *const u8, *mut *const c_void, *mut u32) -> TResult,
}

enum Attr {
    Int(i64),
    Float(f64),
    Str(Vec<u16>),
    Bin(Vec<u8>),
}

#[repr(C)]
struct Attributes {
    vtbl: *const AttributesVtbl,
    map: Vec<(Vec<u8>, Attr)>,
}

#[repr(C)]
struct Message {
    vtbl: *const MessageVtbl,
    id: Vec<u8>,
    attrs: *mut Attributes,
}

fn key(id: *const u8) -> Vec<u8> {
    unsafe { std::ffi::CStr::from_ptr(id.cast()) }.to_bytes().to_vec()
}

fn attr_set(this: *mut c_void, id: *const u8, value: Attr) -> TResult {
    let a = unsafe { &mut *(this as *mut Attributes) };
    let k = key(id);
    a.map.retain(|(x, _)| *x != k);
    a.map.push((k, value));
    OK
}
fn attr_get<'a>(this: *mut c_void, id: *const u8) -> Option<&'a Attr> {
    let a = unsafe { &*(this as *mut Attributes) };
    let k = key(id);
    a.map.iter().find(|(x, _)| *x == k).map(|(_, v)| v)
}

unsafe extern "system" fn at_query(this: *mut c_void, wanted: *const Tuid, out: *mut *mut c_void) -> TResult {
    let want = unsafe { *wanted };
    let found = if want == IID_FUNKNOWN || want == IID_ATTRIBUTE_LIST { this } else { std::ptr::null_mut() };
    unsafe { *out = found };
    if found.is_null() { NO_INTERFACE } else { OK }
}
unsafe extern "system" fn at_set_int(this: *mut c_void, id: *const u8, v: i64) -> TResult {
    attr_set(this, id, Attr::Int(v))
}
unsafe extern "system" fn at_get_int(this: *mut c_void, id: *const u8, out: *mut i64) -> TResult {
    match attr_get(this, id) {
        Some(Attr::Int(v)) => {
            unsafe { *out = *v };
            OK
        }
        _ => 1,
    }
}
unsafe extern "system" fn at_set_float(this: *mut c_void, id: *const u8, v: f64) -> TResult {
    attr_set(this, id, Attr::Float(v))
}
unsafe extern "system" fn at_get_float(this: *mut c_void, id: *const u8, out: *mut f64) -> TResult {
    match attr_get(this, id) {
        Some(Attr::Float(v)) => {
            unsafe { *out = *v };
            OK
        }
        _ => 1,
    }
}
unsafe extern "system" fn at_set_string(this: *mut c_void, id: *const u8, s: *const u16) -> TResult {
    let mut v = Vec::new();
    let mut p = s;
    while !p.is_null() && unsafe { *p } != 0 && v.len() < 65536 {
        v.push(unsafe { *p });
        p = unsafe { p.add(1) };
    }
    attr_set(this, id, Attr::Str(v))
}
unsafe extern "system" fn at_get_string(this: *mut c_void, id: *const u8, out: *mut u16, bytes: u32) -> TResult {
    match attr_get(this, id) {
        Some(Attr::Str(v)) => {
            let room = (bytes as usize / 2).saturating_sub(1);
            for (i, c) in v.iter().take(room).enumerate() {
                unsafe { *out.add(i) = *c };
            }
            unsafe { *out.add(v.len().min(room)) = 0 };
            OK
        }
        _ => 1,
    }
}
unsafe extern "system" fn at_set_binary(this: *mut c_void, id: *const u8, data: *const c_void, bytes: u32) -> TResult {
    let v = if data.is_null() { Vec::new() } else { unsafe { std::slice::from_raw_parts(data.cast::<u8>(), bytes as usize) }.to_vec() };
    attr_set(this, id, Attr::Bin(v))
}
unsafe extern "system" fn at_get_binary(this: *mut c_void, id: *const u8, data: *mut *const c_void, bytes: *mut u32) -> TResult {
    match attr_get(this, id) {
        Some(Attr::Bin(v)) => {
            unsafe {
                *data = v.as_ptr().cast();
                *bytes = v.len() as u32;
            }
            OK
        }
        _ => 1,
    }
}

const AT_BASE: UnknownVtbl = UnknownVtbl { query_interface: at_query, add_ref: h_add, release: h_release };
static ATTRIBUTES_VTBL: AttributesVtbl = AttributesVtbl {
    base: AT_BASE,
    set_int: at_set_int,
    get_int: at_get_int,
    set_float: at_set_float,
    get_float: at_get_float,
    set_string: at_set_string,
    get_string: at_get_string,
    set_binary: at_set_binary,
    get_binary: at_get_binary,
};

fn new_attributes() -> *mut c_void {
    Box::into_raw(Box::new(Attributes { vtbl: &ATTRIBUTES_VTBL, map: Vec::new() })).cast()
}

unsafe extern "system" fn msg_query(this: *mut c_void, wanted: *const Tuid, out: *mut *mut c_void) -> TResult {
    let want = unsafe { *wanted };
    let found = if want == IID_FUNKNOWN || want == IID_MESSAGE { this } else { std::ptr::null_mut() };
    unsafe { *out = found };
    if found.is_null() { NO_INTERFACE } else { OK }
}
unsafe extern "system" fn msg_get_id(this: *mut c_void) -> *const u8 {
    unsafe { &*(this as *mut Message) }.id.as_ptr()
}
unsafe extern "system" fn msg_set_id(this: *mut c_void, id: *const u8) {
    let m = unsafe { &mut *(this as *mut Message) };
    m.id = if id.is_null() { Vec::new() } else { key(id) };
    m.id.push(0);
}
unsafe extern "system" fn msg_attributes(this: *mut c_void) -> *mut c_void {
    unsafe { &*(this as *mut Message) }.attrs.cast()
}
const MSG_BASE: UnknownVtbl = UnknownVtbl { query_interface: msg_query, add_ref: h_add, release: h_release };
static MESSAGE_VTBL: MessageVtbl = MessageVtbl { base: MSG_BASE, get_message_id: msg_get_id, set_message_id: msg_set_id, get_attributes: msg_attributes };

fn new_message() -> *mut c_void {
    let attrs = new_attributes() as *mut Attributes;
    Box::into_raw(Box::new(Message { vtbl: &MESSAGE_VTBL, id: vec![0], attrs })).cast()
}

#[repr(C)]
struct ConnectionVtbl {
    base: UnknownVtbl,
    connect: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    disconnect: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
    notify: unsafe extern "system" fn(*mut c_void, *mut c_void) -> TResult,
}

// ---------- helpers ----------

fn c8(buf: &[u8]) -> String {
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}
fn c16(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Call a method of a COM object through its vtable.
macro_rules! call {
    ($obj:expr, $vt:ty, $method:ident $(, $arg:expr)*) => {{
        let o = $obj as *mut Com<$vt>;
        unsafe { ((*(*o).vtbl).$method)(o as *mut c_void $(, $arg)*) }
    }};
}

/// `queryInterface` on any COM object; null when not offered.
fn query(obj: *mut c_void, wanted: &Tuid) -> *mut c_void {
    let mut out: *mut c_void = std::ptr::null_mut();
    let r = call!(obj, UnknownVtbl, query_interface, wanted, &mut out);
    if r == OK { out } else { std::ptr::null_mut() }
}

struct Class {
    cid: Tuid,
    category: String,
    name: String,
    class_flags: u32,
    sub_categories: String,
    vendor: String,
    version: String,
    sdk_version: String,
}

pub fn probe(bundle: &Path) -> Result<ProbeResult, String> {
    let handle = sys::load_bundle(bundle)?;
    // bundleEntry (macOS) or ModuleEntry (Linux) is required; a missing one is tolerated.
    let entry = sys::bundle_function(handle, sys::MODULE_ENTRY);
    if !entry.is_null() {
        let f: unsafe extern "C" fn(sys::ModuleRef) -> bool = unsafe { std::mem::transmute(entry) };
        if !unsafe { f(handle) } {
            return Err("the module entry point returned false".into());
        }
    }
    let get = sys::bundle_function(handle, "GetPluginFactory");
    if get.is_null() {
        return Err("no GetPluginFactory symbol: not a VST3 plugin".into());
    }
    let get: unsafe extern "C" fn() -> *mut c_void = unsafe { std::mem::transmute(get) };
    let factory = unsafe { get() };
    if factory.is_null() {
        return Err("GetPluginFactory returned null".into());
    }

    let mut finfo = PFactoryInfo { vendor: [0; 64], url: [0; 256], email: [0; 128], flags: 0 };
    let _ = call!(factory, FactoryVtbl, get_factory_info, &mut finfo);
    let f3 = query(factory, &IID_PLUGIN_FACTORY3);
    let f2 = query(factory, &IID_PLUGIN_FACTORY2);
    if !f3.is_null() {
        let _ = call!(f3, FactoryVtbl, set_host_context, host_app());
    }
    let count = call!(factory, FactoryVtbl, count_classes);

    let mut result = ProbeResult::default();
    let mut classes = Vec::new();
    for i in 0..count {
        let mut cl = Class {
            cid: [0; 16],
            category: String::new(),
            name: String::new(),
            class_flags: 0,
            sub_categories: String::new(),
            vendor: String::new(),
            version: String::new(),
            sdk_version: String::new(),
        };
        let mut done = false;
        if !f3.is_null() {
            let mut w: PClassInfoW = unsafe { std::mem::zeroed() };
            if call!(f3, FactoryVtbl, get_class_info_unicode, i, &mut w) == OK {
                cl = Class {
                    cid: w.cid,
                    category: c8(&w.category),
                    name: c16(&w.name),
                    class_flags: w.class_flags,
                    sub_categories: c8(&w.sub_categories),
                    vendor: c16(&w.vendor),
                    version: c16(&w.version),
                    sdk_version: c16(&w.sdk_version),
                };
                done = true;
            }
        }
        if !done && !f2.is_null() {
            let mut c2: PClassInfo2 = unsafe { std::mem::zeroed() };
            if call!(f2, FactoryVtbl, get_class_info2, i, &mut c2) == OK {
                cl = Class {
                    cid: c2.cid,
                    category: c8(&c2.category),
                    name: c8(&c2.name),
                    class_flags: c2.class_flags,
                    sub_categories: c8(&c2.sub_categories),
                    vendor: c8(&c2.vendor),
                    version: c8(&c2.version),
                    sdk_version: c8(&c2.sdk_version),
                };
                done = true;
            }
        }
        if !done {
            let mut c1: PClassInfo = unsafe { std::mem::zeroed() };
            if call!(factory, FactoryVtbl, get_class_info, i, &mut c1) != OK {
                result.notes.push(format!("class {i}: getClassInfo failed"));
                continue;
            }
            cl.cid = c1.cid;
            cl.category = c8(&c1.category);
            cl.name = c8(&c1.name);
        }
        classes.push(cl);
    }

    let mut n_audio = 0;
    for cl in classes.iter().filter(|c| c.category == "Audio Module Class") {
        let mut plugin = Plugin {
            name: cl.name.clone(),
            vendor: if cl.vendor.is_empty() { c8(&finfo.vendor) } else { cl.vendor.clone() },
            version: cl.version.clone(),
            url: c8(&finfo.url),
            email: c8(&finfo.email),
            categories: cl.sub_categories.split('|').filter(|s| !s.is_empty()).map(str::to_string).collect(),
            ..Plugin::default()
        };
        plugin.is_instrument = plugin.categories.iter().any(|c| c == "Instrument");
        plugin.ids.vst3 = Some(Vst3Ids {
            class_id: hex16(&cl.cid),
            controller_class_id: None,
            category: cl.category.clone(),
            sub_categories: cl.sub_categories.clone(),
            class_flags: cl.class_flags,
            sdk_version: cl.sdk_version.clone(),
            single_component: false,
            module_info: false,
            module_info_class: None,
        });
        if let Err(e) = describe(factory, cl, &mut plugin) {
            plugin.warnings.push(e);
        }
        result.plugins.push(plugin);
        n_audio += 1;
    }
    if n_audio == 0 {
        result.notes.push("the factory exports no Audio Module Class".into());
    }
    Ok(result)
}

fn describe(factory: *mut c_void, cl: &Class, plugin: &mut Plugin) -> Result<(), String> {
    let mut comp: *mut c_void = std::ptr::null_mut();
    let r = call!(factory, FactoryVtbl, create_instance, &cl.cid, &IID_COMPONENT, &mut comp);
    if r != OK || comp.is_null() {
        return Err(format!("createInstance(IComponent) failed ({r})"));
    }
    let r = call!(comp, ComponentVtbl, initialize, host_app());
    if r != OK {
        return Err(format!("IComponent::initialize failed ({r})"));
    }

    // Buses.
    {
        let media = 0; // audio
        for dir in 0..2 {
            let n = call!(comp, ComponentVtbl, get_bus_count, media, dir);
            for i in 0..n.clamp(0, 64) {
                let mut info: BusInfo = unsafe { std::mem::zeroed() };
                if call!(comp, ComponentVtbl, get_bus_info, media, dir, i, &mut info) == OK {
                    plugin.buses.push(Bus {
                        direction: if dir == 0 { "input" } else { "output" }.into(),
                        kind: if info.bus_type == 0 { "main" } else { "aux" }.into(),
                        name: c16(&info.name),
                        channels: info.channel_count.max(0) as u32,
                        arrangement: None,
                        layout: match info.channel_count {
                            1 => Some("mono".to_string()),
                            2 => Some("stereo".to_string()),
                            _ => None,
                        },
                    });
                    // Switch the bus on so latency and arrangement are as a host would see them.
                    let _ = call!(comp, ComponentVtbl, activate_bus, media, dir, i, 1u8);
                }
            }
        }
    }

    // Processor: arrangement, latency, tail.
    let proc_ = query(comp, &IID_AUDIO_PROCESSOR);
    if !proc_.is_null() {
        // Bus arrangements in the order the buses were listed.
        let mut seen = [0i32; 2];
        for bus in plugin.buses.iter_mut() {
            let dir = if bus.direction == "input" { 0 } else { 1 };
            let mut arr: u64 = 0;
            if call!(proc_, ProcessorVtbl, get_bus_arrangement, dir, seen[dir as usize], &mut arr) == OK {
                bus.arrangement = Some(format!("{arr:#x}"));
            }
            seen[dir as usize] += 1;
        }
        let mut setup = ProcessSetup { process_mode: 0, symbolic_sample_size: 0, max_samples_per_block: 4096, sample_rate: 48_000.0 };
        let _ = call!(proc_, ProcessorVtbl, setup_processing, &mut setup);
        let _ = call!(comp, ComponentVtbl, set_active, 1u8);
        plugin.latency_samples = Some(call!(proc_, ProcessorVtbl, get_latency_samples));
        plugin.tail_samples = Some(call!(proc_, ProcessorVtbl, get_tail_samples));
    } else {
        plugin.warnings.push("the component has no IAudioProcessor".into());
    }

    // Controller: the component's own, or a separate class.
    let mut controller = query(comp, &IID_EDIT_CONTROLLER);
    let single = !controller.is_null();
    let mut cid: Tuid = [0; 16];
    let has_cid = call!(comp, ComponentVtbl, get_controller_class_id, &mut cid) == OK && cid != [0; 16];
    if controller.is_null() && has_cid {
        let mut obj: *mut c_void = std::ptr::null_mut();
        let r = call!(factory, FactoryVtbl, create_instance, &cid, &IID_EDIT_CONTROLLER, &mut obj);
        if r == OK && !obj.is_null() {
            let r = call!(obj, ControllerVtbl, initialize, host_app());
            if r == OK {
                controller = obj;
            } else {
                plugin.warnings.push(format!("IEditController::initialize failed ({r})"));
            }
        } else {
            plugin.warnings.push(format!("createInstance(IEditController) failed ({r})"));
        }
    }
    if !single && !controller.is_null() {
        // Plugins that share state between the two halves (most do) do it over
        // this connection, after both are initialised.
        let a = query(comp, &IID_CONNECTION_POINT);
        let b = query(controller, &IID_CONNECTION_POINT);
        if !a.is_null() && !b.is_null() {
            let _ = call!(a, ConnectionVtbl, connect, b);
            let _ = call!(b, ConnectionVtbl, connect, a);
        }
    }
    if let Some(ids) = plugin.ids.vst3.as_mut() {
        ids.single_component = single;
        if has_cid {
            ids.controller_class_id = Some(hex16(&cid));
        }
    }
    if controller.is_null() {
        plugin.warnings.push("no edit controller: parameters not read".into());
        return Ok(());
    }
    let _ = call!(controller, ControllerVtbl, set_component_handler, host_handler());
    let n = call!(controller, ControllerVtbl, get_parameter_count);
    for i in 0..n.clamp(0, 100_000) {
        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
        if call!(controller, ControllerVtbl, get_parameter_info, i, &mut info) != OK {
            plugin.warnings.push(format!("parameter {i}: getParameterInfo failed"));
            continue;
        }
        plugin.params.push(read_param(controller, &info));
    }
    Ok(())
}

fn read_param(controller: *mut c_void, info: &ParameterInfo) -> Param {
    let to_text = |normalized: f64| -> String {
        let mut buf = [0u16; 128];
        if call!(controller, ControllerVtbl, get_param_string_by_value, info.id, normalized, buf.as_mut_ptr()) == OK {
            c16(&buf)
        } else {
            String::new()
        }
    };
    let plain = |normalized: f64| call!(controller, ControllerVtbl, normalized_param_to_plain, info.id, normalized);
    let step_count = info.step_count.max(0) as u32;
    let samples: Vec<Sample> = SAMPLE_POINTS.iter().map(|&t| Sample { normalized: t, plain: plain(t), text: to_text(t) }).collect();
    let step_texts = if (1..MAX_STEP_TEXTS).contains(&step_count) {
        (0..=step_count).map(|k| to_text(f64::from(k) / f64::from(step_count))).collect()
    } else {
        Vec::new()
    };
    let mut flags = Vec::new();
    for (bit, name) in [
        (1, "automatable"),
        (2, "read_only"),
        (4, "wrap_around"),
        (8, "list"),
        (16, "hidden"),
        (1 << 15, "program_change"),
        (1 << 16, "bypass"),
    ] {
        if info.flags & bit != 0 {
            flags.push(name.to_string());
        }
    }
    if step_count > 0 {
        flags.push("stepped".into());
    }
    Param {
        id: info.id,
        name: c16(&info.title),
        short_name: Some(c16(&info.short_title)).filter(|s| !s.is_empty()),
        group: (info.unit_id != 0).then(|| info.unit_id.to_string()),
        units: c16(&info.units),
        min: samples[0].plain,
        max: samples[SAMPLE_POINTS.len() - 1].plain,
        default: plain(info.default_normalized),
        default_normalized: info.default_normalized,
        step_count,
        flags,
        flags_raw: info.flags as u32,
        samples,
        step_texts,
    }
}

mod capture;
pub use capture::capture;
mod render;
pub use render::render;
