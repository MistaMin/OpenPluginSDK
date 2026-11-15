//! The X11 editor view against a real X server: pixels read back with
//! `GetImage`, input sent with `SendEvent`, sizes read with `GetGeometry`.
//!
//! Every test passes without doing anything when no X server answers on
//! `DISPLAY` (CI has none) and says so. On a desktop the windows are small,
//! unmanaged (override-redirect) and gone when the test ends.

#![cfg(target_os = "linux")]

use openplugin_core::editor::{DamageRect, Key, Modifiers, SizeRange};
use openplugin_core::{Editor, Frame};
use openplugin_dri::x11::{run_window, EditorView, DOUBLE_CLICK_MS};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    self, AtomEnum, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask, ImageFormat, ImageOrder, KeyButMask, NotifyDetail,
    NotifyMode, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

const DEADLINE: Duration = Duration::from_secs(10);

/// The screen space one test's parent window has to itself (the window is 10 less each way).
const CELL: (u32, u32) = (340, 280);

#[derive(Clone, Debug, PartialEq)]
enum Call {
    Down(f32, f32, u32),
    Move(f32, f32),
    Up(f32, f32),
    Right(f32, f32),
    Wheel(f32, f32, f32),
    KeyDown(Key, Modifiers),
    KeyUp(Key, Modifiers),
    Focus(bool),
    Mods(Modifiers),
    Resize(u32, u32),
}

/// What the test and its editor share.
struct Shared {
    calls: Vec<Call>,
    size: (u32, u32),
    range: SizeRange,
    /// The colours of the left and the right half.
    left: u32,
    right: u32,
    /// What `damage` answers next; `Some` answers go back to "nothing" after one ask.
    damage: Option<Vec<DamageRect>>,
    wants_keys: bool,
    /// The keys the editor uses; the others it leaves to the host.
    used: Vec<Key>,
    ticks: usize,
    renders: usize,
    /// Run inside `mouse_down`, as a host that answers a size request at once would.
    on_down: Option<Box<dyn FnMut()>>,
}

struct TestEditor {
    shared: Rc<RefCell<Shared>>,
    pixels: Vec<u32>,
}

fn editor(size: (u32, u32), range: SizeRange) -> (Box<TestEditor>, Rc<RefCell<Shared>>) {
    let shared = Rc::new(RefCell::new(Shared {
        calls: Vec::new(),
        size,
        range,
        left: 0xffff_2010,
        right: 0xff10_40ff,
        damage: None,
        wants_keys: false,
        used: Vec::new(),
        ticks: 0,
        renders: 0,
        on_down: None,
    }));
    (Box::new(TestEditor { shared: shared.clone(), pixels: Vec::new() }), shared)
}

impl TestEditor {
    fn log(&self, call: Call) {
        self.shared.borrow_mut().calls.push(call);
    }
}

impl Editor for TestEditor {
    fn size(&self) -> (u32, u32) {
        self.shared.borrow().size
    }
    fn size_range(&self) -> SizeRange {
        self.shared.borrow().range
    }
    fn resize(&mut self, width: u32, height: u32) {
        self.log(Call::Resize(width, height));
        self.shared.borrow_mut().size = (width, height);
    }
    fn param_changed(&mut self, _index: usize, _plain: f32) {}
    fn tick(&mut self, _dt: f32) {
        self.shared.borrow_mut().ticks += 1;
    }
    fn render(&mut self, scale: f32) -> Frame<'_> {
        let mut s = self.shared.borrow_mut();
        s.renders += 1;
        let (w, h) = ((s.size.0 as f32 * scale).round() as usize, (s.size.1 as f32 * scale).round() as usize);
        self.pixels.clear();
        for _ in 0..h {
            for x in 0..w {
                self.pixels.push(if x < w / 2 { s.left } else { s.right });
            }
        }
        Frame { pixels: &self.pixels, width: w, height: h }
    }
    fn damage(&mut self) -> Option<Vec<DamageRect>> {
        let mut s = self.shared.borrow_mut();
        let answer = s.damage.clone();
        if s.damage.is_some() {
            s.damage = Some(Vec::new());
        }
        answer
    }
    fn modifiers_changed(&mut self, modifiers: Modifiers) {
        self.log(Call::Mods(modifiers));
    }
    fn wants_keys(&self) -> bool {
        self.shared.borrow().wants_keys
    }
    fn key_down(&mut self, key: Key, modifiers: Modifiers) -> bool {
        self.log(Call::KeyDown(key, modifiers));
        self.shared.borrow().used.contains(&key)
    }
    fn key_up(&mut self, key: Key, modifiers: Modifiers) -> bool {
        self.log(Call::KeyUp(key, modifiers));
        self.shared.borrow().used.contains(&key)
    }
    fn focus_changed(&mut self, focused: bool) {
        self.log(Call::Focus(focused));
    }
    fn mouse_down(&mut self, x: f32, y: f32, clicks: u32) {
        self.log(Call::Down(x, y, clicks));
        let hook = self.shared.borrow_mut().on_down.take();
        if let Some(mut hook) = hook {
            hook();
        }
    }
    fn mouse_move(&mut self, x: f32, y: f32) {
        self.log(Call::Move(x, y));
    }
    fn mouse_up(&mut self, x: f32, y: f32) {
        self.log(Call::Up(x, y));
    }
    fn wheel(&mut self, x: f32, y: f32, dy: f32) {
        self.log(Call::Wheel(x, y, dy));
    }
    fn right_mouse_down(&mut self, x: f32, y: f32) -> bool {
        self.log(Call::Right(x, y));
        true
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
    fn new(test: &str) -> Option<Host> {
        let Ok((conn, screen)) = RustConnection::connect(None) else {
            println!("{test}: skipped, no X server on DISPLAY");
            return None;
        };
        let root = conn.setup().roots[screen].root;
        // Hyprland remembers the id and geometry of any window it saw made on
        // the root and never saw destroyed (one reparented into a host, as
        // plugin editors of other toolkits are), and later moves the window
        // that is given the same id. First ids are the ones such windows had:
        // skip them.
        for _ in 0..16 {
            conn.generate_id().unwrap();
        }
        let parent = conn.generate_id().unwrap();
        // Every test's window has a place of its own: where two overlap, the
        // one below is clipped and reads back the one above.
        static SLOT: AtomicU32 = AtomicU32::new(0);
        let slot = SLOT.fetch_add(1, Ordering::Relaxed);
        let (screen_w, screen_h) = (u32::from(conn.setup().roots[screen].width_in_pixels), u32::from(conn.setup().roots[screen].height_in_pixels));
        let (columns, rows) = ((screen_w / CELL.0).max(1), (screen_h / CELL.1).max(1));
        let (x, y) = ((slot % columns) * CELL.0, (slot / columns % rows) * CELL.1);
        // Unmanaged, so the window manager neither tiles nor decorates it.
        let aux = CreateWindowAux::new().override_redirect(1).background_pixel(0).event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE);
        conn.create_window(0, parent, root, x as i16, y as i16, (CELL.0 - 10) as u16, (CELL.1 - 10) as u16, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap().check().unwrap();
        conn.map_window(parent).unwrap().check().unwrap();
        PARENT.with(|p| p.set(parent));
        Some(Host { conn, root, parent })
    }

    /// Returns once the server has handled everything sent so far.
    fn sync(&self) {
        self.conn.get_input_focus().unwrap().reply().unwrap();
    }

    fn geometry(&self, window: u32) -> Option<(u32, u32)> {
        let g = self.conn.get_geometry(window).ok()?.reply().ok()?;
        Some((u32::from(g.width), u32::from(g.height)))
    }

    /// The colour on screen at a window position, `None` while it cannot be read.
    fn pixel(&self, window: u32, x: u32, y: u32) -> Option<u32> {
        let image = self.conn.get_image(ImageFormat::Z_PIXMAP, window, x as i16, y as i16, 1, 1, !0).ok()?.reply().ok()?;
        let bytes: [u8; 4] = image.data.get(..4)?.try_into().ok()?;
        let word = if self.conn.setup().image_byte_order == ImageOrder::MSB_FIRST { u32::from_be_bytes(bytes) } else { u32::from_le_bytes(bytes) };
        Some(word & 0x00ff_ffff)
    }

    #[allow(clippy::too_many_arguments)]
    fn button(&self, window: u32, press: bool, button: u8, x: f32, y: f32, state: KeyButMask, time: u32) {
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
    }

    fn motion(&self, window: u32, x: f32, y: f32, state: KeyButMask) {
        let event = xproto::MotionNotifyEvent {
            response_type: xproto::MOTION_NOTIFY_EVENT,
            detail: xproto::Motion::NORMAL,
            sequence: 0,
            time: 0,
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
        self.conn.send_event(false, window, EventMask::POINTER_MOTION, event).unwrap();
    }

    fn key(&self, window: u32, press: bool, code: u8, state: KeyButMask, time: u32) {
        let event = xproto::KeyPressEvent {
            response_type: if press { xproto::KEY_PRESS_EVENT } else { xproto::KEY_RELEASE_EVENT },
            detail: code,
            sequence: 0,
            time,
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
    }

    /// The keycode whose unshifted symbol is `sym`.
    fn keycode(&self, sym: u32) -> Option<u8> {
        let setup = self.conn.setup();
        let map = self.conn.get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1).ok()?.reply().ok()?;
        let per = usize::from(map.keysyms_per_keycode);
        let index = map.keysyms.chunks(per).position(|syms| syms[0] == sym)?;
        Some(setup.min_keycode + index as u8)
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.conn.destroy_window(self.parent);
        let _ = self.conn.flush();
    }
}

/// Pump the view until `done`, or fail at the deadline.
fn until(view: &mut EditorView, what: &str, mut done: impl FnMut(&mut EditorView) -> bool) {
    let start = Instant::now();
    loop {
        view.pump();
        if done(view) {
            return;
        }
        assert!(start.elapsed() < DEADLINE, "timed out waiting for {what} ({})", parent_now());
        std::thread::sleep(Duration::from_millis(2));
    }
}

thread_local! {
    /// This test's parent window, for the note a timeout leaves.
    static PARENT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Where the test's parent window is now: a window manager that moved or
/// sized it explains a frame that never showed.
fn parent_now() -> String {
    let parent = PARENT.with(|p| p.get());
    let geometry = RustConnection::connect(None).ok().and_then(|(conn, _)| conn.get_geometry(parent).ok()?.reply().ok());
    match geometry {
        Some(g) => format!("parent window {parent:#x} is {} x {} at {}, {}", g.width, g.height, g.x, g.y),
        None => format!("parent window {parent:#x} is gone"),
    }
}

/// A view attached to a host's parent window, with its first frame on screen.
fn attached(test: &str, size: (u32, u32), range: SizeRange) -> Option<(Host, EditorView, Rc<RefCell<Shared>>)> {
    let host = Host::new(test)?;
    let (ed, shared) = editor(size, range);
    let Some(mut view) = EditorView::new(ed, None) else {
        println!("{test}: skipped, the view found no X server");
        return None;
    };
    assert_ne!(view.raw(), 0);
    assert!(view.fd() >= 0);
    assert!(view.attach(host.parent), "attach");
    let window = view.raw();
    let left = shared.borrow().left & 0x00ff_ffff;
    until(&mut view, "the first frame", |_| host.pixel(window, 1, 1) == Some(left));
    Some((host, view, shared))
}

/// Device pixels of a length in points.
fn px(view: &EditorView, points: u32) -> u32 {
    (points as f32 * view.scale()).round() as u32
}

fn taken(shared: &Rc<RefCell<Shared>>) -> Vec<Call> {
    std::mem::take(&mut shared.borrow_mut().calls)
}

const NONE: Modifiers = Modifiers { shift: false, ctrl: false, alt: false, command: false };
const SHIFT: Modifiers = Modifiers { shift: true, ..NONE };

#[test]
fn frame_reaches_the_screen() {
    let Some((host, view, shared)) = attached("frame_reaches_the_screen", (64, 48), SizeRange::fixed(64, 48)) else { return };
    let window = view.raw();
    let (w, h) = (px(&view, 64), px(&view, 48));
    assert_eq!(host.geometry(window), Some((w, h)), "the window is the editor's size times the scale");
    let tree = host.conn.query_tree(window).unwrap().reply().unwrap();
    assert_eq!(tree.parent, host.parent);
    let (left, right) = (shared.borrow().left & 0xff_ffff, shared.borrow().right & 0xff_ffff);
    assert_eq!(host.pixel(window, 0, 0), Some(left));
    assert_eq!(host.pixel(window, w / 2 - 1, h / 2), Some(left));
    assert_eq!(host.pixel(window, w / 2, h / 2), Some(right));
    assert_eq!(host.pixel(window, w - 1, h - 1), Some(right));
    assert_eq!(host.pixel(window, 0, h - 1), Some(left));
}

#[test]
fn only_damage_is_presented() {
    let Some((host, mut view, shared)) = attached("only_damage_is_presented", (64, 48), SizeRange::fixed(64, 48)) else { return };
    let window = view.raw();
    let old = shared.borrow().left & 0xff_ffff;
    let new = 0x0020_c030;
    // From here the editor reports its damage; the first answer is "nothing".
    shared.borrow_mut().damage = Some(Vec::new());
    let ticks = shared.borrow().ticks;
    until(&mut view, "a tick", |_| shared.borrow().ticks > ticks);

    // New colours and no damage: nothing is rendered or sent.
    {
        let mut s = shared.borrow_mut();
        s.left = new;
        s.right = new;
    }
    let (ticks, renders) = (shared.borrow().ticks, shared.borrow().renders);
    until(&mut view, "three ticks", |_| shared.borrow().ticks >= ticks + 3);
    host.sync();
    assert_eq!(shared.borrow().renders, renders, "an editor with no damage is not rendered");
    assert_eq!(host.pixel(window, 2, 2), Some(old));

    // One rectangle: only it (and a pixel around it) changes on screen.
    shared.borrow_mut().damage = Some(vec![DamageRect::new(10.0, 10.0, 10.0, 10.0)]);
    let inside = (px(&view, 15), px(&view, 15));
    until(&mut view, "the damaged rectangle", |_| host.pixel(window, inside.0, inside.1) == Some(new));
    assert_eq!(host.pixel(window, 2, 2), Some(old));
    assert_eq!(host.pixel(window, px(&view, 10) - 2, px(&view, 15)), Some(old));
    assert_eq!(host.pixel(window, px(&view, 20) + 2, px(&view, 15)), Some(old));
    assert_eq!(host.pixel(window, px(&view, 15), px(&view, 20) + 2), Some(old));
    assert_eq!(host.pixel(window, px(&view, 10), px(&view, 10)), Some(new));
    assert_eq!(host.pixel(window, px(&view, 20) - 1, px(&view, 20) - 1), Some(new));

    // No damage report at all: the whole frame.
    shared.borrow_mut().damage = None;
    until(&mut view, "the whole frame", |_| host.pixel(window, 2, 2) == Some(new));
    assert_eq!(host.pixel(window, px(&view, 64) - 1, px(&view, 48) - 1), Some(new));
}

#[test]
fn mouse_reaches_the_editor_in_points() {
    let Some((host, mut view, shared)) = attached("mouse_reaches_the_editor_in_points", (64, 48), SizeRange::fixed(64, 48)) else { return };
    let window = view.raw();
    let s = view.scale();
    let none = KeyButMask::default();
    taken(&shared);

    // Press, drag (also outside the window, as under the implicit grab), release.
    host.button(window, true, 1, 10.0 * s, 20.0 * s, none, 1000);
    host.motion(window, 12.0 * s, 22.0 * s, KeyButMask::BUTTON1);
    host.button(window, false, 1, -5.0 * s, 60.0 * s, KeyButMask::BUTTON1, 1050);
    host.sync();
    view.pump();
    assert_eq!(taken(&shared), vec![Call::Down(10.0, 20.0, 1), Call::Move(12.0, 22.0), Call::Up(-5.0, 60.0)]);

    // A second press in time and place is a double click; a late one is not, nor is a far one.
    host.button(window, true, 1, 11.0 * s, 20.0 * s, none, 1000 + DOUBLE_CLICK_MS);
    host.button(window, true, 1, 11.0 * s, 20.0 * s, none, 1001 + 2 * DOUBLE_CLICK_MS);
    host.button(window, true, 1, 40.0 * s, 20.0 * s, none, 1002 + 2 * DOUBLE_CLICK_MS);
    host.sync();
    view.pump();
    assert_eq!(taken(&shared), vec![Call::Down(11.0, 20.0, 2), Call::Down(11.0, 20.0, 1), Call::Down(40.0, 20.0, 1)]);

    // Right button, wheel up (away from the user, positive) and down.
    host.button(window, true, 3, 5.0 * s, 6.0 * s, none, 5000);
    host.button(window, false, 3, 5.0 * s, 6.0 * s, none, 5001);
    host.button(window, true, 4, 7.0 * s, 8.0 * s, none, 5002);
    host.button(window, false, 4, 7.0 * s, 8.0 * s, none, 5003);
    host.button(window, true, 5, 7.0 * s, 8.0 * s, none, 5004);
    host.sync();
    view.pump();
    assert_eq!(taken(&shared), vec![Call::Right(5.0, 6.0), Call::Wheel(7.0, 8.0, 1.0), Call::Wheel(7.0, 8.0, -1.0)]);

    // Modifiers are told before the call they belong to, and only when they change.
    host.button(window, true, 1, 1.0 * s, 1.0 * s, KeyButMask::SHIFT, 9000);
    host.button(window, false, 1, 1.0 * s, 1.0 * s, KeyButMask::SHIFT | KeyButMask::BUTTON1, 9001);
    host.motion(window, 2.0 * s, 2.0 * s, KeyButMask::CONTROL);
    host.sync();
    view.pump();
    let ctrl = Modifiers { ctrl: true, ..NONE };
    assert_eq!(taken(&shared), vec![Call::Mods(SHIFT), Call::Down(1.0, 1.0, 1), Call::Up(1.0, 1.0), Call::Mods(ctrl), Call::Move(2.0, 2.0)]);

    // A run of moves that arrives together is one move, the last.
    for i in 0..5 {
        host.motion(window, (10 + i) as f32 * s, 3.0 * s, KeyButMask::CONTROL);
    }
    host.sync();
    view.pump();
    assert_eq!(taken(&shared), vec![Call::Move(14.0, 3.0)]);
}

#[test]
fn keys_go_through_the_keyboard_mapping() {
    let Some((host, mut view, shared)) = attached("keys_go_through_the_keyboard_mapping", (64, 48), SizeRange::fixed(64, 48)) else { return };
    let window = view.raw();
    let (Some(a), Some(b), Some(left), Some(shift)) = (host.keycode(0x61), host.keycode(0x62), host.keycode(0xff51), host.keycode(0xffe1)) else {
        println!("keys_go_through_the_keyboard_mapping: skipped, the keyboard has no a, b, Left or Shift");
        return;
    };
    let none = KeyButMask::default();
    let selects_keys = |host: &Host| {
        let attributes = host.conn.get_window_attributes(window).unwrap().reply().unwrap();
        attributes.all_event_masks.contains(EventMask::KEY_PRESS)
    };

    // An editor that wants no keys does not even select them: they stay with the host.
    assert!(!selects_keys(&host));
    taken(&shared);

    shared.borrow_mut().wants_keys = true;
    shared.borrow_mut().used = vec![Key::Char('a'), Key::Left];
    until(&mut view, "key events to be selected", |_| selects_keys(&host));
    taken(&shared);

    host.key(window, true, a, none, 100);
    host.key(window, false, a, none, 150);
    // Shift goes down (its own press does not carry it yet), then Left with it.
    host.key(window, true, shift, none, 200);
    host.key(window, true, left, KeyButMask::SHIFT, 250);
    host.key(window, false, left, KeyButMask::SHIFT, 300);
    host.key(window, false, shift, KeyButMask::SHIFT, 350);
    host.sync();
    view.pump();
    assert_eq!(
        taken(&shared),
        vec![
            Call::KeyDown(Key::Char('a'), NONE),
            Call::KeyUp(Key::Char('a'), NONE),
            Call::Mods(SHIFT),
            Call::KeyDown(Key::Left, SHIFT),
            Call::KeyUp(Key::Left, SHIFT),
            Call::Mods(NONE),
        ]
    );

    // Auto-repeat (release and press at one instant) is a second press, no release.
    host.key(window, true, a, none, 400);
    host.key(window, false, a, none, 430);
    host.key(window, true, a, none, 430);
    host.key(window, false, a, none, 460);
    host.sync();
    view.pump();
    let down = Call::KeyDown(Key::Char('a'), NONE);
    assert_eq!(taken(&shared), vec![down.clone(), down, Call::KeyUp(Key::Char('a'), NONE)]);

    // A key the editor does not use goes on to the parent window.
    while host.conn.poll_for_event().unwrap().is_some() {}
    host.key(window, true, b, none, 500);
    host.sync();
    view.pump();
    assert_eq!(taken(&shared), vec![Call::KeyDown(Key::Char('b'), NONE)]);
    let start = Instant::now();
    let forwarded = loop {
        match host.conn.poll_for_event().unwrap() {
            Some(Event::KeyPress(e)) => break e,
            _ => {
                assert!(start.elapsed() < DEADLINE, "timed out waiting for the forwarded key");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    };
    assert_eq!((forwarded.event, forwarded.child, forwarded.detail, forwarded.time), (host.parent, window, b, 500));

    // Focus in and out.
    host.focus(window, true);
    host.focus(window, true);
    host.focus(window, false);
    host.sync();
    view.pump();
    assert_eq!(taken(&shared), vec![Call::Focus(true), Call::Focus(false)]);
}

#[test]
fn click_takes_the_input_focus() {
    let Some((host, mut view, shared)) = attached("click_takes_the_input_focus", (64, 48), SizeRange::fixed(64, 48)) else { return };
    let window = view.raw();
    let before = host.conn.get_input_focus().unwrap().reply().unwrap();

    // An editor that wants no keys leaves the focus where it is.
    host.button(window, true, 1, 3.0, 3.0, KeyButMask::default(), 0);
    host.sync();
    view.pump();
    host.sync();
    assert_ne!(host.conn.get_input_focus().unwrap().reply().unwrap().focus, window);

    shared.borrow_mut().wants_keys = true;
    host.button(window, true, 1, 3.0, 3.0, KeyButMask::default(), 0);
    host.sync();
    view.pump();
    host.sync();
    let focus = host.conn.get_input_focus().unwrap().reply().unwrap().focus;
    // Give the focus back before judging: this is somebody's desktop.
    let _ = host.conn.set_input_focus(before.revert_to, before.focus, 0u32);
    host.sync();
    assert_eq!(focus, window, "a click gives the view the input focus");
    until(&mut view, "the focus events", |_| {
        let calls = &shared.borrow().calls;
        calls.contains(&Call::Focus(true)) && calls.last() == Some(&Call::Focus(false))
    });
}

#[test]
fn sizes_follow_the_view_the_host_and_the_editor() {
    let range = SizeRange::between((40, 30), (100, 80));
    let Some((host, mut view, shared)) = attached("sizes_follow_the_view_the_host_and_the_editor", (64, 48), range) else { return };
    let window = view.raw();
    let right = shared.borrow().right & 0xff_ffff;
    taken(&shared);

    // The format bridge resizes (CLAP set_size, LV2 ui:resize).
    assert_eq!(view.resize(80, 60), (80, 60));
    assert_eq!(view.size(), (80, 60));
    assert_eq!(taken(&shared), vec![Call::Resize(80, 60)]);
    let want = (px(&view, 80), px(&view, 60));
    until(&mut view, "the window to be 80 x 60", |_| host.geometry(window) == Some(want));
    until(&mut view, "the frame at 80 x 60", |_| host.pixel(window, want.0 - 1, want.1 - 1) == Some(right));

    // Out of range comes back in range.
    assert_eq!(view.resize(1000, 1000), (100, 80));
    assert_eq!(view.resize(1, 1), (40, 30));
    let want = (px(&view, 40), px(&view, 30));
    until(&mut view, "the window to be 40 x 30", |_| host.geometry(window) == Some(want));
    taken(&shared);

    // The host sizes the window itself: the editor follows, the window stays.
    let want = (px(&view, 90), px(&view, 50));
    host.conn.configure_window(window, &ConfigureWindowAux::new().width(want.0).height(want.1)).unwrap();
    host.sync();
    until(&mut view, "the editor to be 90 x 50", |v| v.size() == (90, 50));
    assert_eq!(taken(&shared), vec![Call::Resize(90, 50)]);
    until(&mut view, "the frame at 90 x 50", |_| host.pixel(window, want.0 - 1, want.1 - 1) == Some(right));
    assert_eq!(host.geometry(window), Some(want));

    // The editor changes its own size: the window follows after the tick.
    shared.borrow_mut().size = (96, 72);
    let want = (px(&view, 96), px(&view, 72));
    until(&mut view, "the window to be 96 x 72", |v| v.size() == (96, 72) && host.geometry(window) == Some(want));
    until(&mut view, "the frame at 96 x 72", |_| host.pixel(window, want.0 - 1, want.1 - 1) == Some(right));
    assert_eq!(taken(&shared), vec![], "the editor is not told a size it chose");

    // Positions stay in points after all that.
    let s = view.scale();
    host.button(window, true, 1, 90.0 * s, 70.0 * s, KeyButMask::default(), 1);
    host.sync();
    view.pump();
    assert_eq!(taken(&shared), vec![Call::Down(90.0, 70.0, 1)]);
}

#[test]
fn fixed_editor_keeps_its_size() {
    let Some((host, mut view, shared)) = attached("fixed_editor_keeps_its_size", (64, 48), SizeRange::fixed(64, 48)) else { return };
    let window = view.raw();
    taken(&shared);
    assert_eq!(view.resize(500, 500), (64, 48));
    assert_eq!(view.size(), (64, 48));
    let ticks = shared.borrow().ticks;
    until(&mut view, "a tick", |_| shared.borrow().ticks > ticks);
    assert_eq!(host.geometry(window), Some((px(&view, 64), px(&view, 48))));
    assert!(!taken(&shared).iter().any(|c| matches!(c, Call::Resize(..))));
}

#[test]
fn resize_inside_an_editor_call_waits_for_it() {
    let range = SizeRange::between((40, 30), (100, 80));
    let Some((host, mut view, shared)) = attached("resize_inside_an_editor_call_waits_for_it", (64, 48), range) else { return };
    let window = view.raw();
    taken(&shared);
    // The editor asks the host for a size in mouse_down and the host answers
    // at once, through the bridge's pointer to the view, as a CLAP host does.
    let pointer: *mut EditorView = &mut view;
    let inside = shared.clone();
    let answer = Rc::new(RefCell::new(None));
    let told = answer.clone();
    shared.borrow_mut().on_down = Some(Box::new(move || {
        let size = unsafe { (*pointer).resize(90, 70) };
        *told.borrow_mut() = Some((size, inside.borrow().size));
    }));
    host.button(window, true, 1, 4.0, 4.0, KeyButMask::default(), 1);
    host.sync();
    view.pump();
    // Inside the call the editor still had its old size; the resize came after.
    assert_eq!(*answer.borrow(), Some(((90, 70), (64, 48))));
    let calls = taken(&shared);
    assert!(matches!(calls[..], [Call::Down(_, _, 1), Call::Resize(90, 70)]), "{calls:?}");
    assert_eq!(view.size(), (90, 70));
    let want = (px(&view, 90), px(&view, 70));
    until(&mut view, "the window to be 90 x 70", |_| host.geometry(window) == Some(want));
}

#[test]
fn parent_destroyed_first_is_survived() {
    let Some((host, mut view, shared)) = attached("parent_destroyed_first_is_survived", (64, 48), SizeRange::between((40, 30), (100, 80))) else { return };
    let window = view.raw();
    host.conn.destroy_window(host.parent).unwrap();
    host.sync();
    assert_eq!(host.geometry(window), None, "the view's window went with its parent");
    // Everything a bridge may still call, with the window gone.
    let ticks = shared.borrow().ticks;
    until(&mut view, "ticks after the parent is gone", |_| shared.borrow().ticks >= ticks + 2);
    assert_eq!(view.resize(80, 60), (80, 60));
    view.set_hidden(true);
    view.set_hidden(false);
    view.pump();
    assert!(!view.attach(host.parent), "a dead parent is refused");
    assert!(view.editor().is_some());
    drop(view);
    host.sync();
}

#[test]
fn refused_parents_and_a_lost_connection_are_survived() {
    let Some(host) = Host::new("refused_parents_and_a_lost_connection_are_survived") else { return };
    let (ed, shared) = editor((64, 48), SizeRange::between((40, 30), (100, 80)));
    let Some(mut view) = EditorView::new(ed, None) else { return };
    assert!(!view.attach(0));
    assert!(!view.attach(0x7f00_0001), "a window that does not exist is refused");
    // Unattached: pump ticks, resize works, nothing is presented.
    until(&mut view, "a tick", |_| shared.borrow().ticks > 0);
    assert_eq!(shared.borrow().renders, 0);
    assert_eq!(view.resize(80, 60), (80, 60));
    // Until it is attached the view has an id and no window: nothing a window manager sees.
    let window = view.raw();
    assert_ne!(window, 0);
    assert_eq!(host.geometry(window), None);
    assert!(view.attach(host.parent));
    assert_eq!(host.geometry(window), Some((px(&view, 80), px(&view, 60))), "the window is made at the size the view has by then");
    until(&mut view, "the first frame", |_| host.pixel(window, 1, 1).is_some() && shared.borrow().renders > 0);
    // A second parent: the window moves there.
    let second = host.conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().override_redirect(1);
    host.conn.create_window(0, second, host.parent, 100, 100, 100, 100, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap().check().unwrap();
    host.conn.map_window(second).unwrap();
    assert!(view.attach(second));
    assert_eq!(host.conn.query_tree(window).unwrap().reply().unwrap().parent, second);

    // The server closes the view's connection under it.
    host.conn.kill_client(window).unwrap();
    host.sync();
    let ticks = shared.borrow().ticks;
    until(&mut view, "ticks after the connection is lost", |_| shared.borrow().ticks >= ticks + 2);
    assert_eq!(view.resize(100, 70), (100, 70));
    view.set_hidden(true);
    view.set_hidden(false);
    assert!(!view.attach(host.parent));
    view.pump();
    drop(view);
}

#[test]
fn hidden_view_is_unmapped_and_shown_again() {
    let Some((host, mut view, shared)) = attached("hidden_view_is_unmapped_and_shown_again", (64, 48), SizeRange::fixed(64, 48)) else { return };
    let window = view.raw();
    let state = |host: &Host| host.conn.get_window_attributes(window).unwrap().reply().unwrap().map_state;
    assert_eq!(state(&host), xproto::MapState::VIEWABLE);
    view.set_hidden(true);
    host.sync();
    assert_eq!(state(&host), xproto::MapState::UNMAPPED);
    // New colours while hidden are on screen when the view is shown again.
    shared.borrow_mut().left = 0x0033_cc66;
    view.set_hidden(false);
    host.sync();
    assert_eq!(state(&host), xproto::MapState::VIEWABLE);
    until(&mut view, "the frame after showing", |_| host.pixel(window, 1, 1) == Some(0x33_cc66));
}

#[test]
fn poll_hook_runs_before_each_tick() {
    let Some((_host, mut view, shared)) = attached("poll_hook_runs_before_each_tick", (64, 48), SizeRange::fixed(64, 48)) else { return };
    let polls = Rc::new(RefCell::new(Vec::new()));
    let seen = polls.clone();
    let ticks = shared.clone();
    view.set_poll(Some(Box::new(move |_editor| seen.borrow_mut().push(ticks.borrow().ticks))));
    let start = shared.borrow().ticks;
    let began = Instant::now();
    until(&mut view, "ten ticks", |_| shared.borrow().ticks >= start + 10);
    // Pumped every 2 ms, the editor still ticks at 30 Hz.
    assert!(began.elapsed() >= Duration::from_millis(9 * 28), "ten ticks took {:?}", began.elapsed());
    let polls = polls.borrow();
    assert_eq!(polls.len(), 10);
    assert!(polls.iter().enumerate().all(|(i, &at)| at == start + i), "{polls:?}");
}

/// The window with this `_NET_WM_NAME`, among the root's children and theirs.
fn find_titled(conn: &RustConnection, root: u32, title: &str) -> Option<u32> {
    let name = conn.intern_atom(false, b"_NET_WM_NAME").ok()?.reply().ok()?.atom;
    let children = conn.query_tree(root).ok()?.reply().ok()?.children;
    let mut windows = children.clone();
    for child in children {
        if let Some(tree) = conn.query_tree(child).ok().and_then(|c| c.reply().ok()) {
            windows.extend(tree.children);
        }
    }
    windows.into_iter().find(|&w| {
        let value = conn.get_property(false, w, name, AtomEnum::ANY, 0, 256).ok().and_then(|c| c.reply().ok());
        value.is_some_and(|p| p.value == title.as_bytes())
    })
}

#[test]
fn run_window_opens_titles_and_closes() {
    let test = "run_window_opens_titles_and_closes";
    let Ok((conn, screen)) = RustConnection::connect(None) else {
        println!("{test}: skipped, no X server on DISPLAY");
        return;
    };
    let root = conn.setup().roots[screen].root;
    let title = format!("OpenPlugin x11 test \u{e9} {}", std::process::id());
    let (done, result) = std::sync::mpsc::channel();
    let thread_title = title.clone();
    let thread = std::thread::spawn(move || {
        let (ed, _shared) = editor((64, 48), SizeRange::fixed(64, 48));
        let _ = done.send(run_window(&thread_title, ed, None));
    });

    let start = Instant::now();
    let window = loop {
        if let Some(window) = find_titled(&conn, root, &title) {
            break window;
        }
        assert!(result.try_recv().is_err(), "run_window returned before its window showed");
        assert!(start.elapsed() < DEADLINE, "timed out waiting for the window");
        std::thread::sleep(Duration::from_millis(5));
    };
    let atom = |name: &[u8]| conn.intern_atom(false, name).unwrap().reply().unwrap().atom;
    let property = |property: u32| conn.get_property(false, window, property, AtomEnum::ANY, 0, 64).unwrap().reply().unwrap();
    let (protocols, delete) = (atom(b"WM_PROTOCOLS"), atom(b"WM_DELETE_WINDOW"));

    assert_eq!(property(AtomEnum::WM_NAME.into()).value, format!("OpenPlugin x11 test \u{e9} {}", std::process::id()).chars().map(|c| c as u8).collect::<Vec<u8>>());
    assert!(property(protocols).value32().unwrap().any(|a| a == delete));
    let hints: Vec<u32> = property(AtomEnum::WM_NORMAL_HINTS.into()).value32().unwrap().collect();
    let (w, h) = (hints[5], hints[6]);
    assert_eq!(hints.len(), 18);
    assert_eq!(hints[0] & (16 | 32), 16 | 32, "minimum and maximum size are set");
    assert_eq!((hints[7], hints[8]), (w, h), "a fixed editor's window cannot be resized");
    assert!(w >= 64 && h >= 48 && w * 48 == h * 64, "{w} x {h}");

    // The frame is on screen (the window manager may have sized the window).
    let start = Instant::now();
    loop {
        let image = conn.get_image(ImageFormat::Z_PIXMAP, window, 1, 1, 1, 1, !0).ok().and_then(|c| c.reply().ok());
        if image.is_some_and(|i| i.data.len() >= 4 && u32::from_le_bytes([i.data[0], i.data[1], i.data[2], 0]) == 0xff_2010) {
            break;
        }
        assert!(start.elapsed() < DEADLINE, "timed out waiting for the window's frame");
        std::thread::sleep(Duration::from_millis(5));
    }

    // The close button.
    let message = xproto::ClientMessageEvent::new(32, window, protocols, [delete, 0, 0, 0, 0]);
    conn.send_event(false, window, EventMask::NO_EVENT, message).unwrap();
    conn.flush().unwrap();
    assert_eq!(result.recv_timeout(DEADLINE), Ok(0), "run_window returns 0 when its window is closed");
    thread.join().unwrap();
    conn.get_input_focus().unwrap().reply().unwrap();
    assert!(conn.get_geometry(window).unwrap().reply().is_err(), "the window is destroyed");
}
