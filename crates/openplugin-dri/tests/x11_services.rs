//! The X11 view's editor services against a real X server: the type-in
//! field, the parameter menu, the clipboard and tooltips, driven with
//! `SendEvent` from a second connection and read back with `GetImage`,
//! `QueryTree` and `GetWindowAttributes`.
//!
//! Every test passes without doing anything when `DISPLAY` is not set, and
//! says so. With `DISPLAY` set, a server that does not answer fails the test
//! (the Xvfb job in CI relies on that: a skip must not pass for coverage). The tests run one at a time (they
//! share the pointer grab and the screen), in small unmanaged windows that
//! are gone when the test ends. The clipboard tests use a selection of their
//! own, not `CLIPBOARD`, so they leave the user's clipboard alone.

#![cfg(target_os = "linux")]

use openplugin_core::editor::{DamageRect, Key, Modifiers, ParamMenu, ParamMenuAction, TextEdit, Tooltip};
use openplugin_core::{Editor, Frame};
use openplugin_dri::x11::{EditorView, CLIPBOARD_WAIT_MS, DOUBLE_CLICK_MS, PARAM_MENU_ITEMS, TOOLTIP_DELAY_MS};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    self, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, GrabMode, GrabStatus, ImageFormat, ImageOrder, KeyButMask, MapState, NotifyDetail,
    NotifyMode, PropMode, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const DEADLINE: Duration = Duration::from_secs(10);

/// The test's connection to the display. `None` (the test skips) only when
/// `DISPLAY` is not set. With `DISPLAY` set, a refused connection is tried
/// again until the deadline (an X server with no client left resets, and
/// refuses connections meanwhile) and then fails the test: a display that is
/// named must be there.
fn connect(test: &str) -> Option<(RustConnection, usize)> {
    if std::env::var_os("DISPLAY").is_none_or(|d| d.is_empty()) {
        println!("{test}: skipped, DISPLAY is not set");
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
const SIZE: (u32, u32) = (300, 200);
const BACKGROUND: u32 = 0x0020_3040;
/// The control the field opens over, and the tooltip's rectangle.
const CONTROL: DamageRect = DamageRect { x: 100.0, y: 80.0, w: 40.0, h: 40.0 };
const TIP: DamageRect = DamageRect { x: 10.0, y: 10.0, w: 80.0, h: 40.0 };
const PARAM: u32 = 7;
// The colours the pop-ups are drawn with.
const FOCUS: u32 = 0x00ff_d36a;
const BAD: u32 = 0x00ff_6a5a;
const EDGE: u32 = 0x003a_3f45;

/// One test at a time: they share the screen, the pointer and the keyboard.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Debug, PartialEq)]
enum Call {
    Down(u32),
    Right,
    Commit(String, bool),
    Cancel,
    Action(u32, ParamMenuAction),
    Key(Key),
}

struct Shared {
    calls: Vec<Call>,
    /// The parameter's value as text: what the field starts with and what copy gives.
    value: String,
    edit: Option<TextEdit>,
    menu: Option<ParamMenu>,
    tips: Vec<Tooltip>,
    tip_asks: usize,
    ticks: usize,
    size: (u32, u32),
}

struct TestEditor {
    shared: Rc<RefCell<Shared>>,
    pixels: Vec<u32>,
}

fn edit_request(value: &str) -> TextEdit {
    TextEdit { rect: CONTROL, text: value.to_string(), label: "Gain".into() }
}

impl Editor for TestEditor {
    fn size(&self) -> (u32, u32) {
        self.shared.borrow().size
    }
    fn resize(&mut self, width: u32, height: u32) {
        self.shared.borrow_mut().size = (width, height);
    }
    fn size_range(&self) -> openplugin_core::editor::SizeRange {
        openplugin_core::editor::SizeRange::between((100, 100), (600, 400))
    }
    fn param_changed(&mut self, _index: usize, _plain: f32) {}
    fn tick(&mut self, _dt: f32) {
        self.shared.borrow_mut().ticks += 1;
    }
    fn render(&mut self, scale: f32) -> Frame<'_> {
        let size = self.shared.borrow().size;
        let (w, h) = ((size.0 as f32 * scale).round() as usize, (size.1 as f32 * scale).round() as usize);
        self.pixels.clear();
        self.pixels.resize(w * h, BACKGROUND);
        Frame::new(&self.pixels, w, h)
    }
    fn key_down(&mut self, key: Key, _modifiers: Modifiers) -> bool {
        self.shared.borrow_mut().calls.push(Call::Key(key));
        true
    }
    fn mouse_down(&mut self, _x: f32, _y: f32, clicks: u32) {
        let mut s = self.shared.borrow_mut();
        s.calls.push(Call::Down(clicks));
        if clicks == 2 {
            s.edit = Some(edit_request(&s.value));
        }
    }
    fn right_mouse_down(&mut self, x: f32, y: f32) -> bool {
        let mut s = self.shared.borrow_mut();
        s.calls.push(Call::Right);
        s.menu = Some(ParamMenu { param: PARAM, x, y, label: "Gain".into() });
        true
    }
    fn take_param_menu(&mut self) -> Option<ParamMenu> {
        self.shared.borrow_mut().menu.take()
    }
    fn param_menu_action(&mut self, param: u32, action: ParamMenuAction) -> Option<String> {
        let mut s = self.shared.borrow_mut();
        s.calls.push(Call::Action(param, action.clone()));
        match action {
            ParamMenuAction::Copy => Some(s.value.clone()),
            ParamMenuAction::Type => {
                s.edit = Some(edit_request(&s.value));
                None
            }
            _ => None,
        }
    }
    fn take_text_edit(&mut self) -> Option<TextEdit> {
        self.shared.borrow_mut().edit.take()
    }
    fn text_edit_commit(&mut self, text: &str) -> bool {
        let valid = text.trim().parse::<f32>().is_ok();
        let mut s = self.shared.borrow_mut();
        s.calls.push(Call::Commit(text.to_string(), valid));
        if valid {
            s.value = text.to_string();
        }
        valid
    }
    fn text_edit_cancel(&mut self) {
        self.shared.borrow_mut().calls.push(Call::Cancel);
    }
    fn tooltips(&self) -> Vec<Tooltip> {
        let mut s = self.shared.borrow_mut();
        s.tip_asks += 1;
        s.tips.clone()
    }
}

/// The host's side: a connection of its own and a parent window.
struct Host {
    conn: RustConnection,
    root: u32,
    parent: u32,
}

impl Host {
    /// `None`, with a note, when there is no X server.
    fn new(test: &str, slot: u32) -> Option<Host> {
        let (conn, screen) = connect(test)?;
        let root = conn.setup().roots[screen].root;
        // See x11_view.rs: Hyprland may hold stale geometry for a connection's first ids.
        for _ in 0..16 {
            conn.generate_id().unwrap();
        }
        let parent = conn.generate_id().unwrap();
        // Unmanaged, so the window manager neither tiles nor decorates it.
        let aux = CreateWindowAux::new().override_redirect(1).background_pixel(0).event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE);
        let x = 40 + slot as i16 * 420;
        conn.create_window(0, parent, root, x, 40, 400, 300, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap().check().unwrap();
        conn.map_window(parent).unwrap().check().unwrap();
        Some(Host { conn, root, parent })
    }

    fn sync(&self) {
        self.conn.get_input_focus().unwrap().reply().unwrap();
    }

    /// A time well after the last one: no press continues an earlier click.
    fn later(&self) -> u32 {
        event_time(DOUBLE_CLICK_MS + 100)
    }

    /// One row of a window as it is on screen, `None` while it cannot be read.
    fn row(&self, window: u32, y: u32, width: u32) -> Option<Vec<u32>> {
        let image = self.conn.get_image(ImageFormat::Z_PIXMAP, window, 0, y as i16, width as u16, 1, !0).ok()?.reply().ok()?;
        let msb = self.conn.setup().image_byte_order == ImageOrder::MSB_FIRST;
        let words = image.data.chunks_exact(4).map(|b| {
            let bytes: [u8; 4] = [b[0], b[1], b[2], b[3]];
            (if msb { u32::from_be_bytes(bytes) } else { u32::from_le_bytes(bytes) }) & 0x00ff_ffff
        });
        Some(words.collect())
    }

    fn pixel(&self, window: u32, x: u32, y: u32) -> Option<u32> {
        let image = self.conn.get_image(ImageFormat::Z_PIXMAP, window, x as i16, y as i16, 1, 1, !0).ok()?.reply().ok()?;
        let bytes: [u8; 4] = image.data.get(..4)?.try_into().ok()?;
        let word = if self.conn.setup().image_byte_order == ImageOrder::MSB_FIRST { u32::from_be_bytes(bytes) } else { u32::from_le_bytes(bytes) };
        Some(word & 0x00ff_ffff)
    }

    fn geometry(&self, window: u32) -> Option<(i32, i32, u32, u32)> {
        let g = self.conn.get_geometry(window).ok()?.reply().ok()?;
        Some((i32::from(g.x), i32::from(g.y), u32::from(g.width), u32::from(g.height)))
    }

    fn exists(&self, window: u32) -> bool {
        self.conn.get_window_attributes(window).ok().and_then(|c| c.reply().ok()).is_some()
    }

    fn children(&self, window: u32) -> Vec<u32> {
        self.conn.query_tree(window).ok().and_then(|c| c.reply().ok()).map(|t| t.children).unwrap_or_default()
    }

    /// Whether the pointer is free: this client can grab it (and lets go at once).
    fn pointer_is_free(&self) -> bool {
        let status = self
            .conn
            .grab_pointer(false, self.parent, EventMask::BUTTON_PRESS, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, x11rb::CURRENT_TIME)
            .unwrap()
            .reply()
            .unwrap()
            .status;
        if status == GrabStatus::SUCCESS {
            self.conn.ungrab_pointer(x11rb::CURRENT_TIME).unwrap();
            self.sync();
        }
        status == GrabStatus::SUCCESS
    }

    fn button(&self, window: u32, press: bool, button: u8, x: i32, y: i32, time: u32) {
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
            state: KeyButMask::default(),
            same_screen: true,
        };
        let mask = if press { EventMask::BUTTON_PRESS } else { EventMask::BUTTON_RELEASE };
        self.conn.send_event(false, window, mask, event).unwrap();
        self.conn.flush().unwrap();
    }

    fn click(&self, window: u32, button: u8, x: i32, y: i32, time: u32) {
        self.button(window, true, button, x, y, time);
        self.button(window, false, button, x, y, time + 10);
    }

    fn motion(&self, window: u32, x: i32, y: i32) {
        let event = xproto::MotionNotifyEvent {
            response_type: xproto::MOTION_NOTIFY_EVENT,
            detail: xproto::Motion::NORMAL,
            sequence: 0,
            time: event_time(1),
            root: self.root,
            event: window,
            child: 0,
            root_x: x as i16,
            root_y: y as i16,
            event_x: x as i16,
            event_y: y as i16,
            state: KeyButMask::default(),
            same_screen: true,
        };
        self.conn.send_event(false, window, EventMask::POINTER_MOTION, event).unwrap();
        self.conn.flush().unwrap();
    }

    fn key(&self, window: u32, code: u8, state: KeyButMask) {
        let event = xproto::KeyPressEvent {
            response_type: xproto::KEY_PRESS_EVENT,
            detail: code,
            sequence: 0,
            time: event_time(1),
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

    /// The keycode whose unshifted symbol is `sym`, and its shifted symbol.
    fn keycode(&self, sym: u32) -> (u8, u32) {
        let setup = self.conn.setup();
        let map = self.conn.get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1).unwrap().reply().unwrap();
        let per = usize::from(map.keysyms_per_keycode);
        let index = map.keysyms.chunks(per).position(|syms| syms[0] == sym).unwrap_or_else(|| panic!("no key for keysym {sym:#x}"));
        (setup.min_keycode + index as u8, map.keysyms.chunks(per).nth(index).and_then(|s| s.get(1).copied()).unwrap_or(0))
    }

    fn atom(&self, name: &str) -> u32 {
        self.conn.intern_atom(false, name.as_bytes()).unwrap().reply().unwrap().atom
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.conn.destroy_window(self.parent);
        let _ = self.conn.flush();
    }
}

/// The server's clock, read from a `PropertyNotify` on a window of its own.
struct Clock {
    conn: RustConnection,
    window: u32,
    last: u32,
}

impl Clock {
    fn new() -> Clock {
        let (conn, screen) = RustConnection::connect(None).expect("the clock's connection");
        let root = conn.setup().roots[screen].root;
        let window = conn.generate_id().unwrap();
        let aux = CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE);
        conn.create_window(0, window, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &aux).unwrap().check().unwrap();
        let mut clock = Clock { conn, window, last: 0 };
        clock.last = clock.server_time();
        clock
    }

    fn server_time(&self) -> u32 {
        self.conn.change_property8(PropMode::APPEND, self.window, AtomEnum::WM_NAME, AtomEnum::STRING, &[]).unwrap();
        self.conn.flush().unwrap();
        loop {
            if let Event::PropertyNotify(e) = self.conn.wait_for_event().unwrap() {
                return e.time;
            }
        }
    }
}

/// An event time `gap` after the last, from one clock for every test. The view
/// hands event times to `SetInputFocus`, which the server ignores when they are
/// ahead of its clock or behind the last focus change, so times follow the
/// server's clock and never run ahead of it (made-up times made the focus
/// depend on how long the machine had been up).
fn event_time(gap: u32) -> u32 {
    static CLOCK: Mutex<Option<Clock>> = Mutex::new(None);
    let mut guard = CLOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let clock = guard.get_or_insert_with(Clock::new);
    clock.last = clock.last.wrapping_add(gap);
    // Room for a release or a second press a little after this time.
    let start = Instant::now();
    while (clock.server_time().wrapping_sub(clock.last) as i32) < 100 {
        assert!(start.elapsed() < DEADLINE, "the server's clock stands still");
        std::thread::sleep(Duration::from_millis(5));
    }
    clock.last
}

/// Pump the views until `done`, or fail at the deadline.
fn until_all(views: &mut [&mut EditorView], what: &str, mut done: impl FnMut(&mut [&mut EditorView]) -> bool) {
    let start = Instant::now();
    loop {
        for view in views.iter_mut() {
            view.pump();
        }
        if done(views) {
            return;
        }
        assert!(start.elapsed() < DEADLINE, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn until(view: &mut EditorView, what: &str, mut done: impl FnMut(&mut EditorView) -> bool) {
    until_all(&mut [view], what, |views| done(views[0]));
}

/// A view attached to a host's parent window, with its first frame on screen.
fn attached(test: &str, slot: u32) -> Option<(Host, EditorView, Rc<RefCell<Shared>>)> {
    let host = Host::new(test, slot)?;
    let shared = Rc::new(RefCell::new(Shared { calls: Vec::new(), value: "1.5".into(), edit: None, menu: None, tips: Vec::new(), tip_asks: 0, ticks: 0, size: SIZE }));
    let editor = Box::new(TestEditor { shared: shared.clone(), pixels: Vec::new() });
    let mut view = EditorView::new(editor, None).unwrap_or_else(|| panic!("{test}: the test connected to the display, but the view did not"));
    assert!(view.attach(host.parent), "attach");
    let window = view.raw();
    until(&mut view, "the first frame", |_| host.pixel(window, 1, 1) == Some(BACKGROUND));
    Some((host, view, shared))
}

/// Device pixels of a length in points.
fn px(view: &EditorView, points: f32) -> i32 {
    (points * view.scale()).round() as i32
}

fn taken(shared: &Rc<RefCell<Shared>>) -> Vec<Call> {
    std::mem::take(&mut shared.borrow_mut().calls)
}

/// Double-click the control and wait for the field. Returns the field's window.
fn open_field(host: &Host, view: &mut EditorView, shared: &Rc<RefCell<Shared>>) -> u32 {
    let (x, y) = (px(view, CONTROL.x + CONTROL.w * 0.5), px(view, CONTROL.y + CONTROL.h * 0.5));
    let time = host.later();
    host.click(view.raw(), 1, x, y, time);
    host.click(view.raw(), 1, x, y, time + 50);
    until(view, "the field", |v| v.text_field().is_some());
    assert_eq!(taken(shared), [Call::Down(1), Call::Down(2)]);
    view.text_field().unwrap()
}

fn type_keys(host: &Host, view: &EditorView, syms: &[u32]) {
    for &sym in syms {
        host.key(view.raw(), host.keycode(sym).0, KeyButMask::default());
    }
}

const RETURN: u32 = 0xff0d;
const ESCAPE: u32 = 0xff1b;
const BACKSPACE: u32 = 0xff08;
const DELETE: u32 = 0xffff;
const HOME: u32 = 0xff50;
const END: u32 = 0xff57;
const LEFT: u32 = 0xff51;

#[test]
fn text_field_opens_types_commits_and_cancels() {
    let _serial = serial();
    let Some((host, mut view, shared)) = attached("text_field_opens_types_commits_and_cancels", 0) else { return };
    let window = view.raw();

    // A double-click opens the field over the control: a child window, drawn by the view.
    let field = open_field(&host, &mut view, &shared);
    assert_eq!(host.children(window), [field]);
    assert_eq!(view.text_field_text().as_deref(), Some("1.5"));
    let (fx, fy, fw, fh) = host.geometry(field).unwrap();
    assert_eq!((fw as i32, fh as i32), (px(&view, 72.0), px(&view, 22.0)));
    // Centred on the control.
    assert_eq!((fx + fw as i32 / 2, fy + fh as i32 / 2), (px(&view, 120.0), px(&view, 100.0)));
    until(&mut view, "the field's frame", |_| host.pixel(field, 0, 0) == Some(FOCUS));
    // The text is drawn: the row through the middle has more than the panel and the selection.
    let colours = |row: Vec<u32>| {
        let mut seen = row;
        seen.sort_unstable();
        seen.dedup();
        seen.len()
    };
    assert!(colours(host.row(field, fh / 2, fw).unwrap()) > 3, "text in the field");
    assert_ne!(host.pixel(window, (fx + 3) as u32, (fy + 3) as u32), Some(BACKGROUND));

    // Typing replaces the selected text; no key reaches the editor.
    type_keys(&host, &view, &[0x32, 0x2e, 0x35, BACKSPACE, 0x37]);
    until(&mut view, "the typed text", |v| v.text_field_text().as_deref() == Some("2.7"));
    // Shift picks the key's second symbol.
    let (two, shifted) = host.keycode(0x32);
    if let Some(c) = char::from_u32(shifted).filter(|_| (0x21..=0x7e).contains(&shifted)) {
        host.key(window, two, KeyButMask::SHIFT);
        until(&mut view, "a shifted character", |v| v.text_field_text() == Some(format!("2.7{c}")));
        type_keys(&host, &view, &[BACKSPACE]);
    }
    // Home, Delete, End, Left.
    type_keys(&host, &view, &[HOME, DELETE, 0x33, END, 0x35, LEFT, 0x30]);
    until(&mut view, "the edited text", |v| v.text_field_text().as_deref() == Some("3.705"));
    assert_eq!(taken(&shared), []);

    // Return commits; the field goes.
    type_keys(&host, &view, &[RETURN]);
    until(&mut view, "the commit", |v| v.text_field().is_none());
    assert_eq!(taken(&shared), [Call::Commit("3.705".into(), true)]);
    host.sync();
    assert!(host.children(window).is_empty() && !host.exists(field));
    until(&mut view, "the frame under the field", |_| host.pixel(window, (fx + 3) as u32, (fy + 3) as u32) == Some(BACKGROUND));
    // With the field gone the keys are the editor's again.
    type_keys(&host, &view, &[0x32]);
    host.sync();
    view.pump();
    assert_eq!(taken(&shared), [], "an editor that wants no keys gets none");

    // Text that is no value: the field stays open and turns red. Escape then cancels.
    let field = open_field(&host, &mut view, &shared);
    assert_eq!(view.text_field_text().as_deref(), Some("3.705"));
    type_keys(&host, &view, &[0x78, RETURN]);
    until(&mut view, "the refused commit", |_| shared.borrow().calls.len() == 1);
    assert_eq!(taken(&shared), [Call::Commit("x".into(), false)]);
    assert_eq!(view.text_field(), Some(field));
    until(&mut view, "the red field", |_| host.pixel(field, 0, 0) == Some(BAD));
    type_keys(&host, &view, &[ESCAPE]);
    until(&mut view, "the cancel", |v| v.text_field().is_none());
    assert_eq!(taken(&shared), [Call::Cancel]);

    // Focus leaving commits valid text...
    open_field(&host, &mut view, &shared);
    type_keys(&host, &view, &[0x34]);
    host.focus(window, true);
    host.focus(window, false);
    until(&mut view, "the commit on focus loss", |v| v.text_field().is_none());
    assert_eq!(taken(&shared), [Call::Commit("4".into(), true)]);
    // ...and drops invalid text.
    open_field(&host, &mut view, &shared);
    type_keys(&host, &view, &[0x78]);
    host.focus(window, true);
    host.focus(window, false);
    until(&mut view, "the drop on focus loss", |v| v.text_field().is_none());
    assert_eq!(taken(&shared), [Call::Commit("x".into(), false), Call::Cancel]);

    // A click elsewhere on the view abandons the field.
    open_field(&host, &mut view, &shared);
    host.click(window, 1, 5, 5, host.later());
    until(&mut view, "the click away", |v| v.text_field().is_none());
    assert_eq!(taken(&shared), [Call::Cancel, Call::Down(1)]);

    // A click in the field is the field's: it stays, and the editor hears nothing.
    let field = open_field(&host, &mut view, &shared);
    host.click(field, 1, 2, 5, host.later());
    type_keys(&host, &view, &[0x39]);
    until(&mut view, "the caret at the click", |v| v.text_field_text().as_deref() == Some("94"));
    assert_eq!(taken(&shared), []);

    // The size changing under the field closes it.
    view.resize(320, 210);
    until(&mut view, "the cancel on resize", |v| v.text_field().is_none());
    assert_eq!(taken(&shared), [Call::Cancel]);

    // Hiding the view commits what is valid.
    open_field(&host, &mut view, &shared);
    type_keys(&host, &view, &[0x38]);
    until(&mut view, "the typed text", |v| v.text_field_text().as_deref() == Some("8"));
    view.set_hidden(true);
    assert!(view.text_field().is_none());
    assert_eq!(taken(&shared), [Call::Commit("8".into(), true)]);
}

/// A selection name of this test's own.
fn selection_name(test: &str) -> String {
    format!("OPENPLUGIN_TEST_{}_{test}", std::process::id())
}

/// Read a selection as a plain client would: `ConvertSelection`, wait for the answer, read the property.
fn read_selection(host: &Host, views: &mut [&mut EditorView], selection: u32, target: u32) -> Option<(u32, Vec<u8>)> {
    let property = host.atom("OPENPLUGIN_TEST_ANSWER");
    host.conn.convert_selection(host.parent, selection, target, property, x11rb::CURRENT_TIME).unwrap();
    host.conn.flush().unwrap();
    let mut answer = None;
    until_all(views, "the selection's answer", |_| {
        while let Some(event) = host.conn.poll_for_event().unwrap() {
            if let Event::SelectionNotify(e) = event {
                answer = Some(e.property);
            }
        }
        answer.is_some()
    });
    if answer == Some(x11rb::NONE) {
        return None;
    }
    let reply = host.conn.get_property(true, host.parent, property, AtomEnum::ANY, 0, 1 << 16).unwrap().reply().unwrap();
    Some((reply.type_, reply.value))
}

/// Answer the selection requests that have arrived, as an owner of `text` would.
fn serve_selection(host: &Host, text: &str, utf8: u32) -> usize {
    let mut served = 0;
    while let Some(event) = host.conn.poll_for_event().unwrap() {
        if let Event::SelectionRequest(e) = event {
            let property = if e.target == utf8 {
                host.conn.change_property8(PropMode::REPLACE, e.requestor, e.property, utf8, text.as_bytes()).unwrap();
                e.property
            } else {
                x11rb::NONE
            };
            let answer = xproto::SelectionNotifyEvent { response_type: xproto::SELECTION_NOTIFY_EVENT, sequence: 0, time: e.time, requestor: e.requestor, selection: e.selection, target: e.target, property };
            host.conn.send_event(false, e.requestor, EventMask::NO_EVENT, answer).unwrap();
            host.conn.flush().unwrap();
            served += 1;
        }
    }
    served
}

#[test]
fn clipboard_round_trips_between_views_and_plain_clients() {
    let _serial = serial();
    let Some((host_a, mut a, shared_a)) = attached("clipboard_round_trips_between_views_and_plain_clients", 0) else { return };
    let Some((host_b, mut b, shared_b)) = attached("clipboard_round_trips_between_views_and_plain_clients", 1) else { return };
    let name = selection_name("clipboard");
    assert!(a.set_clipboard_selection(&name) && b.set_clipboard_selection(&name));
    let (selection, utf8, targets) = (host_a.atom(&name), host_a.atom("UTF8_STRING"), host_a.atom("TARGETS"));
    let control = KeyButMask::CONTROL;
    assert_eq!(a.clipboard_text(), None);

    // Ctrl+C in A's field puts the selected text on the clipboard.
    shared_a.borrow_mut().value = "3.25 \u{b5}s".into();
    open_field(&host_a, &mut a, &shared_a);
    host_a.key(a.raw(), host_a.keycode(0x63).0, control);
    until(&mut a, "the copy", |v| v.clipboard_text().is_some());
    assert_eq!(a.clipboard_text().as_deref(), Some("3.25 \u{b5}s"));
    assert_eq!(host_a.conn.get_selection_owner(selection).unwrap().reply().unwrap().owner, a.raw());

    // A plain client reads it: the targets, UTF-8 and Latin-1.
    let (kind, value) = read_selection(&host_b, &mut [&mut a], selection, targets).expect("TARGETS");
    assert_eq!(kind, u32::from(AtomEnum::ATOM));
    let offered: Vec<u32> = value.chunks_exact(4).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]])).collect();
    assert!(offered.contains(&utf8) && offered.contains(&u32::from(AtomEnum::STRING)) && offered.contains(&targets));
    assert_eq!(read_selection(&host_b, &mut [&mut a], selection, utf8), Some((utf8, "3.25 \u{b5}s".as_bytes().to_vec())));
    assert_eq!(read_selection(&host_b, &mut [&mut a], selection, AtomEnum::STRING.into()), Some((AtomEnum::STRING.into(), b"3.25 \xb5s".to_vec())));
    // A target the view does not have is refused, not left unanswered.
    assert_eq!(read_selection(&host_b, &mut [&mut a], selection, host_b.atom("image/png")), None);

    // Ctrl+V in B's field, on another connection, pastes it over the selected text.
    // B's field takes the focus: A's field drops its text, which is no value.
    open_field(&host_b, &mut b, &shared_b);
    until_all(&mut [&mut a, &mut b], "A's field gone with the focus", |views| views[0].text_field().is_none());
    assert_eq!(taken(&shared_a), [Call::Commit("3.25 \u{b5}s".into(), false), Call::Cancel]);
    host_b.key(b.raw(), host_b.keycode(0x76).0, control);
    until_all(&mut [&mut a, &mut b], "the paste from the other view", |views| views[1].text_field_text().as_deref() == Some("3.25 \u{b5}s"));

    // Ctrl+X in B cuts: B owns the clipboard now and A has lost it.
    host_b.key(b.raw(), host_b.keycode(0x61).0, control);
    host_b.key(b.raw(), host_b.keycode(0x78).0, control);
    until_all(&mut [&mut a, &mut b], "the cut", |views| views[1].text_field_text().as_deref() == Some("") && views[0].clipboard_text().is_none());
    assert_eq!(b.clipboard_text().as_deref(), Some("3.25 \u{b5}s"));
    // Its own text comes back at once.
    host_b.key(b.raw(), host_b.keycode(0x76).0, control);
    until(&mut b, "the paste of its own text", |v| v.text_field_text().as_deref() == Some("3.25 \u{b5}s"));

    // A plain client takes the clipboard: both views paste what it serves.
    host_a.conn.set_selection_owner(host_a.parent, selection, x11rb::CURRENT_TIME).unwrap();
    host_a.sync();
    until(&mut b, "the clipboard taken away", |v| v.clipboard_text().is_none());
    host_b.key(b.raw(), host_b.keycode(0x61).0, control);
    host_b.key(b.raw(), host_b.keycode(0x76).0, control);
    until(&mut b, "the paste from a plain client", |v| {
        serve_selection(&host_a, "9.5", utf8);
        v.text_field_text().as_deref() == Some("9.5")
    });
    // A's field again: B's commits its text as it loses the focus.
    open_field(&host_a, &mut a, &shared_a);
    until_all(&mut [&mut a, &mut b], "B's field gone with the focus", |views| views[1].text_field().is_none());
    assert_eq!(taken(&shared_b), [Call::Commit("9.5".into(), true)]);
    host_a.key(a.raw(), host_a.keycode(0x76).0, control);
    until(&mut a, "the paste from a plain client", |v| {
        serve_selection(&host_a, "9.5", utf8);
        v.text_field_text().as_deref() == Some("9.5")
    });
    assert_eq!(taken(&shared_a), []);
    assert_eq!(taken(&shared_b), []);
}

/// Right-click the view and wait for the menu. Returns the menu's window.
fn open_menu(host: &Host, view: &mut EditorView, shared: &Rc<RefCell<Shared>>) -> u32 {
    host.click(view.raw(), 3, px(view, 150.0), px(view, 100.0), host.later());
    until(view, "the menu", |v| v.menu_window().is_some());
    assert_eq!(taken(shared), [Call::Right]);
    view.menu_window().unwrap()
}

/// A place inside item `index` of the menu, in the menu's device pixels (title 22, rows 22, 4 around).
fn item(view: &EditorView, index: usize) -> (i32, i32) {
    (px(view, 30.0), px(view, 4.0 + 22.0 + 22.0 * (index as f32 + 0.5)))
}

#[test]
fn parameter_menu_pops_up_chooses_and_dismisses() {
    let _serial = serial();
    let Some((host, mut view, shared)) = attached("parameter_menu_pops_up_chooses_and_dismisses", 0) else { return };
    assert!(view.set_clipboard_selection(&selection_name("menu")));
    let window = view.raw();
    assert_eq!(PARAM_MENU_ITEMS, ["Reset to default", "Copy value", "Paste value", "Type value"]);
    assert!(host.pointer_is_free());

    // The right button shows the menu: an unmanaged window on the root, at the click, on the screen.
    let menu = open_menu(&host, &mut view, &shared);
    let attributes = host.conn.get_window_attributes(menu).unwrap().reply().unwrap();
    assert!(attributes.override_redirect && attributes.map_state == MapState::VIEWABLE);
    assert_eq!(host.conn.query_tree(menu).unwrap().reply().unwrap().parent, host.root);
    let (mx, my, mw, mh) = host.geometry(menu).unwrap();
    let click = host.conn.translate_coordinates(view.raw(), host.root, px(&view, 150.0) as i16, px(&view, 100.0) as i16).unwrap().reply().unwrap();
    assert_eq!((mx, my), (i32::from(click.dst_x), i32::from(click.dst_y)));
    let (_, _, sw, sh) = host.geometry(host.root).unwrap();
    assert!(mx >= 0 && my >= 0 && mx + mw as i32 <= sw as i32 && my + mh as i32 <= sh as i32);
    assert_eq!(mh as i32, px(&view, 8.0 + 22.0 + 4.0 * 22.0));
    until(&mut view, "the menu's frame", |_| host.pixel(menu, 0, 0) == Some(EDGE));
    // While it is open the pointer is the menu's.
    assert!(!host.pointer_is_free(), "the menu grabs the pointer");

    // The item under the pointer lights up.
    let (ix, iy) = item(&view, 1);
    let edge = px(&view, 6.0) as u32;
    let plain = host.pixel(menu, edge, iy as u32).unwrap();
    host.motion(menu, ix, iy);
    until(&mut view, "the hover", |_| host.pixel(menu, edge, iy as u32) != Some(plain));
    let (_, other) = item(&view, 2);
    let unlit = host.pixel(menu, edge, other as u32).unwrap();
    host.motion(menu, ix, other);
    until(&mut view, "the hover moving on", |_| host.pixel(menu, edge, iy as u32) == Some(plain) && host.pixel(menu, edge, other as u32) != Some(unlit));

    // Letting the button go on "Copy value" runs it: the text is on the clipboard, the menu is gone.
    host.click(menu, 1, ix, iy, host.later());
    until(&mut view, "the copy", |v| v.menu_window().is_none());
    assert_eq!(taken(&shared), [Call::Action(PARAM, ParamMenuAction::Copy)]);
    assert_eq!(view.clipboard_text().as_deref(), Some("1.5"));
    host.sync();
    assert!(!host.exists(menu));
    assert!(host.pointer_is_free(), "the grab ends with the menu");

    // "Paste value" hands the clipboard's text over.
    let menu = open_menu(&host, &mut view, &shared);
    let (ix, iy) = item(&view, 2);
    host.click(menu, 1, ix, iy, host.later());
    until(&mut view, "the paste", |_| !shared.borrow().calls.is_empty());
    assert_eq!(taken(&shared), [Call::Action(PARAM, ParamMenuAction::Paste("1.5".into()))]);
    assert!(view.menu_window().is_none());

    // "Reset to default".
    let menu = open_menu(&host, &mut view, &shared);
    let (ix, iy) = item(&view, 0);
    host.click(menu, 1, ix, iy, host.later());
    until(&mut view, "the reset", |_| !shared.borrow().calls.is_empty());
    assert_eq!(taken(&shared), [Call::Action(PARAM, ParamMenuAction::Reset)]);

    // "Type value" opens the field.
    let menu = open_menu(&host, &mut view, &shared);
    let (ix, iy) = item(&view, 3);
    host.click(menu, 1, ix, iy, host.later());
    until(&mut view, "the field from the menu", |v| v.text_field().is_some());
    assert_eq!(taken(&shared), [Call::Action(PARAM, ParamMenuAction::Type)]);
    assert!(view.menu_window().is_none() && host.pointer_is_free());
    // A right click abandons the field and shows the menu again.
    host.click(window, 3, px(&view, 150.0), px(&view, 100.0), host.later());
    until(&mut view, "the menu over the field", |v| v.menu_window().is_some());
    assert_eq!(taken(&shared), [Call::Cancel, Call::Right]);
    assert!(view.text_field().is_none());
    let menu = view.menu_window().unwrap();

    // Escape dismisses: nothing is chosen.
    host.key(menu, host.keycode(ESCAPE).0, KeyButMask::default());
    until(&mut view, "the menu gone on Escape", |v| v.menu_window().is_none());
    host.sync();
    assert!(!host.exists(menu) && host.pointer_is_free());

    // A press outside dismisses (the grab reports it to the menu, outside its bounds).
    let menu = open_menu(&host, &mut view, &shared);
    host.button(menu, true, 1, -40, -40, host.later());
    until(&mut view, "the menu gone on a press outside", |v| v.menu_window().is_none());
    host.sync();
    assert!(!host.exists(menu) && host.pointer_is_free());
    // The title and the edge choose nothing.
    let menu = open_menu(&host, &mut view, &shared);
    host.click(menu, 1, px(&view, 30.0), px(&view, 10.0), host.later());
    host.sync();
    view.pump();
    assert_eq!(view.menu_window(), Some(menu));
    // The arrow keys and Return choose too.
    host.key(menu, host.keycode(0xff54).0, KeyButMask::default());
    host.key(menu, host.keycode(RETURN).0, KeyButMask::default());
    until(&mut view, "the choice by keys", |v| v.menu_window().is_none());
    assert_eq!(taken(&shared), [Call::Action(PARAM, ParamMenuAction::Reset)]);

    // Hiding the view takes the menu with it.
    let menu = open_menu(&host, &mut view, &shared);
    view.set_hidden(true);
    host.sync();
    assert!(view.menu_window().is_none() && !host.exists(menu) && host.pointer_is_free());
    view.set_hidden(false);
    until(&mut view, "the view shown again", |_| host.pixel(window, 1, 1) == Some(BACKGROUND));

    // So does dropping it: no window and no grab are left behind.
    let menu = open_menu(&host, &mut view, &shared);
    drop(view);
    host.sync();
    assert!(!host.exists(menu) && host.pointer_is_free());
}

#[test]
fn menu_hook_and_choices_without_a_window() {
    let _serial = serial();
    let Some((host, mut view, shared)) = attached("menu_hook_and_choices_without_a_window", 0) else { return };
    let name = selection_name("hook");
    assert!(view.set_clipboard_selection(&name));
    let asked = Rc::new(RefCell::new(Vec::new()));
    let log = asked.clone();
    view.set_menu_hook(Some(Box::new(move |menu| {
        log.borrow_mut().push(menu.clone());
        true
    })));
    host.click(view.raw(), 3, px(&view, 150.0), px(&view, 100.0), host.later());
    until(&mut view, "the menu request", |_| !asked.borrow().is_empty());
    assert_eq!(asked.borrow()[0], ParamMenu { param: PARAM, x: 150.0, y: 100.0, label: "Gain".into() });
    assert!(view.menu_window().is_none());
    taken(&shared);

    // With a text standing in for the clipboard, copy returns it and leaves the clipboard alone.
    assert_eq!(view.choose_menu_item(1, Some("")).as_deref(), Some("1.5"));
    assert_eq!(view.clipboard_text(), None);
    assert_eq!(view.choose_menu_item(2, Some("7")), None);
    assert_eq!(view.choose_menu_item(0, None), None);
    assert_eq!(view.choose_menu_item(9, None), None);
    assert_eq!(taken(&shared), [Call::Action(PARAM, ParamMenuAction::Copy), Call::Action(PARAM, ParamMenuAction::Paste("7".into())), Call::Action(PARAM, ParamMenuAction::Reset)]);
    // Without one, copy writes the clipboard.
    assert_eq!(view.choose_menu_item(1, None).as_deref(), Some("1.5"));
    assert_eq!(view.clipboard_text().as_deref(), Some("1.5"));
    taken(&shared);

    // An owner that never answers: the paste ends empty after the bounded wait, and the view keeps ticking meanwhile.
    let selection = host.atom(&name);
    host.conn.set_selection_owner(host.parent, selection, x11rb::CURRENT_TIME).unwrap();
    host.sync();
    until(&mut view, "the clipboard taken away", |v| v.clipboard_text().is_none());
    let (start, ticks) = (Instant::now(), shared.borrow().ticks);
    assert_eq!(view.choose_menu_item(2, None), None);
    assert!(start.elapsed() < Duration::from_millis(CLIPBOARD_WAIT_MS / 2), "asking does not wait");
    until(&mut view, "the unanswered paste", |_| !shared.borrow().calls.is_empty());
    assert_eq!(taken(&shared), [Call::Action(PARAM, ParamMenuAction::Paste(String::new()))]);
    let waited = start.elapsed();
    assert!(waited >= Duration::from_millis(CLIPBOARD_WAIT_MS - 50) && waited < Duration::from_millis(CLIPBOARD_WAIT_MS * 3), "{waited:?}");
    assert!(shared.borrow().ticks > ticks + 5, "the editor ticked while the clipboard was awaited");
    // An owner that answers: the paste carries its text.
    let utf8 = host.atom("UTF8_STRING");
    assert_eq!(view.choose_menu_item(2, None), None);
    until(&mut view, "the answered paste", |_| {
        serve_selection(&host, "0.25", utf8);
        !shared.borrow().calls.is_empty()
    });
    assert_eq!(taken(&shared), [Call::Action(PARAM, ParamMenuAction::Paste("0.25".into()))]);
}

#[test]
fn tooltips_show_after_a_rest_and_hide() {
    let _serial = serial();
    let Some((host, mut view, shared)) = attached("tooltips_show_after_a_rest_and_hide", 0) else { return };
    let window = view.raw();
    shared.borrow_mut().tips = vec![
        Tooltip { rect: TIP, text: "Gain: 3.0 dB".into() },
        Tooltip { rect: DamageRect::new(0.0, 0.0, 0.0, 10.0), text: "empty".into() },
        Tooltip { rect: DamageRect::new(f32::NAN, 0.0, 10.0, 10.0), text: "not finite".into() },
    ];
    assert_eq!(view.tooltip_rects(), [([10.0, 10.0, 80.0, 40.0], "Gain: 3.0 dB".to_string())]);

    // The pointer rests in the rectangle: the tip shows after the delay, with the text asked then.
    let asks = shared.borrow().tip_asks;
    let start = Instant::now();
    host.motion(window, px(&view, 20.0), px(&view, 20.0));
    until(&mut view, "the tooltip", |v| v.tooltip_window().is_some());
    assert!(start.elapsed() >= Duration::from_millis(TOOLTIP_DELAY_MS - 50), "{:?}", start.elapsed());
    assert_eq!(shared.borrow().tip_asks, asks + 1);
    assert_eq!(view.tooltip_text().as_deref(), Some("Gain: 3.0 dB"));
    let tip = view.tooltip_window().unwrap();
    let attributes = host.conn.get_window_attributes(tip).unwrap().reply().unwrap();
    assert!(attributes.override_redirect && attributes.map_state == MapState::VIEWABLE);
    assert_eq!(host.conn.query_tree(tip).unwrap().reply().unwrap().parent, host.root);
    // Below and right of the pointer, never under it.
    let pointer = host.conn.translate_coordinates(window, host.root, px(&view, 20.0) as i16, px(&view, 20.0) as i16).unwrap().reply().unwrap();
    let (tx, ty, tw, th) = host.geometry(tip).unwrap();
    assert!(tx > i32::from(pointer.dst_x) && ty > i32::from(pointer.dst_y), "tip at {tx}, {ty}");
    until(&mut view, "the tooltip's frame", |_| host.pixel(tip, 0, 0) == Some(EDGE));
    let mut colours = host.row(tip, th / 2, tw).unwrap();
    colours.sort_unstable();
    colours.dedup();
    assert!(colours.len() > 3, "text in the tooltip");

    // Moving inside the rectangle keeps it; leaving the rectangle hides it.
    host.motion(window, px(&view, 40.0), px(&view, 30.0));
    host.sync();
    view.pump();
    assert_eq!(view.tooltip_window(), Some(tip));
    host.motion(window, px(&view, 200.0), px(&view, 150.0));
    until(&mut view, "the tooltip gone", |v| v.tooltip_window().is_none());
    host.sync();
    assert!(!host.exists(tip));
    // Resting where there is no rectangle asks once and shows nothing.
    let asks = shared.borrow().tip_asks;
    until(&mut view, "the look for a tip", |_| shared.borrow().tip_asks > asks);
    assert!(view.tooltip_window().is_none());

    // The text is asked again each time: a value that changed is current.
    shared.borrow_mut().tips[0].text = "Gain: 4.5 dB".into();
    host.motion(window, px(&view, 30.0), px(&view, 30.0));
    until(&mut view, "the tooltip again", |v| v.tooltip_window().is_some());
    assert_eq!(view.tooltip_text().as_deref(), Some("Gain: 4.5 dB"));
    // A click hides it, and it stays away until the pointer moves.
    let tip = view.tooltip_window().unwrap();
    host.click(window, 1, px(&view, 30.0), px(&view, 30.0), host.later());
    until(&mut view, "the tooltip gone on a click", |v| v.tooltip_window().is_none());
    host.sync();
    assert!(!host.exists(tip));

    // Hiding the view hides the tip.
    host.motion(window, px(&view, 50.0), px(&view, 20.0));
    until(&mut view, "the tooltip a third time", |v| v.tooltip_window().is_some());
    let tip = view.tooltip_window().unwrap();
    view.set_hidden(true);
    host.sync();
    assert!(view.tooltip_window().is_none() && !host.exists(tip));
}
