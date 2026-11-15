// A CLAP plugin the state-import test builds with rustc (`--crate-type cdylib`): one parameter, a state
// stream (the parameter's value as 8 little-endian bytes), and a preset-discovery provider.
//
// The provider declares two locations: the plugin itself (three presets, one of them for another plugin id)
// and, when OPENPLUGIN_PORT_FIXTURE_PRESETS names a folder, that folder as a file location for `.pre` files
// (each file holds a number; the preset is named after the file). Both are loaded through `clap.preset-load/2`.
// The declarations follow the CLAP headers; they are written out again here on purpose, so the test checks
// the host side in the crate against a second reading of them.
#![allow(non_camel_case_types, clippy::missing_safety_doc)]

use std::ffi::{CStr, CString, c_char, c_void};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ver {
    major: u32,
    minor: u32,
    revision: u32,
}
const V: Ver = Ver { major: 1, minor: 2, revision: 0 };

#[repr(C)]
struct Desc {
    v: Ver,
    id: *const c_char,
    name: *const c_char,
    vendor: *const c_char,
    url: *const c_char,
    manual: *const c_char,
    support: *const c_char,
    version: *const c_char,
    description: *const c_char,
    features: *const *const c_char,
}
unsafe impl Sync for Desc {}

#[repr(C)]
struct Host {
    v: Ver,
    data: *mut c_void,
    name: *const c_char,
    vendor: *const c_char,
    url: *const c_char,
    version: *const c_char,
    get_extension: Option<unsafe extern "C" fn(*const Host, *const c_char) -> *const c_void>,
    a: Option<unsafe extern "C" fn(*const Host)>,
    b: Option<unsafe extern "C" fn(*const Host)>,
    c: Option<unsafe extern "C" fn(*const Host)>,
}

#[repr(C)]
struct Plugin {
    desc: *const Desc,
    data: *mut c_void,
    init: Option<unsafe extern "C" fn(*const Plugin) -> bool>,
    destroy: Option<unsafe extern "C" fn(*const Plugin)>,
    activate: Option<unsafe extern "C" fn(*const Plugin, f64, u32, u32) -> bool>,
    deactivate: Option<unsafe extern "C" fn(*const Plugin)>,
    start: Option<unsafe extern "C" fn(*const Plugin) -> bool>,
    stop: Option<unsafe extern "C" fn(*const Plugin)>,
    reset: Option<unsafe extern "C" fn(*const Plugin)>,
    process: Option<unsafe extern "C" fn(*const Plugin, *const c_void) -> i32>,
    get_extension: Option<unsafe extern "C" fn(*const Plugin, *const c_char) -> *const c_void>,
    on_main_thread: Option<unsafe extern "C" fn(*const Plugin)>,
}

#[repr(C)]
struct ParamInfo {
    id: u32,
    flags: u32,
    cookie: *mut c_void,
    name: [c_char; 256],
    module: [c_char; 1024],
    min: f64,
    max: f64,
    default: f64,
}
#[repr(C)]
struct Params {
    count: unsafe extern "C" fn(*const Plugin) -> u32,
    get_info: unsafe extern "C" fn(*const Plugin, u32, *mut ParamInfo) -> bool,
    get_value: unsafe extern "C" fn(*const Plugin, u32, *mut f64) -> bool,
    value_to_text: Option<unsafe extern "C" fn(*const Plugin, u32, f64, *mut c_char, u32) -> bool>,
    text_to_value: Option<unsafe extern "C" fn(*const Plugin, u32, *const c_char, *mut f64) -> bool>,
    flush: Option<unsafe extern "C" fn(*const Plugin, *const c_void, *const c_void)>,
}
unsafe impl Sync for Params {}

#[repr(C)]
struct OStream {
    ctx: *mut c_void,
    write: unsafe extern "C" fn(*const OStream, *const c_void, u64) -> i64,
}
#[repr(C)]
struct IStream {
    ctx: *mut c_void,
    read: unsafe extern "C" fn(*const IStream, *mut c_void, u64) -> i64,
}
#[repr(C)]
struct State {
    save: unsafe extern "C" fn(*const Plugin, *const OStream) -> bool,
    load: unsafe extern "C" fn(*const Plugin, *const IStream) -> bool,
}
unsafe impl Sync for State {}
#[repr(C)]
struct PresetLoad {
    from_location: unsafe extern "C" fn(*const Plugin, u32, *const c_char, *const c_char) -> bool,
}
unsafe impl Sync for PresetLoad {}

#[repr(C)]
pub struct Entry {
    v: Ver,
    init: unsafe extern "C" fn(*const c_char) -> bool,
    deinit: unsafe extern "C" fn(),
    get_factory: unsafe extern "C" fn(*const c_char) -> *const c_void,
}
unsafe impl Sync for Entry {}
#[repr(C)]
struct Factory {
    count: unsafe extern "C" fn(*const Factory) -> u32,
    get: unsafe extern "C" fn(*const Factory, u32) -> *const Desc,
    create: unsafe extern "C" fn(*const Factory, *const Host, *const c_char) -> *const Plugin,
}
unsafe impl Sync for Factory {}

// ---- preset discovery ----
#[repr(C)]
struct PdDesc {
    v: Ver,
    id: *const c_char,
    name: *const c_char,
    vendor: *const c_char,
}
unsafe impl Sync for PdDesc {}
#[repr(C)]
struct Filetype {
    name: *const c_char,
    description: *const c_char,
    extension: *const c_char,
}
#[repr(C)]
struct Location {
    flags: u32,
    name: *const c_char,
    kind: u32,
    location: *const c_char,
}
#[repr(C)]
struct Indexer {
    v: Ver,
    name: *const c_char,
    vendor: *const c_char,
    url: *const c_char,
    version: *const c_char,
    data: *mut c_void,
    declare_filetype: unsafe extern "C" fn(*const Indexer, *const Filetype) -> bool,
    declare_location: unsafe extern "C" fn(*const Indexer, *const Location) -> bool,
    declare_soundpack: *const c_void,
    get_extension: *const c_void,
}
#[repr(C)]
struct UniversalId {
    abi: *const c_char,
    id: *const c_char,
}
#[repr(C)]
struct Receiver {
    data: *mut c_void,
    on_error: *const c_void,
    begin_preset: unsafe extern "C" fn(*const Receiver, *const c_char, *const c_char) -> bool,
    add_plugin_id: unsafe extern "C" fn(*const Receiver, *const UniversalId),
    set_soundpack_id: *const c_void,
    set_flags: *const c_void,
    add_creator: *const c_void,
    set_description: *const c_void,
    set_timestamps: *const c_void,
    add_feature: *const c_void,
    add_extra_info: *const c_void,
}
#[repr(C)]
struct Provider {
    desc: *const PdDesc,
    data: *mut c_void,
    init: unsafe extern "C" fn(*const Provider) -> bool,
    destroy: unsafe extern "C" fn(*const Provider),
    get_metadata: unsafe extern "C" fn(*const Provider, u32, *const c_char, *const Receiver) -> bool,
    get_extension: unsafe extern "C" fn(*const Provider, *const c_char) -> *const c_void,
}
#[repr(C)]
struct PdFactory {
    count: unsafe extern "C" fn(*const PdFactory) -> u32,
    get: unsafe extern "C" fn(*const PdFactory, u32) -> *const PdDesc,
    create: unsafe extern "C" fn(*const PdFactory, *const Indexer, *const c_char) -> *const Provider,
}
unsafe impl Sync for PdFactory {}

const ID: &CStr = c"com.example.preset-fixture";
const OTHER: &CStr = c"com.example.somebody-else";
static DESC: Desc = Desc {
    v: V,
    id: ID.as_ptr(),
    name: c"Preset Fixture".as_ptr(),
    vendor: c"Example".as_ptr(),
    url: c"".as_ptr(),
    manual: c"".as_ptr(),
    support: c"".as_ptr(),
    version: c"1.0".as_ptr(),
    description: c"".as_ptr(),
    features: FEATURES.0.as_ptr(),
};
// A raw-pointer array is not Sync by itself.
struct Features([*const c_char; 1]);
unsafe impl Sync for Features {}
static FEATURES: Features = Features([std::ptr::null()]);

const PRESETS: [(&CStr, f64, &CStr); 3] = [(c"Warm", 0.25, ID), (c"Bright", 0.75, ID), (c"Not Mine", 0.5, OTHER)];

unsafe extern "C" fn p_init(_: *const Plugin) -> bool {
    true
}
unsafe extern "C" fn p_destroy(_: *const Plugin) {}
unsafe extern "C" fn p_activate(_: *const Plugin, _: f64, _: u32, _: u32) -> bool {
    true
}
unsafe extern "C" fn p_noop(_: *const Plugin) {}
unsafe extern "C" fn p_start(_: *const Plugin) -> bool {
    true
}
unsafe extern "C" fn p_process(_: *const Plugin, _: *const c_void) -> i32 {
    1
}

fn value(p: *const Plugin) -> &'static mut f64 {
    unsafe { &mut *((*p).data as *mut f64) }
}

unsafe extern "C" fn pr_count(_: *const Plugin) -> u32 {
    1
}
unsafe extern "C" fn pr_info(_: *const Plugin, i: u32, info: *mut ParamInfo) -> bool {
    if i != 0 {
        return false;
    }
    let info = unsafe { &mut *info };
    info.id = 7;
    info.flags = 1;
    info.cookie = std::ptr::null_mut();
    for (d, s) in info.name.iter_mut().zip(b"Level\0") {
        *d = *s as c_char;
    }
    info.module[0] = 0;
    info.min = 0.0;
    info.max = 1.0;
    info.default = 0.5;
    true
}
unsafe extern "C" fn pr_value(p: *const Plugin, id: u32, out: *mut f64) -> bool {
    if id != 7 {
        return false;
    }
    unsafe { *out = *value(p) };
    true
}
static PARAMS: Params = Params { count: pr_count, get_info: pr_info, get_value: pr_value, value_to_text: None, text_to_value: None, flush: None };

unsafe extern "C" fn st_save(p: *const Plugin, s: *const OStream) -> bool {
    let bytes = value(p).to_le_bytes();
    unsafe { ((*s).write)(s, bytes.as_ptr().cast(), 8) == 8 }
}
unsafe extern "C" fn st_load(p: *const Plugin, s: *const IStream) -> bool {
    let mut bytes = [0u8; 8];
    if unsafe { ((*s).read)(s, bytes.as_mut_ptr().cast(), 8) } != 8 {
        return false;
    }
    *value(p) = f64::from_le_bytes(bytes);
    true
}
static STATE: State = State { save: st_save, load: st_load };

unsafe extern "C" fn pl_from(p: *const Plugin, kind: u32, location: *const c_char, key: *const c_char) -> bool {
    match kind {
        1 => {
            let k = unsafe { CStr::from_ptr(key) }.to_str().unwrap_or("");
            match k.parse::<usize>().ok().and_then(|i| PRESETS.get(i)) {
                Some((_, v, _)) => {
                    *value(p) = *v;
                    true
                }
                None => false,
            }
        }
        0 => {
            let path = unsafe { CStr::from_ptr(location) }.to_string_lossy().into_owned();
            match std::fs::read_to_string(path).ok().and_then(|t| t.trim().parse::<f64>().ok()) {
                Some(v) => {
                    *value(p) = v;
                    true
                }
                None => false,
            }
        }
        _ => false,
    }
}
static PRESET_LOAD: PresetLoad = PresetLoad { from_location: pl_from };

unsafe extern "C" fn p_ext(_: *const Plugin, id: *const c_char) -> *const c_void {
    match unsafe { CStr::from_ptr(id) }.to_bytes() {
        b"clap.params" => (&PARAMS as *const Params).cast(),
        b"clap.state" => (&STATE as *const State).cast(),
        b"clap.preset-load/2" => (&PRESET_LOAD as *const PresetLoad).cast(),
        _ => std::ptr::null(),
    }
}

unsafe extern "C" fn f_count(_: *const Factory) -> u32 {
    1
}
unsafe extern "C" fn f_get(_: *const Factory, i: u32) -> *const Desc {
    if i == 0 { &DESC } else { std::ptr::null() }
}
unsafe extern "C" fn f_create(_: *const Factory, _: *const Host, _: *const c_char) -> *const Plugin {
    let data = Box::into_raw(Box::new(0.5f64));
    Box::into_raw(Box::new(Plugin {
        desc: &DESC,
        data: data.cast(),
        init: Some(p_init),
        destroy: Some(p_destroy),
        activate: Some(p_activate),
        deactivate: Some(p_noop),
        start: Some(p_start),
        stop: Some(p_noop),
        reset: Some(p_noop),
        process: Some(p_process),
        get_extension: Some(p_ext),
        on_main_thread: Some(p_noop),
    }))
}
static FACTORY: Factory = Factory { count: f_count, get: f_get, create: f_create };

// ---- the provider ----
static PD_DESC: PdDesc = PdDesc { v: V, id: c"com.example.preset-fixture.provider".as_ptr(), name: c"Fixture presets".as_ptr(), vendor: c"Example".as_ptr() };

fn preset_dir() -> Option<CString> {
    std::env::var("OPENPLUGIN_PORT_FIXTURE_PRESETS").ok().and_then(|d| CString::new(d).ok())
}

unsafe extern "C" fn pv_init(p: *const Provider) -> bool {
    let ix = unsafe { (*p).data } as *const Indexer;
    let ix = unsafe { &*ix };
    let ft = Filetype { name: c"Fixture preset".as_ptr(), description: c"".as_ptr(), extension: c"pre".as_ptr() };
    unsafe { (ix.declare_filetype)(ix, &ft) };
    let here = Location { flags: 1, name: c"Built in".as_ptr(), kind: 1, location: std::ptr::null() };
    unsafe { (ix.declare_location)(ix, &here) };
    if let Some(dir) = preset_dir() {
        let user = Location { flags: 2, name: c"Folder".as_ptr(), kind: 0, location: dir.as_ptr() };
        unsafe { (ix.declare_location)(ix, &user) };
    }
    true
}
unsafe extern "C" fn pv_destroy(_: *const Provider) {}
unsafe extern "C" fn pv_meta(_: *const Provider, kind: u32, location: *const c_char, r: *const Receiver) -> bool {
    let r = unsafe { &*r };
    if kind == 1 {
        for (i, (name, _, owner)) in PRESETS.iter().enumerate() {
            let key = CString::new(i.to_string()).unwrap_or_default();
            unsafe { (r.begin_preset)(r, name.as_ptr(), key.as_ptr()) };
            let id = UniversalId { abi: c"clap".as_ptr(), id: owner.as_ptr() };
            unsafe { (r.add_plugin_id)(r, &id) };
        }
        return true;
    }
    let path = unsafe { CStr::from_ptr(location) }.to_string_lossy().into_owned();
    let stem = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = CString::new(stem).unwrap_or_default();
    unsafe { (r.begin_preset)(r, name.as_ptr(), std::ptr::null()) };
    let id = UniversalId { abi: c"clap".as_ptr(), id: ID.as_ptr() };
    unsafe { (r.add_plugin_id)(r, &id) };
    true
}
unsafe extern "C" fn pv_ext(_: *const Provider, _: *const c_char) -> *const c_void {
    std::ptr::null()
}

unsafe extern "C" fn pf_count(_: *const PdFactory) -> u32 {
    1
}
unsafe extern "C" fn pf_get(_: *const PdFactory, i: u32) -> *const PdDesc {
    if i == 0 { &PD_DESC } else { std::ptr::null() }
}
unsafe extern "C" fn pf_create(_: *const PdFactory, ix: *const Indexer, _: *const c_char) -> *const Provider {
    Box::into_raw(Box::new(Provider { desc: &PD_DESC, data: ix as *mut c_void, init: pv_init, destroy: pv_destroy, get_metadata: pv_meta, get_extension: pv_ext }))
}
static PD_FACTORY: PdFactory = PdFactory { count: pf_count, get: pf_get, create: pf_create };

unsafe extern "C" fn e_init(_: *const c_char) -> bool {
    true
}
unsafe extern "C" fn e_deinit() {}
unsafe extern "C" fn e_factory(id: *const c_char) -> *const c_void {
    match unsafe { CStr::from_ptr(id) }.to_bytes() {
        b"clap.plugin-factory" => (&FACTORY as *const Factory).cast(),
        b"clap.preset-discovery-factory/2" => (&PD_FACTORY as *const PdFactory).cast(),
        _ => std::ptr::null(),
    }
}

#[unsafe(no_mangle)]
pub static clap_entry: Entry = Entry { v: V, init: e_init, deinit: e_deinit, get_factory: e_factory };
