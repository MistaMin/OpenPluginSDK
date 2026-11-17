//! `export_shell!` emits the C ABI in [`openplugin_ffi::shell`].
//!
//! A native VST3, Audio Unit, or AAX adapter links the plugin static library and
//! calls `op_shell_*`. One plugin binary exports one shell.
//!
//! `export_shell!(P, editor = make_editor)` also exports the editor ABI of
//! `native/include/op_editor.h` (`op_editor_*`, macOS), written here in Rust
//! over [`openplugin_dri::cocoa::EditorView`]. Build the shells with
//! `-DOP_EDITOR=ON` for it. Resizing is a second, optional block of that
//! header (`OP_EDITOR_RESIZE`): the size range, `op_editor_resize`, and a
//! callback for an editor that asks its host for a size. Both forms export `op_shell_tail_seconds` from
//! [`Plugin::tail`](crate::PluginForExport::tail), for `-DOP_TAIL=ON`, and
//! `op_shell_save_state` / `op_shell_load_state` for the whole plugin state.

pub use openplugin_ffi::shell::{
    kind, layout_count, layout_selected, midi_dropped, midi_out, note_ports, process_events,
    set_layout, set_offline, set_track_info, track_info_from_abi, write_layout, OpLayout, OpMidiEvent, OpProcess,
    create, destroy, editor_state, get_normalized, get_plain, latency, load_state, param_format, param_group, param_parse, process,
    reset, save_state, set_normalized, set_plain, tail_seconds, to_normalized, to_plain, trace, write_info,
    write_param, OpParamDesc, OpShellInfo, ShellInstance, ABI, AAX_MAX_PARAMS, PARAM_AUTOMATABLE,
    PARAM_DISCRETE, PARAM_LOG, STATE_MAX, STATE_OK, STATE_TOO_SMALL,
    // ABI 6: telling the host about changes.
    take_changes, CHANGE_LATENCY, CHANGE_PARAM_INFO,
    // ABI 7: per-note expression.
    expression_format, expression_info, expression_parse, expressions, OpExpressionInfo,
    // ABI 8: identity pins.
    au_code, identity, param_aax_id, param_host_id,
    // ABI 9: the instance's own parameter table, and its largest latency.
    instance_param, instance_param_format, instance_param_parse, latency_now, max_latency,
};

#[cfg(any(target_os = "macos", windows))]
pub use editor::OpEditorHost;

/// The editor ABI (`op_editor.h`). The functions take the plugin's
/// [`EditorFactory`](openplugin_core::EditorFactory); `export_shell!` wraps
/// them in the `#[unsafe(no_mangle)]` symbols the shells link.
#[cfg(any(target_os = "macos", windows))]
pub mod editor {
    use super::ShellInstance;
    use openplugin_core::{EditorFactory, EditorHost, Gesture};
    #[cfg(target_os = "macos")]
    use openplugin_dri::cocoa::EditorView;
    #[cfg(windows)]
    use openplugin_dri::win32::EditorView;
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::rc::Rc;
    use std::sync::OnceLock;

    /// `OpEditorRequestResize`: the editor wants this size; non-zero if the
    /// shell's host agreed (and will then call `op_editor_resize`).
    pub type RequestResize = unsafe extern "C" fn(context: *mut c_void, width: u32, height: u32) -> u32;

    /// What `op_editor_create` returns: the view, and the callback an editor
    /// uses to ask for a size (set by `op_editor_set_request_resize`).
    pub struct Handle {
        view: EditorView,
        request: Rc<Cell<Option<(usize, RequestResize)>>>,
        menu: Rc<Cell<Option<(usize, ContextMenu)>>>,
    }

    /// `OpEditorContextMenu`: the editor wants the host's context menu for
    /// parameter `index` at (`x`, `y`) points in the view, origin top left;
    /// non-zero if the shell's host showed one.
    pub type ContextMenu = unsafe extern "C" fn(context: *mut c_void, index: u32, x: i32, y: i32) -> u32;

    /// How the editor reports a gesture to the host (`OpEditorHost` in C).
    /// Plain values; main thread.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct OpEditorHost {
        pub context: *mut c_void,
        pub begin_edit: Option<unsafe extern "C" fn(context: *mut c_void, index: u32)>,
        pub perform_edit: Option<unsafe extern "C" fn(context: *mut c_void, index: u32, plain: f64)>,
        pub end_edit: Option<unsafe extern "C" fn(context: *mut c_void, index: u32)>,
    }

    /// The gestures of an editor, sent to an `OpEditorHost`.
    pub fn host_for(callbacks: OpEditorHost) -> EditorHost {
        EditorHost::new(move |gesture| unsafe {
            let c = callbacks;
            match gesture {
                Gesture::Begin(i) => {
                    if let Some(f) = c.begin_edit {
                        f(c.context, i);
                    }
                }
                Gesture::Perform(i, v) => {
                    if let Some(f) = c.perform_edit {
                        f(c.context, i, f64::from(v));
                    }
                }
                Gesture::End(i) => {
                    if let Some(f) = c.end_edit {
                        f(c.context, i);
                    }
                }
            }
        })
    }

    /// `op_editor_size`. The size of an editor made once with no host.
    ///
    /// # Safety
    /// `width` and `height` are null or writable.
    pub unsafe fn size(factory: EditorFactory, width: *mut u32, height: *mut u32) -> u32 {
        static SIZE: OnceLock<Option<(u32, u32)>> = OnceLock::new();
        let Some((w, h)) = *SIZE.get_or_init(|| factory(EditorHost::detached()).map(|e| e.size())) else { return 0 };
        if width.is_null() || height.is_null() {
            return 0;
        }
        unsafe {
            *width = w;
            *height = h;
        }
        1
    }

    /// `op_editor_create`. Null on failure. The host struct is copied.
    ///
    /// # Safety
    /// `host` is null or points at an `OpEditorHost` whose callbacks stay
    /// valid until [`destroy`].
    pub unsafe fn create(factory: EditorFactory, host: *const OpEditorHost) -> *mut c_void {
        let editor_host = match unsafe { host.as_ref() } {
            Some(callbacks) => host_for(*callbacks),
            None => EditorHost::detached(),
        };
        let request: Rc<Cell<Option<(usize, RequestResize)>>> = Rc::new(Cell::new(None));
        let asked = request.clone();
        let editor_host = editor_host.with_resize(move |w, h| match asked.get() {
            Some((context, f)) => unsafe { f(context as *mut c_void, w, h) != 0 },
            None => false,
        });
        let menu: Rc<Cell<Option<(usize, ContextMenu)>>> = Rc::new(Cell::new(None));
        let shown = menu.clone();
        let editor_host = editor_host.with_context_menu(move |index, x, y| match shown.get() {
            Some((context, f)) => unsafe { f(context as *mut c_void, index, x, y) != 0 },
            None => false,
        });
        let Some(mut editor) = factory(editor_host) else { return std::ptr::null_mut() };
        editor.bind(None);
        match EditorView::new(editor, None) {
            Some(view) => Box::into_raw(Box::new(Handle { view, request, menu })) as *mut c_void,
            None => std::ptr::null_mut(),
        }
    }

    unsafe fn view<'a>(editor: *mut c_void) -> Option<&'a mut EditorView> {
        unsafe { (editor as *mut Handle).as_mut() }.map(|h| &mut h.view)
    }

    /// `op_editor_size_range`. The range of an editor made once with no host.
    /// The aspect is 0, 0 for any shape.
    ///
    /// # Safety
    /// Every pointer is null or writable.
    pub unsafe fn size_range(
        factory: EditorFactory,
        min_w: *mut u32,
        min_h: *mut u32,
        max_w: *mut u32,
        max_h: *mut u32,
        aspect_w: *mut u32,
        aspect_h: *mut u32,
    ) -> u32 {
        static RANGE: OnceLock<Option<openplugin_core::editor::SizeRange>> = OnceLock::new();
        let Some(r) = *RANGE.get_or_init(|| factory(EditorHost::detached()).map(|e| e.size_range())) else { return 0 };
        let (aw, ah) = r.aspect_ratio().unwrap_or((0, 0));
        let values = [(min_w, r.min.0), (min_h, r.min.1), (max_w, r.max.0), (max_h, r.max.1), (aspect_w, aw), (aspect_h, ah)];
        if values.iter().any(|(p, _)| p.is_null()) {
            return 0;
        }
        for (p, v) in values {
            unsafe { *p = v };
        }
        1
    }

    /// `op_editor_get_size`. The editor's current size.
    ///
    /// # Safety
    /// `editor` is null or from [`create`]; the pointers are null or writable.
    pub unsafe fn get_size(editor: *mut c_void, width: *mut u32, height: *mut u32) -> u32 {
        let Some(v) = (unsafe { view(editor) }) else { return 0 };
        if width.is_null() || height.is_null() {
            return 0;
        }
        let (w, h) = v.size();
        unsafe {
            *width = w;
            *height = h;
        }
        1
    }

    /// `op_editor_adjust_size`. Replaces a proposed size with the nearest one
    /// the editor takes (the host's `checkSizeConstraint`, `adjust_size`).
    ///
    /// # Safety
    /// `editor` is null or from [`create`]; the pointers are null or read-write.
    pub unsafe fn adjust_size(editor: *mut c_void, width: *mut u32, height: *mut u32) -> u32 {
        let Some(v) = (unsafe { view(editor) }) else { return 0 };
        if width.is_null() || height.is_null() {
            return 0;
        }
        unsafe {
            let (w, h) = v.size_range().clamp(*width, *height);
            *width = w;
            *height = h;
        }
        1
    }

    /// `op_editor_resize`. 1 if the size is inside the editor's bounds and the
    /// editor and its view now have it; 0, and no change, if not.
    ///
    /// # Safety
    /// `editor` is null or from [`create`].
    pub unsafe fn resize(editor: *mut c_void, width: u32, height: u32) -> u32 {
        match unsafe { view(editor) } {
            Some(v) if v.size_range().contains(width, height) => {
                v.resize(width, height);
                1
            }
            _ => 0,
        }
    }

    /// `op_editor_set_request_resize`. A null `request` removes it.
    ///
    /// # Safety
    /// `editor` is null or from [`create`]. `request` stays valid, with
    /// `context`, until it is replaced or [`destroy`].
    pub unsafe fn set_request_resize(editor: *mut c_void, context: *mut c_void, request: Option<RequestResize>) {
        if let Some(h) = unsafe { (editor as *mut Handle).as_mut() } {
            h.request.set(request.map(|f| (context as usize, f)));
        }
    }

    /// `op_editor_set_context_menu`. A null `show` removes it.
    ///
    /// # Safety
    /// `editor` is null or from [`create`]. `show` stays valid, with
    /// `context`, until it is replaced or [`destroy`].
    pub unsafe fn set_context_menu(editor: *mut c_void, context: *mut c_void, show: Option<ContextMenu>) {
        if let Some(h) = unsafe { (editor as *mut Handle).as_mut() } {
            h.menu.set(show.map(|f| (context as usize, f)));
        }
    }

    /// `op_editor_set_track_info`: tell the editor about the host's track
    /// ([`openplugin_core::Editor::track_info`]).
    ///
    /// # Safety
    /// `editor` is null or from [`create`]; `name` is null or NUL-terminated.
    pub unsafe fn set_track_info(editor: *mut c_void, name: *const std::ffi::c_char, rgba: u32, flags: u32) {
        let info = unsafe { super::track_info_from_abi(name, rgba, flags) };
        if let Some(e) = unsafe { view(editor) }.and_then(|v| v.editor()) {
            e.track_info(&info);
        }
    }

    /// `op_editor_view`.
    ///
    /// # Safety
    /// `editor` is null or from [`create`].
    pub unsafe fn raw_view(editor: *mut c_void) -> *mut c_void {
        unsafe { view(editor) }.map(|v| v.raw()).unwrap_or(std::ptr::null_mut())
    }

    /// `op_editor_attach`.
    ///
    /// # Safety
    /// `editor` is null or from [`create`]; `parent` is null or an `NSView`.
    pub unsafe fn attach(editor: *mut c_void, parent: *mut c_void) -> u32 {
        match unsafe { view(editor) } {
            Some(v) => u32::from(unsafe { v.attach(parent) }),
            None => 0,
        }
    }

    /// `op_editor_bind`. A pointer that is not a live instance binds nothing.
    ///
    /// # Safety
    /// `editor` is null or from [`create`]. `instance` may be anything.
    pub unsafe fn bind(editor: *mut c_void, instance: *mut c_void) {
        let state = super::editor_state(instance as *const ShellInstance);
        if let Some(e) = unsafe { view(editor) }.and_then(|v| v.editor()) {
            e.bind(state);
        }
    }

    /// `op_editor_set_plain`.
    ///
    /// # Safety
    /// `editor` is null or from [`create`].
    pub unsafe fn set_plain(editor: *mut c_void, index: u32, plain: f32) {
        if !plain.is_finite() {
            return;
        }
        super::trace::write("host");
        if let Some(e) = unsafe { view(editor) }.and_then(|v| v.editor()) {
            e.param_changed(index as usize, plain);
        }
    }

    /// `op_editor_set_scale` (Windows, `op_editor_win.h`): pixels per point,
    /// from the host (VST3 `setContentScaleFactor`). The view's window becomes
    /// `points x scale` pixels. 0 for a scale that is not a positive number.
    ///
    /// # Safety
    /// `editor` is null or from [`create`].
    #[cfg(windows)]
    pub unsafe fn set_scale(editor: *mut c_void, scale: f64) -> u32 {
        match unsafe { view(editor) } {
            Some(v) => u32::from(v.set_scale(scale as f32)),
            None => 0,
        }
    }

    /// `op_editor_scale` (Windows): pixels per point in use: the host's
    /// scale, or the DPI of the view's window over 96. 0 for no editor.
    ///
    /// # Safety
    /// `editor` is null or from [`create`].
    #[cfg(windows)]
    pub unsafe fn scale(editor: *mut c_void) -> f64 {
        unsafe { view(editor) }.map(|v| f64::from(v.scale())).unwrap_or(0.0)
    }

    /// `op_editor_set_pixel_size` (Windows): the host made the view's place
    /// `width` x `height` pixels (VST3 `onSize`). The window takes exactly
    /// that, the editor the nearest size in points its range allows. 0 for no editor.
    ///
    /// # Safety
    /// `editor` is null or from [`create`].
    #[cfg(windows)]
    pub unsafe fn set_pixel_size(editor: *mut c_void, width: u32, height: u32) -> u32 {
        match unsafe { view(editor) } {
            Some(v) => {
                v.set_pixel_size(width, height);
                1
            }
            None => 0,
        }
    }

    /// `op_editor_destroy`.
    ///
    /// # Safety
    /// `editor` is null or from [`create`], and not used again.
    pub unsafe fn destroy(editor: *mut c_void) {
        if !editor.is_null() {
            drop(unsafe { Box::from_raw(editor as *mut Handle) });
        }
    }
}

/// Export `op_shell_*` from the plugin crate. `$plugin` implements [`Plugin`](crate::PluginForExport)
/// and [`Default`]. Link the staticlib into `native/`.
///
/// `export_shell!(P, editor = make_editor)` adds `op_editor_*` on macOS.
#[macro_export]
macro_rules! export_shell {
    ($plugin:ty, editor = $editor:path) => {
        $crate::export_shell!($plugin);

        #[cfg(any(target_os = "macos", windows))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_size(width: *mut u32, height: *mut u32) -> u32 {
            $crate::shell::editor::size($editor, width, height)
        }

        #[cfg(any(target_os = "macos", windows))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_create(host: *const $crate::shell::OpEditorHost) -> *mut ::std::ffi::c_void {
            $crate::shell::editor::create($editor, host)
        }

        #[cfg(any(target_os = "macos", windows))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_view(editor: *mut ::std::ffi::c_void) -> *mut ::std::ffi::c_void {
            $crate::shell::editor::raw_view(editor)
        }

        #[cfg(any(target_os = "macos", windows))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_attach(editor: *mut ::std::ffi::c_void, parent: *mut ::std::ffi::c_void) -> u32 {
            $crate::shell::editor::attach(editor, parent)
        }

        #[cfg(any(target_os = "macos", windows))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_bind(editor: *mut ::std::ffi::c_void, instance: *mut ::std::ffi::c_void) {
            $crate::shell::editor::bind(editor, instance)
        }

        #[cfg(any(target_os = "macos", windows))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_set_plain(editor: *mut ::std::ffi::c_void, index: u32, plain: f32) {
            $crate::shell::editor::set_plain(editor, index, plain)
        }

        #[cfg(any(target_os = "macos", windows))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_size_range(
            min_w: *mut u32,
            min_h: *mut u32,
            max_w: *mut u32,
            max_h: *mut u32,
            aspect_w: *mut u32,
            aspect_h: *mut u32,
        ) -> u32 {
            unsafe { $crate::shell::editor::size_range($editor, min_w, min_h, max_w, max_h, aspect_w, aspect_h) }
        }

        #[cfg(any(target_os = "macos", windows))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_get_size(editor: *mut ::std::ffi::c_void, width: *mut u32, height: *mut u32) -> u32 {
            unsafe { $crate::shell::editor::get_size(editor, width, height) }
        }

        #[cfg(any(target_os = "macos", windows))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_adjust_size(editor: *mut ::std::ffi::c_void, width: *mut u32, height: *mut u32) -> u32 {
            unsafe { $crate::shell::editor::adjust_size(editor, width, height) }
        }

        #[cfg(any(target_os = "macos", windows))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_resize(editor: *mut ::std::ffi::c_void, width: u32, height: u32) -> u32 {
            unsafe { $crate::shell::editor::resize(editor, width, height) }
        }

        #[cfg(any(target_os = "macos", windows))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_set_request_resize(
            editor: *mut ::std::ffi::c_void,
            context: *mut ::std::ffi::c_void,
            request: ::std::option::Option<$crate::shell::editor::RequestResize>,
        ) {
            unsafe { $crate::shell::editor::set_request_resize(editor, context, request) }
        }

        #[cfg(any(target_os = "macos", windows))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_set_context_menu(
            editor: *mut ::std::ffi::c_void,
            context: *mut ::std::ffi::c_void,
            show: ::std::option::Option<$crate::shell::editor::ContextMenu>,
        ) {
            unsafe { $crate::shell::editor::set_context_menu(editor, context, show) }
        }

        #[cfg(any(target_os = "macos", windows))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_set_track_info(
            editor: *mut ::std::ffi::c_void,
            name: *const ::std::ffi::c_char,
            rgba: u32,
            flags: u32,
        ) {
            unsafe { $crate::shell::editor::set_track_info(editor, name, rgba, flags) }
        }

        #[cfg(windows)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_set_scale(editor: *mut ::std::ffi::c_void, scale: f64) -> u32 {
            unsafe { $crate::shell::editor::set_scale(editor, scale) }
        }

        #[cfg(windows)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_scale(editor: *mut ::std::ffi::c_void) -> f64 {
            unsafe { $crate::shell::editor::scale(editor) }
        }

        #[cfg(windows)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_set_pixel_size(editor: *mut ::std::ffi::c_void, width: u32, height: u32) -> u32 {
            unsafe { $crate::shell::editor::set_pixel_size(editor, width, height) }
        }

        #[cfg(any(target_os = "macos", windows))]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_editor_destroy(editor: *mut ::std::ffi::c_void) {
            $crate::shell::editor::destroy(editor)
        }
    };
    ($plugin:ty) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_abi_version() -> u32 {
            $crate::shell::ABI
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_kind() -> u32 {
            $crate::shell::kind::<$plugin>()
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_note_ports() -> u32 {
            $crate::shell::note_ports::<$plugin>()
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_layout_count() -> u32 {
            $crate::shell::layout_count::<$plugin>()
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_layout(index: u32, out: *mut $crate::shell::OpLayout) -> u32 {
            unsafe { $crate::shell::write_layout::<$plugin>(index, out) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_set_layout(instance: *mut ::std::ffi::c_void, index: u32) -> u32 {
            unsafe { $crate::shell::set_layout(instance as *mut $crate::shell::ShellInstance, index) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_layout_selected(instance: *mut ::std::ffi::c_void) -> u32 {
            unsafe { $crate::shell::layout_selected(instance as *mut $crate::shell::ShellInstance) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_set_offline(instance: *mut ::std::ffi::c_void, offline: u32) {
            unsafe { $crate::shell::set_offline(instance as *mut $crate::shell::ShellInstance, offline) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_set_track_info(
            instance: *mut ::std::ffi::c_void,
            name: *const ::std::ffi::c_char,
            rgba: u32,
            flags: u32,
        ) -> u32 {
            unsafe { $crate::shell::set_track_info(instance as *mut $crate::shell::ShellInstance, name, rgba, flags) }
        }

        // ABI 6: telling the host about changes.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_take_changes(instance: *mut ::std::ffi::c_void) -> u32 {
            unsafe { $crate::shell::take_changes(instance as *const $crate::shell::ShellInstance) }
        }

        // ABI 7: per-note expression.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_expressions() -> u32 {
            $crate::shell::expressions::<$plugin>()
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_expression_info(id: u32, out: *mut $crate::shell::OpExpressionInfo) -> u32 {
            unsafe { $crate::shell::expression_info::<$plugin>(id, out) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_expression_format(
            id: u32,
            value: f64,
            buf: *mut ::std::ffi::c_char,
            cap: usize,
        ) -> usize {
            unsafe { $crate::shell::expression_format(id, value, buf as *mut u8, cap) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_expression_parse(
            id: u32,
            text: *const ::std::ffi::c_char,
            out: *mut f64,
        ) -> i32 {
            unsafe { $crate::shell::expression_parse(id, text, out) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_process_events(
            instance: *mut ::std::ffi::c_void,
            args: *const $crate::shell::OpProcess,
        ) -> u32 {
            unsafe { $crate::shell::process_events(instance as *mut $crate::shell::ShellInstance, args) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_midi_out(
            instance: *mut ::std::ffi::c_void,
            out: *mut $crate::shell::OpMidiEvent,
            capacity: u32,
        ) -> u32 {
            unsafe { $crate::shell::midi_out(instance as *mut $crate::shell::ShellInstance, out, capacity) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_midi_dropped(instance: *const ::std::ffi::c_void) -> u32 {
            unsafe { $crate::shell::midi_dropped(instance as *const $crate::shell::ShellInstance) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_tail_seconds(instance: *mut ::std::ffi::c_void) -> f64 {
            $crate::shell::tail_seconds(instance as *const $crate::shell::ShellInstance)
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_info(out: *mut $crate::shell::OpShellInfo) -> u32 {
            unsafe { $crate::shell::write_info::<$plugin>(out) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_param(
            index: u32,
            out: *mut $crate::shell::OpParamDesc,
        ) -> u32 {
            unsafe { $crate::shell::write_param::<$plugin>(index, out) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_to_plain(index: u32, normalized: f64) -> f64 {
            $crate::shell::to_plain::<$plugin>(index, normalized)
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_to_normalized(index: u32, plain: f64) -> f64 {
            $crate::shell::to_normalized::<$plugin>(index, plain)
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_param_format(
            index: u32,
            plain: f64,
            buf: *mut ::std::ffi::c_char,
            cap: usize,
        ) -> usize {
            unsafe { $crate::shell::param_format::<$plugin>(index, plain, buf as *mut u8, cap) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_param_parse(
            index: u32,
            text: *const ::std::ffi::c_char,
            out_plain: *mut f64,
        ) -> i32 {
            unsafe { $crate::shell::param_parse::<$plugin>(index, text, out_plain) }
        }

        // ---- ABI 5: parameter groups ----
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_param_group(
            index: u32,
            buf: *mut ::std::ffi::c_char,
            cap: usize,
        ) -> usize {
            unsafe { $crate::shell::param_group::<$plugin>(index, buf as *mut u8, cap) }
        }

        // ---- ABI 8: identity pins ----
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_identity() -> u32 {
            $crate::shell::identity::<$plugin>()
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_param_host_id(format: u32, index: u32) -> u32 {
            $crate::shell::param_host_id::<$plugin>(format, index)
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_param_aax_id(
            index: u32,
            buf: *mut ::std::ffi::c_char,
            cap: usize,
        ) -> usize {
            unsafe { $crate::shell::param_aax_id::<$plugin>(index, buf as *mut u8, cap) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_au_code(which: u32) -> u32 {
            $crate::shell::au_code::<$plugin>(which)
        }

        // ---- ABI 9: the instance's own parameter table, and its largest latency ----
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_instance_param(
            instance: *const ::std::ffi::c_void,
            index: u32,
            out: *mut $crate::shell::OpParamDesc,
        ) -> u32 {
            unsafe { $crate::shell::instance_param::<$plugin>(instance as *const $crate::shell::ShellInstance, index, out) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_instance_param_format(
            instance: *const ::std::ffi::c_void,
            index: u32,
            plain: f64,
            buf: *mut ::std::ffi::c_char,
            cap: usize,
        ) -> usize {
            unsafe {
                $crate::shell::instance_param_format::<$plugin>(
                    instance as *const $crate::shell::ShellInstance,
                    index,
                    plain,
                    buf as *mut u8,
                    cap,
                )
            }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_instance_param_parse(
            instance: *const ::std::ffi::c_void,
            index: u32,
            text: *const ::std::ffi::c_char,
            out_plain: *mut f64,
        ) -> i32 {
            unsafe {
                $crate::shell::instance_param_parse::<$plugin>(
                    instance as *const $crate::shell::ShellInstance,
                    index,
                    text,
                    out_plain,
                )
            }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_max_latency(instance: *const ::std::ffi::c_void) -> u32 {
            unsafe { $crate::shell::max_latency(instance as *const $crate::shell::ShellInstance) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_latency_now(instance: *const ::std::ffi::c_void) -> u32 {
            unsafe { $crate::shell::latency_now(instance as *const $crate::shell::ShellInstance) }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_create(
            sample_rate: f64,
            max_block: u32,
            channels: u32,
        ) -> *mut ::std::ffi::c_void {
            $crate::shell::create::<$plugin>(sample_rate, max_block, channels)
                as *mut ::std::ffi::c_void
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_destroy(instance: *mut ::std::ffi::c_void) {
            $crate::shell::destroy(instance as *mut $crate::shell::ShellInstance);
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_reset(instance: *mut ::std::ffi::c_void) {
            $crate::shell::reset(instance as *mut $crate::shell::ShellInstance);
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_latency(instance: *const ::std::ffi::c_void) -> u32 {
            $crate::shell::latency(instance as *const $crate::shell::ShellInstance)
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_set_plain(
            instance: *mut ::std::ffi::c_void,
            index: u32,
            plain: f32,
        ) -> u32 {
            $crate::shell::set_plain(instance as *mut $crate::shell::ShellInstance, index, plain)
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_get_plain(
            instance: *const ::std::ffi::c_void,
            index: u32,
        ) -> f32 {
            $crate::shell::get_plain(instance as *const $crate::shell::ShellInstance, index)
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_set_normalized(
            instance: *mut ::std::ffi::c_void,
            index: u32,
            normalized: f32,
        ) -> u32 {
            $crate::shell::set_normalized(
                instance as *mut $crate::shell::ShellInstance,
                index,
                normalized,
            )
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_get_normalized(
            instance: *const ::std::ffi::c_void,
            index: u32,
        ) -> f32 {
            $crate::shell::get_normalized(instance as *const $crate::shell::ShellInstance, index)
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_save_state(
            instance: *mut ::std::ffi::c_void,
            out: *mut u8,
            capacity: u32,
            needed: *mut u32,
        ) -> u32 {
            unsafe {
                $crate::shell::save_state(
                    instance as *mut $crate::shell::ShellInstance,
                    out,
                    capacity,
                    needed,
                )
            }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_load_state(
            instance: *mut ::std::ffi::c_void,
            data: *const u8,
            length: u32,
        ) -> u32 {
            unsafe {
                $crate::shell::load_state(instance as *mut $crate::shell::ShellInstance, data, length)
            }
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn op_shell_process(
            instance: *mut ::std::ffi::c_void,
            in_left: *const f32,
            in_right: *const f32,
            out_left: *mut f32,
            out_right: *mut f32,
            frames: u32,
        ) -> u32 {
            $crate::shell::process(
                instance as *mut $crate::shell::ShellInstance,
                in_left,
                in_right,
                out_left,
                out_right,
                frames,
            )
        }
    };
}
