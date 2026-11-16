//! `clap.gui` on Windows (`CLAP_WINDOW_API_WIN32`): the editor in a child
//! `HWND` of the host's window, or floating in a window of its own.
//!
//! The win32 API works in physical pixels: `get_size`, `set_size` and
//! `adjust_size` are the editor's points times the scale, and the scale is
//! the host's (`set_scale`) or, when the host sets none, the DPI of the
//! parent window. The window is as resizable as the editor says, and the
//! editor can ask the host for a size through `clap_host_gui::request_resize`.
//! A right click asks the host for its parameter menu (`clap.context-menu`)
//! at the click in pixels: in the plugin's window when the editor is embedded,
//! on the screen when it floats, as the extension says.
use super::*;
use openplugin_core::editor::SizeRange;
use openplugin_core::{Editor, ValueMirror};
use openplugin_dri::win32::{EditorView, TopWindow};

pub(super) struct Gui {
    pub(super) view: EditorView,
    /// The window of a floating editor.
    top: Option<TopWindow>,
}

fn is_win32(api: *const c_char) -> bool {
    !api.is_null() && unsafe { CStr::from_ptr(api) }.to_bytes_with_nul() == WINDOW_API_WIN32
}

unsafe extern "C" fn is_api_supported(_plugin: *const clap_plugin, api: *const c_char, _floating: bool) -> bool {
    guarded(std::ptr::null(), false, || is_win32(api))
}

unsafe extern "C" fn get_preferred_api(_plugin: *const clap_plugin, api: *mut *const c_char, floating: *mut bool) -> bool {
    guarded(std::ptr::null(), false, || {
        if api.is_null() || floating.is_null() {
            return false;
        }
        unsafe {
            *api = WINDOW_API_WIN32.as_ptr() as *const c_char;
            *floating = false;
        }
        true
    })
}

unsafe extern "C" fn create(plugin: *const clap_plugin, api: *const c_char, floating: bool) -> bool {
    guarded(std::ptr::null(), false, || {
        if !is_win32(api) {
            return false;
        }
        let Some(instance) = instance(plugin) else { return false };
        let Some(factory) = editor_factory() else { return false };
        instance.gui = None;
        // The host's gui extension, if any: how the editor asks for a size.
        let host = instance.host;
        let host_gui = host_call(host, EXT_GUI) as *const clap_host_gui;
        // The editor asks in points; the host hears pixels. The scale is read
        // from the view when the editor asks, so it is the one in use then.
        let raw: *mut Instance = &mut *instance;
        let request = move |w: u32, h: u32| -> bool {
            let Some(gui) = (unsafe { host_gui.as_ref() }) else { return false };
            let Some(view) = (unsafe { (*raw).gui.as_ref() }).map(|g| &g.view) else { return false };
            let (pw, ph) = view.to_pixels(w, h);
            match gui.request_resize {
                Some(f) => unsafe { f(host, pw, ph) },
                None => false,
            }
        };
        // The host's menu on a parameter. CLAP wants the place in pixels relative to the
        // plugin's window when the editor is embedded, and on the screen when it floats;
        // the editor says it in points.
        let menu = move |param: u32, x: i32, y: i32| -> bool {
            let Some(view) = (unsafe { (*raw).gui.as_ref() }).map(|g| &g.view) else { return false };
            let (px, py) = if floating {
                match view.to_screen(x as f32, y as f32) {
                    Some(at) => at,
                    None => return false,
                }
            } else {
                view.to_client(x as f32, y as f32)
            };
            host_context_popup(host, param, px, py)
        };
        let Some(mut editor) = factory(editor_host_for(instance).with_resize(request).with_context_menu(menu)) else { return false };
        editor.bind(instance.editor_state.clone());
        if let Some(info) = instance.track_info.as_ref() {
            editor.track_info(info);
        }
        let plane = instance.plane.clone();
        let mut mirror = ValueMirror::new(plane.len());
        // The current values before the first frame, then whatever changes.
        mirror.update(editor.as_mut(), |i| plane.load(i));
        let poll = Box::new(move |editor: &mut dyn Editor| mirror.update(editor, |i| plane.load(i)));
        let Some(view) = EditorView::new(editor, Some(poll)) else { return false };
        let top = if floating {
            // Closing the window tells the host, which then hides or destroys the editor.
            let closed = Box::new(move || {
                if let Some(f) = (unsafe { host_gui.as_ref() }).and_then(|g| g.closed) {
                    unsafe { f(host, false) };
                }
            });
            match TopWindow::new(instance.info.name, &view, Some(closed)) {
                Some(top) => Some(top),
                None => return false,
            }
        } else {
            None
        };
        instance.gui = Some(Gui { view, top });
        true
    })
}

unsafe extern "C" fn destroy(plugin: *const clap_plugin) {
    guarded(std::ptr::null(), (), || {
        if let Some(instance) = instance(plugin) {
            instance.gui = None;
        }
    })
}

fn gui<'a>(plugin: *const clap_plugin) -> Option<&'a Gui> {
    instance(plugin).and_then(|inst| inst.gui.as_ref())
}

unsafe extern "C" fn set_scale(plugin: *const clap_plugin, scale: f64) -> bool {
    guarded(std::ptr::null(), false, || gui(plugin).is_some_and(|gui| gui.view.set_scale(scale as f32)))
}

/// In pixels.
unsafe extern "C" fn get_size(plugin: *const clap_plugin, width: *mut u32, height: *mut u32) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(gui) = gui(plugin) else { return false };
        if width.is_null() || height.is_null() {
            return false;
        }
        let (w, h) = gui.view.pixel_size();
        unsafe {
            *width = w;
            *height = h;
        }
        true
    })
}

fn range(plugin: *const clap_plugin) -> Option<SizeRange> {
    gui(plugin).map(|gui| gui.view.size_range())
}

unsafe extern "C" fn can_resize(plugin: *const clap_plugin) -> bool {
    guarded(std::ptr::null(), false, || range(plugin).is_some_and(|r| !r.is_fixed()))
}

unsafe extern "C" fn get_resize_hints(plugin: *const clap_plugin, hints: *mut clap_gui_resize_hints) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(r) = range(plugin) else { return false };
        if hints.is_null() || r.is_fixed() {
            return false;
        }
        let (aw, ah) = r.aspect_ratio().unwrap_or((0, 0));
        unsafe {
            *hints = clap_gui_resize_hints {
                can_resize_horizontally: r.can_resize_horizontally(),
                can_resize_vertically: r.can_resize_vertically(),
                preserve_aspect_ratio: aw > 0,
                aspect_ratio_width: aw,
                aspect_ratio_height: ah,
            };
        }
        true
    })
}

/// The size in pixels nearest to the one proposed that the editor takes.
unsafe extern "C" fn adjust_size(plugin: *const clap_plugin, width: *mut u32, height: *mut u32) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(gui) = gui(plugin) else { return false };
        if width.is_null() || height.is_null() {
            return false;
        }
        unsafe {
            let (pw, ph) = gui.view.to_points(*width, *height);
            let (w, h) = gui.view.size_range().clamp(pw, ph);
            // A size the editor takes as it is stays exactly what the host proposed.
            if (w, h) != (pw, ph) {
                (*width, *height) = gui.view.to_pixels(w, h);
            }
        }
        true
    })
}

/// Take a size in pixels whose points are inside the editor's bounds. A fixed
/// editor takes its own size only.
unsafe extern "C" fn set_size(plugin: *const clap_plugin, width: u32, height: u32) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(gui) = gui(plugin) else { return false };
        let (pw, ph) = gui.view.to_points(width, height);
        if !gui.view.size_range().contains(pw, ph) {
            return false;
        }
        gui.view.set_pixel_size(width, height);
        true
    })
}

unsafe extern "C" fn set_parent(plugin: *const clap_plugin, window: *const clap_window) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(gui) = gui(plugin) else { return false };
        if window.is_null() || gui.top.is_some() {
            return false;
        }
        unsafe { is_win32((*window).api) && gui.view.attach((*window).ptr) }
    })
}

unsafe extern "C" fn set_transient(plugin: *const clap_plugin, window: *const clap_window) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(top) = gui(plugin).and_then(|gui| gui.top.as_ref()) else { return false };
        if window.is_null() || !unsafe { is_win32((*window).api) } {
            return false;
        }
        unsafe { top.set_owner((*window).ptr) };
        true
    })
}

unsafe extern "C" fn suggest_title(plugin: *const clap_plugin, title: *const c_char) {
    guarded(std::ptr::null(), (), || {
        let Some(top) = gui(plugin).and_then(|gui| gui.top.as_ref()) else { return };
        if !title.is_null() {
            top.set_title(&unsafe { CStr::from_ptr(title) }.to_string_lossy());
        }
    })
}

fn set_visible(plugin: *const clap_plugin, visible: bool) -> bool {
    match gui(plugin) {
        Some(gui) => {
            match gui.top.as_ref() {
                Some(top) => top.set_visible(visible),
                None => gui.view.set_hidden(!visible),
            }
            true
        }
        None => false,
    }
}

unsafe extern "C" fn show(plugin: *const clap_plugin) -> bool {
    guarded(std::ptr::null(), false, || set_visible(plugin, true))
}

unsafe extern "C" fn hide(plugin: *const clap_plugin) -> bool {
    guarded(std::ptr::null(), false, || set_visible(plugin, false))
}

pub(super) static GUI: clap_plugin_gui = clap_plugin_gui {
    is_api_supported: Some(is_api_supported),
    get_preferred_api: Some(get_preferred_api),
    create: Some(create),
    destroy: Some(destroy),
    set_scale: Some(set_scale),
    get_size: Some(get_size),
    can_resize: Some(can_resize),
    get_resize_hints: Some(get_resize_hints),
    adjust_size: Some(adjust_size),
    set_size: Some(set_size),
    set_parent: Some(set_parent),
    set_transient: Some(set_transient),
    suggest_title: Some(suggest_title),
    show: Some(show),
    hide: Some(hide),
};
