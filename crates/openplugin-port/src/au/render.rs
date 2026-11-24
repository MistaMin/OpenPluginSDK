//! Audio Unit host loop for the null test (a child module of the probe).
//! The bundle's component is registered in this process only. Each host block
//! is one `AudioUnitRender`; the block's parameter changes are scheduled
//! first, as immediate events at their buffer offsets.

use super::*;

struct Source<'a> {
    feed: &'a crate::render::Feed,
}

unsafe extern "C" fn input_proc(refcon: *mut c_void, _flags: *mut u32, time: *const AudioTimeStamp, _bus: u32, frames: u32, data: *mut c_void) -> OSStatus {
    let src = unsafe { &*(refcon as *const Source) };
    let list = data as *mut u32;
    let n = unsafe { *list } as usize;
    let start = unsafe { (*time).sample_time }.max(0.0) as usize;
    // AudioBufferList: a u32 count, padding to 8, then the buffers.
    let buffers = unsafe { (data as *mut u8).add(8) } as *mut AudioBuffer;
    for c in 0..n {
        let b = unsafe { &mut *buffers.add(c) };
        if b.data.is_null() {
            return -50;
        }
        let slice = unsafe { std::slice::from_raw_parts_mut(b.data.cast::<f32>(), frames as usize) };
        src.feed.fill(c, start, slice);
        b.data_byte_size = frames * 4;
    }
    0
}

fn channels_of(unit: AudioComponentInstance, scope: u32) -> Result<usize, String> {
    let mut count: u32 = 0;
    let mut size = 4u32;
    let st = unsafe { AudioUnitGetProperty(unit, AU_PROP_ELEMENT_COUNT, scope, 0, (&mut count as *mut u32).cast(), &mut size) };
    if st != 0 || count == 0 {
        return Ok(0);
    }
    let mut asbd = AudioStreamBasicDescription::default();
    let mut size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
    let st = unsafe { AudioUnitGetProperty(unit, AU_PROP_STREAM_FORMAT, scope, 0, (&mut asbd as *mut AudioStreamBasicDescription).cast(), &mut size) };
    if st != 0 {
        return Err(format!("cannot read the stream format ({st})"));
    }
    Ok(asbd.channels_per_frame as usize)
}

fn set_format(unit: AudioComponentInstance, scope: u32, channels: usize, rate: f64) -> Result<(), String> {
    let asbd = AudioStreamBasicDescription {
        sample_rate: rate,
        format_id: AU_FORMAT_LINEAR_PCM,
        format_flags: AU_FORMAT_FLOAT_PLANAR,
        bytes_per_packet: 4,
        frames_per_packet: 1,
        bytes_per_frame: 4,
        channels_per_frame: channels as u32,
        bits_per_channel: 32,
        reserved: 0,
    };
    let st = unsafe { AudioUnitSetProperty(unit, AU_PROP_STREAM_FORMAT, scope, 0, (&asbd as *const AudioStreamBasicDescription).cast(), std::mem::size_of::<AudioStreamBasicDescription>() as u32) };
    if st != 0 {
        return Err(format!("the unit refused a {channels}-channel float format at {rate} Hz ({st})"));
    }
    Ok(())
}

/// Render child: load the plugin, activate it at `job.rate` and `job.block`, feed it `feed` with
/// `job.events` at their offsets, and return `frames + latency` frames of output. Runs inside the
/// `__render` child only. `Err` with a message when the plugin cannot be loaded or activated.
pub fn render(job: &crate::render::Job, feed: &crate::render::Feed) -> Result<crate::render::Rendered, String> {
    let comps = plist_components(&job.path);
    let comp = comps.get(job.index as usize).ok_or_else(|| format!("no component number {}", job.index))?;
    let cf_bundle = load_bundle(&job.path)?;
    let (Some(t), Some(s), Some(m)) = (fourcc(&comp.component_type), fourcc(&comp.subtype), fourcc(&comp.manufacturer)) else {
        return Err("type, subtype or manufacturer is not four characters".into());
    };
    let desc = AudioComponentDescription { component_type: t, component_sub_type: s, component_manufacturer: m, ..Default::default() };
    let factory = comp.factory.as_ref().ok_or("the bundle's Info.plist has no factoryFunction")?;
    let f = bundle_function(cf_bundle, factory);
    if f.is_null() {
        return Err(format!("factory function {factory} not found in the bundle"));
    }
    let f: AudioComponentFactoryFunction = unsafe { std::mem::transmute(f) };
    let cf_name = cf_string(&comp.name);
    let component = unsafe { AudioComponentRegister(&desc, cf_name, comp.version.unwrap_or(0), f) };
    unsafe { CFRelease(cf_name) };
    if component.is_null() {
        return Err("the component could not be registered".into());
    }
    let mut unit: AudioComponentInstance = std::ptr::null_mut();
    let st = unsafe { AudioComponentInstanceNew(component, &mut unit) };
    if st != 0 || unit.is_null() {
        return Err(format!("AudioComponentInstanceNew failed ({st})"));
    }
    let n_in = channels_of(unit, AU_SCOPE_INPUT)?;
    let n_out = channels_of(unit, AU_SCOPE_OUTPUT)?;
    if n_out == 0 {
        return Err("the unit has no audio output".into());
    }
    if n_in > 0 {
        set_format(unit, AU_SCOPE_INPUT, n_in, job.rate)?;
    }
    set_format(unit, AU_SCOPE_OUTPUT, n_out, job.rate)?;
    let max = job.block;
    let st = unsafe { AudioUnitSetProperty(unit, AU_PROP_MAXIMUM_FRAMES_PER_SLICE, AU_SCOPE_GLOBAL, 0, (&max as *const u32).cast(), 4) };
    if st != 0 {
        return Err(format!("MaximumFramesPerSlice refused ({st})"));
    }
    let source = Source { feed };
    if n_in > 0 {
        let cb = AURenderCallbackStruct { input_proc, input_proc_ref_con: (&source as *const Source).cast_mut().cast() };
        let st = unsafe { AudioUnitSetProperty(unit, AU_PROP_SET_RENDER_CALLBACK, AU_SCOPE_INPUT, 0, (&cb as *const AURenderCallbackStruct).cast(), std::mem::size_of::<AURenderCallbackStruct>() as u32) };
        if st != 0 {
            return Err(format!("cannot set the render callback ({st})"));
        }
    }
    let st = unsafe { AudioUnitInitialize(unit) };
    if st != 0 {
        return Err(format!("AudioUnitInitialize failed ({st})"));
    }
    let mut seconds = 0f64;
    let mut size = 8u32;
    if unsafe { AudioUnitGetProperty(unit, AU_PROP_LATENCY, AU_SCOPE_GLOBAL, 0, (&mut seconds as *mut f64).cast(), &mut size) } != 0 {
        seconds = 0.0;
    }
    let latency = (seconds * job.rate).round().max(0.0) as u32;

    let total = feed.frames + latency as usize;
    let events = job.sorted_events();
    let mut next_event = 0;
    let block = job.block as usize;
    let mut out = vec![vec![0f32; total]; n_out];
    let mut out_bufs = vec![vec![0f32; block]; n_out];
    // AudioBufferList with n_out buffers: u32 count, 4 bytes of padding, then the buffers.
    let mut list = vec![0u8; 8 + n_out * std::mem::size_of::<AudioBuffer>()];
    let mut start = 0;
    let mut block_index = 0;
    while start < total {
        job.checkpoint(block_index);
        block_index += 1;
        let len = block.min(total - start);
        while next_event < events.len() && (events[next_event].frame as usize) < start + len {
            let e = &events[next_event];
            next_event += 1;
            let ev = AudioUnitParameterEvent {
                scope: AU_SCOPE_GLOBAL,
                element: 0,
                parameter: e.param,
                event_type: AU_PARAMETER_EVENT_IMMEDIATE,
                buffer_offset: (e.frame as usize).saturating_sub(start) as u32,
                value: e.value as f32,
                unused: [0; 2],
            };
            let st = unsafe { AudioUnitScheduleParameters(unit, &ev, 1) };
            if st != 0 {
                return Err(format!("AudioUnitScheduleParameters failed ({st}) for parameter {}", e.param));
            }
        }
        unsafe {
            *(list.as_mut_ptr() as *mut u32) = n_out as u32;
            let buffers = list.as_mut_ptr().add(8) as *mut AudioBuffer;
            for (c, b) in out_bufs.iter_mut().enumerate() {
                *buffers.add(c) = AudioBuffer { number_channels: 1, data_byte_size: (len * 4) as u32, data: b.as_mut_ptr().cast() };
            }
        }
        let stamp = AudioTimeStamp { sample_time: start as f64, flags: 1, ..Default::default() };
        let mut flags = 0u32;
        let st = unsafe { AudioUnitRender(unit, &mut flags, &stamp, 0, len as u32, list.as_mut_ptr().cast()) };
        if st != 0 {
            return Err(format!("AudioUnitRender failed ({st}) at frame {start}"));
        }
        // The unit may point the buffers elsewhere; read back through them.
        unsafe {
            let buffers = list.as_ptr().add(8) as *const AudioBuffer;
            for (c, o) in out.iter_mut().enumerate() {
                let b = *buffers.add(c);
                let src = std::slice::from_raw_parts(b.data.cast::<f32>(), len);
                o[start..start + len].copy_from_slice(src);
            }
        }
        start += len;
    }
    Ok(crate::render::Rendered { latency, in_channels: n_in as u32, out })
}
