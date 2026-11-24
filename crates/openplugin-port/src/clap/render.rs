//! CLAP host loop for the null test (a child module of the probe, which owns
//! the host stubs): parameter values as `param_value` events at their offsets
//! in the block.

use super::*;

struct EventList {
    events: Vec<clap_event_param_value>,
}

unsafe extern "C" fn list_size(list: *const clap_input_events) -> u32 {
    unsafe { &*((*list).ctx as *const EventList) }.events.len() as u32
}
unsafe extern "C" fn list_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let l = unsafe { &*((*list).ctx as *const EventList) };
    l.events.get(index as usize).map_or(std::ptr::null(), |e| &e.header as *const clap_event_header)
}
unsafe extern "C" fn out_push(_list: *const clap_output_events, _event: *const clap_event_header) -> bool {
    true
}

/// Render child: load the plugin, activate it at `job.rate` and `job.block`, feed it `feed` with
/// `job.events` at their offsets, and return `frames + latency` frames of output. Runs inside the
/// `__render` child only. `Err` with a message when the plugin cannot be loaded or activated.
pub fn render(job: &crate::render::Job, feed: &crate::render::Feed) -> Result<crate::render::Rendered, String> {
    let exe = crate::bundle::executable(&job.path)?;
    let lib = sys::open_library(&exe)?;
    let entry = sys::symbol(lib, "clap_entry") as *const clap_plugin_entry;
    if entry.is_null() {
        return Err("no clap_entry symbol: not a CLAP plugin".into());
    }
    let entry = unsafe { &*entry };
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
    let host = Box::leak(Box::new(clap_host {
        clap_version: CLAP_VERSION,
        host_data: std::ptr::null_mut(),
        name: HOST_NAME.as_ptr().cast(),
        vendor: EMPTY.as_ptr().cast(),
        url: EMPTY.as_ptr().cast(),
        version: HOST_VERSION.as_ptr().cast(),
        get_extension: Some(host_get_extension),
        request_restart: Some(host_noop),
        request_process: Some(host_noop),
        request_callback: Some(host_noop),
    }));
    let created = unsafe { factory.create_plugin.ok_or("factory without create_plugin")?(factory, host, (*desc).id) };
    if created.is_null() {
        return Err("the factory would not create the plugin".into());
    }
    let p = unsafe { &*created };
    let pp = created;
    if !unsafe { p.init.ok_or("plugin.init is missing")?(pp) } {
        return Err("plugin.init returned false".into());
    }
    if !unsafe { p.activate.ok_or("plugin.activate is missing")?(pp, job.rate, 1, job.block) } {
        return Err("plugin.activate failed".into());
    }
    let ext = |name: &[u8]| -> *const c_void { p.get_extension.map_or(std::ptr::null(), |f| unsafe { f(pp, name.as_ptr().cast()) }) };
    let main_channels = |is_input: bool| -> u32 {
        let ports = ext(EXT_AUDIO_PORTS) as *const clap_plugin_audio_ports;
        if ports.is_null() {
            return 0;
        }
        let ports = unsafe { &*ports };
        let (Some(count), Some(get)) = (ports.count, ports.get) else { return 0 };
        if unsafe { count(pp, is_input) } == 0 {
            return 0;
        }
        let mut info: clap_audio_port_info = unsafe { std::mem::zeroed() };
        if unsafe { get(pp, 0, is_input, &mut info) } { info.channel_count } else { 0 }
    };
    let (n_in, n_out) = (main_channels(true) as usize, main_channels(false) as usize);
    if n_out == 0 {
        return Err("the plugin has no audio output".into());
    }
    let lat = ext(EXT_LATENCY) as *const clap_plugin_latency;
    let latency = if lat.is_null() { 0 } else { unsafe { (*lat).get.map_or(0, |g| g(pp)) } };
    if let Some(start) = p.start_processing {
        if !unsafe { start(pp) } {
            return Err("plugin.start_processing failed".into());
        }
    }
    let total = feed.frames + latency as usize;
    let events = job.sorted_events();
    let mut next_event = 0;
    let block = job.block as usize;
    let mut out = vec![vec![0f32; total]; n_out];
    let mut in_bufs = vec![vec![0f32; block]; n_in];
    let mut out_bufs = vec![vec![0f32; block]; n_out];
    let process = p.process.ok_or("plugin.process is missing")?;
    let mut start = 0;
    let mut block_index = 0;
    while start < total {
        job.checkpoint(block_index);
        block_index += 1;
        let len = block.min(total - start);
        for (c, b) in in_bufs.iter_mut().enumerate() {
            feed.fill(c, start, &mut b[..len]);
        }
        let mut list = EventList { events: Vec::new() };
        while next_event < events.len() && (events[next_event].frame as usize) < start + len {
            let e = &events[next_event];
            next_event += 1;
            list.events.push(clap_event_param_value {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_value>() as u32,
                    time: (e.frame as usize).saturating_sub(start) as u32,
                    space_id: 0,
                    type_: EVENT_PARAM_VALUE,
                    flags: 0,
                },
                param_id: e.param,
                cookie: std::ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value: e.value,
            });
        }
        let in_events = clap_input_events { ctx: (&list as *const EventList).cast_mut().cast(), size: Some(list_size), get: Some(list_get) };
        let out_events = clap_output_events { ctx: std::ptr::null_mut(), try_push: Some(out_push) };
        let mut in_ptrs: Vec<*mut f32> = in_bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        let mut out_ptrs: Vec<*mut f32> = out_bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        let input = clap_audio_buffer { data32: in_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: n_in as u32, latency: 0, constant_mask: 0 };
        let mut output = clap_audio_buffer { data32: out_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: n_out as u32, latency: 0, constant_mask: 0 };
        let proc_ = clap_process {
            steady_time: start as i64,
            frames_count: len as u32,
            transport: std::ptr::null(),
            audio_inputs: if n_in > 0 { &input } else { std::ptr::null() },
            audio_outputs: &mut output,
            audio_inputs_count: u32::from(n_in > 0),
            audio_outputs_count: 1,
            in_events: &in_events,
            out_events: &out_events,
        };
        let status = unsafe { process(pp, &proc_) };
        if status == PROCESS_ERROR {
            return Err(format!("plugin.process returned an error at frame {start}"));
        }
        for (c, b) in out_bufs.iter().enumerate() {
            out[c][start..start + len].copy_from_slice(&b[..len]);
        }
        start += len;
    }
    Ok(crate::render::Rendered { latency, in_channels: n_in as u32, out })
}
