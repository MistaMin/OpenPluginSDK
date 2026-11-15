//! CLAP probe: `dlopen`, `clap_entry`, the plugin factory, then each plugin's
//! parameters, audio ports, latency and tail. Runs in the child process.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::Path;

use openplugin_targets::clap_abi::*;

use crate::manifest::*;
use crate::sys;

mod capture;
pub use capture::capture;
mod render;
pub use render::render;

static HOST_NAME: &[u8] = b"openplugin-port\0";
static EMPTY: &[u8] = b"\0";
static HOST_VERSION: &[u8] = b"0.4.0\0";

unsafe extern "C" fn host_get_extension(_host: *const clap_host, _id: *const c_char) -> *const c_void {
    std::ptr::null()
}
unsafe extern "C" fn host_noop(_host: *const clap_host) {}

fn text(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
    }
}

fn fixed(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A plugin file to open: the bundle's executable (see [`crate::bundle::executable`]).
pub fn probe(bundle: &Path, exe: &Path) -> Result<ProbeResult, String> {
    let lib = sys::open_library(exe)?;
    let entry = sys::symbol(lib, "clap_entry") as *const clap_plugin_entry;
    if entry.is_null() {
        return Err("no clap_entry symbol: not a CLAP plugin".into());
    }
    let entry = unsafe { &*entry };
    let path = CString::new(bundle.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    let init = entry.init.ok_or("clap_entry.init is missing")?;
    if !unsafe { init(path.as_ptr()) } {
        return Err("clap_entry.init returned false".into());
    }
    let get_factory = entry.get_factory.ok_or("clap_entry.get_factory is missing")?;
    let factory = unsafe { get_factory(FACTORY_ID.as_ptr().cast()) } as *const clap_plugin_factory;
    if factory.is_null() {
        return Err("no plugin factory".into());
    }
    let factory = unsafe { &*factory };
    let count = unsafe { factory.get_plugin_count.ok_or("factory without get_plugin_count")?(factory) };
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
    let mut result = ProbeResult::default();
    for i in 0..count {
        let desc = unsafe { factory.get_plugin_descriptor.ok_or("factory without get_plugin_descriptor")?(factory, i) };
        if desc.is_null() {
            result.notes.push(format!("plugin {i}: null descriptor"));
            continue;
        }
        let d = unsafe { &*desc };
        let mut plugin = Plugin {
            name: text(d.name),
            vendor: text(d.vendor),
            version: text(d.version),
            url: text(d.url),
            description: text(d.description),
            ..Plugin::default()
        };
        let mut p = d.features;
        while !p.is_null() && !unsafe { *p }.is_null() {
            plugin.categories.push(text(unsafe { *p }));
            p = unsafe { p.add(1) };
        }
        plugin.is_instrument = plugin.categories.iter().any(|f| f == "instrument");
        let id = text(d.id);
        plugin.ids.clap = Some(ClapIds { id: id.clone() });
        let cid = CString::new(id).unwrap_or_default();
        let created = unsafe { factory.create_plugin.ok_or("factory without create_plugin")?(factory, host, cid.as_ptr()) };
        if created.is_null() {
            plugin.warnings.push("the factory would not create the plugin".into());
        } else if let Err(e) = describe(&mut plugin, unsafe { &*created }) {
            plugin.warnings.push(e);
        }
        result.plugins.push(plugin);
    }
    Ok(result)
}

fn describe(plugin: &mut Plugin, p: &clap_plugin) -> Result<(), String> {
    let pp = p as *const clap_plugin;
    let init = p.init.ok_or("plugin.init is missing")?;
    if !unsafe { init(pp) } {
        return Err("plugin.init returned false".into());
    }
    let ext = |name: &[u8]| -> *const c_void { p.get_extension.map_or(std::ptr::null(), |f| unsafe { f(pp, name.as_ptr().cast()) }) };

    // Ports and latency can depend on activation.
    let mut activated = false;
    if let Some(activate) = p.activate {
        activated = unsafe { activate(pp, 48_000.0, 1, 4096) };
        if !activated {
            plugin.warnings.push("plugin.activate failed; latency read without activation".into());
        }
    }

    let ports = ext(EXT_AUDIO_PORTS) as *const clap_plugin_audio_ports;
    if !ports.is_null() {
        let ports = unsafe { &*ports };
        if let (Some(count), Some(get)) = (ports.count, ports.get) {
            for (is_input, dir) in [(true, "input"), (false, "output")] {
                for i in 0..unsafe { count(pp, is_input) } {
                    let mut info: clap_audio_port_info = unsafe { std::mem::zeroed() };
                    if unsafe { get(pp, i, is_input, &mut info) } {
                        let port_type = text(info.port_type);
                        let layout = match (port_type.as_str(), info.channel_count) {
                            ("mono", _) | (_, 1) => Some("mono".to_string()),
                            ("stereo", _) | (_, 2) => Some("stereo".to_string()),
                            _ => None,
                        };
                        plugin.buses.push(Bus {
                            direction: dir.into(),
                            kind: if info.flags & AUDIO_PORT_IS_MAIN != 0 { "main" } else { "aux" }.into(),
                            name: fixed(&info.name),
                            channels: info.channel_count,
                            arrangement: if port_type.is_empty() { None } else { Some(port_type) },
                            layout,
                        });
                    }
                }
            }
        }
    }
    let lat = ext(EXT_LATENCY) as *const clap_plugin_latency;
    if !lat.is_null() {
        if let Some(get) = unsafe { (*lat).get } {
            plugin.latency_samples = Some(unsafe { get(pp) });
        }
    }
    let tail = ext(EXT_TAIL) as *const clap_plugin_tail;
    if !tail.is_null() {
        if let Some(get) = unsafe { (*tail).get } {
            plugin.tail_samples = Some(unsafe { get(pp) });
        }
    }

    let params = ext(EXT_PARAMS) as *const clap_plugin_params;
    if !params.is_null() {
        let params = unsafe { &*params };
        if let (Some(count), Some(get_info)) = (params.count, params.get_info) {
            let n = unsafe { count(pp) };
            for i in 0..n {
                let mut info: clap_param_info = unsafe { std::mem::zeroed() };
                if !unsafe { get_info(pp, i, &mut info) } {
                    plugin.warnings.push(format!("parameter {i}: get_info failed"));
                    continue;
                }
                plugin.params.push(read_param(pp, params, &info));
            }
        }
    }
    if activated {
        if let Some(deactivate) = p.deactivate {
            unsafe { deactivate(pp) };
        }
    }
    Ok(())
}

fn read_param(pp: *const clap_plugin, params: &clap_plugin_params, info: &clap_param_info) -> Param {
    let (min, max) = (info.min_value, info.max_value);
    let stepped = info.flags & PARAM_IS_STEPPED != 0;
    let step_count = if stepped && max > min { (max - min).round().min(f64::from(u32::MAX)) as u32 } else { 0 };
    let to_text = |value: f64| -> String {
        let Some(f) = params.value_to_text else { return String::new() };
        let mut buf = [0 as c_char; 256];
        if unsafe { f(pp, info.id, value, buf.as_mut_ptr(), buf.len() as u32) } { fixed(&buf) } else { String::new() }
    };
    let mut flags = Vec::new();
    let mut add = |bit: u32, name: &str| {
        if info.flags & bit != 0 {
            flags.push(name.to_string());
        }
    };
    add(PARAM_IS_AUTOMATABLE, "automatable");
    add(PARAM_IS_STEPPED, "stepped");
    add(2, "periodic");
    add(PARAM_IS_HIDDEN, "hidden");
    add(PARAM_IS_READONLY, "read_only");
    add(PARAM_IS_BYPASS, "bypass");
    add(PARAM_REQUIRES_PROCESS, "requires_process");
    let samples = SAMPLE_POINTS
        .iter()
        .map(|&t| {
            let plain = min + (max - min) * t;
            Sample { normalized: t, plain, text: to_text(plain) }
        })
        .collect();
    let step_texts = if (1..MAX_STEP_TEXTS).contains(&step_count) {
        (0..=step_count).map(|k| to_text(min + f64::from(k))).collect()
    } else {
        Vec::new()
    };
    let module = fixed(&info.module);
    let span = max - min;
    Param {
        id: info.id,
        name: fixed(&info.name),
        short_name: None,
        group: if module.is_empty() { None } else { Some(module) },
        units: String::new(),
        min,
        max,
        default: info.default_value,
        default_normalized: if span > 0.0 { ((info.default_value - min) / span).clamp(0.0, 1.0) } else { 0.0 },
        step_count,
        flags,
        flags_raw: info.flags,
        samples,
        step_texts,
    }
}
