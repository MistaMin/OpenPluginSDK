//! CLAP state capture (a child module of the probe, which owns the host stubs).
//!
//! The default state is `clap.state` `save` right after activation, written
//! to a stream the host owns, byte for byte as a host stores it. Factory
//! presets come from the preset-discovery factory (`clap.preset-discovery-factory/2`
//! or its draft name) when the bundle has one: the host declares itself as an
//! indexer, the provider declares file types and locations, the host walks
//! the file locations and asks the provider for each file's presets, then
//! loads each preset of this plugin through the plugin's `clap.preset-load/2`
//! and saves the state it ends up in. The preset-discovery and preset-load
//! structures are declared here by hand from the CLAP headers (MIT).

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};

use super::*;
use crate::capture::{Job, MAX_BLOB, ParamValue, Sink, resolve_sets};

const PD_FACTORY_IDS: [&[u8]; 2] = [b"clap.preset-discovery-factory/2\0", b"clap.preset-discovery-factory/draft-2\0"];
const PRESET_LOAD_IDS: [&[u8]; 2] = [b"clap.preset-load/2\0", b"clap.preset-load.draft/2\0"];
const LOCATION_FILE: u32 = 0;
const LOCATION_PLUGIN: u32 = 1;
const LOCATION_IS_FACTORY: u32 = 1;

// ---------- the CLAP structures this module needs and the shared ABI lacks ----------

#[repr(C)]
struct PdDescriptor {
    clap_version: clap_version,
    id: *const c_char,
    name: *const c_char,
    vendor: *const c_char,
}

#[repr(C)]
struct PdFiletype {
    name: *const c_char,
    description: *const c_char,
    file_extension: *const c_char,
}

#[repr(C)]
struct PdLocation {
    flags: u32,
    name: *const c_char,
    kind: u32,
    location: *const c_char,
}

#[repr(C)]
struct PdUniversalId {
    abi: *const c_char,
    id: *const c_char,
}

#[repr(C)]
struct PdIndexer {
    clap_version: clap_version,
    name: *const c_char,
    vendor: *const c_char,
    url: *const c_char,
    version: *const c_char,
    indexer_data: *mut c_void,
    declare_filetype: Option<unsafe extern "C" fn(*const PdIndexer, *const PdFiletype) -> bool>,
    declare_location: Option<unsafe extern "C" fn(*const PdIndexer, *const PdLocation) -> bool>,
    declare_soundpack: Option<unsafe extern "C" fn(*const PdIndexer, *const c_void) -> bool>,
    get_extension: Option<unsafe extern "C" fn(*const PdIndexer, *const c_char) -> *const c_void>,
}

#[repr(C)]
struct PdReceiver {
    receiver_data: *mut c_void,
    on_error: Option<unsafe extern "C" fn(*const PdReceiver, i32, *const c_char)>,
    begin_preset: Option<unsafe extern "C" fn(*const PdReceiver, *const c_char, *const c_char) -> bool>,
    add_plugin_id: Option<unsafe extern "C" fn(*const PdReceiver, *const PdUniversalId)>,
    set_soundpack_id: Option<unsafe extern "C" fn(*const PdReceiver, *const c_char)>,
    set_flags: Option<unsafe extern "C" fn(*const PdReceiver, u32)>,
    add_creator: Option<unsafe extern "C" fn(*const PdReceiver, *const c_char)>,
    set_description: Option<unsafe extern "C" fn(*const PdReceiver, *const c_char)>,
    set_timestamps: Option<unsafe extern "C" fn(*const PdReceiver, u64, u64)>,
    add_feature: Option<unsafe extern "C" fn(*const PdReceiver, *const c_char)>,
    add_extra_info: Option<unsafe extern "C" fn(*const PdReceiver, *const c_char, *const c_char)>,
}

#[repr(C)]
struct PdProvider {
    desc: *const PdDescriptor,
    provider_data: *mut c_void,
    init: Option<unsafe extern "C" fn(*const PdProvider) -> bool>,
    destroy: Option<unsafe extern "C" fn(*const PdProvider)>,
    get_metadata: Option<unsafe extern "C" fn(*const PdProvider, u32, *const c_char, *const PdReceiver) -> bool>,
    get_extension: Option<unsafe extern "C" fn(*const PdProvider, *const c_char) -> *const c_void>,
}

#[repr(C)]
struct PdFactory {
    count: Option<unsafe extern "C" fn(*const PdFactory) -> u32>,
    get_descriptor: Option<unsafe extern "C" fn(*const PdFactory, u32) -> *const PdDescriptor>,
    create: Option<unsafe extern "C" fn(*const PdFactory, *const PdIndexer, *const c_char) -> *const PdProvider>,
}

#[repr(C)]
struct PluginPresetLoad {
    from_location: Option<unsafe extern "C" fn(*const clap_plugin, u32, *const c_char, *const c_char) -> bool>,
}

#[repr(C)]
struct HostPresetLoad {
    on_error: Option<unsafe extern "C" fn(*const clap_host, u32, *const c_char, *const c_char, i32, *const c_char)>,
    loaded: Option<unsafe extern "C" fn(*const clap_host, u32, *const c_char, *const c_char)>,
}

#[repr(C)]
struct HostParams {
    rescan: Option<unsafe extern "C" fn(*const clap_host, u32)>,
    clear: Option<unsafe extern "C" fn(*const clap_host, u32, u32)>,
    request_flush: Option<unsafe extern "C" fn(*const clap_host)>,
}

#[repr(C)]
struct HostState {
    mark_dirty: Option<unsafe extern "C" fn(*const clap_host)>,
}

unsafe extern "C" fn hp_rescan(_: *const clap_host, _: u32) {}
unsafe extern "C" fn hp_clear(_: *const clap_host, _: u32, _: u32) {}
unsafe extern "C" fn pl_error(_: *const clap_host, _: u32, _: *const c_char, _: *const c_char, _: i32, _: *const c_char) {}
unsafe extern "C" fn pl_loaded(_: *const clap_host, _: u32, _: *const c_char, _: *const c_char) {}

static HOST_PARAMS: HostParams = HostParams { rescan: Some(hp_rescan), clear: Some(hp_clear), request_flush: Some(host_noop) };
static HOST_STATE: HostState = HostState { mark_dirty: Some(host_noop) };
static HOST_PRESET_LOAD: HostPresetLoad = HostPresetLoad { on_error: Some(pl_error), loaded: Some(pl_loaded) };
unsafe impl Sync for HostParams {}
unsafe impl Sync for HostState {}
unsafe impl Sync for HostPresetLoad {}

unsafe extern "C" fn cap_get_extension(_host: *const clap_host, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    match unsafe { CStr::from_ptr(id) }.to_bytes() {
        b"clap.params" => (&HOST_PARAMS as *const HostParams).cast(),
        b"clap.state" => (&HOST_STATE as *const HostState).cast(),
        b"clap.preset-load/2" | b"clap.preset-load.draft/2" => (&HOST_PRESET_LOAD as *const HostPresetLoad).cast(),
        _ => std::ptr::null(),
    }
}

// ---------- streams ----------

struct Out {
    bytes: Vec<u8>,
    overflow: bool,
}

unsafe extern "C" fn out_write(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
    let o = unsafe { &mut *((*stream).ctx as *mut Out) };
    if size == 0 {
        return 0;
    }
    if buffer.is_null() || o.bytes.len() as u64 + size > MAX_BLOB as u64 {
        o.overflow = true;
        return -1;
    }
    o.bytes.extend_from_slice(unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), size as usize) });
    size as i64
}

struct Events {
    events: Vec<clap_event_param_value>,
}

unsafe extern "C" fn ev_size(list: *const clap_input_events) -> u32 {
    unsafe { &*((*list).ctx as *const Events) }.events.len() as u32
}
unsafe extern "C" fn ev_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let l = unsafe { &*((*list).ctx as *const Events) };
    l.events.get(index as usize).map_or(std::ptr::null(), |e| &e.header as *const clap_event_header)
}
unsafe extern "C" fn ev_push(_: *const clap_output_events, _: *const clap_event_header) -> bool {
    true
}

// ---------- one plugin instance ----------

struct Instance {
    p: *const clap_plugin,
    params: *const clap_plugin_params,
    state: *const clap_plugin_state,
    /// (id, name) in the plugin's order.
    table: Vec<(u32, String)>,
}

fn make(factory: &clap_plugin_factory, host: *const clap_host, id: *const c_char, rate: f64) -> Result<Instance, String> {
    let created = unsafe { factory.create_plugin.ok_or("factory without create_plugin")?(factory, host, id) };
    if created.is_null() {
        return Err("the factory would not create the plugin".into());
    }
    let p = unsafe { &*created };
    if !unsafe { p.init.ok_or("plugin.init is missing")?(created) } {
        return Err("plugin.init returned false".into());
    }
    if let Some(activate) = p.activate {
        if !unsafe { activate(created, rate, 1, 4096) } {
            return Err("plugin.activate failed".into());
        }
    }
    let ext = |name: &[u8]| -> *const c_void { p.get_extension.map_or(std::ptr::null(), |f| unsafe { f(created, name.as_ptr().cast()) }) };
    let params = ext(EXT_PARAMS) as *const clap_plugin_params;
    let state = ext(EXT_STATE) as *const clap_plugin_state;
    let mut table = Vec::new();
    if !params.is_null() {
        let pr = unsafe { &*params };
        if let (Some(count), Some(get_info)) = (pr.count, pr.get_info) {
            for i in 0..unsafe { count(created) } {
                let mut info: clap_param_info = unsafe { std::mem::zeroed() };
                if unsafe { get_info(created, i, &mut info) } {
                    table.push((info.id, fixed(&info.name)));
                }
            }
        }
    }
    Ok(Instance { p: created, params, state, table })
}

impl Instance {
    fn save(&self) -> Result<Vec<u8>, String> {
        if self.state.is_null() {
            return Err("the plugin has no clap.state extension".into());
        }
        let save = unsafe { (*self.state).save }.ok_or("clap.state has no save")?;
        let mut out = Out { bytes: Vec::new(), overflow: false };
        let stream = clap_ostream { ctx: (&mut out as *mut Out).cast(), write: Some(out_write) };
        if !unsafe { save(self.p, &stream) } {
            return Err(if out.overflow { "the state is larger than the blob limit".into() } else { "clap.state save returned false".into() });
        }
        Ok(out.bytes)
    }

    fn values(&self) -> Vec<ParamValue> {
        let mut out = Vec::new();
        if self.params.is_null() {
            return out;
        }
        let Some(get) = (unsafe { (*self.params).get_value }) else { return out };
        for (id, name) in &self.table {
            let mut v = 0f64;
            if unsafe { get(self.p, *id, &mut v) } {
                out.push(ParamValue { id: *id, name: name.clone(), plain: v, normalized: None });
            }
        }
        out
    }

    fn set(&self, values: &[(u32, f64)]) -> Result<(), String> {
        if self.params.is_null() {
            return Err("the plugin has no clap.params extension".into());
        }
        let flush = unsafe { (*self.params).flush }.ok_or("clap.params has no flush")?;
        let events = Events {
            events: values
                .iter()
                .map(|(id, value)| clap_event_param_value {
                    header: clap_event_header { size: std::mem::size_of::<clap_event_param_value>() as u32, time: 0, space_id: 0, type_: EVENT_PARAM_VALUE, flags: 0 },
                    param_id: *id,
                    cookie: std::ptr::null_mut(),
                    note_id: -1,
                    port_index: -1,
                    channel: -1,
                    key: -1,
                    value: *value,
                })
                .collect(),
        };
        let input = clap_input_events { ctx: (&events as *const Events).cast_mut().cast(), size: Some(ev_size), get: Some(ev_get) };
        let output = clap_output_events { ctx: std::ptr::null_mut(), try_push: Some(ev_push) };
        unsafe { flush(self.p, &input, &output) };
        Ok(())
    }
}

// ---------- preset discovery ----------

#[derive(Default)]
struct IndexerData {
    extensions: Vec<String>,
    locations: Vec<(u32, u32, Option<String>)>,
}

unsafe extern "C" fn ix_filetype(ix: *const PdIndexer, f: *const PdFiletype) -> bool {
    let d = unsafe { &mut *((*ix).indexer_data as *mut IndexerData) };
    d.extensions.push(text(unsafe { (*f).file_extension }));
    true
}
unsafe extern "C" fn ix_location(ix: *const PdIndexer, l: *const PdLocation) -> bool {
    let d = unsafe { &mut *((*ix).indexer_data as *mut IndexerData) };
    let l = unsafe { &*l };
    d.locations.push((l.flags, l.kind, if l.location.is_null() { None } else { Some(text(l.location)) }));
    true
}
unsafe extern "C" fn ix_soundpack(_: *const PdIndexer, _: *const c_void) -> bool {
    true
}
unsafe extern "C" fn ix_extension(_: *const PdIndexer, _: *const c_char) -> *const c_void {
    std::ptr::null()
}

#[derive(Default, Clone)]
struct Entry {
    name: String,
    load_key: Option<String>,
    plugin_ids: Vec<(String, String)>,
}

#[derive(Default)]
struct ReceiverData {
    entries: Vec<Entry>,
    errors: Vec<String>,
}

fn rd(r: *const PdReceiver) -> &'static mut ReceiverData {
    unsafe { &mut *((*r).receiver_data as *mut ReceiverData) }
}
unsafe extern "C" fn rc_error(r: *const PdReceiver, _os: i32, msg: *const c_char) {
    rd(r).errors.push(text(msg));
}
unsafe extern "C" fn rc_begin(r: *const PdReceiver, name: *const c_char, key: *const c_char) -> bool {
    let key = if key.is_null() { None } else { Some(text(key)) };
    rd(r).entries.push(Entry { name: text(name), load_key: key, plugin_ids: Vec::new() });
    true
}
unsafe extern "C" fn rc_plugin_id(r: *const PdReceiver, id: *const PdUniversalId) {
    if let Some(e) = rd(r).entries.last_mut() {
        e.plugin_ids.push((text(unsafe { (*id).abi }), text(unsafe { (*id).id })));
    }
}
unsafe extern "C" fn rc_str(_: *const PdReceiver, _: *const c_char) {}
unsafe extern "C" fn rc_flags(_: *const PdReceiver, _: u32) {}
unsafe extern "C" fn rc_times(_: *const PdReceiver, _: u64, _: u64) {}
unsafe extern "C" fn rc_extra(_: *const PdReceiver, _: *const c_char, _: *const c_char) {}

/// Files under `root` whose extension is in `extensions` (all files when the list holds an empty one).
fn walk(root: &Path, extensions: &[String], limit: usize) -> Vec<PathBuf> {
    let wants = |p: &Path| {
        extensions.is_empty()
            || extensions.iter().any(|e| e.is_empty() || p.extension().is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case(e.trim_start_matches('.'))))
    };
    let mut out = Vec::new();
    let mut todo = vec![(root.to_path_buf(), 0usize)];
    while let Some((p, depth)) = todo.pop() {
        if out.len() >= limit {
            break;
        }
        if p.is_dir() {
            if depth < 8 {
                if let Ok(rd) = std::fs::read_dir(&p) {
                    let mut kids: Vec<_> = rd.flatten().map(|e| e.path()).collect();
                    kids.sort();
                    kids.reverse();
                    todo.extend(kids.into_iter().map(|k| (k, depth + 1)));
                }
            }
        } else if p.is_file() && wants(&p) {
            out.push(p);
        }
    }
    out
}

struct Found {
    entry: Entry,
    kind: u32,
    location: Option<String>,
    flags: u32,
}

/// Every preset the bundle's preset-discovery providers list, with where to load it from.
fn discover(entry: &clap_plugin_entry, notes: &mut Vec<String>) -> Vec<Found> {
    let Some(get_factory) = entry.get_factory else { return Vec::new() };
    let mut factory: *const PdFactory = std::ptr::null();
    for id in PD_FACTORY_IDS {
        factory = unsafe { get_factory(id.as_ptr().cast()) } as *const PdFactory;
        if !factory.is_null() {
            break;
        }
    }
    if factory.is_null() {
        notes.push("the bundle has no preset-discovery factory".into());
        return Vec::new();
    }
    let f = unsafe { &*factory };
    let (Some(count), Some(get_desc), Some(create)) = (f.count, f.get_descriptor, f.create) else {
        notes.push("the preset-discovery factory is incomplete".into());
        return Vec::new();
    };
    let mut found = Vec::new();
    for i in 0..unsafe { count(factory) } {
        let desc = unsafe { get_desc(factory, i) };
        if desc.is_null() {
            continue;
        }
        let data = Box::leak(Box::new(IndexerData::default()));
        let indexer = Box::leak(Box::new(PdIndexer {
            clap_version: CLAP_VERSION,
            name: HOST_NAME.as_ptr().cast(),
            vendor: EMPTY.as_ptr().cast(),
            url: EMPTY.as_ptr().cast(),
            version: HOST_VERSION.as_ptr().cast(),
            indexer_data: (data as *mut IndexerData).cast(),
            declare_filetype: Some(ix_filetype),
            declare_location: Some(ix_location),
            declare_soundpack: Some(ix_soundpack),
            get_extension: Some(ix_extension),
        }));
        let provider = unsafe { create(factory, indexer, (*desc).id) };
        if provider.is_null() {
            notes.push(format!("preset provider {} would not start", text(unsafe { (*desc).id })));
            continue;
        }
        let pr = unsafe { &*provider };
        if let Some(init) = pr.init {
            if !unsafe { init(provider) } {
                notes.push(format!("preset provider {} refused to initialise", text(unsafe { (*desc).id })));
                continue;
            }
        }
        let Some(get_metadata) = pr.get_metadata else { continue };
        let receiver_data = Box::leak(Box::new(ReceiverData::default()));
        let receiver = Box::leak(Box::new(PdReceiver {
            receiver_data: (receiver_data as *mut ReceiverData).cast(),
            on_error: Some(rc_error),
            begin_preset: Some(rc_begin),
            add_plugin_id: Some(rc_plugin_id),
            set_soundpack_id: Some(rc_str),
            set_flags: Some(rc_flags),
            add_creator: Some(rc_str),
            set_description: Some(rc_str),
            set_timestamps: Some(rc_times),
            add_feature: Some(rc_str),
            add_extra_info: Some(rc_extra),
        }));
        for (flags, kind, location) in data.locations.clone() {
            let targets: Vec<Option<String>> = match (kind, &location) {
                (LOCATION_FILE, Some(root)) => walk(Path::new(root), &data.extensions, 20_000).into_iter().map(|p| Some(p.to_string_lossy().into_owned())).collect(),
                (LOCATION_PLUGIN, _) => vec![None],
                _ => Vec::new(),
            };
            for target in targets {
                let c = target.as_deref().map(|t| CString::new(t).unwrap_or_default());
                receiver_data.entries.clear();
                unsafe { get_metadata(provider, kind, c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()), receiver) };
                for e in receiver_data.entries.drain(..) {
                    found.push(Found { entry: e, kind, location: target.clone(), flags });
                }
            }
        }
        for e in receiver_data.errors.drain(..).take(5) {
            notes.push(format!("preset provider: {e}"));
        }
    }
    found
}

/// Capture child: load the plugin, save its default state, `--set` state and factory presets
/// into `sink`. Runs inside the `__capture` child only, since the plugin's code runs here.
/// `Err` with a message when the plugin cannot be loaded or its state cannot be read.
pub fn capture(job: &Job, sink: &mut Sink) -> Result<(), String> {
    let exe = crate::bundle::executable(&job.path)?;
    let lib = sys::open_library(&exe)?;
    let entry_ptr = sys::symbol(lib, "clap_entry") as *const clap_plugin_entry;
    if entry_ptr.is_null() {
        return Err("no clap_entry symbol: not a CLAP plugin".into());
    }
    let entry = unsafe { &*entry_ptr };
    let path = CString::new(job.path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    if !unsafe { entry.init.ok_or("clap_entry.init is missing")?(path.as_ptr()) } {
        return Err("clap_entry.init returned false".into());
    }
    let factory = unsafe { entry.get_factory.ok_or("clap_entry.get_factory is missing")?(FACTORY_ID.as_ptr().cast()) } as *const clap_plugin_factory;
    if factory.is_null() {
        return Err("no plugin factory".into());
    }
    let factory = unsafe { &*factory };
    let desc = unsafe { factory.get_plugin_descriptor.ok_or("factory without get_plugin_descriptor")?(factory, job.index) };
    if desc.is_null() {
        return Err(format!("no plugin number {}", job.index));
    }
    let plugin_id = unsafe { (*desc).id };
    let host = Box::leak(Box::new(clap_host {
        clap_version: CLAP_VERSION,
        host_data: std::ptr::null_mut(),
        name: HOST_NAME.as_ptr().cast(),
        vendor: EMPTY.as_ptr().cast(),
        url: EMPTY.as_ptr().cast(),
        version: HOST_VERSION.as_ptr().cast(),
        get_extension: Some(cap_get_extension),
        request_restart: Some(host_noop),
        request_process: Some(host_noop),
        request_callback: Some(host_noop),
    }));

    // The default state.
    let first = make(factory, host, plugin_id, job.rate)?;
    match first.save() {
        Ok(bytes) => sink.add("default", "default", None, vec![("clap-state", bytes)], first.values(), Vec::new())?,
        Err(e) => sink.notes.push(format!("default state: {e}")),
    }

    // The state after `--set`, on a fresh instance.
    if !job.sets.is_empty() {
        let inst = make(factory, host, plugin_id, job.rate)?;
        let values = resolve_sets(&job.sets, &inst.table)?;
        inst.set(&values)?;
        match inst.save() {
            Ok(bytes) => sink.add("set", "set", None, vec![("clap-state", bytes)], inst.values(), Vec::new())?,
            Err(e) => sink.notes.push(format!("state after --set: {e}")),
        }
    }

    // Factory presets, through preset discovery and the plugin's preset-load.
    let mut notes = Vec::new();
    let found = discover(entry, &mut notes);
    sink.notes.append(&mut notes);
    let me = text(plugin_id);
    let mine: Vec<&Found> = found.iter().filter(|f| f.entry.plugin_ids.is_empty() || f.entry.plugin_ids.iter().any(|(abi, id)| abi == "clap" && *id == me)).collect();
    if mine.is_empty() {
        if !found.is_empty() {
            sink.notes.push(format!("the provider lists {} presets, none for this plugin", found.len()));
        }
        return Ok(());
    }
    let inst = make(factory, host, plugin_id, job.rate)?;
    let mut load: *const PluginPresetLoad = std::ptr::null();
    if let Some(get_ext) = unsafe { (*inst.p).get_extension } {
        for id in PRESET_LOAD_IDS {
            load = unsafe { get_ext(inst.p, id.as_ptr().cast()) } as *const PluginPresetLoad;
            if !load.is_null() {
                break;
            }
        }
    }
    let from_location = if load.is_null() { None } else { unsafe { (*load).from_location } };
    let Some(from_location) = from_location else {
        sink.notes.push(format!("{} presets found but the plugin has no clap.preset-load extension", mine.len()));
        return Ok(());
    };
    if mine.len() > job.max_presets {
        sink.notes.push(format!("{} presets found; the first {} captured (--max-presets)", mine.len(), job.max_presets));
    }
    for f in mine.into_iter().take(job.max_presets) {
        let loc = f.location.as_deref().map(|l| CString::new(l).unwrap_or_default());
        let key = f.entry.load_key.as_deref().map(|k| CString::new(k).unwrap_or_default());
        let ok = unsafe { from_location(inst.p, f.kind, loc.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()), key.as_ref().map_or(std::ptr::null(), |c| c.as_ptr())) };
        let label = match (&f.location, &f.entry.load_key) {
            (Some(l), Some(k)) => Some(format!("{l}#{k}")),
            (Some(l), None) => Some(l.clone()),
            (None, Some(k)) => Some(format!("plugin#{k}")),
            (None, None) => None,
        };
        if !ok {
            sink.notes.push(format!("preset {:?} would not load", f.entry.name));
            continue;
        }
        let kind = if f.flags & LOCATION_IS_FACTORY != 0 { "factory-preset" } else { "preset" };
        match inst.save() {
            Ok(bytes) => sink.add(&f.entry.name, kind, label, vec![("clap-state", bytes)], inst.values(), Vec::new())?,
            Err(e) => sink.notes.push(format!("preset {:?}: {e}", f.entry.name)),
        }
    }
    Ok(())
}
