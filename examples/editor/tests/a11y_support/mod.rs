//! Shared by the accessibility tests of `example-editor` and
//! `example-design-editor`: an editor in a real `EditorView` inside a
//! borderless window (never shown), read the way VoiceOver reads it, through
//! `accessibilityChildren`, `accessibilityRole`, `accessibilityLabel`,
//! `accessibilityValue`, `accessibilityFrame` and the `accessibilityPerform...`
//! calls, with the gestures that reach the editor's host recorded.

#![cfg(target_os = "macos")]
#![allow(unsafe_code, dead_code)]

use openplugin::core::{Editor, EditorHost, Gesture};
use openplugin::dri::cocoa::{EditorView, Id};
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr};
use std::rc::Rc;

type Sel = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CGSize {
    pub w: f64,
    pub h: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    #[cfg(target_arch = "x86_64")]
    fn objc_msgSend_stret();
}
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

fn sel(name: &CStr) -> Sel {
    unsafe { sel_registerName(name.as_ptr()) }
}
pub fn class(name: &CStr) -> Id {
    unsafe { objc_getClass(name.as_ptr()) }
}

macro_rules! send {
    ($ret:ty, $obj:expr_2021, $sel:expr_2021 $(, $t:ty = $a:expr_2021)*) => {{
        let f: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $ret = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f($obj, sel($sel) $(, $a)*)
    }};
}

pub unsafe fn rect_of(obj: Id, name: &CStr) -> CGRect { unsafe {
    #[cfg(target_arch = "x86_64")]
    {
        let f: unsafe extern "C" fn(*mut CGRect, Id, Sel) = std::mem::transmute(objc_msgSend_stret as unsafe extern "C" fn());
        let mut r = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w: 0.0, h: 0.0 } };
        f(&mut r, obj, sel(name));
        r
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let f: unsafe extern "C" fn(Id, Sel) -> CGRect = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(obj, sel(name))
    }
}}

/// Text of an `NSString`, or `<nil>`.
pub unsafe fn text(s: Id) -> String { unsafe {
    if s.is_null() {
        return "<nil>".into();
    }
    let utf8 = send!(*const c_char, s, c"UTF8String");
    CStr::from_ptr(utf8).to_string_lossy().into_owned()
}}

pub unsafe fn ns_string(t: &CStr) -> Id { unsafe { send!(Id, class(c"NSString"), c"stringWithUTF8String:", *const c_char = t.as_ptr()) }}

/// What the tree says about one element.
#[derive(Debug, PartialEq)]
pub struct Seen {
    pub role: String,
    pub label: String,
    /// `accessibilityValue`: text, a number as `#n`, or `<nil>`.
    pub value: String,
    pub description: String,
    pub focused: bool,
    pub enabled: bool,
}

pub unsafe fn seen(el: Id) -> Seen { unsafe {
    let value = send!(Id, el, c"accessibilityValue");
    let value = if value.is_null() {
        "<nil>".to_string()
    } else if send!(i8, value, c"isKindOfClass:", Id = class(c"NSNumber")) != 0 {
        format!("#{}", send!(i32, value, c"intValue"))
    } else {
        text(value)
    };
    Seen {
        role: text(send!(Id, el, c"accessibilityRole")),
        label: text(send!(Id, el, c"accessibilityLabel")),
        value,
        description: text(send!(Id, el, c"accessibilityValueDescription")),
        focused: send!(i8, el, c"isAccessibilityFocused") != 0,
        enabled: send!(i8, el, c"isAccessibilityEnabled") != 0,
    }
}}

pub unsafe fn children(view: Id) -> Vec<Id> { unsafe {
    let array = send!(Id, view, c"accessibilityChildren");
    let n = send!(usize, array, c"count");
    (0..n).map(|i| send!(Id, array, c"objectAtIndex:", usize = i)).collect()
}}

pub unsafe fn frame(el: Id) -> CGRect { unsafe { rect_of(el, c"accessibilityFrame") }}

/// Pump the main run loop so timers fire.
pub unsafe fn run_loop(seconds: f64) { unsafe {
    let date = send!(Id, class(c"NSDate"), c"dateWithTimeIntervalSinceNow:", f64 = seconds);
    let run_loop = send!(Id, class(c"NSRunLoop"), c"currentRunLoop");
    send!((), run_loop, c"runUntilDate:", Id = date);
}}

pub fn gesture_text(g: &Gesture) -> String {
    match g {
        Gesture::Begin(i) => format!("begin {i}"),
        Gesture::Perform(i, v) => format!("perform {i} {v:.3}"),
        Gesture::End(i) => format!("end {i}"),
    }
}

/// Where the borderless test window sits on the screen (bottom left origin).
pub const WINDOW_AT: (f64, f64) = (300.0, 400.0);

pub struct Rig {
    pub view: Option<EditorView>,
    pub window: Id,
    pub log: Rc<RefCell<Vec<Gesture>>>,
    pub pool: Id,
}

impl Rig {
    /// `make` builds the editor around the host this rig records.
    pub unsafe fn new(make: impl FnOnce(EditorHost) -> Option<Box<dyn Editor>>) -> Rig { unsafe {
        let pool = send!(Id, send!(Id, class(c"NSAutoreleasePool"), c"alloc"), c"init");
        send!(Id, class(c"NSApplication"), c"sharedApplication");
        let log: Rc<RefCell<Vec<Gesture>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = log.clone();
        let editor = make(EditorHost::new(move |g| sink.borrow_mut().push(g))).expect("editor");
        let view = EditorView::new(editor, None).expect("view");
        let (w, h) = view.size();
        let content = CGRect { origin: CGPoint { x: WINDOW_AT.0, y: WINDOW_AT.1 }, size: CGSize { w: w as f64, h: h as f64 } };
        // Borderless (style 0), buffered, deferred: a window that is never shown.
        let window = send!(
            Id,
            send!(Id, class(c"NSWindow"), c"alloc"),
            c"initWithContentRect:styleMask:backing:defer:",
            CGRect = content,
            usize = 0,
            usize = 2,
            i8 = 1
        );
        assert!(!window.is_null());
        send!((), window, c"setContentView:", Id = view.raw());
        Rig { view: Some(view), window, log, pool }
    }}

    pub fn raw(&self) -> Id {
        self.view.as_ref().expect("view").raw()
    }

    pub fn editor(&mut self) -> &mut dyn Editor {
        self.view.as_mut().expect("view").editor().expect("editor")
    }

    /// The gestures since the last call, as text.
    pub fn gestures(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.borrow_mut()).iter().map(gesture_text).collect()
    }

    /// Where a rectangle in editor points (origin top left) is on the screen,
    /// worked out from the window's place alone: x from the left of the window,
    /// y up from its bottom.
    pub unsafe fn expect_screen(&self, r: (f32, f32, f32, f32)) -> CGRect { unsafe {
        let view_h = rect_of(self.raw(), c"bounds").size.h;
        CGRect {
            origin: CGPoint { x: WINDOW_AT.0 + f64::from(r.0), y: WINDOW_AT.1 + view_h - f64::from(r.1) - f64::from(r.3) },
            size: CGSize { w: f64::from(r.2), h: f64::from(r.3) },
        }
    }}

    pub unsafe fn increment(&self, el: Id) -> bool { unsafe { send!(i8, el, c"accessibilityPerformIncrement") != 0 }}
    pub unsafe fn decrement(&self, el: Id) -> bool { unsafe { send!(i8, el, c"accessibilityPerformDecrement") != 0 }}
    pub unsafe fn press(&self, el: Id) -> bool { unsafe { send!(i8, el, c"accessibilityPerformPress") != 0 }}
}

pub unsafe fn group_role(view: Id) -> (String, bool) { unsafe {
    (text(send!(Id, view, c"accessibilityRole")), send!(i8, view, c"isAccessibilityElement") != 0)
}}

pub unsafe fn focused_element(view: Id) -> Id { unsafe { send!(Id, view, c"accessibilityFocusedUIElement") }}

pub unsafe fn hit_test(view: Id, x: f64, y: f64) -> Id { unsafe { send!(Id, view, c"accessibilityHitTest:", CGPoint = CGPoint { x, y }) }}

pub unsafe fn set_focused(el: Id) { unsafe { send!((), el, c"setAccessibilityFocused:", i8 = 1) }}

pub unsafe fn retain(obj: Id) { unsafe { send!(Id, obj, c"retain"); }}

pub unsafe fn release(obj: Id) { unsafe { send!((), obj, c"release") }}

pub fn near(a: CGRect, b: CGRect) -> bool {
    let d = |x: f64, y: f64| (x - y).abs() < 0.51;
    d(a.origin.x, b.origin.x) && d(a.origin.y, b.origin.y) && d(a.size.w, b.size.w) && d(a.size.h, b.size.h)
}
