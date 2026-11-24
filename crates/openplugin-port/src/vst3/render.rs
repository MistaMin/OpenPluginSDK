//! VST3 host loop for the null test (a child module of the probe, which owns
//! the COM binding and the host stubs). Parameter values are points of an
//! `IParameterChanges` queue at their sample offsets in the block; plain
//! values become normalized ones through the plugin's own controller.

use super::*;

const IID_PARAMETER_CHANGES: Tuid = iid(0xA477_9663, 0x0BB6_4A56, 0xB443_84B8, 0xA220_1CED);
const IID_PARAM_VALUE_QUEUE: Tuid = iid(0x0126_3A18, 0xED07_4F6F, 0x98C9_D356, 0x4686_F9BA);

#[repr(C)]
pub(super) struct AudioBusBuffers {
    num_channels: i32,
    silence_flags: u64,
    channel_buffers32: *mut *mut f32,
}

#[repr(C)]
pub(super) struct ProcessData {
    process_mode: i32,
    symbolic_sample_size: i32,
    num_samples: i32,
    num_inputs: i32,
    num_outputs: i32,
    inputs: *mut AudioBusBuffers,
    outputs: *mut AudioBusBuffers,
    input_parameter_changes: *mut c_void,
    output_parameter_changes: *mut c_void,
    input_events: *mut c_void,
    output_events: *mut c_void,
    process_context: *mut c_void,
}

#[repr(C)]
struct ChangesVtbl {
    base: UnknownVtbl,
    get_parameter_count: unsafe extern "system" fn(*mut c_void) -> i32,
    get_parameter_data: unsafe extern "system" fn(*mut c_void, i32) -> *mut c_void,
    add_parameter_data: unsafe extern "system" fn(*mut c_void, *const u32, *mut i32) -> *mut c_void,
}

#[repr(C)]
struct QueueVtbl {
    base: UnknownVtbl,
    get_parameter_id: unsafe extern "system" fn(*mut c_void) -> u32,
    get_point_count: unsafe extern "system" fn(*mut c_void) -> i32,
    get_point: unsafe extern "system" fn(*mut c_void, i32, *mut i32, *mut f64) -> TResult,
    add_point: unsafe extern "system" fn(*mut c_void, i32, f64, *mut i32) -> TResult,
}

#[repr(C)]
struct Queue {
    vtbl: *const QueueVtbl,
    id: u32,
    points: Vec<(i32, f64)>,
}

#[repr(C)]
struct Changes {
    vtbl: *const ChangesVtbl,
    // Boxed so a queue keeps its address while the list grows.
    #[allow(clippy::vec_box)]
    queues: Vec<Box<Queue>>,
}

unsafe extern "system" fn ch_query(this: *mut c_void, wanted: *const Tuid, out: *mut *mut c_void) -> TResult {
    let want = unsafe { *wanted };
    let found = if want == IID_FUNKNOWN || want == IID_PARAMETER_CHANGES { this } else { std::ptr::null_mut() };
    unsafe { *out = found };
    if found.is_null() { NO_INTERFACE } else { OK }
}
unsafe extern "system" fn q_query(this: *mut c_void, wanted: *const Tuid, out: *mut *mut c_void) -> TResult {
    let want = unsafe { *wanted };
    let found = if want == IID_FUNKNOWN || want == IID_PARAM_VALUE_QUEUE { this } else { std::ptr::null_mut() };
    unsafe { *out = found };
    if found.is_null() { NO_INTERFACE } else { OK }
}
unsafe extern "system" fn ref_add(_: *mut c_void) -> u32 {
    2
}
unsafe extern "system" fn ref_release(_: *mut c_void) -> u32 {
    1
}
unsafe extern "system" fn ch_count(this: *mut c_void) -> i32 {
    unsafe { &*(this as *mut Changes) }.queues.len() as i32
}
unsafe extern "system" fn ch_get(this: *mut c_void, index: i32) -> *mut c_void {
    let c = unsafe { &mut *(this as *mut Changes) };
    c.queues.get_mut(index.max(0) as usize).map_or(std::ptr::null_mut(), |q| (&mut **q as *mut Queue).cast())
}
unsafe extern "system" fn ch_add(this: *mut c_void, id: *const u32, index: *mut i32) -> *mut c_void {
    let c = unsafe { &mut *(this as *mut Changes) };
    let id = unsafe { *id };
    let at = match c.queues.iter().position(|q| q.id == id) {
        Some(i) => i,
        None => {
            c.queues.push(Box::new(Queue { vtbl: &QUEUE_VTBL, id, points: Vec::new() }));
            c.queues.len() - 1
        }
    };
    if !index.is_null() {
        unsafe { *index = at as i32 };
    }
    (&mut *c.queues[at] as *mut Queue).cast()
}
unsafe extern "system" fn q_id(this: *mut c_void) -> u32 {
    unsafe { &*(this as *mut Queue) }.id
}
unsafe extern "system" fn q_count(this: *mut c_void) -> i32 {
    unsafe { &*(this as *mut Queue) }.points.len() as i32
}
unsafe extern "system" fn q_get(this: *mut c_void, index: i32, offset: *mut i32, value: *mut f64) -> TResult {
    let q = unsafe { &*(this as *mut Queue) };
    match q.points.get(index.max(0) as usize) {
        Some((o, v)) if index >= 0 => {
            unsafe {
                *offset = *o;
                *value = *v;
            }
            OK
        }
        _ => 1,
    }
}
unsafe extern "system" fn q_add(this: *mut c_void, offset: i32, value: f64, index: *mut i32) -> TResult {
    let q = unsafe { &mut *(this as *mut Queue) };
    q.points.push((offset, value));
    if !index.is_null() {
        unsafe { *index = q.points.len() as i32 - 1 };
    }
    OK
}

static CHANGES_VTBL: ChangesVtbl = ChangesVtbl {
    base: UnknownVtbl { query_interface: ch_query, add_ref: ref_add, release: ref_release },
    get_parameter_count: ch_count,
    get_parameter_data: ch_get,
    add_parameter_data: ch_add,
};
static QUEUE_VTBL: QueueVtbl = QueueVtbl {
    base: UnknownVtbl { query_interface: q_query, add_ref: ref_add, release: ref_release },
    get_parameter_id: q_id,
    get_point_count: q_count,
    get_point: q_get,
    add_point: q_add,
};

/// Render child: load the plugin, activate it at `job.rate` and `job.block`, feed it `feed` with
/// `job.events` at their offsets, and return `frames + latency` frames of output. Runs inside the
/// `__render` child only. `Err` with a message when the plugin cannot be loaded or activated.
pub fn render(job: &crate::render::Job, feed: &crate::render::Feed) -> Result<crate::render::Rendered, String> {
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
    // The job's index counts the Audio Module Classes.
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

    // The controller converts plain values to normalized ones.
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

    // Main buses only: bus 0 each way.
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
    let (n_in, n_out) = (channels[0], channels[1]);
    if n_out == 0 {
        return Err("the plugin has no audio output".into());
    }
    let mut setup = ProcessSetup { process_mode: 0, symbolic_sample_size: 0, max_samples_per_block: job.block as i32, sample_rate: job.rate };
    let r = call!(proc_, ProcessorVtbl, setup_processing, &mut setup);
    if r != OK {
        return Err(format!("setupProcessing failed ({r})"));
    }
    let r = call!(comp, ComponentVtbl, set_active, 1u8);
    if r != OK {
        return Err(format!("setActive failed ({r})"));
    }
    let _ = call!(proc_, ProcessorVtbl, set_processing, 1u8);
    let latency = call!(proc_, ProcessorVtbl, get_latency_samples);

    let total = feed.frames + latency as usize;
    let events = job.sorted_events();
    let mut next_event = 0;
    let block = job.block as usize;
    let mut out = vec![vec![0f32; total]; n_out];
    let mut in_bufs = vec![vec![0f32; block]; n_in];
    let mut out_bufs = vec![vec![0f32; block]; n_out];
    let mut start = 0;
    let mut block_index = 0;
    while start < total {
        job.checkpoint(block_index);
        block_index += 1;
        let len = block.min(total - start);
        for (c, b) in in_bufs.iter_mut().enumerate() {
            feed.fill(c, start, &mut b[..len]);
        }
        let mut changes = Changes { vtbl: &CHANGES_VTBL, queues: Vec::new() };
        while next_event < events.len() && (events[next_event].frame as usize) < start + len {
            let e = &events[next_event];
            next_event += 1;
            let norm = call!(controller, ControllerVtbl, plain_param_to_normalized, e.param, e.value);
            let mut index = 0i32;
            let q = unsafe { ch_add((&mut changes as *mut Changes).cast(), &e.param, &mut index) };
            let mut point = 0i32;
            let offset = (e.frame as usize).saturating_sub(start) as i32;
            unsafe { q_add(q, offset, norm, &mut point) };
        }
        let mut in_ptrs: Vec<*mut f32> = in_bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        let mut out_ptrs: Vec<*mut f32> = out_bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        let mut input = AudioBusBuffers { num_channels: n_in as i32, silence_flags: 0, channel_buffers32: in_ptrs.as_mut_ptr() };
        let mut output = AudioBusBuffers { num_channels: n_out as i32, silence_flags: 0, channel_buffers32: out_ptrs.as_mut_ptr() };
        let mut data = ProcessData {
            process_mode: 0,
            symbolic_sample_size: 0,
            num_samples: len as i32,
            num_inputs: i32::from(n_in > 0),
            num_outputs: 1,
            inputs: if n_in > 0 { &mut input } else { std::ptr::null_mut() },
            outputs: &mut output,
            input_parameter_changes: if changes.queues.is_empty() { std::ptr::null_mut() } else { (&mut changes as *mut Changes).cast() },
            output_parameter_changes: std::ptr::null_mut(),
            input_events: std::ptr::null_mut(),
            output_events: std::ptr::null_mut(),
            process_context: std::ptr::null_mut(),
        };
        let r = call!(proc_, ProcessorVtbl, process, &mut data);
        if r != OK {
            return Err(format!("IAudioProcessor::process failed ({r}) at frame {start}"));
        }
        for (c, b) in out_bufs.iter().enumerate() {
            out[c][start..start + len].copy_from_slice(&b[..len]);
        }
        start += len;
    }
    Ok(crate::render::Rendered { latency, in_channels: n_in as u32, out })
}

/// Run one block of silence through the processor with `changes` (parameter id, normalized value)
/// as points at offset 0, so the component sees them as a host would send them.
pub(super) fn process_changes(proc_: *mut c_void, n_in: usize, n_out: usize, changes: &[(u32, f64)]) -> TResult {
    const FRAMES: usize = 64;
    let mut queued = Changes { vtbl: &CHANGES_VTBL, queues: Vec::new() };
    for (id, value) in changes {
        let mut index = 0i32;
        let q = unsafe { ch_add((&mut queued as *mut Changes).cast(), id, &mut index) };
        let mut point = 0i32;
        unsafe { q_add(q, 0, *value, &mut point) };
    }
    let mut in_bufs = vec![vec![0f32; FRAMES]; n_in];
    let mut out_bufs = vec![vec![0f32; FRAMES]; n_out];
    let mut in_ptrs: Vec<*mut f32> = in_bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
    let mut out_ptrs: Vec<*mut f32> = out_bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
    let mut input = AudioBusBuffers { num_channels: n_in as i32, silence_flags: 0, channel_buffers32: in_ptrs.as_mut_ptr() };
    let mut output = AudioBusBuffers { num_channels: n_out as i32, silence_flags: 0, channel_buffers32: out_ptrs.as_mut_ptr() };
    let mut data = ProcessData {
        process_mode: 0,
        symbolic_sample_size: 0,
        num_samples: FRAMES as i32,
        num_inputs: i32::from(n_in > 0),
        num_outputs: i32::from(n_out > 0),
        inputs: if n_in > 0 { &mut input } else { std::ptr::null_mut() },
        outputs: if n_out > 0 { &mut output } else { std::ptr::null_mut() },
        input_parameter_changes: if queued.queues.is_empty() { std::ptr::null_mut() } else { (&mut queued as *mut Changes).cast() },
        output_parameter_changes: std::ptr::null_mut(),
        input_events: std::ptr::null_mut(),
        output_events: std::ptr::null_mut(),
        process_context: std::ptr::null_mut(),
    };
    call!(proc_, ProcessorVtbl, process, &mut data)
}
