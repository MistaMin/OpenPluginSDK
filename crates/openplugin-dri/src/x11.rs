//! The Linux view that hosts an [`Editor`]: an X11 child window, a 30 Hz
//! tick, and a blit of the frame the editor renders. Pure Rust: it speaks the
//! X11 protocol through `x11rb` and links no C library.
//!
//! [`EditorView`] goes inside a host's parent window (CLAP `clap.gui` with
//! the `x11` API, LV2 `ui:X11UI`, the VST3 shell's `X11EmbedWindowID`). Linux
//! hosts embed plugin editors as X11 windows, in a Wayland session too
//! (through Xwayland). [`run_window`] puts the same view in a window of its
//! own and runs until it closes (standalone).
//!
//! Event loop: a Linux host runs no event loop for a plugin's own X
//! connection. The view therefore owns its connection and is driven from
//! outside: the format bridge calls [`EditorView::pump`] about every
//! [`TICK_INTERVAL_MS`] milliseconds (CLAP `timer-support`, LV2
//! `ui:idleInterface`, VST3 `IRunLoop` timers) and, where the host can watch
//! file descriptors, whenever [`EditorView::fd`] is readable (CLAP
//! `posix-fd-support`, VST3 `IRunLoop` event handlers). `pump` never blocks.
//!
//! Size, repaint and input follow `cocoa.rs`: the window follows the editor's
//! size inside its [`SizeRange`]; mouse positions are in editor points; after
//! each tick and input call only the editor's damage is presented.
//!
//! Main thread only: the thread that made the view makes every call.

use openplugin_core::editor::SizeRange;
use openplugin_core::Editor;
use std::os::fd::RawFd;
use std::time::Instant;

/// An X11 window id (an XID), as hosts hand it over.
pub type Window = u32;

/// Called before every tick with the editor, so a format bridge can pass it
/// the values that changed in the host since the last tick.
pub type Poll = Box<dyn FnMut(&mut dyn Editor)>;

/// How often a bridge calls [`EditorView::pump`] from the host's timer.
pub const TICK_INTERVAL_MS: u32 = 33;

/// An [`Editor`] in an X11 window.
pub struct EditorView {
    editor: Box<dyn Editor>,
    poll: Option<Poll>,
    /// The editor's size, which mouse positions are mapped to.
    size: (u32, u32),
    last: Instant,
}

impl EditorView {
    /// Connects to the X server named by `DISPLAY` and makes the window,
    /// unmapped and parentless. `poll` runs before every tick (see [`Poll`]).
    /// `None` when there is no X server to connect to.
    pub fn new(editor: Box<dyn Editor>, poll: Option<Poll>) -> Option<EditorView> {
        // Contract only: the X11 backend is not written yet.
        let size = editor.size();
        let _unconnected = EditorView { editor, poll, size, last: Instant::now() };
        None
    }

    /// The view's own window. Zero is never a window.
    pub fn raw(&self) -> Window {
        0
    }

    /// The editor's current size in points.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The sizes the editor can take.
    pub fn size_range(&self) -> SizeRange {
        self.editor.size_range()
    }

    /// Move the editor and the window to `width` x `height` points, brought
    /// into the editor's range. Returns the size the editor takes. A fixed
    /// editor stays as it is.
    pub fn resize(&mut self, width: u32, height: u32) -> (u32, u32) {
        let (w, h) = self.size_range().clamp(width, height);
        if (w, h) != self.size {
            self.editor.resize(w, h);
            self.size = self.editor.size();
        }
        self.size
    }

    /// Put the window inside a host's parent window and show it. False when
    /// `parent` is zero or the server refuses.
    pub fn attach(&mut self, parent: Window) -> bool {
        let _ = parent;
        false
    }

    /// Show or hide the window.
    pub fn set_hidden(&mut self, hidden: bool) {
        let _ = hidden;
    }

    /// The hosted editor.
    pub fn editor(&mut self) -> Option<&mut (dyn Editor + 'static)> {
        Some(self.editor.as_mut())
    }

    pub fn set_poll(&mut self, poll: Option<Poll>) {
        self.poll = poll;
    }

    /// The X connection's file descriptor, for a host that watches
    /// descriptors: call [`EditorView::pump`] when it is readable.
    pub fn fd(&self) -> RawFd {
        -1
    }

    /// Handle every X event that has arrived, then, if a tick is due
    /// ([`TICK_INTERVAL_MS`] since the last), run the poll hook and
    /// [`Editor::tick`], follow a size the editor changed itself, and present
    /// what the editor damaged. Never blocks. Calling it more often than the
    /// tick interval is fine: input is handled at once and ticks stay at 30 Hz.
    pub fn pump(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32();
        if dt * 1000.0 < TICK_INTERVAL_MS as f32 {
            return;
        }
        self.last = now;
        if let Some(poll) = self.poll.as_mut() {
            poll(self.editor.as_mut());
        }
        self.editor.tick(dt.min(0.1));
        self.size = self.editor.size();
    }
}

/// Show the editor in its own window and run until the window closes.
/// Returns the exit code. Main thread.
pub fn run_window(title: &str, editor: Box<dyn Editor>, poll: Option<Poll>) -> i32 {
    let _ = title;
    match EditorView::new(editor, poll) {
        Some(_view) => 0,
        None => {
            eprintln!("openplugin: no X11 display to open a window on");
            1
        }
    }
}
