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

// ---- Typing, tooltips and files -------------------------------------------

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    fn class_addMethod(cls: Id, sel: Sel, imp: *const c_void, types: *const c_char) -> i8;
    fn objc_registerClassPair(cls: Id);
}

impl Rig {
    /// The view's size in points (the window is as big).
    pub unsafe fn view_height(&self) -> f64 { unsafe { rect_of(self.raw(), c"bounds").size.h }}

    /// A mouse down and up at editor point (x, y), as events through `mouseDown:` and `mouseUp:`.
    pub unsafe fn click(&self, x: f32, y: f32, clicks: isize, flags: usize) { unsafe {
        let at = CGPoint { x: f64::from(x), y: self.view_height() - f64::from(y) };
        let number = send!(isize, self.window, c"windowNumber");
        for (kind, name) in [(1usize, c"mouseDown:"), (2usize, c"mouseUp:")] {
            let event = send!(
                Id,
                class(c"NSEvent"),
                c"mouseEventWithType:location:modifierFlags:timestamp:windowNumber:context:eventNumber:clickCount:pressure:",
                usize = kind,
                CGPoint = at,
                usize = flags,
                f64 = 0.0,
                isize = number,
                Id = std::ptr::null_mut(),
                isize = 0,
                isize = clicks,
                f32 = 1.0
            );
            assert!(!event.is_null());
            send!((), self.raw(), name, Id = event);
        }
    }}

    /// A key down and up through `keyDown:` and `keyUp:`.
    pub unsafe fn key(&self, code: u16, chars: &CStr, flags: usize) { unsafe {
        let number = send!(isize, self.window, c"windowNumber");
        for (kind, name) in [(10usize, c"keyDown:"), (11usize, c"keyUp:")] {
            let event = send!(
                Id,
                class(c"NSEvent"),
                c"keyEventWithType:location:modifierFlags:timestamp:windowNumber:context:characters:charactersIgnoringModifiers:isARepeat:keyCode:",
                usize = kind,
                CGPoint = CGPoint { x: 0.0, y: 0.0 },
                usize = flags,
                f64 = 0.0,
                isize = number,
                Id = std::ptr::null_mut(),
                Id = ns_string(chars),
                Id = ns_string(chars),
                i8 = 0,
                u16 = code
            );
            assert!(!event.is_null());
            send!((), self.raw(), name, Id = event);
        }
    }}

    /// A key down through `-[NSApplication sendEvent:]`, where AppKit runs local
    /// event monitors, as it does for a key typed in a host.
    pub unsafe fn app_key(&self, code: u16, chars: &CStr, flags: usize) { unsafe {
        let number = send!(isize, self.window, c"windowNumber");
        let event = send!(
            Id,
            class(c"NSEvent"),
            c"keyEventWithType:location:modifierFlags:timestamp:windowNumber:context:characters:charactersIgnoringModifiers:isARepeat:keyCode:",
            usize = 10usize,
            CGPoint = CGPoint { x: 0.0, y: 0.0 },
            usize = flags,
            f64 = 0.0,
            isize = number,
            Id = std::ptr::null_mut(),
            Id = ns_string(chars),
            Id = ns_string(chars),
            i8 = 0,
            u16 = code
        );
        assert!(!event.is_null());
        let app = send!(Id, class(c"NSApplication"), c"sharedApplication");
        send!((), app, c"sendEvent:", Id = event);
    }}

    /// Post `NSWindowDidResignKeyNotification` for the rig's window.
    pub unsafe fn resign_key(&self) { unsafe {
        let center = send!(Id, class(c"NSNotificationCenter"), c"defaultCenter");
        send!((), center, c"postNotificationName:object:", Id = ns_string(c"NSWindowDidResignKeyNotification"), Id = self.window);
    }}

    /// Take the view out of its window (the window gets an empty content view).
    pub unsafe fn remove_view_from_window(&self) { unsafe {
        let empty = send!(Id, send!(Id, class(c"NSView"), c"alloc"), c"init");
        send!((), self.window, c"setContentView:", Id = empty);
    }}

    /// The text field's string, or `<none>`.
    pub unsafe fn field_text(&self) -> String { unsafe {
        match self.view.as_ref().and_then(|v| v.text_field()) {
            Some(f) => text(send!(Id, f, c"stringValue")),
            None => "<none>".into(),
        }
    }}

    pub unsafe fn set_field_text(&self, t: &CStr) { unsafe {
        let f = self.view.as_ref().and_then(|v| v.text_field()).expect("a text field");
        send!((), f, c"setStringValue:", Id = ns_string(t));
    }}

    pub fn has_field(&self) -> bool {
        self.view.as_ref().is_some_and(|v| v.text_field().is_some())
    }

    /// The field editor's command (`insertNewline:`, `cancelOperation:`) as the
    /// text field's delegate (the view) hears it. Returns whether it was taken.
    pub unsafe fn field_command(&self, name: &CStr) -> bool { unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id, Id, Sel) -> i8 = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(self.raw(), sel(c"control:textView:doCommandBySelector:"), std::ptr::null_mut(), std::ptr::null_mut(), sel(name)) != 0
    }}

    /// The field lost focus.
    pub unsafe fn field_end_editing(&self) { unsafe {
        send!((), self.raw(), c"controlTextDidEndEditing:", Id = std::ptr::null_mut());
    }}
}

thread_local! {
    static DRAG: std::cell::Cell<(Id, CGPoint)> = const { std::cell::Cell::new((std::ptr::null_mut(), CGPoint { x: 0.0, y: 0.0 })) };
}

extern "C" fn drag_pasteboard(_this: Id, _cmd: Sel) -> Id {
    DRAG.with(|d| d.get().0)
}

extern "C" fn drag_location(_this: Id, _cmd: Sel) -> CGPoint {
    DRAG.with(|d| d.get().1)
}

/// A stand-in for the `NSDraggingInfo` AppKit passes: a pasteboard holding file
/// URLs, and a place in window coordinates.
pub unsafe fn dragging_info(paths: &[&CStr], window_point: CGPoint) -> Id { unsafe {
    let name = c"OpenPluginTestDraggingInfo";
    let mut cls = objc_getClass(name.as_ptr());
    if cls.is_null() {
        cls = objc_allocateClassPair(class(c"NSObject"), name.as_ptr(), 0);
        class_addMethod(cls, sel(c"draggingPasteboard"), drag_pasteboard as *const c_void, c"@@:".as_ptr());
        class_addMethod(cls, sel(c"draggingLocation"), drag_location as *const c_void, c"{CGPoint=dd}@:".as_ptr());
        objc_registerClassPair(cls);
    }
    let pasteboard = send!(Id, class(c"NSPasteboard"), c"pasteboardWithUniqueName");
    send!(isize, pasteboard, c"clearContents");
    let urls: Vec<Id> = paths.iter().map(|p| send!(Id, class(c"NSURL"), c"fileURLWithPath:", Id = ns_string(p))).collect();
    let array = send!(Id, class(c"NSArray"), c"arrayWithObjects:count:", *const Id = urls.as_ptr(), usize = urls.len());
    assert!(send!(i8, pasteboard, c"writeObjects:", Id = array) != 0);
    DRAG.with(|d| d.set((pasteboard, window_point)));
    send!(Id, send!(Id, cls, c"alloc"), c"init")
}}

/// Send a drag message (`draggingEntered:` and the like) that returns an operation.
pub unsafe fn drag_op(view: Id, name: &CStr, info: Id) -> usize { unsafe { send!(usize, view, name, Id = info) }}
pub unsafe fn drag_bool(view: Id, name: &CStr, info: Id) -> bool { unsafe { send!(i8, view, name, Id = info) != 0 }}

/// Read a property of an object as text: `message` of a panel.
pub unsafe fn string_of(obj: Id, name: &CStr) -> String { unsafe { text(send!(Id, obj, name)) }}
pub unsafe fn count_of(array: Id) -> usize { unsafe { if array.is_null() { 0 } else { send!(usize, array, c"count") } }}
pub unsafe fn id_of(obj: Id, name: &CStr) -> Id { unsafe { send!(Id, obj, name) }}
pub unsafe fn bool_of(obj: Id, name: &CStr) -> bool { unsafe { send!(i8, obj, name) != 0 }}
pub unsafe fn array_item(array: Id, i: usize) -> Id { unsafe { send!(Id, array, c"objectAtIndex:", usize = i) }}

// ---- Menu and event helpers (the menu test) --------------------------------

/// A mouse event of `kind` (NSEventType: 3 is a right mouse down) at a window point.
pub unsafe fn mouse_event(kind: usize, at: CGPoint, flags: usize, window: Id) -> Id { unsafe {
    let number = if window.is_null() { 0 } else { send!(isize, window, c"windowNumber") };
    let event = send!(
        Id,
        class(c"NSEvent"),
        c"mouseEventWithType:location:modifierFlags:timestamp:windowNumber:context:eventNumber:clickCount:pressure:",
        usize = kind,
        CGPoint = at,
        usize = flags,
        f64 = 0.0,
        isize = number,
        Id = std::ptr::null_mut(),
        isize = 0,
        isize = 1,
        f32 = 1.0
    );
    assert!(!event.is_null());
    event
}}

/// `[obj message:arg]` with an object argument and no result.
pub unsafe fn tell(obj: Id, message: &CStr, arg: Id) { unsafe { send!((), obj, message, Id = arg) }}
/// `[obj message]` with no result.
pub unsafe fn tell0(obj: Id, message: &CStr) { unsafe { send!((), obj, message) }}
/// `[obj message:n]` with an integer argument and no result.
pub unsafe fn tell_int(obj: Id, message: &CStr, n: isize) { unsafe { send!((), obj, message, isize = n) }}
/// `[[Class alloc] init]`.
pub unsafe fn new_object(class_name: &CStr) -> Id { unsafe { send!(Id, send!(Id, class(class_name), c"alloc"), c"init") }}
