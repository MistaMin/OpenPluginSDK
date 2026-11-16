//! The parameter menu and the host's track, on the main thread (no harness, so
//! `main` is the process's main thread, as AppKit needs). Nothing is shown to
//! the user: the editor sits in a borderless window that is never ordered in,
//! and no menu is ever popped up.
//!
//! - A right click (and a Control-click) on a control asks the host for its
//!   context menu through `EditorHost::context_menu`, with the parameter and the
//!   click's place; when the host shows one the editor shows nothing; a click on
//!   the bare plate asks nothing and goes to the host.
//! - When the host has none (or declines) the editor queues the SDK's own menu.
//!   The view hands it to a hook instead of an `NSMenu`, and the four items are
//!   chosen by calling the actions directly: reset to default, copy value, paste
//!   value (the clipboard text is passed in, so the user's clipboard is never
//!   touched) and type value (which opens the text field). The real menu action,
//!   `openpluginMenuItem:`, is sent for the two items that do not use the clipboard.
//! - A host menu is modal: while it is open the view's timer still fires, and the
//!   host may close the editor. Neither reaches the editor while its own call is
//!   on the stack: the nested tick is refused, and the editor is dropped when the
//!   call ends.
//! - Through the shell's editor ABI (`shell::editor`, what `op_editor_*` call), a
//!   host menu callback and the host's track reach an editor.

#[cfg(not(target_os = "macos"))]
fn main() {
    // Native view tests need AppKit; there is no Linux (X11/Wayland) editor view yet.
    println!("editor menu: skipped: macOS only (0 tests run)");
}

#[cfg(target_os = "macos")]
mod a11y_support;

#[cfg(target_os = "macos")]
fn main() {
    mac::host_menu_is_asked_for_the_parameter();
    mac::sdk_menu_when_the_host_has_none();
    mac::shell_abi_carries_the_menu_and_the_track();
    mac::a_host_callback_cannot_reenter_the_editor();
    mac::closing_the_view_inside_a_host_callback_waits_for_the_call();
    println!("menu (editor): 5 passed");
}

#[cfg(target_os = "macos")]
mod mac {
    use super::a11y_support::*;
    use example_editor::{layout, make_editor, HEIGHT, WIDTH};
    use openplugin::core::editor::{ParamMenu, SizeRange};
    use openplugin::core::{Editor, EditorHost, Frame, TrackInfo};
    use openplugin::dri::cocoa::PARAM_MENU_ITEMS;
    use std::cell::RefCell;
    use std::ffi::{c_void, CString};
    use std::rc::Rc;

    const CONTROL: usize = 1 << 18;

    fn centre(r: (f32, f32, f32, f32)) -> (f32, f32) {
        (r.0 + r.2 * 0.5, r.1 + r.3 * 0.5)
    }

    /// A right mouse down (`rightMouseDown:`) at an editor point.
    unsafe fn right_click(rig: &Rig, x: f32, y: f32) { unsafe {
        let at = CGPoint { x: f64::from(x), y: rig.view_height() - f64::from(y) };
        tell(rig.raw(), c"rightMouseDown:", mouse_event(3, at, 0, rig.window));
    }}

    type Asked = Rc<RefCell<Vec<(u32, i32, i32)>>>;

    pub fn host_menu_is_asked_for_the_parameter() {
        unsafe {
            let asked: Asked = Rc::new(RefCell::new(Vec::new()));
            let log = asked.clone();
            let rig = Rig::new(move |h| {
                make_editor(h.with_context_menu(move |p, x, y| {
                    log.borrow_mut().push((p, x, y));
                    true
                }))
            });
            let shown: Rc<RefCell<Vec<ParamMenu>>> = Rc::new(RefCell::new(Vec::new()));
            let sink = shown.clone();
            rig.view.as_ref().unwrap().set_menu_hook(Some(Box::new(move |m| {
                sink.borrow_mut().push(m.clone());
                true
            })));
            let lay = layout(WIDTH, HEIGHT);
            let (kx, ky) = centre(lay.knob);
            right_click(&rig, kx, ky);
            run_loop(0.05);
            assert_eq!(*asked.borrow(), [(0, kx.round() as i32, ky.round() as i32)]);
            assert!(shown.borrow().is_empty(), "the host showed its menu: the SDK shows none");
            assert!(rig.gestures().is_empty(), "a menu is not an edit");
            // Control-click is a right click on a Mac.
            rig.click(kx, ky, 1, CONTROL);
            assert_eq!(asked.borrow().len(), 2);
            assert_eq!(asked.borrow()[1].0, 0);
            assert!(shown.borrow().is_empty() && rig.gestures().is_empty(), "no drag started");
            // The Warm toggle is parameter 1.
            let (wx, wy) = centre(lay.switch);
            right_click(&rig, wx, wy);
            assert_eq!(asked.borrow().last().map(|a| a.0), Some(1));
            assert!(rig.gestures().is_empty(), "a right click does not flip the toggle");
            // The bare plate asks nothing.
            right_click(&rig, 3.0, 3.0);
            assert_eq!(asked.borrow().len(), 3);
        }
    }

    pub fn sdk_menu_when_the_host_has_none() {
        unsafe {
            assert_eq!(PARAM_MENU_ITEMS, ["Reset to default", "Copy value", "Paste value", "Type value"]);
            // No host menu at all: the rig's host has no context-menu handler.
            let rig = Rig::new(make_editor);
            let shown: Rc<RefCell<Vec<ParamMenu>>> = Rc::new(RefCell::new(Vec::new()));
            let sink = shown.clone();
            rig.view.as_ref().unwrap().set_menu_hook(Some(Box::new(move |m| {
                sink.borrow_mut().push(m.clone());
                true
            })));
            let lay = layout(WIDTH, HEIGHT);
            let (kx, ky) = centre(lay.knob);
            // Put the gain at -6 dB first, so reset has something to do.
            rig.click(kx, ky, 2, 0);
            run_loop(0.05);
            rig.set_field_text(c"-6 dB");
            assert!(rig.field_command(c"insertNewline:"));
            rig.gestures();

            right_click(&rig, kx, ky);
            run_loop(0.05);
            assert_eq!(shown.borrow().len(), 1);
            let menu = shown.borrow()[0].clone();
            assert_eq!((menu.param, menu.label.as_str()), (0, "Gain"));
            assert_eq!((menu.x.round() as i32, menu.y.round() as i32), (kx.round() as i32, ky.round() as i32));
            assert!(rig.gestures().is_empty());
            let view = rig.view.as_ref().unwrap();

            // Copy: the formatted value, nothing sent.
            assert_eq!(view.choose_menu_item(1, Some("unused")).as_deref(), Some("-6.0 dB"));
            assert!(rig.gestures().is_empty());
            // Paste: the text goes through ParamInfo::parse as one whole gesture.
            assert_eq!(view.choose_menu_item(2, Some("3 dB")), None);
            assert_eq!(rig.gestures(), ["begin 0", "perform 0 3.000", "end 0"]);
            assert_eq!(view.choose_menu_item(2, Some("99")), None);
            assert_eq!(rig.gestures(), ["begin 0", "perform 0 24.000", "end 0"]);
            // Text that is not a value changes nothing.
            assert_eq!(view.choose_menu_item(2, Some("banana")), None);
            assert!(rig.gestures().is_empty());
            // Reset: the default, through the menu's real action (the item's tag is its index).
            let item = new_object(c"NSMenuItem");
            tell_int(item, c"setTag:", 0);
            tell(rig.raw(), c"openpluginMenuItem:", item);
            assert_eq!(rig.gestures(), ["begin 0", "perform 0 0.000", "end 0"]);
            // Type: opens the text field over the control, through the real action too.
            assert!(!rig.has_field());
            tell_int(item, c"setTag:", 3);
            tell(rig.raw(), c"openpluginMenuItem:", item);
            run_loop(0.05);
            assert!(rig.has_field());
            assert_eq!(rig.field_text(), "0.0 dB");
            assert!(rig.field_command(c"cancelOperation:"));
            // An item that does not exist does nothing.
            assert_eq!(view.choose_menu_item(9, None), None);
            tell0(item, c"release");
        }
    }

    /// An editor of one size that remembers the track it was told.
    struct Spy {
        heard: Rc<RefCell<Vec<TrackInfo>>>,
        pixels: Vec<u32>,
        host: EditorHost,
    }

    impl Editor for Spy {
        fn size(&self) -> (u32, u32) {
            (40, 30)
        }
        fn size_range(&self) -> SizeRange {
            SizeRange::fixed(40, 30)
        }
        fn param_changed(&mut self, _: usize, _: f32) {}
        fn render(&mut self, _scale: f32) -> Frame<'_> {
            Frame { pixels: &self.pixels, width: 40, height: 30 }
        }
        fn right_mouse_down(&mut self, x: f32, y: f32) -> bool {
            // Anywhere is "on parameter 7": ask the host for its menu there.
            self.host.context_menu(7, x, y)
        }
        fn track_info(&mut self, info: &TrackInfo) {
            self.heard.borrow_mut().push(info.clone());
        }
    }

    unsafe extern "C" fn menu_callback(context: *mut c_void, index: u32, x: i32, y: i32) -> u32 {
        let log = unsafe { &*(context as *const RefCell<Vec<(u32, i32, i32)>>) };
        log.borrow_mut().push((index, x, y));
        1
    }

    pub fn shell_abi_carries_the_menu_and_the_track() {
        use openplugin::targets::shell::editor;
        unsafe {
            let pool = new_object(c"NSAutoreleasePool");
            thread_local! {
                static HEARD: Rc<RefCell<Vec<TrackInfo>>> = Rc::new(RefCell::new(Vec::new()));
            }
            fn factory(host: EditorHost) -> Option<Box<dyn Editor>> {
                Some(Box::new(Spy { heard: HEARD.with(Clone::clone), pixels: vec![0; 40 * 30], host }))
            }
            let handle = editor::create(factory, std::ptr::null());
            assert!(!handle.is_null());
            let view = editor::raw_view(handle);
            assert!(!view.is_null());

            // The host's menu callback (op_editor_set_context_menu) is asked for the point.
            let asked: RefCell<Vec<(u32, i32, i32)>> = RefCell::new(Vec::new());
            editor::set_context_menu(handle, &asked as *const _ as *mut c_void, Some(menu_callback));
            let event = mouse_event(3, CGPoint { x: 12.0, y: 30.0 - 9.0 }, 0, std::ptr::null_mut());
            tell(view, c"rightMouseDown:", event);
            assert_eq!(*asked.borrow(), [(7, 12, 9)]);
            // Removed: the editor's ask finds no host menu (the Spy shows no SDK menu).
            editor::set_context_menu(handle, std::ptr::null_mut(), None);
            tell(view, c"rightMouseDown:", event);
            assert_eq!(asked.borrow().len(), 1);

            // The track: name, colour (0xRRGGBBAA) and kind, then no name and no colour.
            let name = CString::new("Gtr \u{266a}").unwrap();
            editor::set_track_info(handle, name.as_ptr(), 0x11_22_33_ff, 0x100 | 4);
            editor::set_track_info(handle, std::ptr::null(), 0xdead_beef, 1);
            let heard = HEARD.with(Clone::clone);
            assert_eq!(heard.borrow().len(), 2);
            assert_eq!(heard.borrow()[0], TrackInfo { name: Some("Gtr \u{266a}".into()), colour: Some(0x1122_33ff), kind: TrackInfo::IS_MASTER });
            assert_eq!(heard.borrow()[1], TrackInfo { name: None, colour: None, kind: TrackInfo::IS_RETURN });
            // A null editor is ignored.
            editor::set_track_info(std::ptr::null_mut(), name.as_ptr(), 0, 0);
            editor::set_context_menu(std::ptr::null_mut(), std::ptr::null_mut(), None);
            editor::destroy(handle);
            tell0(pool, c"release");
        }
    }

    /// What a [`Modal`] editor saw.
    #[derive(Default)]
    struct Seen {
        /// Inside `right_mouse_down`.
        inside: bool,
        /// Calls that arrived while inside.
        nested: u32,
        ticks: u32,
        dropped: bool,
        /// The editor was dropped before its own call returned.
        dropped_inside: bool,
    }

    type Callback = Rc<RefCell<Option<Box<dyn FnMut()>>>>;

    /// An editor whose right click runs `callback` the way a host's modal
    /// context menu runs its own loop inside the editor's call.
    struct Modal {
        seen: Rc<RefCell<Seen>>,
        callback: Callback,
        pixels: Vec<u32>,
    }

    impl Drop for Modal {
        fn drop(&mut self) {
            self.seen.borrow_mut().dropped = true;
        }
    }

    impl Editor for Modal {
        fn size(&self) -> (u32, u32) {
            (40, 30)
        }
        fn param_changed(&mut self, _: usize, _: f32) {}
        fn tick(&mut self, _dt: f32) {
            let mut seen = self.seen.borrow_mut();
            seen.ticks += 1;
            seen.nested += u32::from(seen.inside);
        }
        fn render(&mut self, _scale: f32) -> Frame<'_> {
            let mut seen = self.seen.borrow_mut();
            seen.nested += u32::from(seen.inside);
            Frame { pixels: &self.pixels, width: 40, height: 30 }
        }
        fn right_mouse_down(&mut self, _x: f32, _y: f32) -> bool {
            self.seen.borrow_mut().inside = true;
            let call = self.callback.borrow_mut().take();
            if let Some(mut call) = call {
                call();
            }
            let mut seen = self.seen.borrow_mut();
            seen.inside = false;
            seen.dropped_inside = seen.dropped;
            true
        }
    }

    unsafe fn modal_rig() -> (Rig, Rc<RefCell<Seen>>, Callback) { unsafe {
        let seen: Rc<RefCell<Seen>> = Rc::default();
        let callback: Callback = Rc::default();
        let (s, c) = (seen.clone(), callback.clone());
        let rig = Rig::new(move |_| Some(Box::new(Modal { seen: s, callback: c, pixels: vec![0; 40 * 30] })));
        (rig, seen, callback)
    }}

    pub fn a_host_callback_cannot_reenter_the_editor() {
        unsafe {
            let (rig, seen, callback) = modal_rig();
            let view = rig.raw();
            // The timer is in the common modes, so it fires inside a menu's loop.
            *callback.borrow_mut() = Some(Box::new(move || tell(view, c"openpluginTick:", std::ptr::null_mut())));
            right_click(&rig, 5.0, 5.0);
            assert_eq!(seen.borrow().nested, 0, "no editor call while right_mouse_down is on the stack");
            assert_eq!(seen.borrow().ticks, 0, "the nested tick was refused");
            // The view works as before once the call has ended.
            tell(view, c"openpluginTick:", std::ptr::null_mut());
            assert_eq!(seen.borrow().ticks, 1);
        }
    }

    pub fn closing_the_view_inside_a_host_callback_waits_for_the_call() {
        unsafe {
            let (mut rig, seen, callback) = modal_rig();
            let raw = rig.raw();
            let at = CGPoint { x: 5.0, y: rig.view_height() - 5.0 };
            let event = mouse_event(3, at, 0, rig.window);
            // The host closes the editor from inside its menu (the view is dropped).
            let mut owned = rig.view.take();
            *callback.borrow_mut() = Some(Box::new(move || drop(owned.take())));
            tell(raw, c"rightMouseDown:", event);
            assert!(!seen.borrow().dropped_inside, "the editor outlives its own call");
            assert!(seen.borrow().dropped, "and is dropped when the call has ended");
            assert_eq!(seen.borrow().nested, 0);
        }
    }
}
