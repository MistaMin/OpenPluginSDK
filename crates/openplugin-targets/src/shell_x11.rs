//! The editor ABI's view on Linux: [`openplugin_dri::x11::EditorView`] with
//! the calls the shared `shell::editor` module makes of a platform view, so
//! that module needs no Linux code of its own. `native/include/op_editor_linux.h`
//! is the C side.
//!
//! What differs from the macOS view:
//!
//! - The window handle is an X11 window id, not a pointer: `op_editor_view`
//!   returns the id as a pointer-sized integer and `op_editor_attach` takes the
//!   parent's id the same way, which is how VST3 passes it
//!   (`kPlatformTypeX11EmbedWindowID`).
//! - Nobody runs an event loop for the view. The shell asks the host's run
//!   loop (VST3 `Linux::IRunLoop`) for a timer of [`TICK_INTERVAL_MS`] and for
//!   the descriptor [`EditorView::fd`], and calls [`EditorView::pump`] from both.
//! - A Linux host sizes a view in pixels. Sizes here stay in points; the shell
//!   converts with [`EditorView::scale`], or [`display_scale`] before a view
//!   exists.
//!
//! Main thread only, like the view.

use openplugin_core::editor::SizeRange;
use openplugin_core::Editor;
use openplugin_dri::x11;
use std::ffi::c_void;
use std::os::fd::RawFd;
use std::sync::OnceLock;

pub use openplugin_dri::x11::TICK_INTERVAL_MS;

/// [`x11::EditorView`] behind the names `shell::editor` uses.
pub struct EditorView(x11::EditorView);

impl EditorView {
    /// `None` when there is no X server to connect to.
    pub fn new(editor: Box<dyn Editor>, poll: Option<x11::Poll>) -> Option<EditorView> {
        x11::EditorView::new(editor, poll).map(EditorView)
    }

    /// The editor's current size in points.
    pub fn size(&self) -> (u32, u32) {
        self.0.size()
    }

    pub fn size_range(&self) -> SizeRange {
        self.0.size_range()
    }

    pub fn resize(&mut self, width: u32, height: u32) -> (u32, u32) {
        self.0.resize(width, height)
    }

    pub fn editor(&mut self) -> Option<&mut (dyn Editor + 'static)> {
        self.0.editor()
    }

    /// The view's X11 window id as a pointer-sized integer.
    pub fn raw(&self) -> *mut c_void {
        self.0.raw() as usize as *mut c_void
    }

    /// Puts the view's window inside `parent`, an X11 window id as a
    /// pointer-sized integer. False for zero, for a value that is no window
    /// id (an id has 32 bits), and when the server refuses.
    ///
    /// # Safety
    /// None of its own: `parent` is a number, never dereferenced. Unsafe to
    /// match the other platforms' views, whose parent is a pointer.
    pub unsafe fn attach(&mut self, parent: *mut c_void) -> bool {
        match x11::Window::try_from(parent as usize) {
            Ok(id) => self.0.attach(id),
            Err(_) => false,
        }
    }

    /// Device pixels per point.
    pub fn scale(&self) -> f32 {
        self.0.scale()
    }

    /// The X connection's descriptor.
    pub fn fd(&self) -> RawFd {
        self.0.fd()
    }

    pub fn pump(&mut self) {
        self.0.pump()
    }
}

/// `op_editor_display_scale`: pixels per point a view made now would have,
/// for the sizes a host asks before an editor exists. Read from the display
/// once; 1.0 when there is no X server.
pub fn display_scale() -> f64 {
    static SCALE: OnceLock<f64> = OnceLock::new();
    *SCALE.get_or_init(|| x11::display_scale().map(f64::from).filter(|s| s.is_finite() && *s > 0.0).unwrap_or(1.0))
}
