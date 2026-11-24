//! VST3 state capture (a child module of the probe, which owns the COM
//! binding and the host stubs).
//!
//! A host stores two streams per plugin: `IComponent::getState` and
//! `IEditController::getState` (the second is often empty or unimplemented).
//! The streams here are `IBStream` objects the host owns; the bytes are what
//! the plugin wrote. Factory presets are the program lists of `IUnitInfo`:
//! each program is selected the way a host does it, through the list's
//! program-change parameter (set on the controller and sent to the component
//! in a process block), and the states are saved afterwards; where
//! `IProgramListData` offers the program's data it is saved too.
//! The `IUnitInfo`, `IProgramListData` and `IBStream` vtables follow the
//! public VST3 interface headers (MIT).

use std::ffi::c_void;

use super::render::process_changes;
use super::*;
use crate::capture::{Job, MAX_BLOB, ParamValue, Sink, resolve_sets};

const IID_BSTREAM: Tuid = iid(0xC3BF_6EA2, 0x3099_4752, 0x9B6B_F990, 0x1EE3_3E9B);
const IID_UNIT_INFO: Tuid = iid(0x3D4B_D6B5, 0x913A_4FD2, 0xA886_E768, 0xA5EB_92C1);
const IID_PROGRAM_LIST_DATA: Tuid = iid(0x8683_B01F, 0x7B35_4F70, 0xA265_1DEC, 0x353A_F4FF);
const PROGRAM_CHANGE: u32 = 1 << 15;

// ---------- IBStream ----------

#[repr(C)]
struct StreamVtbl {
    base: UnknownVtbl,
    read: unsafe extern "system" fn(*mut c_void, *mut c_void, i32, *mut i32) -> TResult,
    write: unsafe extern "system" fn(*mut c_void, *const c_void, i32, *mut i32) -> TResult,
    seek: unsafe extern "system" fn(*mut c_void, i64, i32, *mut i64) -> TResult,
    tell: unsafe extern "system" fn(*mut c_void, *mut i64) -> TResult,
}

#[repr(C)]
struct MemStream {
    vtbl: *const StreamVtbl,
    data: Vec<u8>,
    pos: usize,
    overflow: bool,
}

unsafe extern "system" fn s_query(this: *mut c_void, wanted: *const Tuid, out: *mut *mut c_void) -> TResult {
    let want = unsafe { *wanted };
    let found = if want == IID_FUNKNOWN || want == IID_BSTREAM { this } else { std::ptr::null_mut() };
    unsafe { *out = found };
    if found.is_null() { NO_INTERFACE } else { OK }
}
unsafe extern "system" fn s_ref(_: *mut c_void) -> u32 {
    2
}
unsafe extern "system" fn s_unref(_: *mut c_void) -> u32 {
    1
}
unsafe extern "system" fn s_read(this: *mut c_void, buffer: *mut c_void, n: i32, read: *mut i32) -> TResult {
    let s = unsafe { &mut *(this as *mut MemStream) };
    let want = n.max(0) as usize;
    let have = s.data.len().saturating_sub(s.pos).min(want);
    if have > 0 && !buffer.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(s.data.as_ptr().add(s.pos), buffer.cast::<u8>(), have) };
    }
    s.pos += have;
    if !read.is_null() {
        unsafe { *read = have as i32 };
    }
    OK
}
unsafe extern "system" fn s_write(this: *mut c_void, buffer: *const c_void, n: i32, written: *mut i32) -> TResult {
    let s = unsafe { &mut *(this as *mut MemStream) };
    let n = n.max(0) as usize;
    if s.pos + n > MAX_BLOB {
        s.overflow = true;
        return 1;
    }
    if s.pos + n > s.data.len() {
        s.data.resize(s.pos + n, 0);
    }
    if n > 0 && !buffer.is_null() {
        s.data[s.pos..s.pos + n].copy_from_slice(unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), n) });
    }
    s.pos += n;
    if !written.is_null() {
        unsafe { *written = n as i32 };
    }
    OK
}
unsafe extern "system" fn s_seek(this: *mut c_void, pos: i64, mode: i32, result: *mut i64) -> TResult {
    let s = unsafe { &mut *(this as *mut MemStream) };
    let base = match mode {
        0 => 0i64,
        1 => s.pos as i64,
        2 => s.data.len() as i64,
        _ => return 1,
    };
    let target = base.saturating_add(pos);
    if target < 0 || target > MAX_BLOB as i64 {
        return 1;
    }
    s.pos = target as usize;
    if !result.is_null() {
        unsafe { *result = target };
    }
    OK
}
unsafe extern "system" fn s_tell(this: *mut c_void, pos: *mut i64) -> TResult {
    let s = unsafe { &*(this as *mut MemStream) };
    if !pos.is_null() {
        unsafe { *pos = s.pos as i64 };
    }
    OK
}

static STREAM_VTBL: StreamVtbl = StreamVtbl { base: UnknownVtbl { query_interface: s_query, add_ref: s_ref, release: s_unref }, read: s_read, write: s_write, seek: s_seek, tell: s_tell };

impl MemStream {
    fn new(data: Vec<u8>) -> Self {
        Self { vtbl: &STREAM_VTBL, data, pos: 0, overflow: false }
    }
    fn ptr(&mut self) -> *mut c_void {
        (self as *mut MemStream).cast()
    }
}

// ---------- IUnitInfo and IProgramListData ----------

#[repr(C)]
struct UnitInfoVtbl {
    base: UnknownVtbl,
    get_unit_count: usize,
    get_unit_info: usize,
    get_program_list_count: unsafe extern "system" fn(*mut c_void) -> i32,
    get_program_list_info: unsafe extern "system" fn(*mut c_void, i32, *mut ProgramListInfo) -> TResult,
    get_program_name: unsafe extern "system" fn(*mut c_void, i32, i32, *mut u16) -> TResult,
}

#[repr(C)]
struct ProgramListInfo {
    id: i32,
    name: [u16; 128],
    program_count: i32,
}

#[repr(C)]
struct ProgramListDataVtbl {
    base: UnknownVtbl,
    program_data_supported: unsafe extern "system" fn(*mut c_void, i32) -> TResult,
    get_program_data: unsafe extern "system" fn(*mut c_void, i32, i32, *mut c_void) -> TResult,
}

// ---------- one plugin instance ----------

struct Session {
    comp: *mut c_void,
    proc_: *mut c_void,
    controller: *mut c_void,
    n_in: usize,
    n_out: usize,
    /// (id, name, flags, step count) of the controller's parameters.
    params: Vec<(u32, String, i32, i32)>,
}

fn open(job: &Job) -> Result<Session, String> {
    let handle = sys::load_bundle(&job.path)?;
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
    let f3 = query(factory, &IID_PLUGIN_FACTORY3);
    if !f3.is_null() {
        let _ = call!(f3, FactoryVtbl, set_host_context, host_app());
    }
    let mut seen = 0;
    let mut cid: Option<Tuid> = None;
    for i in 0..call!(factory, FactoryVtbl, count_classes) {
        let mut c1: PClassInfo = unsafe { std::mem::zeroed() };
        if call!(factory, FactoryVtbl, get_class_info, i, &mut c1) == OK && c8(&c1.category) == "Audio Module Class" {
            if seen == job.index {
                cid = Some(c1.cid);
                break;
            }
            seen += 1;
        }
    }
    let cid = cid.ok_or_else(|| format!("no audio class number {}", job.index))?;
    let mut comp: *mut c_void = std::ptr::null_mut();
    let r = call!(factory, FactoryVtbl, create_instance, &cid, &IID_COMPONENT, &mut comp);
    if r != OK || comp.is_null() {
        return Err(format!("createInstance(IComponent) failed ({r})"));
    }
    let r = call!(comp, ComponentVtbl, initialize, host_app());
    if r != OK {
        return Err(format!("IComponent::initialize failed ({r})"));
    }
    let proc_ = query(comp, &IID_AUDIO_PROCESSOR);
    if proc_.is_null() {
        return Err("the component has no IAudioProcessor".into());
    }
    let mut controller = query(comp, &IID_EDIT_CONTROLLER);
    let single = !controller.is_null();
    let mut ccid: Tuid = [0; 16];
    let has_cid = call!(comp, ComponentVtbl, get_controller_class_id, &mut ccid) == OK && ccid != [0; 16];
    if controller.is_null() && has_cid {
        let mut obj: *mut c_void = std::ptr::null_mut();
        let r = call!(factory, FactoryVtbl, create_instance, &ccid, &IID_EDIT_CONTROLLER, &mut obj);
        if r == OK && !obj.is_null() && call!(obj, ControllerVtbl, initialize, host_app()) == OK {
            controller = obj;
        }
    }
    if controller.is_null() {
        return Err("the plugin has no edit controller".into());
    }
    if !single {
        let a = query(comp, &IID_CONNECTION_POINT);
        let b = query(controller, &IID_CONNECTION_POINT);
        if !a.is_null() && !b.is_null() {
            let _ = call!(a, ConnectionVtbl, connect, b);
            let _ = call!(b, ConnectionVtbl, connect, a);
        }
    }
    let _ = call!(controller, ControllerVtbl, set_component_handler, host_handler());

    let mut channels = [0usize; 2];
    for dir in 0..2 {
        if call!(comp, ComponentVtbl, get_bus_count, 0, dir) > 0 {
            let mut info: BusInfo = unsafe { std::mem::zeroed() };
            if call!(comp, ComponentVtbl, get_bus_info, 0, dir, 0, &mut info) == OK {
                channels[dir as usize] = info.channel_count.max(0) as usize;
            }
            let _ = call!(comp, ComponentVtbl, activate_bus, 0, dir, 0, 1u8);
        }
    }
    let mut setup = ProcessSetup { process_mode: 0, symbolic_sample_size: 0, max_samples_per_block: 4096, sample_rate: job.rate };
    let _ = call!(proc_, ProcessorVtbl, setup_processing, &mut setup);
    let _ = call!(comp, ComponentVtbl, set_active, 1u8);
    let _ = call!(proc_, ProcessorVtbl, set_processing, 1u8);

    // A host hands the component's state to the controller after creating both.
    let mut params = Vec::new();
    for i in 0..call!(controller, ControllerVtbl, get_parameter_count).clamp(0, 100_000) {
        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
        if call!(controller, ControllerVtbl, get_parameter_info, i, &mut info) == OK {
            params.push((info.id, c16(&info.title), info.flags, info.step_count));
        }
    }
    Ok(Session { comp, proc_, controller, n_in: channels[0], n_out: channels[1], params })
}

impl Session {
    fn table(&self) -> Vec<(u32, String)> {
        self.params.iter().map(|(id, name, ..)| (*id, name.clone())).collect()
    }

    fn component_state(&self) -> Result<Vec<u8>, String> {
        let mut s = MemStream::new(Vec::new());
        let r = call!(self.comp, ComponentVtbl, get_state, s.ptr());
        if r != OK || s.overflow {
            return Err(format!("IComponent::getState failed ({r})"));
        }
        Ok(s.data)
    }

    /// The controller's own state; `None` when it has none to give.
    fn controller_state(&self) -> Option<Vec<u8>> {
        let mut s = MemStream::new(Vec::new());
        let r = call!(self.controller, ControllerVtbl, get_state, s.ptr());
        (r == OK && !s.overflow && !s.data.is_empty()).then_some(s.data)
    }

    fn values(&self) -> Vec<ParamValue> {
        self.params
            .iter()
            .map(|(id, name, ..)| {
                let norm = call!(self.controller, ControllerVtbl, get_param_normalized, *id);
                let plain = call!(self.controller, ControllerVtbl, normalized_param_to_plain, *id, norm);
                ParamValue { id: *id, name: name.clone(), plain, normalized: Some(norm) }
            })
            .collect()
    }

    /// Set normalized values on the controller and send them to the component in a block.
    fn set_normalized(&self, changes: &[(u32, f64)]) -> Result<(), String> {
        for (id, v) in changes {
            let _ = call!(self.controller, ControllerVtbl, set_param_normalized, *id, *v);
        }
        let r = process_changes(self.proc_, self.n_in, self.n_out, changes);
        if r != OK {
            return Err(format!("IAudioProcessor::process failed ({r})"));
        }
        Ok(())
    }

    fn blobs(&self) -> Result<Vec<(&'static str, Vec<u8>)>, String> {
        let mut blobs = vec![("vst3-component", self.component_state()?)];
        if let Some(c) = self.controller_state() {
            blobs.push(("vst3-controller", c));
        }
        Ok(blobs)
    }
}

/// Capture child: load the plugin, save its default state, `--set` state and factory presets
/// into `sink`. Runs inside the `__capture` child only, since the plugin's code runs here.
/// `Err` with a message when the plugin cannot be loaded or its state cannot be read.
pub fn capture(job: &Job, sink: &mut Sink) -> Result<(), String> {
    let first = open(job)?;
    match first.blobs() {
        Ok(blobs) => sink.add("default", "default", None, blobs, first.values(), Vec::new())?,
        Err(e) => sink.notes.push(format!("default state: {e}")),
    }

    if !job.sets.is_empty() {
        let s = open(job)?;
        let values = resolve_sets(&job.sets, &s.table())?;
        let mut changes = Vec::new();
        for (id, plain) in values {
            changes.push((id, call!(s.controller, ControllerVtbl, plain_param_to_normalized, id, plain)));
        }
        s.set_normalized(&changes)?;
        match s.blobs() {
            Ok(blobs) => sink.add("set", "set", None, blobs, s.values(), Vec::new())?,
            Err(e) => sink.notes.push(format!("state after --set: {e}")),
        }
    }

    // A saved session restored the way a host does it.
    if job.load_component.is_some() || job.load_controller.is_some() {
        let s = open(job)?;
        let mut notes = Vec::new();
        if let Some(path) = &job.load_component {
            let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut stream = MemStream::new(bytes);
            let r = call!(s.comp, ComponentVtbl, set_state, stream.ptr());
            notes.push(format!("IComponent::setState returned {r}"));
            stream.pos = 0;
            let r = call!(s.controller, ControllerVtbl, set_component_state, stream.ptr());
            notes.push(format!("IEditController::setComponentState returned {r}"));
        }
        if let Some(path) = &job.load_controller {
            let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut stream = MemStream::new(bytes);
            let r = call!(s.controller, ControllerVtbl, set_state, stream.ptr());
            notes.push(format!("IEditController::setState returned {r}"));
        }
        match s.blobs() {
            Ok(blobs) => sink.add("loaded", "loaded", None, blobs, s.values(), notes)?,
            Err(e) => sink.notes.push(format!("state after loading: {e}")),
        }
    }

    // Program lists.
    let units = query(first.controller, &IID_UNIT_INFO);
    if units.is_null() {
        sink.notes.push("the controller has no IUnitInfo: no program lists".into());
        return Ok(());
    }
    let lists = call!(units, UnitInfoVtbl, get_program_list_count).clamp(0, 1024);
    if lists == 0 {
        sink.notes.push("the plugin has no program lists".into());
        return Ok(());
    }
    let data_if = query(first.controller, &IID_PROGRAM_LIST_DATA);
    let inst = open(job)?;
    let mut remaining = job.max_presets;
    for l in 0..lists {
        let mut info: ProgramListInfo = unsafe { std::mem::zeroed() };
        if call!(units, UnitInfoVtbl, get_program_list_info, l, &mut info) != OK {
            continue;
        }
        let count = info.program_count.max(0);
        let list_name = c16(&info.name);
        // The parameter a host moves to select a program: flagged as program change, one position per program.
        let selector = inst
            .params
            .iter()
            .find(|(_, _, flags, steps)| *flags as u32 & PROGRAM_CHANGE != 0 && *steps + 1 == count)
            .or_else(|| inst.params.iter().find(|(_, _, flags, _)| *flags as u32 & PROGRAM_CHANGE != 0));
        let supported = !data_if.is_null() && call!(data_if, ProgramListDataVtbl, program_data_supported, info.id) == OK;
        if selector.is_none() && !supported {
            sink.notes.push(format!("program list {list_name:?}: {count} programs but no program-change parameter and no program data"));
            continue;
        }
        for idx in 0..count {
            if remaining == 0 {
                sink.notes.push(format!("program list {list_name:?}: stopped at {} programs (--max-presets)", job.max_presets));
                break;
            }
            remaining -= 1;
            let mut name_buf = [0u16; 128];
            let name = if call!(units, UnitInfoVtbl, get_program_name, info.id, idx, name_buf.as_mut_ptr()) == OK { c16(&name_buf) } else { format!("program {idx}") };
            let mut notes = Vec::new();
            if let Some((pid, _, _, steps)) = selector {
                let norm = if *steps > 0 { f64::from(idx) / f64::from(*steps) } else { 0.0 };
                if let Err(e) = inst.set_normalized(&[(*pid, norm)]) {
                    sink.notes.push(format!("program {name:?}: {e}"));
                    continue;
                }
            } else {
                notes.push("no program-change parameter: parameter values are those of the default state".into());
            }
            let mut blobs = match inst.blobs() {
                Ok(b) => b,
                Err(e) => {
                    sink.notes.push(format!("program {name:?}: {e}"));
                    continue;
                }
            };
            if supported {
                let mut s = MemStream::new(Vec::new());
                if call!(data_if, ProgramListDataVtbl, get_program_data, info.id, idx, s.ptr()) == OK && !s.data.is_empty() {
                    blobs.push(("vst3-program-data", s.data));
                }
            }
            sink.add(&name, "factory-preset", Some(format!("program-list:{}/{idx}", info.id)), blobs, inst.values(), notes)?;
        }
    }
    Ok(())
}
