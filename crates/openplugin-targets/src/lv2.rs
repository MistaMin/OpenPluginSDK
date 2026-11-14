//! LV2 `lv2_descriptor` for one stereo insert, its editor, and the two
//! description files a host reads.
//!
//! Ports: 0 and 1 are the audio inputs, 2 and 3 the audio outputs (the map
//! OpenGridder's shell uses), and every parameter is a control input from
//! port 4 on, in descriptor order. `run` stores each connected control port
//! in the [`ParamPlane`] and the plugin pulls the slots that changed, so it
//! does not lock. The control ports are the whole state, as the host saves
//! them.
//!
//! [`export_lv2!`] emits `lv2_descriptor` from a cdylib, and with
//! `editor = make_editor` also `lv2ui_descriptor`: a `ui:CocoaUI` on macOS.
//! Its gestures are control-port writes plus `ui:touch` grabs, values come
//! back through `port_event`, and with `instance-access` it is bound to the
//! engine's [`EditorState`]. [`manifest_ttl`] and [`plugin_ttl`] write the
//! description files from the parameter table ([`write_bundle`]).
//! [`Lv2Harness`] drives the same render path in-process.

use std::ffi::{CStr, CString, c_char, c_void};
use std::fmt::Write as _;
use std::sync::{Mutex, OnceLock};

use openplugin_core::{
    Category, EditorFactory, EditorState, MeterFrame, ParamCursor, ParamPlane, Plugin, Seqlock,
    Taper, Unit,
};

type CreateFn = fn() -> Box<dyn Plugin>;

static CREATOR: Mutex<Option<CreateFn>> = Mutex::new(None);
static EDITOR: Mutex<Option<EditorFactory>> = Mutex::new(None);

/// Audio ports in front of the control ports.
pub const AUDIO_PORTS: u32 = 4;

/// Install the plugin `lv2_descriptor` will instantiate.
pub fn set_creator(create: CreateFn) {
    *CREATOR.lock().expect("lv2 creator lock") = Some(create);
}

fn creator() -> Option<CreateFn> {
    *CREATOR.lock().expect("lv2 creator lock")
}

/// Install the editor `lv2ui_descriptor` will open.
pub fn set_editor(factory: EditorFactory) {
    if let Ok(mut slot) = EDITOR.lock() {
        *slot = Some(factory);
    }
}

fn editor_factory() -> Option<EditorFactory> {
    EDITOR.lock().ok().and_then(|slot| *slot)
}

#[repr(C)]
pub struct Lv2Feature {
    pub uri: *const c_char,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct Lv2Descriptor {
    pub uri: *const c_char,
    pub instantiate: extern "C" fn(*const Lv2Descriptor, f64, *const c_char, *const *const Lv2Feature) -> *mut c_void,
    pub connect_port: extern "C" fn(*mut c_void, u32, *mut c_void),
    pub activate: extern "C" fn(*mut c_void),
    pub run: extern "C" fn(*mut c_void, u32),
    pub deactivate: extern "C" fn(*mut c_void),
    pub cleanup: extern "C" fn(*mut c_void),
    pub extension_data: extern "C" fn(*const c_char) -> *const c_void,
}

// The URI points at a process-lifetime `CString`. The function pointers do not
// capture thread-local state. The descriptor is published once and then only read.
unsafe impl Send for Lv2Descriptor {}
unsafe impl Sync for Lv2Descriptor {}

/// Marks an [`Instance`] of this binary, so an editor given an
/// `instance-access` handle can tell one of ours from anything else.
const MAGIC: u64 = 0x4f50_4c56_3249_4e53;

struct Instance {
    magic: u64,
    inner: Box<dyn Plugin>,
    plane: ParamPlane,
    cursor: ParamCursor,
    meters: Seqlock<MeterFrame>,
    ports: [*mut f32; 4],
    controls: Vec<*const f32>,
    scratch: [Vec<f32>; 2],
    prepared: usize,
    editor_state: Option<EditorState>,
}

const PREPARED: usize = 8192;

/// The plugin's URI: [`Plugin::lv2_uri`], else its id.
pub fn plugin_uri(plugin: &dyn Plugin) -> &'static str {
    plugin.lv2_uri().unwrap_or(plugin.info().id)
}

fn uri() -> &'static CStr {
    static URI: OnceLock<CString> = OnceLock::new();
    URI.get_or_init(|| {
        let id = creator().and_then(|create| CString::new(plugin_uri(create().as_ref())).ok());
        id.unwrap_or_else(|| CString::new("urn:openplugin:plugin").expect("static uri"))
    })
    .as_c_str()
}

/// Parameters of the installed plugin, read once.
fn param_count() -> usize {
    static COUNT: OnceLock<usize> = OnceLock::new();
    *COUNT.get_or_init(|| creator().map(|create| create().info().params.len()).unwrap_or(0))
}

fn new_instance(sample_rate: f64) -> Option<Box<Instance>> {
    let create = creator()?;
    let mut inner = create();
    let info = inner.info();
    let plane = ParamPlane::new(info);
    plane.seed_from(inner.as_ref());
    let mut cursor = ParamCursor::new(plane.len());
    let rate = if sample_rate.is_finite() && sample_rate >= 8_000.0 {
        sample_rate as f32
    } else {
        48_000.0
    };
    inner.prepare(rate, PREPARED, 2);
    cursor.invalidate();
    cursor.pull(&plane, inner.as_mut());
    let editor_state = inner.editor_state();
    let controls = vec![std::ptr::null(); plane.len()];
    Some(Box::new(Instance {
        magic: MAGIC,
        inner,
        plane,
        cursor,
        meters: Seqlock::new(MeterFrame::SILENT),
        ports: [std::ptr::null_mut(); 4],
        controls,
        scratch: [vec![0.0; PREPARED], vec![0.0; PREPARED]],
        prepared: PREPARED,
        editor_state,
    }))
}

/// Copy the connected control ports into the plane. A port the host never
/// connected keeps the plugin's value.
fn read_controls(instance: &mut Instance) {
    for (index, port) in instance.controls.iter().enumerate() {
        if !port.is_null() {
            instance.plane.store(index, unsafe { **port });
        }
    }
}

fn render(instance: &mut Instance, frames: usize) {
    let frames = frames.min(instance.prepared);
    if frames == 0 {
        return;
    }
    instance.cursor.pull(&instance.plane, instance.inner.as_mut());
    for channel in 0..2 {
        let src = instance.ports[channel];
        if src.is_null() {
            instance.scratch[channel][..frames].fill(0.0);
        } else {
            let samples = unsafe { std::slice::from_raw_parts(src, frames) };
            instance.scratch[channel][..frames].copy_from_slice(samples);
        }
    }
    instance.inner.process(&mut instance.scratch, frames);
    for channel in 0..2 {
        let dst = instance.ports[channel + 2];
        if !dst.is_null() {
            let out = unsafe { std::slice::from_raw_parts_mut(dst, frames) };
            out.copy_from_slice(&instance.scratch[channel][..frames]);
        }
    }
    instance.meters.publish(instance.inner.meters());
}

extern "C" fn lv2_instantiate(
    _descriptor: *const Lv2Descriptor,
    sample_rate: f64,
    _bundle_path: *const c_char,
    _features: *const *const Lv2Feature,
) -> *mut c_void {
    match new_instance(sample_rate) {
        Some(instance) => Box::into_raw(instance) as *mut c_void,
        None => std::ptr::null_mut(),
    }
}

extern "C" fn lv2_connect(instance: *mut c_void, port: u32, data: *mut c_void) {
    if instance.is_null() {
        return;
    }
    let instance = unsafe { &mut *(instance as *mut Instance) };
    if port < AUDIO_PORTS {
        instance.ports[port as usize] = data as *mut f32;
    } else if let Some(slot) = instance.controls.get_mut((port - AUDIO_PORTS) as usize) {
        *slot = data as *const f32;
    }
}

extern "C" fn lv2_activate(instance: *mut c_void) {
    if instance.is_null() {
        return;
    }
    unsafe {
        let instance = &mut *(instance as *mut Instance);
        read_controls(instance);
        instance.inner.reset();
        instance.cursor.invalidate();
        instance.cursor.pull(&instance.plane, instance.inner.as_mut());
    }
}

extern "C" fn lv2_run(instance: *mut c_void, sample_count: u32) {
    if instance.is_null() || sample_count == 0 {
        return;
    }
    unsafe {
        let instance = &mut *(instance as *mut Instance);
        read_controls(instance);
        let mut done = 0;
        let frames = sample_count as usize;
        while done < frames {
            let n = (frames - done).min(instance.prepared);
            let saved = instance.ports;
            for port in &mut instance.ports {
                if !port.is_null() {
                    *port = port.add(done);
                }
            }
            render(instance, n);
            instance.ports = saved;
            done += n;
        }
    }
}

extern "C" fn lv2_deactivate(instance: *mut c_void) {
    if instance.is_null() {
        return;
    }
    unsafe {
        (*(instance as *mut Instance)).inner.reset();
    }
}

extern "C" fn lv2_cleanup(instance: *mut c_void) {
    if instance.is_null() {
        return;
    }
    unsafe {
        let mut instance = Box::from_raw(instance as *mut Instance);
        instance.magic = 0;
        drop(instance);
    }
}

extern "C" fn lv2_extension(_uri: *const c_char) -> *const c_void {
    std::ptr::null()
}

fn published() -> &'static Lv2Descriptor {
    static DESCRIPTOR: OnceLock<Lv2Descriptor> = OnceLock::new();
    DESCRIPTOR.get_or_init(|| Lv2Descriptor {
        uri: uri().as_ptr(),
        instantiate: lv2_instantiate,
        connect_port: lv2_connect,
        activate: lv2_activate,
        run: lv2_run,
        deactivate: lv2_deactivate,
        cleanup: lv2_cleanup,
        extension_data: lv2_extension,
    })
}

/// `lv2_descriptor`. Index 0 is the plugin. Call [`set_creator`] first.
pub fn descriptor(index: u32) -> *const Lv2Descriptor {
    if index == 0 && creator().is_some() {
        published()
    } else {
        std::ptr::null()
    }
}

/// The editor state of an `instance-access` handle, if it is a live
/// instance of this binary.
///
/// # Safety
/// `handle` is null or the handle a host passes as `instance-access`, which
/// is a plugin instance in the same process.
unsafe fn instance_state(handle: *const c_void) -> Option<EditorState> {
    let instance = unsafe { (handle as *const Instance).as_ref()? };
    if instance.magic != MAGIC {
        return None;
    }
    instance.editor_state.clone()
}

#[cfg(target_os = "macos")]
pub use ui::{lv2ui_descriptor, Lv2UiDescriptor};

#[cfg(target_os = "macos")]
mod ui {
    //! `ui:CocoaUI`: the editor in an `NSView`, handed to the host as the widget.
    use super::*;
    use openplugin_core::{EditorHost, Gesture};
    use openplugin_dri::cocoa::EditorView;

    pub type WriteFn = unsafe extern "C" fn(controller: *mut c_void, port: u32, size: u32, protocol: u32, buffer: *const c_void);

    #[repr(C)]
    pub struct Lv2UiDescriptor {
        pub uri: *const c_char,
        pub instantiate: unsafe extern "C" fn(
            *const Lv2UiDescriptor,
            *const c_char,
            *const c_char,
            Option<WriteFn>,
            *mut c_void,
            *mut *mut c_void,
            *const *const Lv2Feature,
        ) -> *mut c_void,
        pub cleanup: unsafe extern "C" fn(*mut c_void),
        pub port_event: unsafe extern "C" fn(*mut c_void, u32, u32, u32, *const c_void),
        pub extension_data: unsafe extern "C" fn(*const c_char) -> *const c_void,
    }

    unsafe impl Send for Lv2UiDescriptor {}
    unsafe impl Sync for Lv2UiDescriptor {}

    #[repr(C)]
    struct Touch {
        handle: *mut c_void,
        touch: Option<unsafe extern "C" fn(handle: *mut c_void, port: u32, grabbed: bool)>,
    }

    #[repr(C)]
    struct Resize {
        handle: *mut c_void,
        ui_resize: Option<unsafe extern "C" fn(handle: *mut c_void, width: i32, height: i32) -> i32>,
    }

    struct Ui {
        view: EditorView,
        count: usize,
    }

    #[derive(Clone, Copy)]
    struct Link {
        write: Option<WriteFn>,
        controller: *mut c_void,
        touch: *const Touch,
    }

    fn host_for(link: Link) -> EditorHost {
        EditorHost::new(move |gesture| unsafe {
            let link = link;
            match gesture {
                Gesture::Begin(i) | Gesture::End(i) => {
                    if let Some(t) = link.touch.as_ref() {
                        if let Some(f) = t.touch {
                            f(t.handle, AUDIO_PORTS + i, matches!(gesture, Gesture::Begin(_)));
                        }
                    }
                }
                Gesture::Perform(i, value) => {
                    if let Some(write) = link.write {
                        // Protocol 0: a float written to a control port.
                        write(link.controller, AUDIO_PORTS + i, 4, 0, &value as *const f32 as *const c_void);
                    }
                }
            }
        })
    }

    fn ui_uri() -> &'static CStr {
        static URI: OnceLock<CString> = OnceLock::new();
        URI.get_or_init(|| CString::new(format!("{}#ui", uri().to_string_lossy())).unwrap_or_default())
            .as_c_str()
    }

    unsafe extern "C" fn instantiate(
        _descriptor: *const Lv2UiDescriptor,
        plugin_uri: *const c_char,
        _bundle_path: *const c_char,
        write: Option<WriteFn>,
        controller: *mut c_void,
        widget: *mut *mut c_void,
        features: *const *const Lv2Feature,
    ) -> *mut c_void {
        let Some(factory) = editor_factory() else { return std::ptr::null_mut() };
        if plugin_uri.is_null() || unsafe { CStr::from_ptr(plugin_uri) } != uri() {
            return std::ptr::null_mut();
        }
        let mut parent = std::ptr::null_mut();
        let mut instance: *const c_void = std::ptr::null();
        let mut touch: *const Touch = std::ptr::null();
        let mut resize: *const Resize = std::ptr::null();
        let mut next = features;
        while !next.is_null() && !unsafe { *next }.is_null() {
            let feature = unsafe { &**next };
            if !feature.uri.is_null() {
                match unsafe { CStr::from_ptr(feature.uri) }.to_bytes() {
                    b"http://lv2plug.in/ns/extensions/ui#parent" => parent = feature.data,
                    b"http://lv2plug.in/ns/ext/instance-access" => instance = feature.data,
                    b"http://lv2plug.in/ns/extensions/ui#touch" => touch = feature.data as *const Touch,
                    b"http://lv2plug.in/ns/extensions/ui#resize" => resize = feature.data as *const Resize,
                    _ => {}
                }
            }
            next = unsafe { next.add(1) };
        }
        let Some(mut editor) = factory(host_for(Link { write, controller, touch })) else {
            return std::ptr::null_mut();
        };
        editor.bind(unsafe { instance_state(instance) });
        let count = param_count();
        let Some(view) = EditorView::new(editor, None) else { return std::ptr::null_mut() };
        if !parent.is_null() {
            unsafe { view.attach(parent) };
        }
        if let Some(r) = unsafe { resize.as_ref() } {
            if let Some(f) = r.ui_resize {
                let (w, h) = view.size();
                unsafe { f(r.handle, w as i32, h as i32) };
            }
        }
        if !widget.is_null() {
            unsafe { *widget = view.raw() };
        }
        Box::into_raw(Box::new(Ui { view, count })) as *mut c_void
    }

    unsafe extern "C" fn cleanup(handle: *mut c_void) {
        if !handle.is_null() {
            drop(unsafe { Box::from_raw(handle as *mut Ui) });
        }
    }

    unsafe extern "C" fn port_event(handle: *mut c_void, port: u32, size: u32, format: u32, buffer: *const c_void) {
        let Some(ui) = (unsafe { (handle as *mut Ui).as_mut() }) else { return };
        if format != 0 || size != 4 || buffer.is_null() || port < AUDIO_PORTS {
            return;
        }
        let index = (port - AUDIO_PORTS) as usize;
        let value = unsafe { *(buffer as *const f32) };
        if index >= ui.count || !value.is_finite() {
            return;
        }
        if let Some(editor) = ui.view.editor() {
            editor.param_changed(index, value);
        }
    }

    unsafe extern "C" fn extension_data(_uri: *const c_char) -> *const c_void {
        std::ptr::null()
    }

    fn published() -> &'static Lv2UiDescriptor {
        static DESCRIPTOR: OnceLock<Lv2UiDescriptor> = OnceLock::new();
        DESCRIPTOR.get_or_init(|| Lv2UiDescriptor { uri: ui_uri().as_ptr(), instantiate, cleanup, port_event, extension_data })
    }

    /// `lv2ui_descriptor`. Index 0 is the editor, once [`set_editor`] ran.
    pub fn lv2ui_descriptor(index: u32) -> *const Lv2UiDescriptor {
        if index == 0 && creator().is_some() && editor_factory().is_some() {
            published()
        } else {
            std::ptr::null()
        }
    }
}

/// The plugin library's file name in an LV2 bundle on this system.
pub fn binary_file(stem: &str) -> String {
    let ext = if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(target_os = "windows") {
        "dll"
    } else {
        "so"
    };
    format!("{stem}.{ext}")
}

fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A number the way Turtle reads a decimal: always a point, never an exponent.
fn number(v: f32) -> String {
    let text = format!("{v}");
    if text.contains('.') || text.contains("inf") || text.contains("NaN") {
        text
    } else {
        format!("{text}.0")
    }
}

/// A port symbol: letters, digits and underscores, not starting with a digit.
pub fn symbol(id: &str) -> String {
    let mut s: String = id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if s.chars().next().map_or(true, |c| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

fn class_of(category: Category, instrument: bool) -> &'static str {
    if instrument {
        return "lv2:InstrumentPlugin";
    }
    match category {
        Category::Eq => "lv2:EQPlugin",
        Category::Dynamics => "lv2:DynamicsPlugin",
        Category::PitchShift => "lv2:PitchPlugin",
        Category::Reverb => "lv2:ReverbPlugin",
        Category::Delay => "lv2:DelayPlugin",
        Category::Modulation => "lv2:ModulatorPlugin",
        Category::Harmonic => "lv2:DistortionPlugin",
        Category::Instrument => "lv2:InstrumentPlugin",
        Category::Dither | Category::Utility => "lv2:UtilityPlugin",
        Category::Other => "lv2:Plugin",
    }
}

/// `minor.micro` for LV2 from a version such as `1.2.3-beta`. LV2 orders
/// releases by these two numbers only.
fn lv2_version(version: &str) -> (u32, u32) {
    let numbers: Vec<u32> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    (numbers.get(1).copied().unwrap_or(0), numbers.get(2).copied().unwrap_or(0))
}

/// `manifest.ttl`: what a host reads to find the plugin and its editor.
/// `binary_stem` is the library's name in the bundle without its extension.
pub fn manifest_ttl(plugin: &dyn Plugin, binary_stem: &str, with_ui: bool) -> String {
    let uri = plugin_uri(plugin);
    let binary = binary_file(binary_stem);
    let mut out = String::new();
    out.push_str("@prefix lv2:  <http://lv2plug.in/ns/lv2core#> .\n");
    out.push_str("@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n");
    out.push_str("@prefix ui:   <http://lv2plug.in/ns/extensions/ui#> .\n\n");
    let _ = writeln!(out, "<{uri}>\n    a lv2:Plugin ;\n    lv2:binary <{binary}> ;\n    rdfs:seeAlso <{binary_stem}.ttl> .");
    if with_ui {
        let _ = writeln!(out, "\n<{uri}#ui>\n    a ui:CocoaUI ;\n    ui:binary <{binary}> ;\n    rdfs:seeAlso <{binary_stem}.ttl> .");
    }
    out
}

/// `<binary_stem>.ttl`: the plugin, its ports, and its editor.
pub fn plugin_ttl(plugin: &dyn Plugin, with_ui: bool) -> String {
    let info = plugin.info();
    let uri = plugin_uri(plugin);
    let mut out = String::new();
    for (prefix, iri) in [
        ("doap", "http://usefulinc.com/ns/doap#"),
        ("foaf", "http://xmlns.com/foaf/0.1/"),
        ("lv2", "http://lv2plug.in/ns/lv2core#"),
        ("pprop", "http://lv2plug.in/ns/ext/port-props#"),
        ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
        ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
        ("ui", "http://lv2plug.in/ns/extensions/ui#"),
        ("units", "http://lv2plug.in/ns/extensions/units#"),
    ] {
        let _ = writeln!(out, "@prefix {prefix}: <{iri}> .");
    }
    out.push('\n');
    let (minor, micro) = lv2_version(info.version);
    let _ = writeln!(out, "<{uri}>");
    let _ = writeln!(out, "    a {} , doap:Project ;", class_of(info.category, info.is_instrument));
    let _ = writeln!(out, "    doap:name {} ;", quoted(info.name));
    if info.url.is_empty() {
        let _ = writeln!(out, "    doap:maintainer [ foaf:name {} ] ;", quoted(info.vendor));
    } else {
        let _ = writeln!(out, "    doap:maintainer [ foaf:name {} ; foaf:homepage <{}> ] ;", quoted(info.vendor), info.url);
    }
    let _ = writeln!(out, "    rdfs:comment {} ;", quoted(&format!("{} {}", info.name, info.version)));
    let _ = writeln!(out, "    lv2:minorVersion {minor} ;");
    let _ = writeln!(out, "    lv2:microVersion {micro} ;");
    out.push_str("    lv2:optionalFeature lv2:hardRTCapable ;\n");
    if with_ui {
        let _ = writeln!(out, "    ui:ui <{uri}#ui> ;");
    }
    out.push_str("    lv2:port\n");
    let audio = [
        ("lv2:InputPort", "in_l", "Input L"),
        ("lv2:InputPort", "in_r", "Input R"),
        ("lv2:OutputPort", "out_l", "Output L"),
        ("lv2:OutputPort", "out_r", "Output R"),
    ];
    let mut ports: Vec<String> = audio
        .iter()
        .enumerate()
        .map(|(index, (kind, sym, name))| {
            format!("    [ a lv2:AudioPort , {kind} ; lv2:index {index} ; lv2:symbol \"{sym}\" ; lv2:name \"{name}\" ]")
        })
        .collect();
    for (i, p) in info.params.iter().enumerate() {
        let mut lines = vec![
            "    [ a lv2:ControlPort , lv2:InputPort".to_string(),
            format!("      lv2:index {} ; lv2:symbol \"{}\" ; lv2:name {}", AUDIO_PORTS as usize + i, symbol(p.id), quoted(p.name)),
            format!("      lv2:default {} ; lv2:minimum {} ; lv2:maximum {}", number(p.clamp(p.default)), number(p.min), number(p.max)),
        ];
        let whole = |v: f32| v.fract() == 0.0;
        if p.unit == Unit::Toggle {
            lines.push("      lv2:portProperty lv2:toggled".into());
        } else if p.unit == Unit::Choice {
            lines.push("      lv2:portProperty lv2:integer , lv2:enumeration".into());
            for (value, label) in p.choices.iter().enumerate() {
                lines.push(format!("      lv2:scalePoint [ rdfs:label {} ; rdf:value {} ]", quoted(label), number(value as f32)));
            }
        } else if p.steps > 1 && whole(p.min) && (p.max - p.min) == (p.steps - 1) as f32 {
            lines.push("      lv2:portProperty lv2:integer".into());
        } else if p.taper == Taper::Log && p.min > 0.0 {
            lines.push("      lv2:portProperty pprop:logarithmic".into());
        }
        let unit = match p.unit {
            Unit::Seconds => Some("units:s"),
            Unit::Hz => Some("units:hz"),
            Unit::Db => Some("units:db"),
            Unit::Ms => Some("units:ms"),
            Unit::Percent => Some("units:pc"),
            Unit::Semitones => Some("units:semitone12TET"),
            Unit::Cents => Some("units:cent"),
            _ => None,
        };
        if let Some(unit) = unit {
            lines.push(format!("      units:unit {unit}"));
        }
        ports.push(format!("{} ]", lines.join(" ;\n")));
    }
    out.push_str(&ports.join(" ,\n"));
    out.push_str(" .\n");
    if with_ui {
        let _ = writeln!(out, "\n<{uri}#ui>");
        out.push_str("    a ui:CocoaUI ;\n");
        out.push_str(
            "    lv2:optionalFeature <http://lv2plug.in/ns/ext/instance-access> , ui:parent , ui:touch , ui:resize , ui:noUserResize .\n",
        );
    }
    out
}

/// Write `manifest.ttl` and `<binary_stem>.ttl` into an LV2 bundle folder.
/// The editor is described on macOS when `with_ui` is set.
pub fn write_bundle(plugin: &dyn Plugin, dir: &std::path::Path, binary_stem: &str, with_ui: bool) -> std::io::Result<()> {
    let with_ui = with_ui && cfg!(target_os = "macos");
    std::fs::write(dir.join("manifest.ttl"), manifest_ttl(plugin, binary_stem, with_ui))?;
    std::fs::write(dir.join(format!("{binary_stem}.ttl")), plugin_ttl(plugin, with_ui))
}

/// In-process stereo LV2 run. Ports are owned vectors: 0 and 1 in, 2 and 3 out.
pub struct Lv2Harness<P: Plugin> {
    inner: P,
    plane: ParamPlane,
    cursor: ParamCursor,
    meters: Seqlock<MeterFrame>,
    ports: [Vec<f32>; 4],
    scratch: [Vec<f32>; 2],
}

impl<P: Plugin> Lv2Harness<P> {
    pub fn new(mut plugin: P, sample_rate: f32) -> Self {
        let info = plugin.info();
        let plane = ParamPlane::new(info);
        plane.seed_from(&plugin);
        let mut cursor = ParamCursor::new(plane.len());
        let rate = if sample_rate.is_finite() && sample_rate >= 8_000.0 {
            sample_rate
        } else {
            48_000.0
        };
        plugin.prepare(rate, PREPARED, 2);
        cursor.invalidate();
        cursor.pull(&plane, &mut plugin);
        Self {
            inner: plugin,
            plane,
            cursor,
            meters: Seqlock::new(MeterFrame::SILENT),
            ports: [vec![0.0; PREPARED], vec![0.0; PREPARED], vec![0.0; PREPARED], vec![0.0; PREPARED]],
            scratch: [vec![0.0; PREPARED], vec![0.0; PREPARED]],
        }
    }

    pub fn plane(&self) -> &ParamPlane {
        &self.plane
    }

    pub fn meters(&self) -> MeterFrame {
        self.meters.read()
    }

    pub fn plugin(&self) -> &P {
        &self.inner
    }

    pub fn input(&mut self, channel: usize) -> &mut [f32] {
        &mut self.ports[channel.min(1)]
    }

    pub fn output(&self, channel: usize) -> &[f32] {
        &self.ports[2 + channel.min(1)]
    }

    pub fn run(&mut self, frames: usize) {
        let frames = frames.min(PREPARED);
        self.cursor.pull(&self.plane, &mut self.inner);
        for channel in 0..2 {
            self.scratch[channel][..frames].copy_from_slice(&self.ports[channel][..frames]);
        }
        self.inner.process(&mut self.scratch, frames);
        for channel in 0..2 {
            self.ports[channel + 2][..frames].copy_from_slice(&self.scratch[channel][..frames]);
        }
        self.meters.publish(self.inner.meters());
    }
}

/// Export `lv2_descriptor` from a cdylib. `$plugin` implements [`Plugin`] and [`Default`].
///
/// `export_lv2!(P, editor = make_editor)` also exports `lv2ui_descriptor`
/// (a `ui:CocoaUI`, macOS). Describe it with [`write_bundle`]`(.., true)`.
#[macro_export]
macro_rules! export_lv2 {
    ($plugin:ty, editor = $editor:path) => {
        $crate::export_lv2!($plugin);

        #[cfg(target_os = "macos")]
        #[no_mangle]
        pub extern "C" fn lv2ui_descriptor(index: u32) -> *const $crate::lv2::Lv2UiDescriptor {
            $crate::lv2::set_creator(__openplugin_lv2_create);
            $crate::lv2::set_editor($editor);
            $crate::lv2::lv2ui_descriptor(index)
        }
    };
    ($plugin:ty) => {
        fn __openplugin_lv2_create() -> ::std::boxed::Box<dyn $crate::PluginForExport> {
            ::std::boxed::Box::new(<$plugin as ::std::default::Default>::default())
        }

        #[no_mangle]
        pub extern "C" fn lv2_descriptor(index: u32) -> *const $crate::lv2::Lv2Descriptor {
            $crate::lv2::set_creator(__openplugin_lv2_create);
            $crate::lv2::descriptor(index)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use openplugin_core::{Category, ParamInfo, PluginInfo, Unit};

    struct Gain {
        db: f32,
        linear: f32,
    }

    static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];

    static INFO: PluginInfo = PluginInfo {
        id: "urn:openplugin:test-gain",
        name: "Test Gain",
        vendor: "OpenGrid",
        url: "https://github.com/MistaMin/OpenPluginSDK",
        email: "contact@mistamin.com",
        version: "0.1.0-beta.1",
        category: Category::Utility,
        params: &PARAMS,
        is_instrument: false,
        has_editor: false,
    };

    impl Plugin for Gain {
        fn info(&self) -> &'static PluginInfo {
            &INFO
        }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {
            self.linear = 10f32.powf(self.db / 20.0);
        }
        fn reset(&mut self) {}
        fn set_param(&mut self, id: &str, value: f32) -> bool {
            if id != "gain" {
                return false;
            }
            self.db = PARAMS[0].clamp(value);
            self.linear = 10f32.powf(self.db / 20.0);
            true
        }
        fn param(&self, id: &str) -> Option<f32> {
            (id == "gain").then_some(self.db)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            for channel in io {
                for sample in &mut channel[..frames] {
                    *sample *= self.linear;
                }
            }
        }
    }

    #[test]
    fn harness_pulls_the_plane_before_audio() {
        let mut harness = Lv2Harness::new(Gain { db: 0.0, linear: 1.0 }, 48_000.0);
        harness.input(0).fill(0.5);
        harness.input(1).fill(0.25);
        assert!(harness.plane().store(0, 6.0));
        harness.run(8);
        let expected = 0.5 * 10f32.powf(6.0 / 20.0);
        assert!((harness.output(0)[0] - expected).abs() < 1e-4);
        assert!((harness.output(1)[3] - 0.25 * 10f32.powf(6.0 / 20.0)).abs() < 1e-4);
    }

    #[test]
    fn control_ports_reach_the_plugin_and_audio_runs_in_place() {
        set_creator(|| Box::new(Gain { db: 0.0, linear: 1.0 }));
        let desc = descriptor(0);
        unsafe {
            let raw = ((*desc).instantiate)(desc, 48_000.0, std::ptr::null(), std::ptr::null());
            assert!(!raw.is_null());
            let mut audio = vec![0.5f32; 10_000];
            let gain: *mut f32 = Box::into_raw(Box::new(6.0f32));
            ((*desc).connect_port)(raw, 0, audio.as_mut_ptr() as *mut c_void);
            ((*desc).connect_port)(raw, 2, audio.as_mut_ptr() as *mut c_void);
            ((*desc).connect_port)(raw, AUDIO_PORTS, gain as *mut c_void);
            ((*desc).connect_port)(raw, AUDIO_PORTS + 9, std::ptr::null_mut()); // no such port: ignored
            ((*desc).activate)(raw);
            // Longer than one internal block, in place.
            ((*desc).run)(raw, 10_000);
            let expected = 0.5 * 10f32.powf(6.0 / 20.0);
            assert!((audio[0] - expected).abs() < 1e-4);
            assert!((audio[9_999] - expected).abs() < 1e-4);
            *gain = 99.0; // clamped by the plane
            ((*desc).run)(raw, 1);
            assert_eq!((*(raw as *const Instance)).plane.load(0), Some(24.0));
            assert!(instance_state(raw).is_none(), "this plugin shares nothing");
            ((*desc).cleanup)(raw);
            drop(Box::from_raw(gain));
        }
    }

    #[test]
    fn instance_access_is_checked() {
        let not_ours = [0u64; 32];
        assert!(unsafe { instance_state(not_ours.as_ptr() as *const c_void) }.is_none());
        assert!(unsafe { instance_state(std::ptr::null()) }.is_none());
    }

    #[test]
    fn ttl_describes_every_port() {
        static MODES: [&str; 2] = ["Soft", "Hard"];
        static MORE: [ParamInfo; 4] = [
            ParamInfo::new("gain", "Gain \"Out\"", -24.0, 24.0, 0.0, Unit::Db),
            ParamInfo::new("mode", "Mode", 0.0, 1.0, 0.0, Unit::None).with_choices(&MODES),
            ParamInfo::new("on", "On", 0.0, 1.0, 1.0, Unit::Toggle),
            ParamInfo::new("2freq", "Freq", 20.0, 20_000.0, 1_000.0, Unit::Hz).with_taper(openplugin_core::Taper::Log),
        ];
        static MORE_INFO: PluginInfo = PluginInfo { params: &MORE, category: Category::Reverb, version: "1.4.2-beta", ..INFO };
        struct Wide;
        impl Plugin for Wide {
            fn info(&self) -> &'static PluginInfo {
                &MORE_INFO
            }
            fn prepare(&mut self, _: f32, _: usize, _: usize) {}
            fn reset(&mut self) {}
            fn set_param(&mut self, _: &str, _: f32) -> bool {
                true
            }
            fn param(&self, _: &str) -> Option<f32> {
                None
            }
            fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
            fn lv2_uri(&self) -> Option<&'static str> {
                Some("https://example.com/plugins/wide")
            }
        }
        let ttl = plugin_ttl(&Wide, true);
        assert!(ttl.contains("<https://example.com/plugins/wide>"));
        assert!(ttl.contains("lv2:ReverbPlugin"));
        assert!(ttl.contains("lv2:minorVersion 4 ;"));
        assert!(ttl.contains("lv2:microVersion 2 ;"));
        assert!(ttl.contains(r#"lv2:name "Gain \"Out\"""#));
        assert!(ttl.contains(r#"lv2:index 7 ; lv2:symbol "_2freq""#));
        assert!(ttl.contains("pprop:logarithmic"));
        assert!(ttl.contains("lv2:toggled"));
        assert!(ttl.contains(r#"rdfs:label "Hard" ; rdf:value 1.0"#));
        assert!(ttl.contains("units:unit units:db"));
        assert!(ttl.contains("a ui:CocoaUI"));
        assert!(ttl.trim_end().ends_with('.'));
        // Every port block is closed and separated.
        assert_eq!(ttl.matches("lv2:index").count(), 8);
        assert_eq!(ttl.matches(" ] ,\n").count(), 7);
        let manifest = manifest_ttl(&Wide, "Wide", false);
        assert!(manifest.contains("lv2:binary <Wide."));
        assert!(!manifest.contains("CocoaUI"));
        assert_eq!(lv2_version("2"), (0, 0));
    }

    #[test]
    fn descriptor_uses_the_plugin_uri() {
        set_creator(|| Box::new(Gain { db: 0.0, linear: 1.0 }));
        let desc = descriptor(0);
        assert!(!desc.is_null());
        unsafe {
            assert_eq!(CStr::from_ptr((*desc).uri), c"urn:openplugin:test-gain");
            let raw = ((*desc).instantiate)(desc, 48_000.0, std::ptr::null(), std::ptr::null());
            assert!(!raw.is_null());
            let mut input = [0.5f32; 4];
            let mut output = [0.0f32; 4];
            ((*desc).connect_port)(raw, 0, input.as_mut_ptr() as *mut c_void);
            ((*desc).connect_port)(raw, 2, output.as_mut_ptr() as *mut c_void);
            ((*desc).run)(raw, 4);
            assert!((output[0] - 0.5).abs() < 1e-5);
            ((*desc).cleanup)(raw);
        }
        assert!(descriptor(1).is_null());
    }
}
