//! The shell's editor ABI on Linux (`native/include/op_editor.h` with
//! `op_editor_linux.h`), which the VST3 shell links: the `op_editor_*` symbols
//! `export_shell!(P, editor = ...)` exports, called as a shell calls them, with
//! the example editor in the X11 view on the display `DISPLAY` names. The
//! native `vst3_editor_proof` checks the same through the VST3 shell; this
//! test needs no CMake and no VST3 SDK. It has no harness: everything runs on
//! the process's main thread, as the view needs. When `DISPLAY` is not set it
//! prints `skipped: DISPLAY is not set` and passes; with `DISPLAY` set, a
//! server that does not answer fails the test.
//!
//! The test plays the shell and its host on a connection of its own: it makes
//! the parent window, sends a click as X events, and pumps the view.
//!
//! - Before a parent: the editor has a window id but no window; attach to no
//!   window, and to a number that is no window id, fails.
//! - Attached: the window is a child of the parent, on screen, and
//!   `points x op_editor_scale` pixels; the scale is the display's
//!   (`op_editor_display_scale`); the descriptor is the view's X connection.
//! - Driven: pumping when the descriptor is readable and every
//!   `op_editor_tick_ms` shows the editor's picture, and a click on the Warm
//!   toggle reaches the `OpEditorHost` as begin, perform, end.
//! - Sizes stay in points: `op_editor_resize` inside the range resizes the
//!   window to the points times the scale; outside the range it is refused.
//! - `op_editor_destroy` takes the window out. Null editors answer 0 or -1.
//!
//! The windows are small, unmanaged and short-lived. As in `x11.rs`, the host
//! skips the first ids of its connection (Hyprland keeps stale geometry for
//! them).

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("editor x11 shell: skipped: Linux only (0 tests run)");
}

#[cfg(target_os = "linux")]
fn main() {
    linux::run();
}

#[cfg(target_os = "linux")]
mod linux {
    use example_editor::{layout, HEIGHT, MAX_SIZE, MIN_SIZE, WIDTH};
    use openplugin::targets::shell::OpEditorHost;
    use std::ffi::{c_ulong, c_void};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{self, ConnectionExt as _, CreateWindowAux, EventMask, ImageFormat, KeyButMask, MapState, WindowClass};
    use x11rb::rust_connection::RustConnection;

    const DEADLINE: Duration = Duration::from_secs(10);
    const NAME: &str = "editor x11 shell";

    type Id = *mut c_void;

    unsafe extern "C" {
        fn op_editor_size(width: *mut u32, height: *mut u32) -> u32;
        fn op_editor_create(host: *const OpEditorHost) -> Id;
        fn op_editor_view(editor: Id) -> Id;
        fn op_editor_attach(editor: Id, parent: Id) -> u32;
        fn op_editor_destroy(editor: Id);
        fn op_editor_get_size(editor: Id, width: *mut u32, height: *mut u32) -> u32;
        fn op_editor_resize(editor: Id, width: u32, height: u32) -> u32;
        fn op_editor_scale(editor: Id) -> f64;
        fn op_editor_display_scale() -> f64;
        fn op_editor_tick_ms() -> u32;
        fn op_editor_fd(editor: Id) -> i32;
        fn op_editor_pump(editor: Id);
    }

    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    const POLLIN: i16 = 1;

    unsafe extern "C" {
        fn poll(fds: *mut PollFd, count: c_ulong, timeout_ms: i32) -> i32;
    }

    // What the editor told its host.
    static BEGINS: AtomicU32 = AtomicU32::new(0);
    static PERFORMS: AtomicU32 = AtomicU32::new(0);
    static ENDS: AtomicU32 = AtomicU32::new(0);
    static LAST: AtomicU32 = AtomicU32::new(u32::MAX);

    unsafe extern "C" fn begin(_: *mut c_void, index: u32) {
        BEGINS.fetch_add(1, Ordering::Relaxed);
        LAST.store(index, Ordering::Relaxed);
    }
    unsafe extern "C" fn perform(_: *mut c_void, _: u32, _: f64) {
        PERFORMS.fetch_add(1, Ordering::Relaxed);
    }
    unsafe extern "C" fn end(_: *mut c_void, _: u32) {
        ENDS.fetch_add(1, Ordering::Relaxed);
    }

    /// The test's connection to the display. `None` (the test skips) only when
    /// `DISPLAY` is not set; see `x11.rs`.
    fn connect() -> Option<(RustConnection, usize)> {
        if std::env::var_os("DISPLAY").is_none_or(|d| d.is_empty()) {
            println!("{NAME}: skipped: DISPLAY is not set");
            return None;
        }
        let start = Instant::now();
        loop {
            match RustConnection::connect(None) {
                Ok(connection) => return Some(connection),
                Err(error) => {
                    assert!(start.elapsed() < DEADLINE, "{NAME}: DISPLAY is set, but no X server answered: {error}");
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    struct Host {
        conn: RustConnection,
        root: u32,
        parent: u32,
    }

    impl Host {
        fn sync(&self) {
            self.conn.get_input_focus().unwrap().reply().unwrap();
        }
        fn children(&self, window: u32) -> Vec<u32> {
            self.conn.query_tree(window).unwrap().reply().map(|r| r.children).unwrap_or_default()
        }
        fn size(&self, window: u32) -> Option<(u32, u32)> {
            let g = self.conn.get_geometry(window).ok()?.reply().ok()?;
            Some((u32::from(g.width), u32::from(g.height)))
        }
        fn viewable(&self, window: u32) -> bool {
            self.conn.get_window_attributes(window).ok().and_then(|c| c.reply().ok()).is_some_and(|a| a.map_state == MapState::VIEWABLE)
        }
        /// Whether the window shows more than one colour.
        fn painted(&self, window: u32) -> bool {
            let Some((w, h)) = self.size(window) else { return false };
            let Ok(reply) = self.conn.get_image(ImageFormat::Z_PIXMAP, window, 0, 0, w as u16, h as u16, !0).unwrap().reply() else { return false };
            let step = reply.data.len() / (w * h).max(1) as usize;
            step > 0 && reply.data.chunks_exact(step).any(|pixel| pixel != &reply.data[..step])
        }
        fn button(&self, window: u32, press: bool, (x, y): (i16, i16), time: u32) {
            let event = xproto::ButtonPressEvent {
                response_type: if press { xproto::BUTTON_PRESS_EVENT } else { xproto::BUTTON_RELEASE_EVENT },
                detail: 1,
                sequence: 0,
                time,
                root: self.root,
                event: window,
                child: 0,
                root_x: x,
                root_y: y,
                event_x: x,
                event_y: y,
                state: if press { KeyButMask::default() } else { KeyButMask::BUTTON1 },
                same_screen: true,
            };
            let mask = if press { EventMask::BUTTON_PRESS } else { EventMask::BUTTON_RELEASE };
            self.conn.send_event(false, window, mask, event).unwrap();
        }
    }

    /// One turn of a host's run loop for the editor: wait for the descriptor or
    /// the tick, then pump.
    fn turn(editor: Id) {
        let mut watched = [PollFd { fd: unsafe { op_editor_fd(editor) }, events: POLLIN, revents: 0 }];
        unsafe {
            poll(watched.as_mut_ptr(), 1, op_editor_tick_ms() as i32);
            op_editor_pump(editor);
        }
    }

    fn until(what: &str, editor: Id, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        loop {
            turn(editor);
            if done() {
                return;
            }
            assert!(start.elapsed() < DEADLINE, "timed out waiting for {what}");
        }
    }

    fn as_id(window: u32) -> Id {
        window as usize as Id
    }

    pub fn run() {
        let Some((conn, screen)) = connect() else { return };
        let root = conn.setup().roots[screen].root;
        for _ in 0..16 {
            conn.generate_id().expect("an id from the server");
        }
        let parent = conn.generate_id().expect("an id from the server");
        let aux = CreateWindowAux::new().override_redirect(1).background_pixel(0);
        conn.create_window(0, parent, root, 520, 40, 500, 340, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap().check().expect("the parent window");
        conn.map_window(parent).unwrap().check().expect("the parent window on screen");
        let host = Host { conn, root, parent };

        unsafe {
            // Null editors.
            assert_eq!(op_editor_scale(std::ptr::null_mut()), 0.0);
            assert_eq!(op_editor_fd(std::ptr::null_mut()), -1);
            op_editor_pump(std::ptr::null_mut());
            assert!(op_editor_view(std::ptr::null_mut()).is_null());
            assert_eq!(op_editor_tick_ms(), openplugin::dri::x11::TICK_INTERVAL_MS);

            let (mut w, mut h) = (0u32, 0u32);
            assert_eq!(op_editor_size(&mut w, &mut h), 1);
            assert_eq!((w, h), (WIDTH, HEIGHT), "the size is in points");
            let display_scale = op_editor_display_scale();
            assert!((1.0..=3.0).contains(&display_scale), "display scale {display_scale}");

            let callbacks = OpEditorHost { context: std::ptr::null_mut(), begin_edit: Some(begin), perform_edit: Some(perform), end_edit: Some(end) };
            let editor = op_editor_create(&callbacks);
            assert!(!editor.is_null(), "an editor on the display");
            let scale = op_editor_scale(editor);
            assert_eq!(scale, display_scale, "the view's scale is the display's");
            let fd = op_editor_fd(editor);
            assert!(fd >= 0);
            let px = |points: u32| (f64::from(points) * scale).round() as u32;

            // Before a parent: an id, no window.
            let view = op_editor_view(editor) as usize as u32;
            assert_ne!(view, 0);
            host.sync();
            assert!(host.size(view).is_none(), "no window before attach");
            assert_eq!(op_editor_attach(editor, std::ptr::null_mut()), 0);
            assert_eq!(op_editor_attach(editor, (1usize << 40) as Id), 0, "more than 32 bits is no window id");
            assert!(host.children(host.parent).is_empty());

            // Attached.
            assert_eq!(op_editor_attach(editor, as_id(host.parent)), 1);
            host.sync();
            assert_eq!(host.children(host.parent), vec![view]);
            assert_eq!(host.size(view), Some((px(WIDTH), px(HEIGHT))));
            assert!(host.viewable(view));
            assert_eq!(op_editor_get_size(editor, &mut w, &mut h), 1);
            assert_eq!((w, h), (WIDTH, HEIGHT));

            // Driven from outside: the picture arrives, and a click is heard.
            until("the editor's picture", editor, || host.painted(view));
            let (sx, sy, sw, sh) = layout(WIDTH, HEIGHT).switch;
            let at = ((f64::from(sx + sw * 0.5) * scale).round() as i16, (f64::from(sy + sh * 0.5) * scale).round() as i16);
            host.button(view, true, at, 200_000);
            host.button(view, false, at, 200_001);
            host.sync();
            until("the click on Warm", editor, || ENDS.load(Ordering::Relaxed) == 1);
            assert_eq!((BEGINS.load(Ordering::Relaxed), ENDS.load(Ordering::Relaxed), LAST.load(Ordering::Relaxed)), (1, 1, 1));
            assert!(PERFORMS.load(Ordering::Relaxed) >= 1);

            // Sizes in points; the window follows in pixels.
            assert_eq!(op_editor_resize(editor, MAX_SIZE.0, MAX_SIZE.1), 1);
            until("the larger window", editor, || host.size(view) == Some((px(MAX_SIZE.0), px(MAX_SIZE.1))));
            assert_eq!(op_editor_get_size(editor, &mut w, &mut h), 1);
            assert_eq!((w, h), MAX_SIZE);
            assert_eq!(op_editor_resize(editor, MAX_SIZE.0 + 1, MAX_SIZE.1), 0, "outside the range");
            assert_eq!(op_editor_resize(editor, MIN_SIZE.0 - 1, MIN_SIZE.1), 0, "outside the range");
            assert_eq!(op_editor_get_size(editor, &mut w, &mut h), 1);
            assert_eq!((w, h), MAX_SIZE);

            op_editor_destroy(editor);
            host.sync();
            assert!(host.children(host.parent).is_empty(), "the window goes with the editor");
        }
        let _ = host.conn.destroy_window(host.parent).unwrap().check();
        println!("x11 shell: 1 passed");
    }
}
