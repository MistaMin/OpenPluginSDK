//! `ui:X11UI`: the editor in an X11 child window, handed to the host as the
//! widget (the window id cast to a pointer, as `ui:X11UI` prescribes).
//!
//! A Linux host runs no event loop for the view's own X connection, so the
//! UI requires `ui:idleInterface`: the host calls `idle` from its GUI thread
//! and `idle` calls [`EditorView::pump`], which handles input and ticks the
//! editor at its own 30 Hz. The host's `ui:parent` window is required; the
//! view is put inside it. Gestures are control-port writes plus `ui:touch`
//! grabs, values come back through `port_event`, and with `instance-access`
//! the editor is bound to the engine's [`EditorState`]. A resizable editor
//! offers `ui:resize` to the host and asks for sizes through the host's
//! `ui:resize`, as on macOS.
//!
//! Written from the LV2 UI specification (`ui.h`, `ui.ttl`).
use super::*;
use openplugin_core::{Editor, EditorHost, Gesture};
use openplugin_dri::x11::EditorView;
use std::cell::Cell;
use std::rc::Rc;

/// `LV2UI_Write_Function`.
pub type WriteFn = unsafe extern "C" fn(controller: *mut c_void, port: u32, size: u32, protocol: u32, buffer: *const c_void);

/// `LV2UI_Descriptor`.
#[repr(C)]
pub struct Lv2UiDescriptor {
    /// The editor's URI (`<plugin uri>#ui`), a NUL-terminated static string.
    pub uri: *const c_char,
    /// Opens the editor in the host's parent widget (main thread). Null on failure.
    pub instantiate: unsafe extern "C" fn(
        *const Lv2UiDescriptor,
        *const c_char,
        *const c_char,
        Option<WriteFn>,
        *mut c_void,
        *mut *mut c_void,
        *const *const Lv2Feature,
    ) -> *mut c_void,
    /// Closes the editor and frees it (main thread).
    pub cleanup: unsafe extern "C" fn(*mut c_void),
    /// A control port's new value from the host (main thread); other ports and protocols are ignored.
    pub port_event: unsafe extern "C" fn(*mut c_void, u32, u32, u32, *const c_void),
    /// `ui:idleInterface`, `ui:resize` and the other UI extensions by URI; null for one it does not have.
    pub extension_data: unsafe extern "C" fn(*const c_char) -> *const c_void,
}

// The URI points at a process-lifetime `CString`; the rest are plain function pointers.
unsafe impl Send for Lv2UiDescriptor {}
unsafe impl Sync for Lv2UiDescriptor {}

const PARENT: &[u8] = b"http://lv2plug.in/ns/extensions/ui#parent";
const INSTANCE_ACCESS: &[u8] = b"http://lv2plug.in/ns/ext/instance-access";
const TOUCH: &[u8] = b"http://lv2plug.in/ns/extensions/ui#touch";
const RESIZE: &[u8] = b"http://lv2plug.in/ns/extensions/ui#resize";
const IDLE_INTERFACE: &[u8] = b"http://lv2plug.in/ns/extensions/ui#idleInterface";

/// `LV2UI_Touch`.
#[repr(C)]
struct Touch {
    handle: *mut c_void,
    touch: Option<unsafe extern "C" fn(handle: *mut c_void, port: u32, grabbed: bool)>,
}

/// `LV2UI_Resize`.
#[repr(C)]
struct Resize {
    handle: *mut c_void,
    ui_resize: Option<unsafe extern "C" fn(handle: *mut c_void, width: i32, height: i32) -> i32>,
}

/// `LV2UI_Idle_Interface`.
#[repr(C)]
struct IdleInterface {
    idle: Option<unsafe extern "C" fn(handle: *mut c_void) -> i32>,
}

/// The `ui:resize` the UI itself offers the host through `extension_data`.
struct OfferedResize(Resize);
// Plain function pointer and a null handle; the host passes the UI's own handle.
unsafe impl Sync for OfferedResize {}
static OFFERED: OfferedResize = OfferedResize(Resize { handle: std::ptr::null_mut(), ui_resize: Some(host_resizes) });

/// The `ui:idleInterface` the UI offers the host through `extension_data`.
static IDLE: IdleInterface = IdleInterface { idle: Some(idle) };

/// X11 pixels per editor point: the scale the view renders at. `ui:X11UI`
/// sizes (`ui:resize` both ways) are in pixels, the view's sizes are in points.
fn pixels_per_point(view: &EditorView) -> f64 {
    f64::from(view.scale())
}

/// Editor points to X11 pixels, for the host.
fn to_pixels(points: u32, scale: f64) -> i32 {
    let pixels = (f64::from(points) * scale).round();
    if pixels >= f64::from(i32::MAX) { i32::MAX } else { pixels.max(0.0) as i32 }
}

/// X11 pixels from the host to editor points; `None` for a negative size.
fn to_points(pixels: i32, scale: f64) -> Option<u32> {
    let pixels = u32::try_from(pixels).ok()?;
    let points = (f64::from(pixels) / scale).round();
    (points.is_finite() && points <= f64::from(u32::MAX)).then_some(points as u32)
}

/// The host sets the editor's size in pixels. 0 on success; non-zero if the
/// size is outside the editor's range (a fixed editor takes only its own size).
unsafe extern "C" fn host_resizes(handle: *mut c_void, width: i32, height: i32) -> i32 {
    guarded(1, || {
        let Some(ui) = (unsafe { (handle as *mut Ui).as_mut() }) else { return 1 };
        let scale = pixels_per_point(&ui.view);
        let (Some(w), Some(h)) = (to_points(width, scale), to_points(height, scale)) else { return 1 };
        if !ui.view.size_range().contains(w, h) {
            return 1;
        }
        ui.view.resize(w, h);
        0
    })
}

/// The host's idle call: handle X events and tick the editor. 0 while the UI
/// is alive; non-zero tells the host the UI is gone (closed, or its display lost).
unsafe extern "C" fn idle(handle: *mut c_void) -> i32 {
    guarded(1, || {
        let Some(ui) = (unsafe { (handle as *mut Ui).as_mut() }) else { return 1 };
        if ui.idling {
            return 0;
        }
        ui.idling = true;
        ui.view.pump();
        ui.idling = false;
        if ui.closing {
            drop(unsafe { Box::from_raw(handle as *mut Ui) });
            return 1;
        }
        i32::from(!ui.view.usable())
    })
}

struct Ui {
    view: EditorView,
    count: usize,
    first: u32,
    /// Inside `view.pump()`.
    idling: bool,
    /// `cleanup` came inside `view.pump()`: `idle` frees the UI when the pump returns.
    closing: bool,
}

#[derive(Clone, Copy)]
struct Link {
    write: Option<WriteFn>,
    controller: *mut c_void,
    touch: *const Touch,
    first: u32,
}

fn host_for(link: Link) -> EditorHost {
    EditorHost::new(move |gesture| unsafe {
        let link = link;
        match gesture {
            Gesture::Begin(i) | Gesture::End(i) => {
                if let Some(t) = link.touch.as_ref() {
                    if let Some(f) = t.touch {
                        f(t.handle, link.first + i, matches!(gesture, Gesture::Begin(_)));
                    }
                }
            }
            Gesture::Perform(i, value) => {
                if let Some(write) = link.write {
                    // Protocol 0: a float written to a control port.
                    write(link.controller, link.first + i, 4, 0, &value as *const f32 as *const c_void);
                }
            }
            _ => {}
        }
    })
}

fn ui_uri() -> &'static CStr {
    static URI: OnceLock<CString> = OnceLock::new();
    URI.get_or_init(|| CString::new(format!("{}#ui", uri().to_string_lossy())).unwrap_or_default())
        .as_c_str()
}

/// The host's parent window from `ui:parent`: an X11 Window as a
/// pointer-sized integer. `None` for zero or a value no XID can have.
fn parent_window(data: *mut c_void) -> Option<openplugin_dri::x11::Window> {
    u32::try_from(data as usize).ok().filter(|&window| window != 0)
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
    guarded(std::ptr::null_mut(), || {
        let Some(factory) = editor_factory() else { return std::ptr::null_mut() };
        if plugin_uri.is_null() || unsafe { CStr::from_ptr(plugin_uri) } != uri() {
            return std::ptr::null_mut();
        }
        let mut parent = None;
        let mut instance: *const c_void = std::ptr::null();
        let mut touch: *const Touch = std::ptr::null();
        let mut resize: *const Resize = std::ptr::null();
        let mut next = features;
        while !next.is_null() && !unsafe { *next }.is_null() {
            let feature = unsafe { &**next };
            if !feature.uri.is_null() {
                match unsafe { CStr::from_ptr(feature.uri) }.to_bytes() {
                    PARENT => parent = parent_window(feature.data),
                    INSTANCE_ACCESS => instance = feature.data,
                    TOUCH => touch = feature.data as *const Touch,
                    RESIZE => resize = feature.data as *const Resize,
                    _ => {}
                }
            }
            next = unsafe { next.add(1) };
        }
        // An X11 UI is a child of the host's window; without one there is nothing to show.
        let Some(parent) = parent else { return std::ptr::null_mut() };
        // A size the host agreed to, for the view to apply on its next tick.
        let granted: Rc<Cell<Option<(u32, u32)>>> = Rc::new(Cell::new(None));
        // The view's scale, known once the view exists; the editor can only ask after that.
        let scale: Rc<Cell<f64>> = Rc::new(Cell::new(1.0));
        let ask = {
            let granted = granted.clone();
            let scale = scale.clone();
            let (handle, call) = match unsafe { resize.as_ref() } {
                Some(r) => (r.handle as usize, r.ui_resize),
                None => (0, None),
            };
            move |w: u32, h: u32| -> bool {
                let Some(call) = call else { return false };
                if unsafe { call(handle as *mut c_void, to_pixels(w, scale.get()), to_pixels(h, scale.get())) } != 0 {
                    return false;
                }
                granted.set(Some((w, h)));
                true
            }
        };
        let first = first_param();
        let Some(mut editor) = factory(host_for(Link { write, controller, touch, first }).with_resize(ask)) else {
            return std::ptr::null_mut();
        };
        editor.bind(unsafe { instance_state(instance) });
        let count = param_count();
        let poll = Box::new(move |editor: &mut dyn Editor| {
            if let Some((w, h)) = granted.take() {
                let (w, h) = editor.size_range().clamp(w, h);
                editor.resize(w, h);
            }
        });
        let Some(mut view) = EditorView::new(editor, Some(poll)) else { return std::ptr::null_mut() };
        scale.set(pixels_per_point(&view));
        let window = view.raw();
        if window == 0 || !view.attach(parent) {
            return std::ptr::null_mut();
        }
        if let Some(r) = unsafe { resize.as_ref() } {
            if let Some(f) = r.ui_resize {
                let (w, h) = view.size();
                unsafe { f(r.handle, to_pixels(w, scale.get()), to_pixels(h, scale.get())) };
            }
        }
        if !widget.is_null() {
            unsafe { *widget = window as usize as *mut c_void };
        }
        Box::into_raw(Box::new(Ui { view, count, first, idling: false, closing: false })) as *mut c_void
    })
}

unsafe extern "C" fn cleanup(handle: *mut c_void) {
    guarded((), || {
        let Some(ui) = (unsafe { (handle as *mut Ui).as_mut() }) else { return };
        if ui.idling {
            ui.closing = true;
        } else {
            drop(unsafe { Box::from_raw(handle as *mut Ui) });
        }
    })
}

unsafe extern "C" fn port_event(handle: *mut c_void, port: u32, size: u32, format: u32, buffer: *const c_void) {
    guarded((), || {
        let Some(ui) = (unsafe { (handle as *mut Ui).as_mut() }) else { return };
        if format != 0 || size != 4 || buffer.is_null() || port < ui.first {
            return;
        }
        let index = (port - ui.first) as usize;
        let value = unsafe { *(buffer as *const f32) };
        if index >= ui.count || !value.is_finite() {
            return;
        }
        if let Some(editor) = ui.view.editor() {
            editor.param_changed(index, value);
        }
    })
}

unsafe extern "C" fn extension_data(uri: *const c_char) -> *const c_void {
    guarded(std::ptr::null(), || {
        if uri.is_null() {
            return std::ptr::null();
        }
        match unsafe { CStr::from_ptr(uri) }.to_bytes() {
            IDLE_INTERFACE => &IDLE as *const IdleInterface as *const c_void,
            RESIZE => &OFFERED.0 as *const Resize as *const c_void,
            _ => std::ptr::null(),
        }
    })
}

fn published() -> &'static Lv2UiDescriptor {
    static DESCRIPTOR: OnceLock<Lv2UiDescriptor> = OnceLock::new();
    DESCRIPTOR.get_or_init(|| Lv2UiDescriptor { uri: ui_uri().as_ptr(), instantiate, cleanup, port_event, extension_data })
}

/// `lv2ui_descriptor`. Index 0 is the editor, once [`set_editor`] ran.
pub fn lv2ui_descriptor(index: u32) -> *const Lv2UiDescriptor {
    guarded(std::ptr::null(), || {
        if index == 0 && creator().is_some() && editor_factory().is_some() {
            published()
        } else {
            std::ptr::null()
        }
    })
}

/// The editor's entry in `manifest.ttl`.
pub(super) fn manifest_entry(uri: &str, binary: &str, binary_stem: &str) -> String {
    format!("\n<{uri}#ui>\n    a ui:X11UI ;\n    ui:binary <{binary}> ;\n    rdfs:seeAlso <{binary_stem}.ttl> .\n")
}

/// The editor's description in `<binary_stem>.ttl`. The host must drive it
/// through `ui:idleInterface` and give it a `ui:parent` window.
pub(super) fn ui_ttl(uri: &str, resizable: bool) -> String {
    let mut out = format!("\n<{uri}#ui>\n    a ui:X11UI ;\n");
    out.push_str("    lv2:requiredFeature ui:idleInterface , ui:parent ;\n");
    if resizable {
        out.push_str("    lv2:optionalFeature <http://lv2plug.in/ns/ext/instance-access> , ui:touch , ui:resize ;\n");
        out.push_str("    lv2:extensionData ui:idleInterface ;\n");
        out.push_str("    lv2:extensionData ui:resize .\n");
    } else {
        out.push_str("    lv2:optionalFeature <http://lv2plug.in/ns/ext/instance-access> , ui:touch , ui:resize , ui:noUserResize ;\n");
        out.push_str("    lv2:extensionData ui:idleInterface .\n");
    }
    out
}
