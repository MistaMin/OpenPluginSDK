//! Audio Unit state capture (a child module of the probe).
//!
//! A host saves a unit's state as the `kAudioUnitProperty_ClassInfo`
//! dictionary (a property list). The bytes stored here are that dictionary
//! serialised by CoreFoundation as a binary property list (`bplist00`), the
//! form hosts write into sessions and preset files. Factory presets are
//! `kAudioUnitProperty_FactoryPresets`; each is selected through
//! `kAudioUnitProperty_PresentPreset`, then the dictionary is read again.

use std::ffi::c_void;

use super::*;
use crate::capture::{Job, MAX_BLOB, ParamValue, Sink, resolve_sets};

fn register(job: &Job) -> Result<AudioComponent, String> {
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
    Ok(component)
}

/// A new initialised instance. The component is registered once per process, so each state gets its own instance.
fn new_unit(component: AudioComponent) -> Result<AudioComponentInstance, String> {
    let mut unit: AudioComponentInstance = std::ptr::null_mut();
    let st = unsafe { AudioComponentInstanceNew(component, &mut unit) };
    if st != 0 || unit.is_null() {
        return Err(format!("AudioComponentInstanceNew failed ({st})"));
    }
    let st = unsafe { AudioUnitInitialize(unit) };
    if st != 0 {
        return Err(format!("AudioUnitInitialize failed ({st})"));
    }
    Ok(unit)
}

fn param_table(unit: AudioComponentInstance) -> Vec<(u32, String)> {
    let mut size = 0u32;
    let mut writable = 0u8;
    if unsafe { AudioUnitGetPropertyInfo(unit, AU_PROP_PARAMETER_LIST, AU_SCOPE_GLOBAL, 0, &mut size, &mut writable) } != 0 || size < 4 {
        return Vec::new();
    }
    let mut ids = vec![0u32; (size / 4) as usize];
    if unsafe { AudioUnitGetProperty(unit, AU_PROP_PARAMETER_LIST, AU_SCOPE_GLOBAL, 0, ids.as_mut_ptr().cast(), &mut size) } != 0 {
        return Vec::new();
    }
    ids.into_iter().filter_map(|id| read_param(unit, id).map(|p| (id, p.name))).collect()
}

fn values(unit: AudioComponentInstance, table: &[(u32, String)]) -> Vec<ParamValue> {
    table
        .iter()
        .filter_map(|(id, name)| {
            let mut v = 0f32;
            (unsafe { AudioUnitGetParameter(unit, *id, AU_SCOPE_GLOBAL, 0, &mut v) } == 0).then(|| ParamValue { id: *id, name: name.clone(), plain: f64::from(v), normalized: None })
        })
        .collect()
}

/// The class-info dictionary as a binary property list.
fn class_info(unit: AudioComponentInstance) -> Result<Vec<u8>, String> {
    let mut dict: CFTypeRef = std::ptr::null();
    let mut size = std::mem::size_of::<CFTypeRef>() as u32;
    let st = unsafe { AudioUnitGetProperty(unit, AU_PROP_CLASS_INFO, AU_SCOPE_GLOBAL, 0, (&mut dict as *mut CFTypeRef).cast(), &mut size) };
    if st != 0 || dict.is_null() {
        return Err(format!("reading ClassInfo failed ({st})"));
    }
    let mut error: CFTypeRef = std::ptr::null();
    let data = unsafe { CFPropertyListCreateData(std::ptr::null(), dict, K_CF_PROPERTY_LIST_BINARY_FORMAT, 0, &mut error) };
    unsafe { CFRelease(dict) };
    if data.is_null() {
        if !error.is_null() {
            unsafe { CFRelease(error) };
        }
        return Err("the ClassInfo dictionary would not serialise".into());
    }
    let len = unsafe { CFDataGetLength(data) }.max(0) as usize;
    let out = if len > MAX_BLOB { Err("the state is larger than the blob limit".to_string()) } else { Ok(unsafe { std::slice::from_raw_parts(CFDataGetBytePtr(data), len) }.to_vec()) };
    unsafe { CFRelease(data) };
    out
}

pub fn capture(job: &Job, sink: &mut Sink) -> Result<(), String> {
    let component = register(job)?;
    let unit = new_unit(component)?;
    let table = param_table(unit);
    match class_info(unit) {
        Ok(b) => sink.add("default", "default", None, vec![("au-classinfo", b)], values(unit, &table), Vec::new())?,
        Err(e) => sink.notes.push(format!("default state: {e}")),
    }

    if !job.sets.is_empty() {
        let u = new_unit(component)?;
        for (id, v) in resolve_sets(&job.sets, &table)? {
            let st = unsafe { AudioUnitSetParameter(u, id, AU_SCOPE_GLOBAL, 0, v as f32, 0) };
            if st != 0 {
                return Err(format!("AudioUnitSetParameter({id}) failed ({st})"));
            }
        }
        match class_info(u) {
            Ok(b) => sink.add("set", "set", None, vec![("au-classinfo", b)], values(u, &table), Vec::new())?,
            Err(e) => sink.notes.push(format!("state after --set: {e}")),
        }
    }

    // A saved session restored the way a host does: the ClassInfo dictionary set on a new unit.
    if let Some(path) = &job.load_component {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let u = new_unit(component)?;
        let data = unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize) };
        if data.is_null() {
            return Err("the session file would not load into memory".into());
        }
        let mut error: CFTypeRef = std::ptr::null();
        let dict = unsafe { CFPropertyListCreateWithData(std::ptr::null(), data, 0, std::ptr::null_mut(), &mut error) };
        unsafe { CFRelease(data) };
        if dict.is_null() {
            if !error.is_null() {
                unsafe { CFRelease(error) };
            }
            return Err(format!("{} is not a property list", path.display()));
        }
        let st = unsafe { AudioUnitSetProperty(u, AU_PROP_CLASS_INFO, AU_SCOPE_GLOBAL, 0, (&dict as *const CFTypeRef).cast(), std::mem::size_of::<CFTypeRef>() as u32) };
        unsafe { CFRelease(dict) };
        let notes = vec![format!("setting ClassInfo returned {st}")];
        match class_info(u) {
            Ok(b) => sink.add("loaded", "loaded", None, vec![("au-classinfo", b)], values(u, &table), notes)?,
            Err(e) => sink.notes.push(format!("state after loading: {e}")),
        }
    }

    // Factory presets: an array of pointers to AUPreset structures.
    let mut array: *const c_void = std::ptr::null();
    let mut size = std::mem::size_of::<*const c_void>() as u32;
    let st = unsafe { AudioUnitGetProperty(unit, AU_PROP_FACTORY_PRESETS, AU_SCOPE_GLOBAL, 0, (&mut array as *mut *const c_void).cast(), &mut size) };
    if st != 0 || array.is_null() {
        sink.notes.push("the unit has no factory presets".into());
        return Ok(());
    }
    let n = unsafe { CFArrayGetCount(array) }.clamp(0, 100_000) as usize;
    let inst = new_unit(component)?;
    if n > job.max_presets {
        sink.notes.push(format!("{n} factory presets found; the first {} captured (--max-presets)", job.max_presets));
    }
    for i in 0..n.min(job.max_presets) {
        let p = unsafe { CFArrayGetValueAtIndex(array, i as isize) } as *const AUPreset;
        if p.is_null() {
            continue;
        }
        let preset = unsafe { *p };
        let name = cf_to_string(preset.preset_name);
        let st = unsafe { AudioUnitSetProperty(inst, AU_PROP_PRESENT_PRESET, AU_SCOPE_GLOBAL, 0, (&preset as *const AUPreset).cast(), std::mem::size_of::<AUPreset>() as u32) };
        if st != 0 {
            sink.notes.push(format!("preset {name:?} would not load ({st})"));
            continue;
        }
        match class_info(inst) {
            Ok(b) => sink.add(&name, "factory-preset", Some(format!("factory-preset:{}", preset.preset_number)), vec![("au-classinfo", b)], values(inst, &table), Vec::new())?,
            Err(e) => sink.notes.push(format!("preset {name:?}: {e}")),
        }
    }
    unsafe { CFRelease(array) };
    Ok(())
}
