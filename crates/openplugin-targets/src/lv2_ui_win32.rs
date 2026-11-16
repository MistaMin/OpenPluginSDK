//! `ui:WindowsUI`: the editor in a child `HWND` of the parent window the host
//! gives (`ui:parent`, which this UI requires: a child window needs its
//! parent), handed back to the host as the widget. The view runs on its own
//! timer in the host's message loop, so the host needs no idle interface.
//! `ui:resize` sizes are pixels: the editor's points times the scale, which is
//! the DPI of the parent window.
use super::*;
use openplugin_core::{Editor, EditorHost, Gesture};
use openplugin_dri::win32::EditorView;
use std::cell::Cell;
use std::rc::Rc;

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

/// The `ui:resize` the UI itself offers the host through `extension_data`.
struct OfferedResize(Resize);
// Plain function pointer and a null handle; the host passes the UI's own handle.
unsafe impl Sync for OfferedResize {}
static OFFERED: OfferedResize = OfferedResize(Resize { handle: std::ptr::null_mut(), ui_resize: Some(host_resizes) });

/// The host sets the editor's size. 0 on success; non-zero if the size is
/// outside the editor's range (a fixed editor takes only its own size).
unsafe extern "C" fn host_resizes(handle: *mut c_void, width: i32, height: i32) -> i32 {
    guarded(1, || {
        let Some(ui) = (unsafe { (handle as *mut Ui).as_mut() }) else { return 1 };
        let (Ok(w), Ok(h)) = (u32::try_from(width), u32::try_from(height)) else { return 1 };
        // Pixels from the host; the editor's range is in points.
        let (pw, ph) = ui.view.to_points(w, h);
        if !ui.view.size_range().contains(pw, ph) {
            return 1;
        }
        ui.view.set_pixel_size(w, h);
        0
    })
}

struct Ui {
    view: EditorView,
    count: usize,
    first: u32,
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
    guarded(std::ptr::null_mut(), || {
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
        // A size the host agreed to, for the view to apply on its next tick.
        let granted: Rc<Cell<Option<(u32, u32)>>> = Rc::new(Cell::new(None));
        // Pixels per point, known once the view has its window in the parent.
        let scale: Rc<Cell<f32>> = Rc::new(Cell::new(1.0));
        let ask = {
            let granted = granted.clone();
            let scale = scale.clone();
            let (handle, call) = match unsafe { resize.as_ref() } {
                Some(r) => (r.handle as usize, r.ui_resize),
                None => (0, None),
            };
            move |w: u32, h: u32| -> bool {
                let Some(call) = call else { return false };
                // The editor asks in points; the host's `ui:resize` is in pixels.
                let px = |v: u32| i32::try_from((v as f32 * scale.get()).round() as u32);
                let (Ok(wi), Ok(hi)) = (px(w), px(h)) else { return false };
                if unsafe { call(handle as *mut c_void, wi, hi) } != 0 {
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
        let Some(view) = EditorView::new(editor, Some(poll)) else { return std::ptr::null_mut() };
        // No parent, no window: the host must give `ui:parent` (the description requires it).
        if !unsafe { view.attach(parent) } {
            return std::ptr::null_mut();
        }
        scale.set(view.scale());
        if let Some(r) = unsafe { resize.as_ref() } {
            if let Some(f) = r.ui_resize {
                let (w, h) = view.pixel_size();
                unsafe { f(r.handle, w as i32, h as i32) };
            }
        }
        if !widget.is_null() {
            unsafe { *widget = view.raw() };
        }
        Box::into_raw(Box::new(Ui { view, count, first })) as *mut c_void
    })
}

unsafe extern "C" fn cleanup(handle: *mut c_void) {
    guarded((), || {
        if !handle.is_null() {
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
        if !uri.is_null() && unsafe { CStr::from_ptr(uri) }.to_bytes() == b"http://lv2plug.in/ns/extensions/ui#resize" {
            return &OFFERED.0 as *const Resize as *const c_void;
        }
        std::ptr::null()
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
    format!("\n<{uri}#ui>\n    a ui:WindowsUI ;\n    ui:binary <{binary}> ;\n    rdfs:seeAlso <{binary_stem}.ttl> .\n")
}

/// The editor's description in `<binary_stem>.ttl`. The host must give it a
/// `ui:parent` window.
pub(super) fn ui_ttl(uri: &str, resizable: bool) -> String {
    let mut out = format!("\n<{uri}#ui>\n    a ui:WindowsUI ;\n");
    out.push_str("    lv2:requiredFeature ui:parent ;\n");
    if resizable {
        out.push_str("    lv2:optionalFeature <http://lv2plug.in/ns/ext/instance-access> , ui:touch , ui:resize ;\n");
        out.push_str("    lv2:extensionData ui:resize .\n");
    } else {
        out.push_str("    lv2:optionalFeature <http://lv2plug.in/ns/ext/instance-access> , ui:touch , ui:resize , ui:noUserResize .\n");
    }
    out
}
