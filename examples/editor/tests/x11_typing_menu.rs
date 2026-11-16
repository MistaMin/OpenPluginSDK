//! Typing a value, tooltips and the parameter menu on Linux, with the example
//! editor in the X11 view on the display `DISPLAY` names: the counterpart of
//! `typing.rs` and `menu.rs` (this test has no harness: everything runs on the
//! process's main thread, as the view needs). When `DISPLAY` is not set it
//! prints `skipped: DISPLAY is not set` and passes; with `DISPLAY` set, a
//! server that does not answer fails the test.
//!
//! The test plays the host on a connection of its own: it makes the parent
//! window, sends the clicks and keys as X events, and pumps the view.
//!
//! - Typing: a double-click on the knob (and Return on the focused knob) opens
//!   the type-in field over it with the formatted value; typed keys and Return
//!   commit through `ParamInfo::parse` as begin, perform, end at a recording
//!   `EditorHost`; Escape cancels; focus loss commits valid text and drops
//!   invalid text; invalid text on Return keeps the field open; keys typed
//!   while it is open reach neither the editor nor the host; Alt-double-click
//!   resets; a resize closes the field.
//! - Tooltips: one rectangle per control, at the control's place, with the
//!   parameter's name and value, moving with the layout.
//! - The menu: a right click on a control asks the host for its context menu
//!   with the parameter and the click's place; when the host shows one the
//!   editor shows nothing; the bare plate asks nothing. When the host has none
//!   the editor queues the SDK's own menu, which the view hands to a hook here
//!   instead of a window, and the four items are chosen directly: reset, copy,
//!   paste (the text is passed in, so the user's clipboard is never touched)
//!   and type (which opens the field).
//!
//! Files (drops and file dialogs) and the shell ABI are not served on X11 yet
//! and have no counterpart here.
//!
//! The windows are small, unmanaged and short-lived. As in `x11.rs`, the host
//! skips the first ids of its connection (Hyprland keeps stale geometry for
//! them).

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("editor x11 typing and menu: skipped: Linux only (0 tests run)");
}

#[cfg(target_os = "linux")]
fn main() {
    linux::run();
}

#[cfg(target_os = "linux")]
mod linux {
    use example_editor::{layout, make_editor, HEIGHT, WIDTH};
    use openplugin::core::editor::ParamMenu;
    use openplugin::core::{Editor, EditorHost, Gesture};
    use openplugin::dri::x11::{EditorView, PARAM_MENU_ITEMS};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::time::{Duration, Instant};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{self, ConnectionExt as _, CreateWindowAux, EventMask, KeyButMask, MapState, NotifyDetail, NotifyMode, WindowClass};
    use x11rb::rust_connection::RustConnection;

    const DEADLINE: Duration = Duration::from_secs(10);

    const RETURN: u32 = 0xff0d;
    const ESCAPE: u32 = 0xff1b;
    const BACKSPACE: u32 = 0xff08;
    const UP: u32 = 0xff52;

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

    const NAME: &str = "editor x11 typing and menu";

    pub fn run() {
        // Held to the end: an X server with no client left resets between the tests.
        let Some(_display) = connect(NAME) else { return };
        if let Some(missing) = Host::open(0).missing_key() {
            println!("editor x11 typing and menu: skipped: the keyboard has no key for keysym {missing:#x}");
            return;
        }
        typing_a_value();
        tooltips_follow_layout_and_values();
        host_menu_is_asked_for_the_parameter();
        sdk_menu_when_the_host_has_none();
        println!("x11 typing and menu: 4 passed");
    }

    // ---- the host's side of the display ----

    struct Host {
        conn: RustConnection,
        root: u32,
        parent: u32,
        /// Event times: every event is later than the one before.
        clock: Cell<u32>,
    }

    impl Host {
        fn open(slot: i16) -> Host {
            let (conn, screen) = connect(NAME).expect("DISPLAY was set when the test began");
            let root = conn.setup().roots[screen].root;
            for _ in 0..16 {
                conn.generate_id().expect("an id from the server");
            }
            let parent = conn.generate_id().expect("an id from the server");
            // Unmanaged, so the window manager neither tiles nor decorates it.
            let aux = CreateWindowAux::new().override_redirect(1).background_pixel(0).event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE);
            conn.create_window(0, parent, root, 40 + slot * 420, 380, 400, 300, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap().check().expect("the parent window");
            conn.map_window(parent).unwrap().check().expect("the parent window on screen");
            Host { conn, root, parent, clock: Cell::new(100_000) }
        }

        /// A round trip: the server has handled everything sent before it.
        fn sync(&self) {
            self.conn.get_input_focus().unwrap().reply().unwrap();
        }

        /// A time well after the last one: no press continues an earlier click.
        fn later(&self) -> u32 {
            self.clock.set(self.clock.get() + 5_000);
            self.clock.get()
        }

        fn viewable(&self, window: u32) -> bool {
            self.conn.get_window_attributes(window).ok().and_then(|c| c.reply().ok()).is_some_and(|a| a.map_state == MapState::VIEWABLE)
        }

        fn button(&self, window: u32, press: bool, button: u8, (x, y): (i32, i32), time: u32, state: KeyButMask) {
            let event = xproto::ButtonPressEvent {
                response_type: if press { xproto::BUTTON_PRESS_EVENT } else { xproto::BUTTON_RELEASE_EVENT },
                detail: button,
                sequence: 0,
                time,
                root: self.root,
                event: window,
                child: 0,
                root_x: x as i16,
                root_y: y as i16,
                event_x: x as i16,
                event_y: y as i16,
                state,
                same_screen: true,
            };
            let mask = if press { EventMask::BUTTON_PRESS } else { EventMask::BUTTON_RELEASE };
            self.conn.send_event(false, window, mask, event).unwrap();
            self.conn.flush().unwrap();
        }

        fn key(&self, window: u32, code: u8) {
            let event = xproto::KeyPressEvent {
                response_type: xproto::KEY_PRESS_EVENT,
                detail: code,
                sequence: 0,
                time: self.later(),
                root: self.root,
                event: window,
                child: 0,
                root_x: 0,
                root_y: 0,
                event_x: 0,
                event_y: 0,
                state: KeyButMask::default(),
                same_screen: true,
            };
            self.conn.send_event(false, window, EventMask::KEY_PRESS, event).unwrap();
            self.conn.flush().unwrap();
        }

        fn focus(&self, window: u32, focused: bool) {
            let event = xproto::FocusInEvent {
                response_type: if focused { xproto::FOCUS_IN_EVENT } else { xproto::FOCUS_OUT_EVENT },
                detail: NotifyDetail::NONLINEAR,
                sequence: 0,
                event: window,
                mode: NotifyMode::NORMAL,
            };
            self.conn.send_event(false, window, EventMask::FOCUS_CHANGE, event).unwrap();
            self.conn.flush().unwrap();
        }

        /// The keycode whose unshifted symbol is `sym`.
        fn keycode(&self, sym: u32) -> Option<u8> {
            let setup = self.conn.setup();
            let map = self.conn.get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1).ok()?.reply().ok()?;
            let per = usize::from(map.keysyms_per_keycode).max(1);
            let index = map.keysyms.chunks(per).position(|syms| syms[0] == sym)?;
            Some(setup.min_keycode + index as u8)
        }

        /// The first keysym these tests type that the keyboard has no unshifted key for.
        fn missing_key(&self) -> Option<u32> {
            let typed = "-12369ban".chars().map(u32::from);
            typed.chain([RETURN, ESCAPE, BACKSPACE, UP]).find(|&sym| self.keycode(sym).is_none())
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = self.conn.destroy_window(self.parent);
            let _ = self.conn.flush();
        }
    }

    // ---- the editor in the view, with what reaches its host recorded ----

    struct Rig {
        host: Host,
        view: EditorView,
        log: Rc<RefCell<Vec<Gesture>>>,
    }

    fn gesture_text(g: &Gesture) -> String {
        match g {
            Gesture::Begin(i) => format!("begin {i}"),
            Gesture::Perform(i, v) => format!("perform {i} {v:.3}"),
            Gesture::End(i) => format!("end {i}"),
            other => format!("{other:?}"),
        }
    }

    fn centre(r: (f32, f32, f32, f32)) -> (f32, f32) {
        (r.0 + r.2 * 0.5, r.1 + r.3 * 0.5)
    }

    impl Rig {
        fn new(slot: i16, make: impl FnOnce(EditorHost) -> Option<Box<dyn Editor>>) -> Rig {
            let host = Host::open(slot);
            let log: Rc<RefCell<Vec<Gesture>>> = Rc::new(RefCell::new(Vec::new()));
            let sink = log.clone();
            let editor = make(EditorHost::new(move |g| sink.borrow_mut().push(g))).expect("editor");
            let mut view = EditorView::new(editor, None).expect("the test connected to the display, but the view did not");
            assert!(view.attach(host.parent), "attach");
            // The clipboard items must never touch the user's clipboard.
            assert!(view.set_clipboard_selection("OPENPLUGIN_TEST_TYPING_MENU"));
            let mut rig = Rig { host, view, log };
            let window = rig.view.raw();
            rig.until("the window on screen", |r| r.host.viewable(window));
            rig.settle();
            rig.gestures();
            rig
        }

        /// Pump the view until `done`, or fail at the deadline.
        fn until(&mut self, what: &str, mut done: impl FnMut(&mut Rig) -> bool) {
            let start = Instant::now();
            loop {
                self.view.pump();
                if done(self) {
                    return;
                }
                assert!(start.elapsed() < DEADLINE, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        /// Let every event sent so far reach the editor.
        fn settle(&mut self) {
            for _ in 0..3 {
                self.host.sync();
                self.view.pump();
            }
        }

        /// The gestures since the last call, as text.
        fn gestures(&mut self) -> Vec<String> {
            self.log.borrow_mut().drain(..).map(|g| gesture_text(&g)).collect()
        }

        /// Wait for `count` gestures and take them.
        fn gestures_after(&mut self, count: usize) -> Vec<String> {
            self.until("the gestures", |r| r.log.borrow().len() >= count);
            self.settle();
            self.gestures()
        }

        fn px(&self, points: f32) -> i32 {
            (points * self.view.scale()).round() as i32
        }

        /// `clicks` presses and releases of `button` at an editor point.
        fn click(&mut self, x: f32, y: f32, button: u8, clicks: u32, state: KeyButMask) {
            let (px, py) = (self.px(x), self.px(y));
            let time = self.host.later();
            for n in 0..clicks {
                self.host.button(self.view.raw(), true, button, (px, py), time + n * 50, state);
                self.host.button(self.view.raw(), false, button, (px, py), time + n * 50 + 10, state);
            }
        }

        fn key(&mut self, sym: u32) {
            let code = self.host.keycode(sym).expect("a key for the keysym");
            self.host.key(self.view.raw(), code);
        }

        fn type_text(&mut self, text: &str) {
            for c in text.chars() {
                self.key(u32::from(c));
            }
        }

        fn has_field(&self) -> bool {
            self.view.text_field().is_some()
        }

        fn field_text(&self) -> String {
            self.view.text_field_text().unwrap_or_default()
        }

        /// Double-click the point and wait for the field.
        fn open_field(&mut self, x: f32, y: f32) {
            self.click(x, y, 1, 2, KeyButMask::default());
            self.until("the field", |r| r.has_field());
            self.settle();
        }

        /// Type `text` into the open field (it replaces the selected value) and wait for it.
        fn fill_field(&mut self, text: &str) {
            self.type_text(text);
            self.until("the typed text", |r| r.field_text() == text);
        }

        fn wait_closed(&mut self) {
            self.until("the field to close", |r| !r.has_field());
            self.settle();
        }

        /// The value the editor shows for the gain: its tooltip's text.
        fn gain_text(&self) -> String {
            self.view.tooltip_rects().first().map(|t| t.1.clone()).unwrap_or_default()
        }
    }

    fn typing_a_value() {
        let mut rig = Rig::new(0, make_editor);
        let lay = layout(WIDTH, HEIGHT);
        let (kx, ky) = centre(lay.knob);
        let none = KeyButMask::default();

        // A single click is a begin and an end, and no field.
        rig.click(kx, ky, 1, 1, none);
        assert_eq!(rig.gestures_after(2), ["begin 0", "end 0"]);
        assert!(!rig.has_field());

        // A double-click opens the field, prefilled with ParamInfo::format.
        rig.open_field(kx, ky);
        assert_eq!(rig.field_text(), "0.0 dB");
        assert!(rig.gestures().iter().all(|g| g != "perform 0 0.000"), "opening sends no value");

        // Typing and Return commits: one whole gesture, through parse.
        rig.fill_field("-6");
        rig.key(RETURN);
        rig.wait_closed();
        assert_eq!(rig.gestures(), ["begin 0", "perform 0 -6.000", "end 0"]);
        assert_eq!(rig.gain_text(), "Gain: -6.0 dB");

        // Escape cancels: nothing is sent, the value stays.
        rig.open_field(kx, ky);
        assert_eq!(rig.field_text(), "-6.0 dB");
        rig.gestures();
        rig.fill_field("12");
        rig.key(ESCAPE);
        rig.wait_closed();
        assert!(rig.gestures().is_empty());
        assert_eq!(rig.gain_text(), "Gain: -6.0 dB");

        // Losing focus commits valid text...
        rig.open_field(kx, ky);
        rig.gestures();
        rig.fill_field("12");
        rig.host.focus(rig.view.raw(), true);
        rig.host.focus(rig.view.raw(), false);
        rig.wait_closed();
        assert_eq!(rig.gestures(), ["begin 0", "perform 0 12.000", "end 0"]);
        assert_eq!(rig.gain_text(), "Gain: 12.0 dB");
        // ...and drops invalid text without a stuck field.
        rig.open_field(kx, ky);
        rig.gestures();
        rig.fill_field("banana");
        rig.host.focus(rig.view.raw(), true);
        rig.host.focus(rig.view.raw(), false);
        rig.wait_closed();
        assert!(rig.gestures().is_empty());
        assert_eq!(rig.gain_text(), "Gain: 12.0 dB");
        // Put the value back for the rest.
        rig.open_field(kx, ky);
        rig.fill_field("-6");
        rig.key(RETURN);
        rig.wait_closed();
        rig.gestures();

        // Invalid text is refused: the field stays open and nothing is sent.
        rig.open_field(kx, ky);
        rig.gestures();
        rig.fill_field("banana");
        rig.key(RETURN);
        rig.settle();
        assert!(rig.has_field() && rig.gestures().is_empty());
        assert_eq!(rig.field_text(), "banana");
        assert_eq!(rig.gain_text(), "Gain: -6.0 dB");

        // Keys typed into the open field reach neither the editor nor the host:
        // Up would raise the focused knob, and Backspace would reset it.
        rig.key(UP);
        rig.key(BACKSPACE);
        rig.settle();
        assert!(rig.gestures().is_empty() && rig.has_field());
        assert_eq!(rig.gain_text(), "Gain: -6.0 dB");
        rig.key(ESCAPE);
        rig.wait_closed();
        // A value above the range is clamped (units are optional).
        rig.open_field(kx, ky);
        rig.gestures();
        rig.fill_field("99");
        rig.key(RETURN);
        rig.wait_closed();
        assert_eq!(rig.gestures(), ["begin 0", "perform 0 24.000", "end 0"]);

        // With the field closed the same keys act again: the knob has focus.
        rig.key(BACKSPACE);
        assert_eq!(rig.gestures_after(3), ["begin 0", "perform 0 0.000", "end 0"]);

        // Return on the focused knob opens the field too.
        rig.key(RETURN);
        rig.until("the field from Return", |r| r.has_field());
        assert_eq!(rig.field_text(), "0.0 dB");
        // A click elsewhere on the view abandons it.
        rig.click(5.0, 5.0, 1, 1, none);
        rig.wait_closed();
        rig.gestures();

        // Alt-double-click resets instead of typing.
        rig.view.editor().expect("editor").param_changed(0, 9.0);
        rig.click(kx, ky, 1, 2, KeyButMask::MOD1);
        rig.until("the reset", |r| r.log.borrow().iter().any(|g| gesture_text(g) == "perform 0 0.000"));
        rig.settle();
        assert!(!rig.has_field());
        rig.gestures();

        // A resize closes an open field (the control moved under it).
        rig.open_field(kx, ky);
        rig.view.resize(360, 240);
        rig.wait_closed();
    }

    fn tooltips_follow_layout_and_values() {
        let mut rig = Rig::new(1, make_editor);
        let close = |got: [f64; 4], want: (f32, f32, f32, f32)| {
            let want = [want.0, want.1, want.2, want.3].map(f64::from);
            got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-3)
        };

        // One rectangle per control, at the control's place, with name and value.
        let lay = layout(WIDTH, HEIGHT);
        let t = rig.view.tooltip_rects();
        assert_eq!(t.len(), 3, "{t:?}");
        assert!(close(t[0].0, lay.knob), "{:?} vs {:?}", t[0].0, lay.knob);
        assert!(close(t[1].0, lay.switch), "{:?} vs {:?}", t[1].0, lay.switch);
        assert_eq!(t[0].1, "Gain: 0.0 dB");
        assert_eq!(t[1].1, "Warm: Off");
        assert!(t[2].1.contains("Drop an audio file"), "{}", t[2].1);

        // The text follows the values the host sets.
        let editor = rig.view.editor().expect("editor");
        editor.param_changed(0, -12.0);
        editor.param_changed(1, 1.0);
        let t = rig.view.tooltip_rects();
        assert_eq!((t[0].1.as_str(), t[1].1.as_str()), ("Gain: -12.0 dB", "Warm: On"));

        // The rectangles follow the layout.
        assert_eq!(rig.view.resize(360, 240), (360, 240));
        rig.until("the resize", |r| r.view.size() == (360, 240));
        let lay = layout(360, 240);
        let t = rig.view.tooltip_rects();
        assert_eq!(t.len(), 3);
        assert!(close(t[0].0, lay.knob), "{:?} vs {:?}", t[0].0, lay.knob);
        assert!(close(t[1].0, lay.switch), "{:?} vs {:?}", t[1].0, lay.switch);
    }

    type Asked = Rc<RefCell<Vec<(u32, i32, i32)>>>;

    /// The view's menu requests go to a list instead of a menu window.
    fn hook_menus(rig: &mut Rig) -> Rc<RefCell<Vec<ParamMenu>>> {
        let shown: Rc<RefCell<Vec<ParamMenu>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = shown.clone();
        rig.view.set_menu_hook(Some(Box::new(move |m| {
            sink.borrow_mut().push(m.clone());
            true
        })));
        shown
    }

    fn host_menu_is_asked_for_the_parameter() {
        let asked: Asked = Rc::new(RefCell::new(Vec::new()));
        let log = asked.clone();
        let mut rig = Rig::new(2, move |h| {
            make_editor(h.with_context_menu(move |p, x, y| {
                log.borrow_mut().push((p, x, y));
                true
            }))
        });
        let shown = hook_menus(&mut rig);
        let lay = layout(WIDTH, HEIGHT);
        let (kx, ky) = centre(lay.knob);
        let none = KeyButMask::default();

        // A right click on the knob asks the host, with the parameter and the place.
        rig.click(kx, ky, 3, 1, none);
        rig.until("the host's menu to be asked", |_| !asked.borrow().is_empty());
        rig.settle();
        assert_eq!(*asked.borrow(), [(0, kx.round() as i32, ky.round() as i32)]);
        assert!(shown.borrow().is_empty(), "the host showed its menu: the SDK shows none");
        assert!(rig.gestures().is_empty(), "a menu is not an edit");
        assert!(rig.view.menu_window().is_none());

        // The Warm toggle is parameter 1.
        let (wx, wy) = centre(lay.switch);
        rig.click(wx, wy, 3, 1, none);
        rig.until("the second ask", |_| asked.borrow().len() == 2);
        rig.settle();
        assert_eq!(asked.borrow().last().map(|a| a.0), Some(1));
        assert!(rig.gestures().is_empty(), "a right click does not flip the toggle");

        // The bare plate asks nothing.
        rig.click(3.0, 3.0, 3, 1, none);
        rig.settle();
        assert_eq!(asked.borrow().len(), 2);
        assert!(shown.borrow().is_empty());
    }

    fn sdk_menu_when_the_host_has_none() {
        assert_eq!(PARAM_MENU_ITEMS, ["Reset to default", "Copy value", "Paste value", "Type value"]);
        // No host menu at all: the rig's host has no context-menu handler.
        let mut rig = Rig::new(3, make_editor);
        let shown = hook_menus(&mut rig);
        let lay = layout(WIDTH, HEIGHT);
        let (kx, ky) = centre(lay.knob);

        // Put the gain at -6 dB first, so reset has something to do.
        rig.open_field(kx, ky);
        rig.fill_field("-6");
        rig.key(RETURN);
        rig.wait_closed();
        rig.gestures();

        rig.click(kx, ky, 3, 1, KeyButMask::default());
        rig.until("the SDK's menu", |_| !shown.borrow().is_empty());
        rig.settle();
        assert_eq!(shown.borrow().len(), 1);
        let menu = shown.borrow()[0].clone();
        assert_eq!((menu.param, menu.label.as_str()), (0, "Gain"));
        assert_eq!((menu.x.round() as i32, menu.y.round() as i32), (kx.round() as i32, ky.round() as i32));
        assert!(rig.gestures().is_empty());
        assert!(rig.view.menu_window().is_none(), "the hook took the menu: no window");

        // Copy: the formatted value, nothing sent.
        assert_eq!(rig.view.choose_menu_item(1, Some("unused")).as_deref(), Some("-6.0 dB"));
        assert!(rig.gestures().is_empty());
        // Paste: the text goes through ParamInfo::parse as one whole gesture.
        assert_eq!(rig.view.choose_menu_item(2, Some("3 dB")), None);
        assert_eq!(rig.gestures(), ["begin 0", "perform 0 3.000", "end 0"]);
        assert_eq!(rig.view.choose_menu_item(2, Some("99")), None);
        assert_eq!(rig.gestures(), ["begin 0", "perform 0 24.000", "end 0"]);
        // Text that is not a value changes nothing.
        assert_eq!(rig.view.choose_menu_item(2, Some("banana")), None);
        assert!(rig.gestures().is_empty());
        // Reset: the default.
        assert_eq!(rig.view.choose_menu_item(0, None), None);
        assert_eq!(rig.gestures(), ["begin 0", "perform 0 0.000", "end 0"]);
        // Type: opens the text field over the control.
        assert!(!rig.has_field());
        assert_eq!(rig.view.choose_menu_item(3, None), None);
        rig.until("the field from the menu", |r| r.has_field());
        assert_eq!(rig.field_text(), "0.0 dB");
        rig.key(ESCAPE);
        rig.wait_closed();
        // An item that does not exist does nothing.
        assert_eq!(rig.view.choose_menu_item(9, None), None);
        assert!(rig.gestures().is_empty());
    }
}
