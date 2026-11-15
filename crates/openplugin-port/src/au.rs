//! Audio Unit probe. The bundle's `Info.plist` names each component and, for
//! current units, a factory function in the bundle's binary. The probe loads
//! the bundle in its own process, registers the component there with
//! `AudioComponentRegister` (nothing is installed, and nothing outside this
//! process learns of it), makes one instance and reads its properties. A unit
//! whose plist has no factory function (an old Component Manager unit) can
//! only be found if the system already knows it; then the probe looks it up
//! by its description. Runs in the child process.

use std::ffi::c_void;
use std::path::Path;

use crate::bundle;
use crate::manifest::*;
use crate::sys::*;

/// What `Info.plist` says about one component.
#[derive(Debug, Clone)]
pub struct PlistComponent {
    pub name: String,
    pub description: String,
    pub component_type: String,
    pub subtype: String,
    pub manufacturer: String,
    pub factory: Option<String>,
    pub version: Option<u32>,
}

/// `AudioComponents` from the bundle's `Info.plist`.
pub fn plist_components(bundle_path: &Path) -> Vec<PlistComponent> {
    let Some(plist) = bundle::info_plist(bundle_path) else { return Vec::new() };
    let Some(list) = plist.get("AudioComponents").and_then(|v| v.as_array()) else { return Vec::new() };
    list.iter()
        .filter_map(|c| {
            let s = |k: &str| c.get(k).and_then(|v| v.as_str()).map(str::to_string);
            Some(PlistComponent {
                name: s("name").unwrap_or_default(),
                description: s("description").unwrap_or_default(),
                component_type: s("type")?,
                subtype: s("subtype")?,
                manufacturer: s("manufacturer")?,
                factory: s("factoryFunction"),
                version: c.get("version").and_then(|v| v.as_u64()).map(|v| v as u32),
            })
        })
        .collect()
}

pub fn probe(bundle_path: &Path) -> Result<ProbeResult, String> {
    let comps = plist_components(bundle_path);
    if comps.is_empty() {
        return Err("Info.plist has no AudioComponents entry".into());
    }
    let cf_bundle = load_bundle(bundle_path)?;
    let plist = bundle::info_plist(bundle_path);
    let bundle_version = plist.as_ref().and_then(|p| bundle::plist_str(p, "CFBundleShortVersionString").map(str::to_string)).unwrap_or_default();

    let mut result = ProbeResult::default();
    for comp in comps {
        let mut plugin = Plugin { version: bundle_version.clone(), description: comp.description.clone(), ..Plugin::default() };
        match comp.name.split_once(": ") {
            Some((vendor, name)) => {
                plugin.vendor = vendor.to_string();
                plugin.name = name.to_string();
            }
            None => plugin.name = comp.name.clone(),
        }
        plugin.is_instrument = comp.component_type == "aumu";
        plugin.categories.push(comp.component_type.clone());
        plugin.ids.au = Some(AuIds {
            component_type: comp.component_type.clone(),
            subtype: comp.subtype.clone(),
            manufacturer: comp.manufacturer.clone(),
            factory_function: comp.factory.clone(),
            version: comp.version,
        });
        if let Err(e) = describe(cf_bundle, &comp, &mut plugin) {
            plugin.warnings.push(e);
        }
        result.plugins.push(plugin);
    }
    Ok(result)
}

fn describe(cf_bundle: CFBundleRef, comp: &PlistComponent, plugin: &mut Plugin) -> Result<(), String> {
    let (Some(t), Some(s), Some(m)) = (fourcc(&comp.component_type), fourcc(&comp.subtype), fourcc(&comp.manufacturer)) else {
        return Err("type, subtype or manufacturer is not four characters".into());
    };
    let desc = AudioComponentDescription { component_type: t, component_sub_type: s, component_manufacturer: m, ..Default::default() };

    let component: AudioComponent = match &comp.factory {
        Some(name) => {
            let f = bundle_function(cf_bundle, name);
            if f.is_null() {
                return Err(format!("factory function {name} not found in the bundle"));
            }
            let f: AudioComponentFactoryFunction = unsafe { std::mem::transmute(f) };
            let cf_name = cf_string(&comp.name);
            let c = unsafe { AudioComponentRegister(&desc, cf_name, comp.version.unwrap_or(0), f) };
            unsafe { CFRelease(cf_name) };
            c
        }
        None => {
            plugin.warnings.push("no factoryFunction in Info.plist: looked the unit up by its description in the system's registry".into());
            unsafe { AudioComponentFindNext(std::ptr::null_mut(), &desc) }
        }
    };
    if component.is_null() {
        return Err("the component could not be registered".into());
    }
    let mut unit: AudioComponentInstance = std::ptr::null_mut();
    let status = unsafe { AudioComponentInstanceNew(component, &mut unit) };
    if status != 0 || unit.is_null() {
        return Err(format!("AudioComponentInstanceNew failed ({status})"));
    }

    // Buses and the sample rate their format reports.
    let mut rate = 0.0;
    for (scope, dir) in [(AU_SCOPE_INPUT, "input"), (AU_SCOPE_OUTPUT, "output")] {
        let mut count: u32 = 0;
        let mut size = 4u32;
        let st = unsafe { AudioUnitGetProperty(unit, AU_PROP_ELEMENT_COUNT, scope, 0, (&mut count as *mut u32).cast(), &mut size) };
        if st != 0 {
            continue;
        }
        for el in 0..count.min(64) {
            let mut asbd = AudioStreamBasicDescription::default();
            let mut size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
            let st = unsafe { AudioUnitGetProperty(unit, AU_PROP_STREAM_FORMAT, scope, el, (&mut asbd as *mut AudioStreamBasicDescription).cast(), &mut size) };
            if st != 0 {
                continue;
            }
            if rate == 0.0 {
                rate = asbd.sample_rate;
            }
            plugin.buses.push(Bus {
                direction: dir.into(),
                kind: if el == 0 { "main" } else { "aux" }.into(),
                name: String::new(),
                channels: asbd.channels_per_frame,
                arrangement: None,
                layout: match asbd.channels_per_frame {
                    1 => Some("mono".to_string()),
                    2 => Some("stereo".to_string()),
                    _ => None,
                },
            });
        }
    }
    let rate = if rate > 0.0 { rate } else { 44_100.0 };

    // Channel layouts.
    let mut size = 0u32;
    let mut writable = 0u8;
    if unsafe { AudioUnitGetPropertyInfo(unit, AU_PROP_SUPPORTED_NUM_CHANNELS, AU_SCOPE_GLOBAL, 0, &mut size, &mut writable) } == 0 && size >= 4 {
        let n = (size / 4) as usize;
        let mut infos = vec![AUChannelInfo { in_channels: 0, out_channels: 0 }; n];
        if unsafe { AudioUnitGetProperty(unit, AU_PROP_SUPPORTED_NUM_CHANNELS, AU_SCOPE_GLOBAL, 0, infos.as_mut_ptr().cast(), &mut size) } == 0 {
            plugin.channel_layouts = infos.iter().map(|i| ChannelLayout { inputs: i32::from(i.in_channels), outputs: i32::from(i.out_channels) }).collect();
        }
    }

    // Latency and tail are seconds; initialising first lets units that size them from the format answer.
    let init = unsafe { AudioUnitInitialize(unit) };
    if init != 0 {
        plugin.warnings.push(format!("AudioUnitInitialize failed ({init}); latency and tail may be unset"));
    }
    let seconds = |prop: u32| -> Option<f64> {
        let mut v = 0f64;
        let mut size = 8u32;
        (unsafe { AudioUnitGetProperty(unit, prop, AU_SCOPE_GLOBAL, 0, (&mut v as *mut f64).cast(), &mut size) } == 0).then_some(v)
    };
    if let Some(l) = seconds(AU_PROP_LATENCY) {
        plugin.latency_samples = Some((l * rate).round().max(0.0) as u32);
        if l != 0.0 {
            plugin.warnings.push(format!("Audio Unit latency is reported in seconds; converted at {rate} Hz"));
        }
    }
    if let Some(t) = seconds(AU_PROP_TAIL_TIME) {
        plugin.tail_seconds = Some(t);
        plugin.tail_samples = Some(if t.is_finite() { (t * rate).round().max(0.0) as u32 } else { u32::MAX });
    }

    // Parameters.
    let mut size = 0u32;
    if unsafe { AudioUnitGetPropertyInfo(unit, AU_PROP_PARAMETER_LIST, AU_SCOPE_GLOBAL, 0, &mut size, &mut writable) } == 0 && size >= 4 {
        let mut ids = vec![0u32; (size / 4) as usize];
        if unsafe { AudioUnitGetProperty(unit, AU_PROP_PARAMETER_LIST, AU_SCOPE_GLOBAL, 0, ids.as_mut_ptr().cast(), &mut size) } == 0 {
            for id in ids {
                match read_param(unit, id) {
                    Some(p) => plugin.params.push(p),
                    None => plugin.warnings.push(format!("parameter {id}: ParameterInfo failed")),
                }
            }
        }
    }
    Ok(())
}

const UNIT_INDEXED: u32 = 1;
const UNIT_BOOLEAN: u32 = 2;

fn unit_text(unit: u32, custom: &str) -> String {
    match unit {
        3 => "%",
        4 => "s",
        5 => "samples",
        6 => "degrees",
        8 => "Hz",
        9 => "ct",
        10 => "st",
        13 => "dB",
        14 => "gain",
        15 => "degrees",
        19 => "m",
        22 => "BPM",
        24 => "ms",
        25 => ":1",
        26 => return custom.to_string(),
        _ => "",
    }
    .to_string()
}

fn read_param(unit: AudioComponentInstance, id: u32) -> Option<Param> {
    let mut info: AudioUnitParameterInfo = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<AudioUnitParameterInfo>() as u32;
    let st = unsafe { AudioUnitGetProperty(unit, AU_PROP_PARAMETER_INFO, AU_SCOPE_GLOBAL, id, (&mut info as *mut AudioUnitParameterInfo).cast(), &mut size) };
    if st != 0 {
        return None;
    }
    const HAS_CF_NAME: u32 = 1 << 27;
    let name = if info.flags & HAS_CF_NAME != 0 && !info.cf_name_string.is_null() {
        cf_to_string(info.cf_name_string)
    } else {
        let bytes: Vec<u8> = info.name.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let custom = cf_to_string(info.unit_name);
    let (min, max, default) = (f64::from(info.min_value), f64::from(info.max_value), f64::from(info.default_value));
    let discrete = info.unit == UNIT_INDEXED || info.unit == UNIT_BOOLEAN;
    let step_count = if discrete && max > min { (max - min).round() as u32 } else { 0 };

    let to_text = |value: f64| -> String {
        let v = value as f32;
        let mut q = AudioUnitParameterStringFromValue { param_id: id, value: &v, out_string: std::ptr::null() };
        let mut size = std::mem::size_of::<AudioUnitParameterStringFromValue>() as u32;
        let st = unsafe { AudioUnitGetProperty(unit, AU_PROP_PARAMETER_STRING_FROM_VALUE, AU_SCOPE_GLOBAL, 0, (&mut q as *mut AudioUnitParameterStringFromValue).cast(), &mut size) };
        if st != 0 || q.out_string.is_null() {
            return String::new();
        }
        let t = cf_to_string(q.out_string);
        unsafe { CFRelease(q.out_string) };
        t
    };
    let value_strings = indexed_strings(unit, id);

    let mut flags = Vec::new();
    let f = info.flags;
    if f & (1 << 31) != 0 {
        flags.push("automatable".to_string());
    } else {
        flags.push("read_only".to_string());
    }
    if discrete {
        flags.push("stepped".into());
    }
    if f & (1 << 15) != 0 {
        flags.push("meter".into());
    }
    if f & (1 << 24) != 0 {
        flags.push("non_realtime".into());
    }
    if f & (1 << 26) != 0 {
        flags.push("expert".into());
    }
    if f & (1 << 13) != 0 {
        flags.push("preset_exclude".into());
    }
    match f & ((7 << 16) | (1 << 22)) {
        x if x == (1 << 22) => flags.push("log_display".into()),
        x if x == (5 << 16) => flags.push("exp_display".into()),
        x if x == (1 << 16) => flags.push("sqrt_display".into()),
        x if x == (2 << 16) => flags.push("squared_display".into()),
        x if x == (3 << 16) => flags.push("cubed_display".into()),
        x if x == (4 << 16) => flags.push("cube_root_display".into()),
        _ => {}
    }

    // The sample points are linear in the plain value; an Audio Unit has no
    // other normalisation, only a display curve named in the flags.
    let samples = SAMPLE_POINTS
        .iter()
        .map(|&t| {
            let plain = min + (max - min) * t;
            let text = value_strings.as_ref().and_then(|v| v.get((plain - min).round() as usize).cloned()).unwrap_or_else(|| to_text(plain));
            Sample { normalized: t, plain, text }
        })
        .collect();
    let step_texts = if (1..MAX_STEP_TEXTS).contains(&step_count) {
        (0..=step_count)
            .map(|k| value_strings.as_ref().and_then(|v| v.get(k as usize).cloned()).unwrap_or_else(|| to_text(min + f64::from(k))))
            .collect()
    } else {
        Vec::new()
    };
    let span = max - min;
    Some(Param {
        id,
        name,
        short_name: None,
        group: if info.flags & (1 << 20) != 0 { Some(info.clump_id.to_string()) } else { None },
        units: unit_text(info.unit, &custom),
        min,
        max,
        default,
        default_normalized: if span > 0.0 { ((default - min) / span).clamp(0.0, 1.0) } else { 0.0 },
        step_count,
        flags,
        flags_raw: info.flags,
        samples,
        step_texts,
    })
}

/// `kAudioUnitProperty_ParameterValueStrings`: the names of an indexed parameter's values.
fn indexed_strings(unit: AudioComponentInstance, id: u32) -> Option<Vec<String>> {
    let mut array: *const c_void = std::ptr::null();
    let mut size = std::mem::size_of::<*const c_void>() as u32;
    let st = unsafe { AudioUnitGetProperty(unit, AU_PROP_PARAMETER_VALUE_STRINGS, AU_SCOPE_GLOBAL, id, (&mut array as *mut *const c_void).cast(), &mut size) };
    if st != 0 || array.is_null() {
        return None;
    }
    let n = unsafe { CFArrayGetCount(array) }.clamp(0, 4096);
    let out = (0..n).map(|i| cf_to_string(unsafe { CFArrayGetValueAtIndex(array, i) })).collect();
    unsafe { CFRelease(array) };
    Some(out)
}
