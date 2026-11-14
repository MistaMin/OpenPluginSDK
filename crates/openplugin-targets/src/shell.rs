//! `export_shell!` emits the C ABI in [`openplugin_ffi::shell`].
//!
//! A native VST3, Audio Unit, or AAX adapter links the plugin static library and
//! calls `op_shell_*`. One plugin binary exports one shell.
//!
//! `export_shell!(P, editor = make_editor)` also exports the editor ABI of
//! `native/include/op_editor.h` (`op_editor_*`, macOS), written here in Rust
//! over [`openplugin_dri::cocoa::EditorView`]. Build the shells with
//! `-DOP_EDITOR=ON` for it. Both forms export `op_shell_tail_seconds` from
//! [`Plugin::tail`](crate::PluginForExport::tail), for `-DOP_TAIL=ON`.

pub use openplugin_ffi::shell::{
    create, destroy, editor_state, get_normalized, get_plain, latency, process, reset,
    set_normalized, set_plain, tail_seconds, to_normalized, to_plain, trace, write_info,
    write_param, OpParamDesc, OpShellInfo, ShellInstance, ABI, AAX_MAX_PARAMS, PARAM_AUTOMATABLE,
    PARAM_DISCRETE, PARAM_LOG,
};

#[cfg(target_os = "macos")]
pub use editor::OpEditorHost;

/// The editor ABI (`op_editor.h`). The functions take the plugin's
/// [`EditorFactory`](openplugin_core::EditorFactory); `export_shell!` wraps
/// them in the `#[no_mangle]` symbols the shells link.
#[cfg(target_os = "macos")]
pub mod editor {
    use super::ShellInstance;
    use openplugin_core::{EditorFactory, EditorHost, Gesture};
    use openplugin_dri::cocoa::EditorView;
    use std::ffi::c_void;
    use std::sync::OnceLock;

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
        let Some(mut editor) = factory(editor_host) else { return std::ptr::null_mut() };
        editor.bind(None);
        match EditorView::new(editor, None) {
            Some(view) => Box::into_raw(Box::new(view)) as *mut c_void,
            None => std::ptr::null_mut(),
        }
    }

    unsafe fn view<'a>(editor: *mut c_void) -> Option<&'a mut EditorView> {
        unsafe { (editor as *mut EditorView).as_mut() }
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

    /// `op_editor_destroy`.
    ///
    /// # Safety
    /// `editor` is null or from [`create`], and not used again.
    pub unsafe fn destroy(editor: *mut c_void) {
        if !editor.is_null() {
            drop(unsafe { Box::from_raw(editor as *mut EditorView) });
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

        #[cfg(target_os = "macos")]
        #[no_mangle]
        pub unsafe extern "C" fn op_editor_size(width: *mut u32, height: *mut u32) -> u32 {
            $crate::shell::editor::size($editor, width, height)
        }

        #[cfg(target_os = "macos")]
        #[no_mangle]
        pub unsafe extern "C" fn op_editor_create(host: *const $crate::shell::OpEditorHost) -> *mut ::std::ffi::c_void {
            $crate::shell::editor::create($editor, host)
        }

        #[cfg(target_os = "macos")]
        #[no_mangle]
        pub unsafe extern "C" fn op_editor_view(editor: *mut ::std::ffi::c_void) -> *mut ::std::ffi::c_void {
            $crate::shell::editor::raw_view(editor)
        }

        #[cfg(target_os = "macos")]
        #[no_mangle]
        pub unsafe extern "C" fn op_editor_attach(editor: *mut ::std::ffi::c_void, parent: *mut ::std::ffi::c_void) -> u32 {
            $crate::shell::editor::attach(editor, parent)
        }

        #[cfg(target_os = "macos")]
        #[no_mangle]
        pub unsafe extern "C" fn op_editor_bind(editor: *mut ::std::ffi::c_void, instance: *mut ::std::ffi::c_void) {
            $crate::shell::editor::bind(editor, instance)
        }

        #[cfg(target_os = "macos")]
        #[no_mangle]
        pub unsafe extern "C" fn op_editor_set_plain(editor: *mut ::std::ffi::c_void, index: u32, plain: f32) {
            $crate::shell::editor::set_plain(editor, index, plain)
        }

        #[cfg(target_os = "macos")]
        #[no_mangle]
        pub unsafe extern "C" fn op_editor_destroy(editor: *mut ::std::ffi::c_void) {
            $crate::shell::editor::destroy(editor)
        }
    };
    ($plugin:ty) => {
        #[no_mangle]
        pub unsafe extern "C" fn op_shell_tail_seconds(instance: *mut ::std::ffi::c_void) -> f64 {
            $crate::shell::tail_seconds(instance as *const $crate::shell::ShellInstance)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_info(out: *mut $crate::shell::OpShellInfo) -> u32 {
            $crate::shell::write_info::<$plugin>(out)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_param(
            index: u32,
            out: *mut $crate::shell::OpParamDesc,
        ) -> u32 {
            $crate::shell::write_param::<$plugin>(index, out)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_to_plain(index: u32, normalized: f64) -> f64 {
            $crate::shell::to_plain::<$plugin>(index, normalized)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_to_normalized(index: u32, plain: f64) -> f64 {
            $crate::shell::to_normalized::<$plugin>(index, plain)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_create(
            sample_rate: f64,
            max_block: u32,
            channels: u32,
        ) -> *mut ::std::ffi::c_void {
            $crate::shell::create::<$plugin>(sample_rate, max_block, channels)
                as *mut ::std::ffi::c_void
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_destroy(instance: *mut ::std::ffi::c_void) {
            $crate::shell::destroy(instance as *mut $crate::shell::ShellInstance);
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_reset(instance: *mut ::std::ffi::c_void) {
            $crate::shell::reset(instance as *mut $crate::shell::ShellInstance);
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_latency(instance: *const ::std::ffi::c_void) -> u32 {
            $crate::shell::latency(instance as *const $crate::shell::ShellInstance)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_set_plain(
            instance: *mut ::std::ffi::c_void,
            index: u32,
            plain: f32,
        ) -> u32 {
            $crate::shell::set_plain(instance as *mut $crate::shell::ShellInstance, index, plain)
        }

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_get_plain(
            instance: *const ::std::ffi::c_void,
            index: u32,
        ) -> f32 {
            $crate::shell::get_plain(instance as *const $crate::shell::ShellInstance, index)
        }

        #[no_mangle]
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

        #[no_mangle]
        pub unsafe extern "C" fn op_shell_get_normalized(
            instance: *const ::std::ffi::c_void,
            index: u32,
        ) -> f32 {
            $crate::shell::get_normalized(instance as *const $crate::shell::ShellInstance, index)
        }

        #[no_mangle]
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
