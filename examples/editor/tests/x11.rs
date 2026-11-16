//! The editor in real X11 windows, through the plugin formats, on the X
//! display `DISPLAY` names (this test has no harness: everything runs on the
//! main thread, one part after the other). When `DISPLAY` is not set it prints
//! `skipped: DISPLAY is not set` and passes; with `DISPLAY` set, a server that
//! does not answer fails the test.
//!
//! The test is the host. It has an X connection of its own, makes the parent
//! window, reads the screen back with `GetImage`, sends input with `SendEvent`
//! and runs the host's loop: it watches the descriptors the plugin registered
//! with `poll` and fires the timers the plugin registered.
//!
//! - CLAP: `clap.gui` with the `x11` API creates, parents, shows, hides and
//!   destroys the view; `clap.timer-support` and `clap.posix-fd-support` drive
//!   it, each alone too; the frame on screen is the editor's own render of the
//!   same state; a drag and a click reach the host as `request_flush`, gesture
//!   and value events; a host value repaints; destroy unregisters the timer and
//!   the descriptor and takes the window away, with the parent gone as well.
//! - LV2: `lv2ui_descriptor` makes the `ui:X11UI` widget inside `ui:parent`,
//!   `ui:idleInterface` drives it, a click writes the control port and touches
//!   it, `port_event` repaints, `ui:resize` works both ways.
//! - Resizing in both: `adjust_size`, `set_size`, the host's `ui:resize`, and
//!   the editor asking its host (a double-click on the plate).
//! - Keys through CLAP: a click takes the input focus, arrows are gestures,
//!   keys the editor does not use arrive at the parent window.
//! - Lifecycle: twenty editors opened and closed in each format leave no
//!   window, no descriptor and no X error behind.
//!
//! Hyprland never forgets a window first made on the root that it did not see
//! destroyed, and later moves windows that reuse such an id, so the host skips
//! the first ids of its connection, and every parent has a place of its own.

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("editor x11: skipped: Linux only (0 tests run)");
}

#[cfg(target_os = "linux")]
fn main() {
    linux::run();
}

#[cfg(target_os = "linux")]
mod linux {
    use example_editor::*;
    use openplugin::core::{Editor, EditorHost};
    use openplugin::dri::x11::{EditorView, TICK_INTERVAL_MS};
    use openplugin::targets::clap_abi::*;
    use openplugin::targets::lv2::{Lv2Feature, Lv2UiDescriptor, AUDIO_PORTS};
    use std::cell::{Cell, RefCell};
    use std::ffi::{c_char, c_ulong, c_void, CStr};
    use std::time::{Duration, Instant};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{self, ConnectionExt as _, CreateWindowAux, EventMask, ImageFormat, ImageOrder, KeyButMask, MapState, WindowClass};
    use x11rb::protocol::Event;
    use x11rb::rust_connection::RustConnection;

    const DEADLINE: Duration = Duration::from_secs(10);

    /// The test's connection to the display. `None` (the test skips) only when
    /// `DISPLAY` is not set. With `DISPLAY` set, a refused connection is tried
    /// again until the deadline (an X server with no client left resets, and
    /// refuses connections meanwhile) and then fails the test: a display that is
    /// named must be there.
    fn connect(test: &str) -> Option<(RustConnection, usize)> {
        if std::env::var_os("DISPLAY").is_none_or(|d| d.is_empty()) {
            println!("{test}: skipped: DISPLAY is not set");
            return None;
        }
        let start = Instant::now();
        loop {
            match RustConnection::connect(None) {
                Ok(connection) => return Some(connection),
                Err(error) => {
                    assert!(start.elapsed() < DEADLINE, "{test}: DISPLAY is set, but no X server answered: {error}");
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    pub fn run() {
        let Some(desk) = Desk::open() else { return };
        clap_gui_lifecycle(&desk);
        clap_gui_resizes(&desk);
        lv2_ui_lifecycle(&desk);
        lv2_ui_resizes(&desk);
        clap_gui_keys(&desk);
        editors_open_and_close_without_leaks(&desk);
        println!("x11: 6 passed");
    }

    // ---- the host's side of the display ----

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

    /// Wait a few milliseconds, or less when a descriptor is readable. Returns the readable ones.
    fn wait_on(fds: &[i32], ms: i32) -> Vec<i32> {
        let mut watched: Vec<PollFd> = fds.iter().map(|&fd| PollFd { fd, events: POLLIN, revents: 0 }).collect();
        unsafe { poll(watched.as_mut_ptr(), watched.len() as c_ulong, ms) };
        watched.iter().filter(|w| w.revents != 0).map(|w| w.fd).collect()
    }

    /// Pixels read back from a window, or rendered by the editor: `0xRRGGBB`.
    struct Image {
        w: usize,
        h: usize,
        px: Vec<u32>,
    }

    impl Image {
        /// How many pixels differ; all of them when the sizes do.
        fn differing(&self, other: &Image) -> usize {
            if (self.w, self.h) != (other.w, other.h) {
                return usize::MAX;
            }
            self.px.iter().zip(&other.px).filter(|(a, b)| a != b).count()
        }

        fn is_blank(&self) -> bool {
            self.px.iter().all(|&p| p == self.px[0])
        }

        /// The pixel at an editor point.
        fn at(&self, scale: f32, x: f32, y: f32) -> u32 {
            let (x, y) = (((x * scale) as usize).min(self.w - 1), ((y * scale) as usize).min(self.h - 1));
            self.px[y * self.w + x]
        }
    }

    struct Desk {
        conn: RustConnection,
        root: u32,
        screen: (u32, u32),
        /// Device pixels per editor point, as the view reads it from the display.
        scale: f32,
        slot: Cell<u32>,
        clock: Cell<u32>,
    }

    impl Desk {
        fn open() -> Option<Desk> {
            let (conn, screen) = connect("editor x11")?;
            let scale = EditorView::new(Box::new(GainEditor::new(EditorHost::detached())), None).expect("the test connected to the display, but the view did not").scale();
            let (root, size) = {
                let s = &conn.setup().roots[screen];
                (s.root, (u32::from(s.width_in_pixels), u32::from(s.height_in_pixels)))
            };
            // See the module notes: the first ids are the ones Hyprland may remember.
            for _ in 0..16 {
                conn.generate_id().expect("an id from the server");
            }
            Some(Desk { conn, root, screen: size, scale, slot: Cell::new(0), clock: Cell::new(10_000) })
        }

        /// Points to device pixels, rounded as the view and the bridges round.
        fn px(&self, (w, h): (u32, u32)) -> (u32, u32) {
            let px = |v: u32| (v as f32 * self.scale).round() as u32;
            (px(w), px(h))
        }

        /// A parent window: unmanaged, mapped, large enough for the largest size used, in a place of its own.
        fn parent(&self) -> u32 {
            let (w, h) = self.px(LARGE_SIZE);
            let cell = (w + 30, h + 30);
            let slot = self.slot.get();
            self.slot.set(slot + 1);
            let (columns, rows) = ((self.screen.0 / cell.0).max(1), (self.screen.1 / cell.1).max(1));
            let (x, y) = ((slot % columns) * cell.0, (slot / columns % rows) * cell.1);
            let id = self.conn.generate_id().unwrap();
            let aux = CreateWindowAux::new().override_redirect(1).background_pixel(0).event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE);
            self.conn
                .create_window(0, id, self.root, x as i16, y as i16, (cell.0 - 10) as u16, (cell.1 - 10) as u16, 0, WindowClass::INPUT_OUTPUT, 0, &aux)
                .unwrap()
                .check()
                .unwrap();
            self.conn.map_window(id).unwrap().check().unwrap();
            id
        }

        fn close(&self, window: u32) {
            let _ = self.conn.destroy_window(window).unwrap().check();
        }

        /// Returns once the server has handled everything sent so far.
        fn sync(&self) {
            self.conn.get_input_focus().unwrap().reply().unwrap();
        }

        fn geometry(&self, window: u32) -> Option<(u32, u32)> {
            let g = self.conn.get_geometry(window).ok()?.reply().ok()?;
            Some((u32::from(g.width), u32::from(g.height)))
        }

        fn exists(&self, window: u32) -> bool {
            self.geometry(window).is_some()
        }

        /// The window's children, `None` when the window is gone.
        fn children(&self, window: u32) -> Option<Vec<u32>> {
            Some(self.conn.query_tree(window).ok()?.reply().ok()?.children)
        }

        fn parent_of(&self, window: u32) -> Option<u32> {
            Some(self.conn.query_tree(window).ok()?.reply().ok()?.parent)
        }

        /// Every window the server has below the root.
        fn window_count(&self) -> usize {
            let (mut count, mut stack) = (0, vec![self.root]);
            while let Some(window) = stack.pop() {
                if let Some(children) = self.children(window) {
                    count += children.len();
                    stack.extend(children);
                }
            }
            count
        }

        fn map_state(&self, window: u32) -> Option<MapState> {
            Some(self.conn.get_window_attributes(window).ok()?.reply().ok()?.map_state)
        }

        fn selects_keys(&self, window: u32) -> bool {
            let attributes = self.conn.get_window_attributes(window).unwrap().reply().unwrap();
            attributes.all_event_masks.contains(EventMask::KEY_PRESS)
        }

        fn focus(&self) -> u32 {
            self.conn.get_input_focus().unwrap().reply().unwrap().focus
        }

        /// What is on screen in a window, `None` while it cannot be read.
        fn image(&self, window: u32) -> Option<Image> {
            let g = self.conn.get_geometry(window).ok()?.reply().ok()?;
            let (w, h) = (usize::from(g.width), usize::from(g.height));
            let reply = self.conn.get_image(ImageFormat::Z_PIXMAP, window, 0, 0, g.width, g.height, !0).ok()?.reply().ok()?;
            if reply.data.len() != w * h * 4 {
                return None;
            }
            let msb = self.conn.setup().image_byte_order == ImageOrder::MSB_FIRST;
            let word = |b: &[u8]| {
                let b = [b[0], b[1], b[2], b[3]];
                (if msb { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) }) & 0x00ff_ffff
            };
            Some(Image { w, h, px: (0..w * h).map(|i| word(&reply.data[i * 4..i * 4 + 4])).collect() })
        }

        /// A time for a synthetic click: a second after the last, so clicks are single ones.
        fn tick(&self) -> u32 {
            self.clock.set(self.clock.get() + 1000);
            self.clock.get()
        }

        /// The device position the server is told for an editor point, and the
        /// point the view makes of it again in an editor of `size` points.
        fn place(&self, size: (u32, u32), (x, y): (f32, f32)) -> ((i16, i16), (f32, f32)) {
            let win = self.px(size);
            let (dx, dy) = ((x * self.scale).round() as i16, (y * self.scale).round() as i16);
            ((dx, dy), (f32::from(dx) * size.0 as f32 / win.0 as f32, f32::from(dy) * size.1 as f32 / win.1 as f32))
        }

        fn button(&self, window: u32, press: bool, (x, y): (i16, i16), state: KeyButMask, time: u32) {
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
                state,
                same_screen: true,
            };
            let mask = if press { EventMask::BUTTON_PRESS } else { EventMask::BUTTON_RELEASE };
            self.conn.send_event(false, window, mask, event).unwrap();
        }

        fn motion(&self, window: u32, (x, y): (i16, i16)) {
            let event = xproto::MotionNotifyEvent {
                response_type: xproto::MOTION_NOTIFY_EVENT,
                detail: xproto::Motion::NORMAL,
                sequence: 0,
                time: 0,
                root: self.root,
                event: window,
                child: 0,
                root_x: x,
                root_y: y,
                event_x: x,
                event_y: y,
                state: KeyButMask::BUTTON1,
                same_screen: true,
            };
            self.conn.send_event(false, window, EventMask::POINTER_MOTION, event).unwrap();
        }

        /// Press and release the left button at an editor point of an editor of `size`.
        fn click(&self, window: u32, size: (u32, u32), at: (f32, f32)) {
            let (device, _) = self.place(size, at);
            let time = self.tick();
            self.button(window, true, device, KeyButMask::default(), time);
            self.button(window, false, device, KeyButMask::BUTTON1, time + 1);
            self.sync();
        }

        /// Two presses in time and place: a double click.
        fn double_click(&self, window: u32, size: (u32, u32), at: (f32, f32)) {
            let (device, _) = self.place(size, at);
            let time = self.tick();
            for t in [time, time + 50] {
                self.button(window, true, device, KeyButMask::default(), t);
                self.button(window, false, device, KeyButMask::BUTTON1, t + 1);
            }
            self.sync();
        }

        fn key(&self, window: u32, press: bool, code: u8, state: KeyButMask) {
            let event = xproto::KeyPressEvent {
                response_type: if press { xproto::KEY_PRESS_EVENT } else { xproto::KEY_RELEASE_EVENT },
                detail: code,
                sequence: 0,
                time: self.tick(),
                root: self.root,
                event: window,
                child: 0,
                root_x: 0,
                root_y: 0,
                event_x: 0,
                event_y: 0,
                state,
                same_screen: true,
            };
            let mask = if press { EventMask::KEY_PRESS } else { EventMask::KEY_RELEASE };
            self.conn.send_event(false, window, mask, event).unwrap();
            self.sync();
        }

        /// The keycode whose unshifted symbol is `sym`.
        fn keycode(&self, sym: u32) -> Option<u8> {
            let setup = self.conn.setup();
            let map = self.conn.get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1).ok()?.reply().ok()?;
            let per = usize::from(map.keysyms_per_keycode);
            let index = map.keysyms.chunks(per).position(|syms| syms[0] == sym)?;
            Some(setup.min_keycode + index as u8)
        }

        /// The next key event that arrives on the host's connection: (press, event).
        fn next_key(&self, mut turn: impl FnMut()) -> (bool, xproto::KeyPressEvent) {
            let start = Instant::now();
            loop {
                match self.conn.poll_for_event().unwrap() {
                    Some(Event::KeyPress(e)) => return (true, e),
                    Some(Event::KeyRelease(e)) => return (false, e),
                    Some(Event::Error(e)) => panic!("an X error on the host's connection: {e:?}"),
                    Some(_) => {}
                    None => {
                        assert!(start.elapsed() < DEADLINE, "timed out waiting for a key at the parent window");
                        turn();
                    }
                }
            }
        }

        /// Nothing the host did was refused by the server, and no key is waiting.
        fn no_errors(&self, what: &str) {
            self.sync();
            while let Some(event) = self.conn.poll_for_event().unwrap() {
                assert!(!matches!(event, Event::Error(_)), "{what}: an X error on the host's connection: {event:?}");
            }
        }
    }

    /// Run the host's loop until `done`, or fail at the deadline.
    fn until(what: &str, mut turn: impl FnMut(), mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        loop {
            turn();
            if done() {
                return;
            }
            assert!(start.elapsed() < DEADLINE, "timed out waiting for {what}");
        }
    }

    /// Run the host's loop until the window shows exactly `want`.
    fn until_frame(desk: &Desk, what: &str, window: u32, want: &Image, mut turn: impl FnMut()) {
        assert!(!want.is_blank(), "{what}: the editor's own render is not blank");
        let start = Instant::now();
        loop {
            turn();
            let seen = desk.image(window);
            let differing = seen.as_ref().map_or(usize::MAX, |seen| seen.differing(want));
            if differing == 0 {
                return;
            }
            let size = seen.map(|i| (i.w, i.h));
            assert!(
                start.elapsed() < DEADLINE,
                "timed out waiting for {what}: the window ({size:?}) differs from the editor's render ({} x {}) in {differing} pixels",
                want.w,
                want.h
            );
        }
    }

    /// The editor on its own, fed what the plugin's editor is fed: its render
    /// is what the window must show.
    struct Model {
        editor: GainEditor,
        scale: f32,
    }

    impl Model {
        fn new(desk: &Desk, size: (u32, u32), values: [f32; 2]) -> Model {
            let mut editor = GainEditor::new(EditorHost::detached());
            editor.resize(size.0, size.1);
            editor.param_changed(0, values[0]);
            editor.param_changed(1, values[1]);
            Model { editor, scale: desk.scale }
        }

        fn frame(&mut self) -> Image {
            self.editor.tick(1.0 / 30.0);
            let frame = self.editor.render(self.scale);
            Image { w: frame.width, h: frame.height, px: frame.pixels.iter().map(|p| p & 0x00ff_ffff).collect() }
        }
    }

    fn green(p: u32) -> bool {
        let (r, g, b) = ((p >> 16) & 255, (p >> 8) & 255, p & 255);
        g > r + 60 && g > b + 60
    }

    fn blue(p: u32) -> bool {
        let (r, g, b) = ((p >> 16) & 255, (p >> 8) & 255, p & 255);
        b > r + 60 && b > g + 20
    }

    fn centre(r: (f32, f32, f32, f32)) -> (f32, f32) {
        (r.0 + r.2 * 0.5, r.1 + r.3 * 0.5)
    }

    /// The window is `size` points, the meter is drawn where that size puts it
    /// (bottom lamp lit, top lamp dark) and the plate reaches the far corner.
    fn check_drawn_at(desk: &Desk, window: u32, size: (u32, u32), what: &str) {
        assert_eq!(desk.geometry(window), Some(desk.px(size)), "{what}: the window's size");
        let image = desk.image(window).unwrap_or_else(|| panic!("{what}: the window cannot be read"));
        let (mx, my, mw, mh) = layout(size.0, size.1).meter;
        let lit = |from: f32, to: f32| (0..12).any(|i| green(image.at(desk.scale, mx + mw * 0.5, from + (to - from) * i as f32 / 11.0)));
        assert!(lit(my + mh * 0.75, my + mh - 2.0) && !lit(my + 2.0, my + mh * 0.25), "{what}: the meter is drawn where this size puts it");
        let corner = image.at(desk.scale, size.0 as f32 - 3.0, size.1 as f32 - 3.0);
        assert!(!green(corner) && corner != 0, "{what}: corner {corner:06x}");
    }

    // ---- the CLAP host ----

    #[derive(Default)]
    struct HostState {
        /// Registered timers: id, period in milliseconds, when last fired.
        timers: Vec<(u32, u32, Instant)>,
        /// Registered descriptors and their flags.
        fds: Vec<(i32, u32)>,
        next_timer: u32,
        /// Timers and descriptors registered and unregistered, in all.
        registered: [u32; 2],
        unregistered: [u32; 2],
        /// Unregistrations of something that was not registered.
        unknown: u32,
        timer_calls: u32,
        flushes: u32,
        asked: Vec<(u32, u32)>,
        /// The plugin and its gui: the host answers a size request with `set_size` inside the call.
        answer: Option<(usize, usize)>,
    }

    thread_local! {
        static HOST: RefCell<HostState> = RefCell::new(HostState::default());
    }

    fn host<R>(f: impl FnOnce(&mut HostState) -> R) -> R {
        HOST.with(|h| f(&mut h.borrow_mut()))
    }

    unsafe extern "C" fn register_timer(_h: *const clap_host, period: u32, id: *mut u32) -> bool {
        host(|h| {
            h.next_timer += 1;
            let timer = 100 + h.next_timer;
            h.timers.push((timer, period, Instant::now()));
            h.registered[0] += 1;
            unsafe { *id = timer };
            true
        })
    }
    unsafe extern "C" fn unregister_timer(_h: *const clap_host, id: u32) -> bool {
        host(|h| {
            let before = h.timers.len();
            h.timers.retain(|t| t.0 != id);
            h.unregistered[0] += 1;
            h.unknown += u32::from(h.timers.len() == before);
            true
        })
    }
    unsafe extern "C" fn register_fd(_h: *const clap_host, fd: i32, flags: u32) -> bool {
        host(|h| {
            h.fds.push((fd, flags));
            h.registered[1] += 1;
            true
        })
    }
    unsafe extern "C" fn unregister_fd(_h: *const clap_host, fd: i32) -> bool {
        host(|h| {
            let before = h.fds.len();
            h.fds.retain(|f| f.0 != fd);
            h.unregistered[1] += 1;
            h.unknown += u32::from(h.fds.len() == before);
            true
        })
    }
    unsafe extern "C" fn host_rescan(_h: *const clap_host, _f: u32) {}
    unsafe extern "C" fn host_clear(_h: *const clap_host, _p: u32, _f: u32) {}
    unsafe extern "C" fn host_flush(_h: *const clap_host) {
        host(|h| h.flushes += 1);
    }
    unsafe extern "C" fn host_hints_changed(_h: *const clap_host) {}
    unsafe extern "C" fn host_request_resize(_h: *const clap_host, w: u32, h: u32) -> bool {
        let answer = host(|s| {
            s.asked.push((w, h));
            s.answer
        });
        match answer {
            Some((plugin, gui)) => unsafe { (*(gui as *const clap_plugin_gui)).set_size.unwrap()(plugin as *const clap_plugin, w, h) },
            None => false,
        }
    }
    unsafe extern "C" fn host_show_hide(_h: *const clap_host) -> bool {
        true
    }
    unsafe extern "C" fn host_closed(_h: *const clap_host, _destroyed: bool) {}
    unsafe extern "C" fn host_noop(_h: *const clap_host) {}

    static HOST_PARAMS: clap_host_params = clap_host_params { rescan: Some(host_rescan), clear: Some(host_clear), request_flush: Some(host_flush) };
    static HOST_GUI: clap_host_gui = clap_host_gui {
        resize_hints_changed: Some(host_hints_changed),
        request_resize: Some(host_request_resize),
        request_show: Some(host_show_hide),
        request_hide: Some(host_show_hide),
        closed: Some(host_closed),
    };
    static HOST_TIMERS: clap_host_timer_support = clap_host_timer_support { register_timer: Some(register_timer), unregister_timer: Some(unregister_timer) };
    static HOST_FDS: clap_host_posix_fd_support = clap_host_posix_fd_support { register_fd: Some(register_fd), modify_fd: None, unregister_fd: Some(unregister_fd) };

    unsafe extern "C" fn host_extension(_h: *const clap_host, id: *const c_char) -> *const c_void {
        match unsafe { CStr::from_ptr(id) }.to_bytes_with_nul() {
            EXT_PARAMS => &HOST_PARAMS as *const clap_host_params as *const c_void,
            EXT_GUI => &HOST_GUI as *const clap_host_gui as *const c_void,
            EXT_TIMER_SUPPORT => &HOST_TIMERS as *const clap_host_timer_support as *const c_void,
            EXT_POSIX_FD_SUPPORT => &HOST_FDS as *const clap_host_posix_fd_support as *const c_void,
            _ => std::ptr::null(),
        }
    }

    /// Which of the plugin's two drivers the host's loop runs.
    #[derive(Clone, Copy, PartialEq)]
    enum Drive {
        Both,
        Timer,
        Fd,
    }

    /// A gesture or value event the plugin handed the host: type, parameter, value.
    type Heard = (u16, u32, f64);

    unsafe extern "C" fn capture(list: *const clap_output_events, event: *const clap_event_header) -> bool {
        unsafe {
            let heard = &mut *((*list).ctx as *mut Vec<Heard>);
            match (*event).type_ {
                EVENT_PARAM_VALUE => {
                    let e = &*(event as *const clap_event_param_value);
                    heard.push((EVENT_PARAM_VALUE, e.param_id, e.value));
                }
                kind @ (EVENT_PARAM_GESTURE_BEGIN | EVENT_PARAM_GESTURE_END) => {
                    let e = &*(event as *const clap_event_param_gesture);
                    heard.push((kind, e.param_id, 0.0));
                }
                _ => {}
            }
        }
        true
    }

    #[repr(C)]
    struct One {
        event: clap_event_param_value,
    }
    unsafe extern "C" fn one_size(_l: *const clap_input_events) -> u32 {
        1
    }
    unsafe extern "C" fn one_get(l: *const clap_input_events, _i: u32) -> *const clap_event_header {
        unsafe { &(*((*l).ctx as *const One)).event.header }
    }

    const BEGIN: u16 = EVENT_PARAM_GESTURE_BEGIN;
    const VALUE: u16 = EVENT_PARAM_VALUE;
    const END: u16 = EVENT_PARAM_GESTURE_END;

    /// The example plugin, loaded through its CLAP entry as a host loads it.
    struct Plug {
        plugin: *const clap_plugin,
        gui: &'static clap_plugin_gui,
        params: &'static clap_plugin_params,
        timer: &'static clap_plugin_timer_support,
        fd: &'static clap_plugin_posix_fd_support,
    }

    impl Plug {
        fn load() -> Plug {
            host(|h| *h = HostState::default());
            let host = Box::leak(Box::new(clap_host {
                clap_version: CLAP_VERSION,
                host_data: std::ptr::null_mut(),
                name: c"test".as_ptr(),
                vendor: c"test".as_ptr(),
                url: c"".as_ptr(),
                version: c"1".as_ptr(),
                get_extension: Some(host_extension),
                request_restart: Some(host_noop),
                request_process: Some(host_noop),
                request_callback: Some(host_noop),
            }));
            unsafe {
                let entry = &example_editor::clap_entry;
                assert!(entry.init.unwrap()(c"".as_ptr()));
                let factory = entry.get_factory.unwrap()(FACTORY_ID.as_ptr() as *const c_char) as *const clap_plugin_factory;
                let desc = (*factory).get_plugin_descriptor.unwrap()(factory, 0);
                let plugin = (*factory).create_plugin.unwrap()(factory, host, (*desc).id);
                assert!(!plugin.is_null());
                assert!((*plugin).init.unwrap()(plugin));
                let extension = |id: &[u8]| {
                    let found = (*plugin).get_extension.unwrap()(plugin, id.as_ptr() as *const c_char);
                    assert!(!found.is_null(), "the plugin offers {}", String::from_utf8_lossy(id));
                    found
                };
                Plug {
                    plugin,
                    gui: &*(extension(EXT_GUI) as *const clap_plugin_gui),
                    params: &*(extension(EXT_PARAMS) as *const clap_plugin_params),
                    timer: &*(extension(EXT_TIMER_SUPPORT) as *const clap_plugin_timer_support),
                    fd: &*(extension(EXT_POSIX_FD_SUPPORT) as *const clap_plugin_posix_fd_support),
                }
            }
        }

        /// One turn of the host's loop: wait on the registered descriptors,
        /// call `on_fd` for the readable ones and `on_timer` for the timers due.
        fn turn(&self, drive: Drive) {
            let fds: Vec<i32> = if drive == Drive::Timer { Vec::new() } else { host(|h| h.fds.iter().map(|f| f.0).collect()) };
            for fd in wait_on(&fds, 4) {
                unsafe { self.fd.on_fd.unwrap()(self.plugin, fd, POSIX_FD_READ) };
            }
            if drive == Drive::Fd {
                return;
            }
            let due: Vec<u32> = host(|h| {
                let now = Instant::now();
                let mut due = Vec::new();
                for timer in h.timers.iter_mut().filter(|t| now.duration_since(t.2) >= Duration::from_millis(u64::from(t.1))) {
                    timer.2 = now;
                    due.push(timer.0);
                }
                h.timer_calls += due.len() as u32;
                due
            });
            for id in due {
                unsafe { self.timer.on_timer.unwrap()(self.plugin, id) };
            }
        }

        fn x11() -> *const c_char {
            WINDOW_API_X11.as_ptr() as *const c_char
        }

        fn create(&self) -> bool {
            unsafe { self.gui.create.unwrap()(self.plugin, Self::x11(), false) }
        }

        fn set_parent(&self, parent: u32) -> bool {
            let window = clap_window { api: Self::x11(), ptr: parent as usize as *mut c_void };
            unsafe { self.gui.set_parent.unwrap()(self.plugin, &window) }
        }

        fn show(&self) -> bool {
            unsafe { self.gui.show.unwrap()(self.plugin) }
        }

        fn hide(&self) -> bool {
            unsafe { self.gui.hide.unwrap()(self.plugin) }
        }

        fn destroy_gui(&self) {
            unsafe { self.gui.destroy.unwrap()(self.plugin) }
        }

        fn destroy(self) {
            unsafe { (*self.plugin).destroy.unwrap()(self.plugin) }
        }

        fn size(&self) -> Option<(u32, u32)> {
            let (mut w, mut h) = (0u32, 0u32);
            unsafe { self.gui.get_size.unwrap()(self.plugin, &mut w, &mut h) }.then_some((w, h))
        }

        fn set_size(&self, (w, h): (u32, u32)) -> bool {
            unsafe { self.gui.set_size.unwrap()(self.plugin, w, h) }
        }

        fn adjust(&self, (w, h): (u32, u32)) -> (u32, u32) {
            let (mut w, mut h) = (w, h);
            assert!(unsafe { self.gui.adjust_size.unwrap()(self.plugin, &mut w, &mut h) });
            (w, h)
        }

        fn value(&self, param: u32) -> f64 {
            let mut value = 0.0f64;
            assert!(unsafe { self.params.get_value.unwrap()(self.plugin, param, &mut value) });
            value
        }

        /// The host sets a parameter, as automation does.
        fn set_value(&self, param: u32, value: f64) {
            let one = One {
                event: clap_event_param_value {
                    header: clap_event_header { size: std::mem::size_of::<clap_event_param_value>() as u32, time: 0, space_id: 0, type_: EVENT_PARAM_VALUE, flags: 0 },
                    param_id: param,
                    cookie: std::ptr::null_mut(),
                    note_id: -1,
                    port_index: -1,
                    channel: -1,
                    key: -1,
                    value,
                },
            };
            let input = clap_input_events { ctx: &one as *const One as *mut c_void, size: Some(one_size), get: Some(one_get) };
            unsafe { self.params.flush.unwrap()(self.plugin, &input, std::ptr::null()) };
        }

        /// `params.flush`: what the editor did since the last one.
        fn heard(&self) -> Vec<Heard> {
            let mut heard: Vec<Heard> = Vec::new();
            let out = clap_output_events { ctx: &mut heard as *mut Vec<Heard> as *mut c_void, try_push: Some(capture) };
            unsafe { self.params.flush.unwrap()(self.plugin, std::ptr::null(), &out) };
            heard
        }

        /// Run the loop, flushing every turn, until the events heard end the gesture on `param`.
        fn gesture(&self, drive: Drive, what: &str, param: u32) -> Vec<Heard> {
            let mut heard = Vec::new();
            let start = Instant::now();
            loop {
                self.turn(drive);
                heard.extend(self.heard());
                if heard.last() == Some(&(END, param, 0.0)) {
                    return heard;
                }
                assert!(start.elapsed() < DEADLINE, "timed out waiting for {what}: heard {heard:?}");
            }
        }

        /// A gui created, parented to a new parent window and shown. Returns the parent and the view's window.
        fn open(&self, desk: &Desk) -> (u32, u32) {
            assert!(self.create(), "gui.create");
            let parent = desk.parent();
            assert!(self.set_parent(parent), "gui.set_parent");
            assert!(self.show(), "gui.show");
            let children = desk.children(parent).unwrap();
            assert_eq!(children.len(), 1, "the view's window is the parent's only child");
            (parent, children[0])
        }

        fn values(&self) -> [f32; 2] {
            [self.value(0) as f32, self.value(1) as f32]
        }
    }

    /// Nothing is registered with the host any more, and every unregistration was of something registered.
    fn assert_unregistered(what: &str) {
        let (timers, fds, registered, unregistered, unknown) = host(|h| (h.timers.len(), h.fds.len(), h.registered, h.unregistered, h.unknown));
        assert_eq!((timers, fds), (0, 0), "{what}: the timer and the descriptor are unregistered");
        assert_eq!(registered, unregistered, "{what}: one unregistration for every registration");
        assert_eq!(unknown, 0, "{what}: only what was registered is unregistered");
    }

    fn clap_gui_lifecycle(desk: &Desk) {
        let plug = Plug::load();
        let gui = plug.gui;
        unsafe {
            assert!(gui.is_api_supported.unwrap()(plug.plugin, Plug::x11(), false));
            assert!(!gui.is_api_supported.unwrap()(plug.plugin, Plug::x11(), true), "never floating");
            assert!(!gui.is_api_supported.unwrap()(plug.plugin, c"cocoa".as_ptr(), false));
            let (mut api, mut floating) = (std::ptr::null(), true);
            assert!(gui.get_preferred_api.unwrap()(plug.plugin, &mut api, &mut floating));
            assert_eq!((CStr::from_ptr(api).to_bytes(), floating), (&b"x11"[..], false));
        }
        assert_eq!(plug.size(), None, "no size before create");
        assert!(plug.create());
        let size = (WIDTH, HEIGHT);
        assert_eq!(plug.size(), Some(desk.px(size)), "get_size is in pixels");

        // The plugin asked the host for a timer at the tick interval and for its X descriptor to be watched.
        let (timers, fds) = host(|h| (h.timers.clone(), h.fds.clone()));
        assert_eq!(timers.len(), 1);
        assert_eq!(timers[0].1, TICK_INTERVAL_MS);
        assert_eq!(fds.len(), 1);
        assert!(fds[0].0 >= 0 && fds[0].1 == POSIX_FD_READ, "{fds:?}");

        // Until set_parent there is no window at all; then it is inside the host's.
        let parent = desk.parent();
        assert_eq!(desk.children(parent), Some(vec![]));
        assert!(!plug.set_parent(0), "no parent");
        assert!(plug.set_parent(parent));
        let children = desk.children(parent).unwrap();
        assert_eq!(children.len(), 1);
        let window = children[0];
        assert_eq!(desk.parent_of(window), Some(parent));
        assert_eq!(desk.geometry(window), Some(desk.px(size)));
        assert!(plug.show());

        // The frame on screen is the editor's own render of the same state.
        let mut model = Model::new(desk, size, [0.0, 0.0]);
        until_frame(desk, "the first frame", window, &model.frame(), || plug.turn(Drive::Both));
        assert!(host(|h| h.timer_calls) > 0);

        // A value the host sets reaches the editor through the view's poll, and the knob is repainted.
        let before = model.frame();
        plug.set_value(0, 24.0);
        model.editor.param_changed(0, 24.0);
        let turned = model.frame();
        assert!(turned.differing(&before) > 0, "the model's knob turned");
        until_frame(desk, "the knob at +24 dB", window, &turned, || plug.turn(Drive::Both));

        // A drag on the knob: press, move 30 points down, release. The host is
        // asked to flush and hears the gesture; the plane has the value.
        let flushes = host(|h| h.flushes);
        let (kx, ky) = centre(KNOB);
        let (down, down_at) = desk.place(size, (kx, ky));
        let (moved, moved_at) = desk.place(size, (kx, ky + 30.0));
        let time = desk.tick();
        desk.button(window, true, down, KeyButMask::default(), time);
        desk.motion(window, moved);
        desk.button(window, false, moved, KeyButMask::BUTTON1, time + 1);
        desk.sync();
        let heard = plug.gesture(Drive::Both, "the knob's gesture", 0);
        assert!(host(|h| h.flushes) > flushes, "request_flush");
        assert_eq!(heard.first(), Some(&(BEGIN, 0, 0.0)), "{heard:?}");
        let values: Vec<f64> = heard[1..heard.len() - 1].iter().map(|e| if (e.0, e.1) == (VALUE, 0) { e.2 } else { f64::NAN }).collect();
        assert!(!values.is_empty() && values.iter().all(|v| v.is_finite()), "{heard:?}");
        let value = plug.value(0);
        assert_eq!(values.last(), Some(&value), "the last value heard is the parameter's");
        // 30 of 150 points is a fifth of 48 dB.
        assert!((value - 14.4).abs() < 0.5, "the knob went down to {value} dB");
        // The same drag on the model gives the same value and the same picture.
        model.editor.mouse_down(down_at.0, down_at.1, 1);
        model.editor.mouse_move(moved_at.0, moved_at.1);
        model.editor.mouse_up(moved_at.0, moved_at.1);
        assert!((f64::from(model.editor.values()[0]) - value).abs() < 1e-4, "model {} and plugin {value}", model.editor.values()[0]);
        until_frame(desk, "the knob after the drag", window, &model.frame(), || plug.turn(Drive::Both));

        // The descriptor alone drives input: a click on the switch with no timer firing.
        let switch = centre(SWITCH);
        desk.click(window, size, switch);
        let heard = plug.gesture(Drive::Fd, "the switch's gesture with only the descriptor watched", 1);
        assert_eq!(heard, vec![(BEGIN, 1, 0.0), (VALUE, 1, 1.0), (END, 1, 0.0)]);
        assert_eq!(plug.value(1), 1.0);
        let on = desk.image(window).unwrap();
        // And the timer alone does: the same click with no descriptor watched.
        desk.click(window, size, switch);
        let heard = plug.gesture(Drive::Timer, "the switch's gesture with only the timer", 1);
        assert_eq!(heard, vec![(BEGIN, 1, 0.0), (VALUE, 1, 0.0), (END, 1, 0.0)]);
        assert_eq!(plug.value(1), 0.0);
        let (sx, sy) = desk.place(size, switch).1;
        for _ in 0..2 {
            model.editor.mouse_down(sx, sy, 1);
            model.editor.mouse_up(sx, sy);
        }
        let off = model.frame();
        until_frame(desk, "the switch off again", window, &off, || plug.turn(Drive::Timer));
        assert!(on.differing(&off) > 0, "the switch was drawn on in between");

        // hide unmaps, show maps and the frame is back.
        assert!(plug.hide());
        until("the window to be unmapped", || plug.turn(Drive::Both), || desk.map_state(window) == Some(MapState::UNMAPPED));
        assert!(plug.show());
        until("the window to be mapped", || plug.turn(Drive::Both), || desk.map_state(window) == Some(MapState::VIEWABLE));
        until_frame(desk, "the frame after show", window, &off, || plug.turn(Drive::Both));

        // destroy: nothing registered, no window, no size; a late timer or descriptor call is nothing.
        let (timer, fd) = (timers[0].0, fds[0].0);
        plug.destroy_gui();
        assert_unregistered("gui.destroy");
        until("the window to be destroyed", || plug.turn(Drive::Both), || desk.children(parent) == Some(vec![]));
        assert!(!desk.exists(window));
        assert_eq!(plug.size(), None);
        unsafe {
            plug.timer.on_timer.unwrap()(plug.plugin, timer);
            plug.fd.on_fd.unwrap()(plug.plugin, fd, POSIX_FD_READ);
        }
        plug.destroy_gui();
        assert_unregistered("a second gui.destroy");
        desk.close(parent);

        // A second gui shows the values the plugin has. Then the host's window
        // goes first: everything a host may still call is survived.
        let (parent, window) = plug.open(desk);
        let mut model = Model::new(desk, size, plug.values());
        until_frame(desk, "the second gui's frame", window, &model.frame(), || plug.turn(Drive::Both));
        desk.close(parent);
        assert!(!desk.exists(window), "the view's window went with its parent");
        let calls = host(|h| h.timer_calls);
        until("ticks after the parent is gone", || plug.turn(Drive::Both), || host(|h| h.timer_calls) >= calls + 3);
        assert!(plug.set_size(desk.px(LARGE_SIZE)));
        assert_eq!(plug.size(), Some(desk.px(LARGE_SIZE)));
        assert!(plug.hide());
        assert!(plug.show());
        plug.set_value(0, -6.0);
        plug.turn(Drive::Both);
        plug.destroy_gui();
        assert_unregistered("gui.destroy after the parent");

        // A second create replaces the first gui; the plugin's destroy takes the last one down.
        let (first_parent, first) = plug.open(desk);
        let (parent, window) = plug.open(desk);
        assert_eq!(host(|h| (h.timers.len(), h.fds.len())), (1, 1), "the first gui's timer and descriptor are unregistered");
        until("the first gui's window to be destroyed", || plug.turn(Drive::Both), || desk.children(first_parent) == Some(vec![]));
        // The server may give the new view's connection the old one's ids: the old window is gone when no window of that id is in the first parent.
        assert_ne!(desk.parent_of(first), Some(first_parent));
        until("the gui's frame", || plug.turn(Drive::Both), || desk.image(window).is_some_and(|i| !i.is_blank()));
        plug.destroy();
        assert_unregistered("plugin.destroy");
        until("the window to be destroyed with the plugin", || wait_on(&[], 2).clear(), || desk.children(parent) == Some(vec![]));
        desk.close(first_parent);
        desk.close(parent);
        desk.no_errors("clap gui");
        println!("clap gui: ok");
    }

    fn clap_gui_resizes(desk: &Desk) {
        let plug = Plug::load();
        let gui = plug.gui;
        let (parent, window) = plug.open(desk);
        let px = |size: (u32, u32)| desk.px(size);
        let turn = || plug.turn(Drive::Both);

        // What the host may ask.
        unsafe {
            assert!(gui.can_resize.unwrap()(plug.plugin));
            let mut hints = std::mem::zeroed::<clap_gui_resize_hints>();
            assert!(gui.get_resize_hints.unwrap()(plug.plugin, &mut hints));
            assert!(hints.can_resize_horizontally && hints.can_resize_vertically && hints.preserve_aspect_ratio);
            assert_eq!((hints.aspect_ratio_width, hints.aspect_ratio_height), (WIDTH, HEIGHT));
            assert!(!gui.set_scale.unwrap()(plug.plugin, 2.0), "the view decides its scale");
        }
        assert_eq!(plug.adjust((50_000, 50_000)), px(MAX_SIZE));
        assert_eq!(plug.adjust((10, 10)), px(MIN_SIZE));
        assert_eq!(plug.adjust(px((WIDTH, HEIGHT))), px((WIDTH, HEIGHT)));
        let (w, h) = plug.adjust(px((300, 100)));
        assert!(px(MIN_SIZE).0 <= w && w <= px(MAX_SIZE).0 && (i64::from(w) * 2 - i64::from(h) * 3).abs() <= 3, "{w}x{h} keeps 3:2");
        assert_eq!(plug.adjust((w, h)), (w, h), "an adjusted size adjusts to itself");
        assert_eq!(plug.size(), Some(px((WIDTH, HEIGHT))), "adjust_size changes nothing");

        // set_size follows the range; the window and the frame follow set_size.
        for size in [(360, 240), MIN_SIZE, (300, 200), (WIDTH, HEIGHT)] {
            assert!(plug.set_size(px(size)), "set_size {size:?}");
            assert_eq!(plug.size(), Some(px(size)));
            assert!(!plug.set_size((50_000, 50_000)));
            assert!(!plug.set_size(px((MIN_SIZE.0 - 20, MIN_SIZE.1))));
            assert_eq!(plug.size(), Some(px(size)), "a refused size changes nothing");
            until("the window to take the size", turn, || desk.geometry(window) == Some(px(size)));
            until_frame(desk, &format!("the frame at {size:?}"), window, &Model::new(desk, size, [0.0, 0.0]).frame(), turn);
            check_drawn_at(desk, window, size, "clap set_size");
        }

        // The editor asks the host (a double-click on the plate); the host
        // answers with set_size inside the request, and the view takes it.
        host(|h| h.answer = Some((plug.plugin as usize, plug.gui as *const clap_plugin_gui as usize)));
        desk.double_click(window, (WIDTH, HEIGHT), (10.0, 10.0));
        until("the editor's size request", turn, || !host(|h| h.asked.is_empty()));
        assert_eq!(host(|h| h.asked.clone()), vec![px(LARGE_SIZE)], "clap_host_gui::request_resize, in pixels");
        assert_eq!(plug.size(), Some(px(LARGE_SIZE)));
        until("the window to take the size the editor asked for", turn, || desk.geometry(window) == Some(px(LARGE_SIZE)));
        until_frame(desk, "the frame after the editor's request", window, &Model::new(desk, LARGE_SIZE, [0.0, 0.0]).frame(), turn);
        assert_eq!(host(|h| h.asked.clone()), vec![px(LARGE_SIZE)], "asked once");

        // A host that refuses: asked, and nothing changes.
        host(|h| {
            h.answer = None;
            h.asked.clear();
        });
        desk.double_click(window, LARGE_SIZE, (10.0, 10.0));
        until("the second size request", turn, || !host(|h| h.asked.is_empty()));
        assert_eq!(host(|h| h.asked.clone()), vec![px((WIDTH, HEIGHT))]);
        assert_eq!(plug.size(), Some(px(LARGE_SIZE)));
        assert_eq!(desk.geometry(window), Some(px(LARGE_SIZE)));

        // The scaled controls: the opening-size switch position is bare plate
        // now, the scaled one is the switch. Both clicks, one gesture.
        assert!(plug.heard().is_empty());
        let off = desk.image(window).unwrap();
        desk.click(window, LARGE_SIZE, centre(SWITCH));
        desk.click(window, LARGE_SIZE, centre(layout(LARGE_SIZE.0, LARGE_SIZE.1).switch));
        let heard = plug.gesture(Drive::Both, "the scaled switch's gesture", 1);
        assert_eq!(heard, vec![(BEGIN, 1, 0.0), (VALUE, 1, 1.0), (END, 1, 0.0)], "only the click on the scaled switch is a gesture");
        until("the scaled switch to be drawn on", turn, || desk.image(window).is_some_and(|i| i.differing(&off) > 0));
        check_drawn_at(desk, window, LARGE_SIZE, "clap after the click");

        plug.destroy_gui();
        assert_unregistered("gui.destroy");
        plug.destroy();
        desk.close(parent);
        desk.no_errors("clap resize");
        println!("clap resize: ok");
    }

    // ---- the LV2 host ----

    #[repr(C)]
    struct Resize {
        handle: *mut c_void,
        ui_resize: Option<unsafe extern "C" fn(handle: *mut c_void, width: i32, height: i32) -> i32>,
    }

    #[repr(C)]
    struct Touch {
        handle: *mut c_void,
        touch: Option<unsafe extern "C" fn(handle: *mut c_void, port: u32, grabbed: bool)>,
    }

    #[repr(C)]
    struct IdleInterface {
        idle: Option<unsafe extern "C" fn(handle: *mut c_void) -> i32>,
    }

    #[derive(Default)]
    struct Lv2State {
        /// Control-port writes: port and value.
        writes: Vec<(u32, f32)>,
        touches: Vec<(u32, bool)>,
        asked: Vec<(u32, u32)>,
        refuse: bool,
    }

    thread_local! {
        static LV2: RefCell<Lv2State> = RefCell::new(Lv2State::default());
    }

    fn lv2<R>(f: impl FnOnce(&mut Lv2State) -> R) -> R {
        LV2.with(|h| f(&mut h.borrow_mut()))
    }

    unsafe extern "C" fn lv2_write(_c: *mut c_void, port: u32, size: u32, protocol: u32, buffer: *const c_void) {
        assert_eq!((size, protocol), (4, 0), "a float to a control port");
        let value = unsafe { *(buffer as *const f32) };
        lv2(|h| h.writes.push((port, value)));
    }
    unsafe extern "C" fn lv2_touch(_h: *mut c_void, port: u32, grabbed: bool) {
        lv2(|h| h.touches.push((port, grabbed)));
    }
    unsafe extern "C" fn lv2_host_resize(_h: *mut c_void, width: i32, height: i32) -> i32 {
        lv2(|h| {
            h.asked.push((width as u32, height as u32));
            i32::from(h.refuse)
        })
    }

    const UI_PARENT: &CStr = c"http://lv2plug.in/ns/extensions/ui#parent";
    const UI_RESIZE: &CStr = c"http://lv2plug.in/ns/extensions/ui#resize";
    const UI_TOUCH: &CStr = c"http://lv2plug.in/ns/extensions/ui#touch";
    const UI_IDLE: &CStr = c"http://lv2plug.in/ns/extensions/ui#idleInterface";
    const INSTANCE_ACCESS: &CStr = c"http://lv2plug.in/ns/ext/instance-access";

    /// The example's engine and its UI, opened as an LV2 host opens them.
    struct Lv2Ui {
        ui: &'static Lv2UiDescriptor,
        instance: *mut c_void,
        handle: *mut c_void,
        /// The widget the UI returned: its window.
        widget: u32,
        idle: unsafe extern "C" fn(*mut c_void) -> i32,
        host_sets: unsafe extern "C" fn(*mut c_void, i32, i32) -> i32,
    }

    /// `instantiate` with the features given. The handle (null when refused) and the widget.
    fn lv2_instantiate(ui: &Lv2UiDescriptor, uri: *const c_char, parent: Option<usize>, instance: *mut c_void) -> (*mut c_void, usize) {
        let resize: &'static Resize = Box::leak(Box::new(Resize { handle: std::ptr::null_mut(), ui_resize: Some(lv2_host_resize) }));
        let touch: &'static Touch = Box::leak(Box::new(Touch { handle: std::ptr::null_mut(), touch: Some(lv2_touch) }));
        let mut features = vec![
            Lv2Feature { uri: INSTANCE_ACCESS.as_ptr(), data: instance },
            Lv2Feature { uri: UI_RESIZE.as_ptr(), data: resize as *const Resize as *mut c_void },
            Lv2Feature { uri: UI_TOUCH.as_ptr(), data: touch as *const Touch as *mut c_void },
        ];
        if let Some(parent) = parent {
            features.push(Lv2Feature { uri: UI_PARENT.as_ptr(), data: parent as *mut c_void });
        }
        let mut list: Vec<*const Lv2Feature> = features.iter().map(|f| f as *const Lv2Feature).collect();
        list.push(std::ptr::null());
        let mut widget: *mut c_void = std::ptr::null_mut();
        let handle = unsafe { (ui.instantiate)(ui, uri, c"".as_ptr(), Some(lv2_write), std::ptr::null_mut(), &mut widget, list.as_ptr()) };
        (handle, widget as usize)
    }

    impl Lv2Ui {
        fn open(parent: u32) -> Lv2Ui {
            lv2(|h| *h = Lv2State::default());
            unsafe {
                let desc = &*example_editor::lv2_descriptor(0);
                let instance = (desc.instantiate)(desc, 48_000.0, std::ptr::null(), std::ptr::null());
                assert!(!instance.is_null());
                let ui = &*example_editor::lv2ui_descriptor(0);
                let (handle, widget) = lv2_instantiate(ui, desc.uri, Some(parent as usize), instance);
                assert!(!handle.is_null(), "the UI is instantiated");
                let idle = (ui.extension_data)(UI_IDLE.as_ptr()) as *const IdleInterface;
                assert!(!idle.is_null(), "the UI offers ui:idleInterface");
                let offered = (ui.extension_data)(UI_RESIZE.as_ptr()) as *const Resize;
                assert!(!offered.is_null(), "the UI offers ui:resize");
                Lv2Ui { ui, instance, handle, widget: widget as u32, idle: (*idle).idle.unwrap(), host_sets: (*offered).ui_resize.unwrap() }
            }
        }

        /// One turn of the host's loop: its idle call, every few milliseconds.
        fn turn(&self) {
            wait_on(&[], 5);
            assert_eq!(unsafe { (self.idle)(self.handle) }, 0, "idle says the UI is alive");
        }

        fn port_event(&self, port: u32, value: f32) {
            unsafe { (self.ui.port_event)(self.handle, port, 4, 0, &value as *const f32 as *const c_void) };
        }

        fn resize(&self, (w, h): (u32, u32)) -> i32 {
            unsafe { (self.host_sets)(self.handle, w as i32, h as i32) }
        }

        fn close(self) {
            unsafe {
                (self.ui.cleanup)(self.handle);
                let desc = &*example_editor::lv2_descriptor(0);
                (desc.cleanup)(self.instance);
            }
        }
    }

    fn lv2_ui_lifecycle(desk: &Desk) {
        let size = (WIDTH, HEIGHT);
        let parent = desk.parent();
        unsafe {
            let desc = &*example_editor::lv2_descriptor(0);
            let ui = &*example_editor::lv2ui_descriptor(0);
            assert!(CStr::from_ptr(ui.uri).to_string_lossy().ends_with("#ui"));
            assert!(example_editor::lv2ui_descriptor(1).is_null());
            assert!((ui.extension_data)(c"http://example.com/nothing".as_ptr()).is_null());
            // An X11 UI needs a parent window that exists, and its own plugin.
            assert!(lv2_instantiate(ui, desc.uri, None, std::ptr::null_mut()).0.is_null(), "no ui:parent");
            assert!(lv2_instantiate(ui, desc.uri, Some(0), std::ptr::null_mut()).0.is_null(), "a null ui:parent");
            assert!(lv2_instantiate(ui, desc.uri, Some(0x7f00_0001), std::ptr::null_mut()).0.is_null(), "a ui:parent that is no window");
            assert!(lv2_instantiate(ui, c"urn:another".as_ptr(), Some(parent as usize), std::ptr::null_mut()).0.is_null(), "another plugin's URI");
            assert_eq!(desk.children(parent), Some(vec![]), "a refused UI leaves no window");
        }

        let ui = Lv2Ui::open(parent);
        let window = ui.widget;
        // The widget is the view's window, inside the host's.
        assert_eq!(desk.children(parent), Some(vec![window]));
        assert_eq!(desk.parent_of(window), Some(parent));
        assert_eq!(desk.geometry(window), Some(desk.px(size)));
        assert_eq!(lv2(|h| h.asked.clone()), vec![desk.px(size)], "the opening size is reported to the host, in pixels");

        // idle drives the view: the editor's frame is on screen.
        let mut model = Model::new(desk, size, [0.0, 0.0]);
        until_frame(desk, "the first frame", window, &model.frame(), || ui.turn());

        // port_event reaches the editor and the knob is repainted.
        ui.port_event(AUDIO_PORTS, 24.0);
        ui.port_event(AUDIO_PORTS + 7, 1.0); // no such port: ignored
        model.editor.param_changed(0, 24.0);
        until_frame(desk, "the knob at +24 dB", window, &model.frame(), || ui.turn());

        // A click on the switch writes the Warm port, between two touches.
        desk.click(window, size, centre(SWITCH));
        until("the switch's touch to end", || ui.turn(), || lv2(|h| h.touches.len()) >= 2);
        assert_eq!(lv2(|h| h.writes.clone()), vec![(AUDIO_PORTS + 1, 1.0)]);
        assert_eq!(lv2(|h| h.touches.clone()), vec![(AUDIO_PORTS + 1, true), (AUDIO_PORTS + 1, false)]);
        let (sx, sy) = desk.place(size, centre(SWITCH)).1;
        model.editor.mouse_down(sx, sy, 1);
        model.editor.mouse_up(sx, sy);
        until_frame(desk, "the switch on", window, &model.frame(), || ui.turn());

        // A drag on the knob writes the Gain port.
        lv2(|h| *h = Lv2State::default());
        let (kx, ky) = centre(KNOB);
        let (down, _) = desk.place(size, (kx, ky));
        let (moved, _) = desk.place(size, (kx, ky + 30.0));
        let time = desk.tick();
        desk.button(window, true, down, KeyButMask::default(), time);
        desk.motion(window, moved);
        desk.button(window, false, moved, KeyButMask::BUTTON1, time + 1);
        desk.sync();
        until("the knob's touch to end", || ui.turn(), || lv2(|h| h.touches.len()) >= 2);
        let writes = lv2(|h| h.writes.clone());
        assert!(!writes.is_empty() && writes.iter().all(|w| w.0 == AUDIO_PORTS), "{writes:?}");
        assert!((writes[writes.len() - 1].1 - 14.4).abs() < 0.5, "{writes:?}");
        assert_eq!(lv2(|h| h.touches.clone()), vec![(AUDIO_PORTS, true), (AUDIO_PORTS, false)]);

        // cleanup takes the window away.
        ui.close();
        until("the window to be destroyed", || wait_on(&[], 2).clear(), || desk.children(parent) == Some(vec![]));
        assert!(!desk.exists(window));

        // The host's window goes first: idle, port_event, ui:resize and cleanup are survived.
        let ui = Lv2Ui::open(parent);
        let window = ui.widget;
        until("the second UI's frame", || ui.turn(), || desk.image(window).is_some_and(|i| !i.is_blank()));
        desk.close(parent);
        assert!(!desk.exists(window), "the view's window went with its parent");
        for _ in 0..10 {
            ui.turn();
        }
        ui.port_event(AUDIO_PORTS, -6.0);
        assert_eq!(ui.resize(desk.px(LARGE_SIZE)), 0);
        ui.turn();
        ui.close();
        desk.no_errors("lv2 ui");
        println!("lv2 ui: ok");
    }

    fn lv2_ui_resizes(desk: &Desk) {
        let parent = desk.parent();
        let ui = Lv2Ui::open(parent);
        let window = ui.widget;
        let px = |size: (u32, u32)| desk.px(size);

        // The host sets sizes through the ui:resize the UI offers: inside the range only.
        assert_ne!(ui.resize(px((100, 100))), 0, "below the minimum");
        assert_ne!(ui.resize((50_000, 50_000)), 0, "above the maximum");
        assert_ne!(unsafe { (ui.host_sets)(ui.handle, -1, 100) }, 0);
        assert_eq!(desk.geometry(window), Some(px((WIDTH, HEIGHT))));
        for size in [(300, 200), MIN_SIZE, (WIDTH, HEIGHT)] {
            assert_eq!(ui.resize(px(size)), 0, "ui:resize {size:?}");
            until("the window to take the size", || ui.turn(), || desk.geometry(window) == Some(px(size)));
            until_frame(desk, &format!("the frame at {size:?}"), window, &Model::new(desk, size, [0.0, 0.0]).frame(), || ui.turn());
            check_drawn_at(desk, window, size, "lv2 ui:resize");
        }

        // The editor asks the host (a double-click on the plate); the host's
        // ui:resize agrees; the view follows on its next tick.
        lv2(|h| h.asked.clear());
        desk.double_click(window, (WIDTH, HEIGHT), (10.0, 10.0));
        until("the editor's size request", || ui.turn(), || !lv2(|h| h.asked.is_empty()));
        assert_eq!(lv2(|h| h.asked.clone()), vec![px(LARGE_SIZE)], "the host's ui:resize feature, in pixels");
        until("the window to take the size the host agreed to", || ui.turn(), || desk.geometry(window) == Some(px(LARGE_SIZE)));
        until_frame(desk, "the frame after the editor's request", window, &Model::new(desk, LARGE_SIZE, [0.0, 0.0]).frame(), || ui.turn());

        // A host that refuses: asked, and nothing changes.
        lv2(|h| {
            h.asked.clear();
            h.refuse = true;
        });
        desk.double_click(window, LARGE_SIZE, (10.0, 10.0));
        until("the second size request", || ui.turn(), || !lv2(|h| h.asked.is_empty()));
        assert_eq!(lv2(|h| h.asked.clone()), vec![px((WIDTH, HEIGHT))]);
        for _ in 0..10 {
            ui.turn();
        }
        assert_eq!(desk.geometry(window), Some(px(LARGE_SIZE)));

        // A port event still reaches the resized editor, and the click lands on the scaled switch only.
        ui.port_event(AUDIO_PORTS, 24.0);
        let mut model = Model::new(desk, LARGE_SIZE, [24.0, 0.0]);
        until_frame(desk, "the resized knob at +24 dB", window, &model.frame(), || ui.turn());
        desk.click(window, LARGE_SIZE, centre(SWITCH));
        desk.click(window, LARGE_SIZE, centre(layout(LARGE_SIZE.0, LARGE_SIZE.1).switch));
        until("the scaled switch's touch to end", || ui.turn(), || lv2(|h| h.touches.len()) >= 2);
        assert_eq!(lv2(|h| h.writes.clone()), vec![(AUDIO_PORTS + 1, 1.0)], "only the click on the scaled switch wrote");
        check_drawn_at(desk, window, LARGE_SIZE, "lv2 after the click");

        ui.close();
        until("the window to be destroyed", || wait_on(&[], 2).clear(), || desk.children(parent) == Some(vec![]));
        desk.close(parent);
        desk.no_errors("lv2 resize");
        println!("lv2 resize: ok");
    }

    // ---- keys and focus ----

    fn clap_gui_keys(desk: &Desk) {
        let keys = [0xff52u32, 0x61, 0xff09, 0x20, 0xff1b].map(|sym| desk.keycode(sym));
        let [Some(up), Some(a), Some(tab), Some(space), Some(escape)] = keys else {
            println!("keys and focus: skipped: the keyboard has no Up, a, Tab, Space or Escape");
            return;
        };
        let plug = Plug::load();
        let (parent, window) = plug.open(desk);
        let size = (WIDTH, HEIGHT);
        let turn = || plug.turn(Drive::Both);
        until_frame(desk, "the first frame", window, &Model::new(desk, size, [0.0, 0.0]).frame(), turn);
        // The editor has controls that take keys, so the view selects them.
        until("key events to be selected", turn, || desk.selects_keys(window));
        let none = KeyButMask::default();
        let ring = (KNOB.0 + KNOB.2 * 0.5, KNOB.1 - 4.0);
        let ring_now = || desk.image(window).map(|i| i.at(desk.scale, ring.0, ring.1));
        let plate = ring_now().unwrap();
        assert!(!blue(plate));

        // A click on the knob (at the server's current time) gives the view the
        // input focus and the knob the editor's: a ring is drawn around it.
        let focus_before = desk.conn.get_input_focus().unwrap().reply().unwrap();
        assert_ne!(focus_before.focus, window);
        let (device, _) = desk.place(size, centre(KNOB));
        desk.button(window, true, device, none, x11rb::CURRENT_TIME);
        desk.button(window, false, device, KeyButMask::BUTTON1, x11rb::CURRENT_TIME);
        desk.sync();
        let heard = plug.gesture(Drive::Both, "the click on the knob", 0);
        assert_eq!(heard, vec![(BEGIN, 0, 0.0), (END, 0, 0.0)]);
        until("the view to have the input focus", turn, || desk.focus() == window);
        until("the focus ring", turn, || ring_now().is_some_and(blue));

        // Up arrow: a handled key is a whole gesture, one percent of 48 dB.
        desk.key(window, true, up, none);
        let heard = plug.gesture(Drive::Both, "the Up key", 0);
        assert_eq!(heard.len(), 3, "{heard:?}");
        assert!((plug.value(0) - 0.48).abs() < 1e-3, "{heard:?}");
        desk.key(window, false, up, none);
        // With Shift a tenth as far.
        desk.key(window, true, up, KeyButMask::SHIFT);
        plug.gesture(Drive::Both, "Shift-Up", 0);
        assert!((plug.value(0) - 0.528).abs() < 1e-3, "{}", plug.value(0));
        desk.key(window, false, up, KeyButMask::SHIFT);

        // A key the editor has no use for goes on to the host's window, and so
        // does its release. Neither arrow, which the editor took, came before it.
        desk.key(window, true, a, none);
        let (press, event) = desk.next_key(turn);
        assert_eq!((press, event.detail, event.event, event.child), (true, a, parent, window), "the first key the parent hears is the unused one");
        desk.key(window, false, a, none);
        let (press, event) = desk.next_key(turn);
        assert_eq!((press, event.detail, event.event), (false, a, parent));
        assert!(plug.heard().is_empty());

        // Tab moves the focus to the switch and Space flips it.
        desk.key(window, true, tab, none);
        desk.key(window, false, tab, none);
        desk.key(window, true, space, none);
        desk.key(window, false, space, none);
        let heard = plug.gesture(Drive::Both, "Space on the switch", 1);
        assert_eq!(heard, vec![(BEGIN, 1, 0.0), (VALUE, 1, 1.0), (END, 1, 0.0)]);
        until("the ring to leave the knob", turn, || ring_now().is_some_and(|p| !blue(p)));

        // Escape clears the editor's focus; after that Escape and arrows are the host's.
        desk.key(window, true, escape, none);
        desk.key(window, true, up, none);
        let (press, event) = desk.next_key(turn);
        assert_eq!((press, event.detail, event.event), (true, up, parent), "the handled keys stayed with the editor, the arrow after Escape did not");
        assert!(plug.heard().is_empty());
        assert!((plug.value(0) - 0.528).abs() < 1e-3);

        // Give the focus back: this is somebody's desktop.
        let _ = desk.conn.set_input_focus(focus_before.revert_to, focus_before.focus, x11rb::CURRENT_TIME).unwrap().check();
        until("the focus to be given back", turn, || desk.focus() != window);
        plug.destroy_gui();
        assert_unregistered("gui.destroy");
        plug.destroy();
        desk.close(parent);
        desk.no_errors("keys and focus");
        println!("keys and focus: ok");
    }

    // ---- lifecycle ----

    fn open_descriptors() -> usize {
        std::fs::read_dir("/proc/self/fd").map_or(0, |d| d.count())
    }

    fn editors_open_and_close_without_leaks(desk: &Desk) {
        let parent = desk.parent();
        let plug = Plug::load();
        let clap_cycle = |round: usize| {
            assert!(plug.create(), "round {round}: gui.create");
            assert!(plug.set_parent(parent));
            assert!(plug.show());
            let children = desk.children(parent).unwrap();
            assert_eq!(children.len(), 1, "round {round}");
            until("the frame", || plug.turn(Drive::Both), || desk.image(children[0]).is_some_and(|i| !i.is_blank()));
            plug.destroy_gui();
            assert_unregistered("gui.destroy");
            until("the window to be destroyed", || wait_on(&[], 1).clear(), || desk.children(parent) == Some(vec![]));
            assert!(!desk.exists(children[0]), "round {round}");
        };
        let lv2_cycle = |round: usize| {
            let ui = Lv2Ui::open(parent);
            let window = ui.widget;
            assert_eq!(desk.children(parent), Some(vec![window]), "round {round}");
            until("the frame", || ui.turn(), || desk.image(window).is_some_and(|i| !i.is_blank()));
            ui.close();
            until("the window to be destroyed", || wait_on(&[], 1).clear(), || desk.children(parent) == Some(vec![]));
            assert!(!desk.exists(window), "round {round}");
        };
        // Once before counting: what a first editor sets up for good is not a leak.
        clap_cycle(0);
        lv2_cycle(0);
        desk.sync();
        let (descriptors, windows) = (open_descriptors(), desk.window_count());
        for round in 1..=20 {
            clap_cycle(round);
        }
        assert_eq!(open_descriptors(), descriptors, "CLAP: every editor's X connection is closed");
        for round in 1..=20 {
            lv2_cycle(round);
        }
        assert_eq!(open_descriptors(), descriptors, "LV2: every editor's X connection is closed");
        assert_eq!(host(|h| (h.registered, h.unregistered)), ([21, 21], [21, 21]));
        // The server has no more windows than before (another client may be closing one of its own).
        let mut now = 0;
        until("the server's window count to be what it was", || wait_on(&[], 20).clear(), || {
            now = desk.window_count();
            now <= windows
        });
        plug.destroy();
        desk.close(parent);
        desk.no_errors("lifecycle");
        println!("open and close 20 times in CLAP and LV2: ok ({now} windows before and after)");
    }
}
