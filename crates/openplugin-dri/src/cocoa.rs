//! The macOS view that hosts an [`Editor`]: an `NSView` subclass made through
//! the Objective-C runtime, a 30 Hz timer, and a CoreGraphics blit of the
//! frame the editor renders. Pure Rust; it links AppKit, Foundation and
//! CoreGraphics and nothing else.
//!
//! [`EditorView`] goes inside a host's parent view (CLAP `clap.gui`, LV2
//! `ui:CocoaUI`, the VST3, Audio Unit and AAX shells). [`run_window`] puts the
//! same view in a window of its own and runs the application (standalone).
//!
//! Objective-C class names are global to the host process, and a host may
//! load several plugins built on this SDK. The class name therefore carries
//! the address of a static in this binary, so every loaded binary registers
//! its own class and never runs another binary's methods.
//!
//! Size: the view follows the editor's size. [`EditorView::resize`] (CLAP
//! `set_size`, LV2 `ui:resize`) and a host that sets the view's frame
//! (`setFrameSize:`) both go to [`Editor::resize`] with a size inside the
//! editor's [`SizeRange`]. Mouse positions are always in editor points, even
//! while the frame and the editor differ for a moment. A resize that arrives
//! while the editor is inside one of its own calls (a click that asked the
//! host for a size, and the host answered at once) waits until that call ends.
//!
//! Main thread only.

#![allow(unsafe_code, clippy::missing_safety_doc)]

use openplugin_core::editor::SizeRange;
use openplugin_core::editor::{Key, Modifiers};
use openplugin_core::Editor;
use std::ffi::{c_char, c_void, CString};
use std::ptr::null_mut;
use std::time::Instant;

pub type Id = *mut c_void;
type Sel = *mut c_void;
type Class = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    w: f64,
    h: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Class;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_msgSendSuper();
    #[cfg(target_arch = "x86_64")]
    fn objc_msgSend_stret();
    fn objc_allocateClassPair(superclass: Class, name: *const c_char, extra: usize) -> Class;
    fn class_addMethod(cls: Class, sel: Sel, imp: *const c_void, types: *const c_char) -> i8;
    fn class_addIvar(cls: Class, name: *const c_char, size: usize, align: u8, types: *const c_char) -> i8;
    fn objc_registerClassPair(cls: Class);
    fn object_setInstanceVariable(obj: Id, name: *const c_char, value: *mut c_void) -> *mut c_void;
    fn object_getInstanceVariable(obj: Id, name: *const c_char, out: *mut *mut c_void) -> *mut c_void;
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    static NSRunLoopCommonModes: Id;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    fn CGColorSpaceRelease(space: *mut c_void);
    fn CGDataProviderCreateWithData(info: *mut c_void, data: *const c_void, size: usize, release: *const c_void) -> *mut c_void;
    fn CGDataProviderRelease(provider: *mut c_void);
    #[allow(clippy::too_many_arguments)]
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: *mut c_void,
        bitmap_info: u32,
        provider: *mut c_void,
        decode: *const f64,
        interpolate: u8,
        intent: i32,
    ) -> *mut c_void;
    fn CGImageRelease(image: *mut c_void);
    fn CGContextDrawImage(ctx: *mut c_void, rect: CGRect, image: *mut c_void);
    fn CGContextSaveGState(ctx: *mut c_void);
    fn CGContextRestoreGState(ctx: *mut c_void);
    fn CGContextTranslateCTM(ctx: *mut c_void, tx: f64, ty: f64);
    fn CGContextScaleCTM(ctx: *mut c_void, sx: f64, sy: f64);
    fn CGContextSetInterpolationQuality(ctx: *mut c_void, quality: i32);
}

fn sel(name: &[u8]) -> Sel {
    unsafe { sel_registerName(name.as_ptr() as *const c_char) }
}

fn class(name: &[u8]) -> Class {
    unsafe { objc_getClass(name.as_ptr() as *const c_char) }
}

/// `send!(Ret, receiver, b"selector:\0", ArgType = value, ...)`
macro_rules! send {
    ($ret:ty, $obj:expr_2021, $sel:expr_2021 $(, $t:ty = $a:expr_2021)*) => {{
        let f: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $ret =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f($obj, sel($sel) $(, $a)*)
    }};
}

unsafe fn send_rect(obj: Id, name: &[u8]) -> CGRect { unsafe {
    #[cfg(target_arch = "x86_64")]
    {
        let f: unsafe extern "C" fn(*mut CGRect, Id, Sel) = std::mem::transmute(objc_msgSend_stret as unsafe extern "C" fn());
        let mut rect = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w: 0.0, h: 0.0 } };
        f(&mut rect, obj, sel(name));
        rect
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let f: unsafe extern "C" fn(Id, Sel) -> CGRect = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(obj, sel(name))
    }
}}

unsafe fn ns_string(text: &str) -> Id { unsafe {
    let c = CString::new(text.replace('\0', "")).unwrap_or_default();
    send!(Id, class(b"NSString\0") as Id, b"stringWithUTF8String:\0", *const c_char = c.as_ptr())
}}

const IVAR: &[u8] = b"openpluginState\0";

/// Called before every tick with the editor, so a format bridge can pass it
/// the values that changed in the host since the last tick.
pub type Poll = Box<dyn FnMut(&mut dyn Editor)>;

struct State {
    editor: Box<dyn Editor>,
    poll: Option<Poll>,
    /// The editor's size, which mouse positions are mapped to.
    size: (u32, u32),
    last: Instant,
    /// The modifier keys the editor was last told about.
    modifiers: Modifiers,
    /// The editor is inside one of its calls.
    busy: bool,
    /// A resize that came in while `busy`: width, height, and whether the
    /// view's frame must follow (it must not when the host set the frame).
    pending: Option<(u32, u32, bool)>,
    /// The view is changing its own frame.
    in_frame: bool,
}

#[repr(C)]
struct ObjcSuper {
    receiver: Id,
    super_class: Class,
}

unsafe fn state<'a>(this: Id) -> Option<&'a mut State> { unsafe {
    let mut p: *mut c_void = null_mut();
    object_getInstanceVariable(this, IVAR.as_ptr() as *const c_char, &mut p);
    (p as *mut State).as_mut()
}}

/// Run an editor call. A resize that arrived meanwhile is applied after it.
unsafe fn guarded(this: Id, call: impl FnOnce(&mut State)) { unsafe {
    let Some(st) = state(this) else { return };
    st.busy = true;
    call(st);
    st.busy = false;
    if let Some((w, h, frame)) = st.pending.take() {
        apply(this, st, w, h, frame);
    }
}}

/// Give the editor a size inside its range, and follow the size it ends up with.
unsafe fn apply(this: Id, st: &mut State, width: u32, height: u32, frame: bool) { unsafe {
    let (w, h) = st.editor.size_range().clamp(width, height);
    st.editor.resize(w, h);
    follow(this, st, frame);
}}

/// Make the view the editor's size if the editor's size changed.
unsafe fn follow(this: Id, st: &mut State, frame: bool) { unsafe {
    let now = st.editor.size();
    if now == st.size || now.0 == 0 || now.1 == 0 {
        return;
    }
    st.size = now;
    if frame {
        st.in_frame = true;
        send!((), this, b"setFrameSize:\0", CGSize = CGSize { w: f64::from(now.0), h: f64::from(now.1) });
        st.in_frame = false;
    }
    redraw(this);
}}

/// A size from outside: now, or after the editor's current call.
unsafe fn resize_to(this: Id, width: u32, height: u32, frame: bool) { unsafe {
    let Some(st) = state(this) else { return };
    if st.busy {
        st.pending = Some((width, height, frame));
    } else {
        apply(this, st, width, height, frame);
    }
}}

extern "C" fn set_frame_size(this: Id, _cmd: Sel, size: CGSize) {
    unsafe {
        let sup = ObjcSuper { receiver: this, super_class: class(b"NSView\0") };
        let f: unsafe extern "C" fn(*const ObjcSuper, Sel, CGSize) = std::mem::transmute(objc_msgSendSuper as unsafe extern "C" fn());
        f(&sup, sel(b"setFrameSize:\0"), size);
        let Some(st) = state(this) else { return };
        if st.in_frame || !size.w.is_finite() || !size.h.is_finite() {
            return;
        }
        let (w, h) = (size.w.round().clamp(1.0, 1e5) as u32, size.h.round().clamp(1.0, 1e5) as u32);
        if (w, h) != st.size {
            resize_to(this, w, h, false);
        }
    }
}

unsafe fn backing_scale(view: Id) -> f32 { unsafe {
    let window = send!(Id, view, b"window\0");
    if !window.is_null() {
        let scale = send!(f64, window, b"backingScaleFactor\0");
        if scale >= 1.0 {
            return scale as f32;
        }
    }
    2.0
}}

extern "C" fn draw_rect(this: Id, _cmd: Sel, _dirty: CGRect) {
    unsafe { guarded(this, |st| draw(this, st)) }
}

unsafe fn draw(this: Id, st: &mut State) {
    unsafe {
        let bounds = send_rect(this, b"bounds\0");
        let scale = backing_scale(this).clamp(1.0, 3.0);
        let frame = st.editor.render(scale);
        if frame.width == 0 || frame.height == 0 || frame.pixels.len() < frame.width * frame.height {
            return;
        }
        let context = send!(Id, class(b"NSGraphicsContext\0") as Id, b"currentContext\0");
        if context.is_null() {
            return;
        }
        let ctx = send!(*mut c_void, context, b"CGContext\0");
        if ctx.is_null() {
            return;
        }
        let space = CGColorSpaceCreateDeviceRGB();
        let bytes = frame.width * frame.height * 4;
        let provider = CGDataProviderCreateWithData(null_mut(), frame.pixels.as_ptr() as *const c_void, bytes, std::ptr::null());
        // kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little: a 0x00RRGGBB word.
        let image = CGImageCreate(frame.width, frame.height, 8, 32, frame.width * 4, space, 6 | (2 << 12), provider, std::ptr::null(), 0, 0);
        if !image.is_null() {
            CGContextSaveGState(ctx);
            CGContextSetInterpolationQuality(ctx, 1);
            // The view is flipped; flip back so the first row is on top.
            CGContextTranslateCTM(ctx, 0.0, bounds.size.h);
            CGContextScaleCTM(ctx, 1.0, -1.0);
            CGContextDrawImage(ctx, CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: bounds.size }, image);
            CGContextRestoreGState(ctx);
            CGImageRelease(image);
        }
        CGDataProviderRelease(provider);
        CGColorSpaceRelease(space);
    }
}

/// The event position in editor points (a host may show the view at another size).
unsafe fn local(this: Id, size: (u32, u32), event: Id) -> (f32, f32) { unsafe {
    let p = send!(CGPoint, event, b"locationInWindow\0");
    let q = send!(CGPoint, this, b"convertPoint:fromView:\0", CGPoint = p, Id = null_mut());
    let b = send_rect(this, b"bounds\0");
    let sx = if b.size.w > 1.0 { size.0 as f64 / b.size.w } else { 1.0 };
    let sy = if b.size.h > 1.0 { size.1 as f64 / b.size.h } else { 1.0 };
    ((q.x * sx) as f32, (q.y * sy) as f32)
}}

/// The modifier keys of an event (`NSEventModifierFlags`).
fn modifiers_from_flags(flags: usize) -> Modifiers {
    Modifiers {
        shift: flags & (1 << 17) != 0,
        ctrl: flags & (1 << 18) != 0,
        alt: flags & (1 << 19) != 0,
        command: flags & (1 << 20) != 0,
    }
}

/// Tell the editor about the event's modifier keys when they differ from the
/// last ones it heard. Returns the current modifiers.
unsafe fn sync_modifiers(st: &mut State, event: Id) -> Modifiers { unsafe {
    let now = modifiers_from_flags(send!(usize, event, b"modifierFlags\0"));
    if now != st.modifiers {
        st.modifiers = now;
        st.editor.modifiers_changed(now);
    }
    now
}}

/// A key from a macOS virtual key code (the `kVK_` values of the keys that do
/// not type a character) or, failing that, the first character the key types
/// without modifiers, lower case.
fn key_from_code(code: u16, typed: Option<char>) -> Option<Key> {
    Some(match code {
        123 => Key::Left,
        124 => Key::Right,
        125 => Key::Down,
        126 => Key::Up,
        116 => Key::PageUp,
        121 => Key::PageDown,
        115 => Key::Home,
        119 => Key::End,
        48 => Key::Tab,
        36 | 76 => Key::Return,
        53 => Key::Escape,
        117 => Key::Delete,
        51 => Key::Backspace,
        49 => Key::Space,
        _ => {
            let c = typed?;
            // Function keys type private-use characters.
            if c.is_control() || ('\u{f700}'..='\u{f8ff}').contains(&c) {
                return None;
            }
            Key::Char(c.to_lowercase().next().unwrap_or(c))
        }
    })
}

/// The event's key, or `None` for a key the contract has no name for.
unsafe fn event_key(event: Id) -> Option<Key> { unsafe {
    let code = send!(u16, event, b"keyCode\0");
    let chars = send!(Id, event, b"charactersIgnoringModifiers\0");
    let typed = if chars.is_null() {
        None
    } else {
        let utf8 = send!(*const c_char, chars, b"UTF8String\0");
        if utf8.is_null() { None } else { std::ffi::CStr::from_ptr(utf8).to_str().ok().and_then(|t| t.chars().next()) }
    };
    key_from_code(code, typed)
}}

/// Hand an event the editor did not use to the next responder, so the host
/// keeps its transport keys and shortcuts.
unsafe fn pass_on(this: Id, name: &[u8], event: Id) { unsafe {
    let next = send!(Id, this, b"nextResponder\0");
    if !next.is_null() {
        send!((), next, name, Id = event);
    }
}}

unsafe fn key_event(this: Id, event: Id, down: bool) { unsafe {
    let mut handled = false;
    guarded(this, |st| {
        let mods = sync_modifiers(st, event);
        if let Some(key) = event_key(event).filter(|_| st.editor.wants_keys()) {
            handled = if down { st.editor.key_down(key, mods) } else { st.editor.key_up(key, mods) };
        }
    });
    if handled {
        redraw(this);
    } else {
        pass_on(this, if down { b"keyDown:\0" } else { b"keyUp:\0" }, event);
    }
}}

extern "C" fn key_down(this: Id, _cmd: Sel, event: Id) {
    unsafe { key_event(this, event, true) }
}

extern "C" fn key_up(this: Id, _cmd: Sel, event: Id) {
    unsafe { key_event(this, event, false) }
}

extern "C" fn flags_changed(this: Id, _cmd: Sel, event: Id) {
    unsafe {
        guarded(this, |st| {
            sync_modifiers(st, event);
        });
        pass_on(this, b"flagsChanged:\0", event);
    }
}

extern "C" fn accepts_first_responder(this: Id, _cmd: Sel) -> i8 {
    unsafe { state(this).is_some_and(|st| st.editor.wants_keys()) as i8 }
}

extern "C" fn become_first_responder(this: Id, _cmd: Sel) -> i8 {
    unsafe {
        guarded(this, |st| st.editor.focus_changed(true));
        redraw(this);
    }
    1
}

extern "C" fn resign_first_responder(this: Id, _cmd: Sel) -> i8 {
    unsafe {
        guarded(this, |st| st.editor.focus_changed(false));
        redraw(this);
    }
    1
}

unsafe fn redraw(this: Id) { unsafe {
    send!((), this, b"setNeedsDisplay:\0", i8 = 1);
}}

extern "C" fn mouse_down(this: Id, _cmd: Sel, event: Id) {
    unsafe {
        // A click gives the editor the keyboard, when it wants keys. Outside the
        // editor call: taking focus calls the editor again.
        if state(this).is_some_and(|st| st.editor.wants_keys()) {
            let window = send!(Id, this, b"window\0");
            if !window.is_null() {
                send!(i8, window, b"makeFirstResponder:\0", Id = this);
            }
        }
        guarded(this, |st| {
            sync_modifiers(st, event);
            let (x, y) = local(this, st.size, event);
            let clicks = send!(isize, event, b"clickCount\0").max(1) as u32;
            st.editor.mouse_down(x, y, clicks);
        });
        redraw(this);
    }
}

extern "C" fn mouse_moved(this: Id, _cmd: Sel, event: Id) {
    unsafe {
        guarded(this, |st| {
            sync_modifiers(st, event);
            let (x, y) = local(this, st.size, event);
            st.editor.mouse_move(x, y);
        });
    }
}

extern "C" fn mouse_up(this: Id, _cmd: Sel, event: Id) {
    unsafe {
        guarded(this, |st| {
            sync_modifiers(st, event);
            let (x, y) = local(this, st.size, event);
            st.editor.mouse_up(x, y);
        });
        redraw(this);
    }
}

extern "C" fn scroll_wheel(this: Id, _cmd: Sel, event: Id) {
    unsafe {
        guarded(this, |st| {
            sync_modifiers(st, event);
            let (x, y) = local(this, st.size, event);
            let dy = send!(f64, event, b"scrollingDeltaY\0");
            let precise = send!(i8, event, b"hasPreciseScrollingDeltas\0") != 0;
            st.editor.wheel(x, y, (if precise { dy * 0.1 } else { dy }) as f32);
        });
    }
}

extern "C" fn tick(this: Id, _cmd: Sel, _timer: Id) {
    unsafe {
        guarded(this, |st| {
            if let Some(poll) = st.poll.as_mut() {
                poll(st.editor.as_mut());
            }
            let now = Instant::now();
            let dt = now.duration_since(st.last).as_secs_f32().min(0.1);
            st.last = now;
            st.editor.tick(dt);
            // An editor may change its own size (a poll hook resized it).
            follow(this, st, true);
        });
        redraw(this);
    }
}

extern "C" fn yes(_this: Id, _cmd: Sel) -> i8 {
    1
}

extern "C" fn yes_event(_this: Id, _cmd: Sel, _event: Id) -> i8 {
    1
}

static MARKER: u8 = 0;

/// `OpenPluginEditorView_<address>`, one per loaded binary.
fn class_name() -> CString {
    CString::new(format!("OpenPluginEditorView_{:x}", &MARKER as *const u8 as usize)).unwrap_or_default()
}

unsafe fn view_class() -> Class { unsafe {
    let name = class_name();
    let existing = objc_getClass(name.as_ptr());
    if !existing.is_null() {
        return existing;
    }
    let cls = objc_allocateClassPair(class(b"NSView\0"), name.as_ptr(), 0);
    if cls.is_null() {
        return null_mut();
    }
    class_addIvar(cls, IVAR.as_ptr() as *const c_char, std::mem::size_of::<*mut c_void>(), 3, c"^v".as_ptr());
    let add = |name: &[u8], imp: *const c_void, types: &[u8]| {
        class_addMethod(cls, sel(name), imp, types.as_ptr() as *const c_char);
    };
    add(b"drawRect:\0", draw_rect as *const c_void, b"v@:{CGRect={CGPoint=dd}{CGSize=dd}}\0");
    add(b"setFrameSize:\0", set_frame_size as *const c_void, b"v@:{CGSize=dd}\0");
    add(b"isFlipped\0", yes as *const c_void, b"c@:\0");
    add(b"isOpaque\0", yes as *const c_void, b"c@:\0");
    add(b"acceptsFirstResponder\0", accepts_first_responder as *const c_void, b"c@:\0");
    add(b"becomeFirstResponder\0", become_first_responder as *const c_void, b"c@:\0");
    add(b"resignFirstResponder\0", resign_first_responder as *const c_void, b"c@:\0");
    add(b"keyDown:\0", key_down as *const c_void, b"v@:@\0");
    add(b"keyUp:\0", key_up as *const c_void, b"v@:@\0");
    add(b"flagsChanged:\0", flags_changed as *const c_void, b"v@:@\0");
    add(b"acceptsFirstMouse:\0", yes_event as *const c_void, b"c@:@\0");
    add(b"mouseDown:\0", mouse_down as *const c_void, b"v@:@\0");
    add(b"mouseDragged:\0", mouse_moved as *const c_void, b"v@:@\0");
    add(b"mouseMoved:\0", mouse_moved as *const c_void, b"v@:@\0");
    add(b"mouseUp:\0", mouse_up as *const c_void, b"v@:@\0");
    add(b"scrollWheel:\0", scroll_wheel as *const c_void, b"v@:@\0");
    add(b"openpluginTick:\0", tick as *const c_void, b"v@:@\0");
    objc_registerClassPair(cls);
    cls
}}

/// An editor in an `NSView`. Dropping it stops the timer, removes the view
/// from its parent, and drops the editor. Main thread only.
pub struct EditorView {
    view: Id,
    timer: Id,
    state: *mut State,
}

impl EditorView {
    /// `poll` runs before every tick (see [`Poll`]). `None` when AppKit is not
    /// available in this process.
    pub fn new(editor: Box<dyn Editor>, poll: Option<Poll>) -> Option<EditorView> {
        unsafe {
            let cls = view_class();
            if cls.is_null() {
                return None;
            }
            let size = editor.size();
            let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w: size.0 as f64, h: size.1 as f64 } };
            let view = send!(Id, send!(Id, cls as Id, b"alloc\0"), b"initWithFrame:\0", CGRect = frame);
            if view.is_null() {
                return None;
            }
            let state = Box::into_raw(Box::new(State { editor, poll, size, last: Instant::now(), modifiers: Modifiers::NONE, busy: false, pending: None, in_frame: false }));
            object_setInstanceVariable(view, IVAR.as_ptr() as *const c_char, state as *mut c_void);

            // Mouse-moved events need a tracking area: moved | active always | in visible rect.
            let area = send!(
                Id,
                send!(Id, class(b"NSTrackingArea\0") as Id, b"alloc\0"),
                b"initWithRect:options:owner:userInfo:\0",
                CGRect = frame,
                usize = 0x02 | 0x80 | 0x200,
                Id = view,
                Id = null_mut()
            );
            if !area.is_null() {
                send!((), view, b"addTrackingArea:\0", Id = area);
                send!((), area, b"release\0");
            }

            let timer = send!(
                Id,
                class(b"NSTimer\0") as Id,
                b"timerWithTimeInterval:target:selector:userInfo:repeats:\0",
                f64 = 1.0 / 30.0,
                Id = view,
                Sel = sel(b"openpluginTick:\0"),
                Id = null_mut(),
                i8 = 1
            );
            if !timer.is_null() {
                send!(Id, timer, b"retain\0");
                let run_loop = send!(Id, class(b"NSRunLoop\0") as Id, b"currentRunLoop\0");
                send!((), run_loop, b"addTimer:forMode:\0", Id = timer, Id = NSRunLoopCommonModes);
            }
            Some(EditorView { view, timer, state })
        }
    }

    /// The `NSView`. The view keeps its own reference until drop.
    pub fn raw(&self) -> Id {
        self.view
    }

    /// The editor's current size in points.
    pub fn size(&self) -> (u32, u32) {
        unsafe { self.state.as_ref().map(|s| s.size).unwrap_or((0, 0)) }
    }

    /// The sizes the editor can take.
    pub fn size_range(&self) -> SizeRange {
        match unsafe { self.state.as_ref() } {
            Some(s) => s.editor.size_range(),
            None => SizeRange::fixed(0, 0),
        }
    }

    /// Move the editor and the view to `width` x `height` points, brought
    /// into the editor's range. Returns the size the editor takes. If the
    /// editor is inside one of its own calls, the change is made when that
    /// call ends. A fixed editor stays as it is.
    pub fn resize(&self, width: u32, height: u32) -> (u32, u32) {
        let (w, h) = self.size_range().clamp(width, height);
        unsafe { resize_to(self.view, w, h, true) };
        (w, h)
    }

    /// Put the view inside a host's parent `NSView`.
    ///
    /// # Safety
    /// `parent` is null or a live `NSView`.
    pub unsafe fn attach(&self, parent: Id) -> bool { unsafe {
        if parent.is_null() {
            return false;
        }
        send!((), parent, b"addSubview:\0", Id = self.view);
        true
    }}

    pub fn set_hidden(&self, hidden: bool) {
        unsafe { send!((), self.view, b"setHidden:\0", i8 = hidden as i8) }
    }

    /// The hosted editor.
    pub fn editor(&mut self) -> Option<&mut (dyn Editor + 'static)> {
        unsafe { self.state.as_mut().map(|s| s.editor.as_mut()) }
    }

    /// Replace the poll hook.
    pub fn set_poll(&mut self, poll: Option<Poll>) {
        if let Some(s) = unsafe { self.state.as_mut() } {
            s.poll = poll;
        }
    }
}

impl Drop for EditorView {
    fn drop(&mut self) {
        unsafe {
            if !self.timer.is_null() {
                send!((), self.timer, b"invalidate\0");
                send!((), self.timer, b"release\0");
            }
            // Detach the state first: the host may keep the view a while longer.
            object_setInstanceVariable(self.view, IVAR.as_ptr() as *const c_char, null_mut());
            send!((), self.view, b"removeFromSuperview\0");
            send!((), self.view, b"release\0");
            if !self.state.is_null() {
                drop(Box::from_raw(self.state));
            }
        }
    }
}

/// Show the editor in its own window and run the application until the
/// window closes or the user quits. Returns the exit code. Main thread.
pub fn run_window(title: &str, editor: Box<dyn Editor>, poll: Option<Poll>) -> i32 {
    unsafe {
        let pool = send!(Id, send!(Id, class(b"NSAutoreleasePool\0") as Id, b"alloc\0"), b"init\0");
        let app = send!(Id, class(b"NSApplication\0") as Id, b"sharedApplication\0");
        send!((), app, b"setActivationPolicy:\0", isize = 0);
        let Some(view) = EditorView::new(editor, poll) else { return 1 };
        let size = view.size();
        let range = view.size_range();
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w: size.0 as f64, h: size.1 as f64 } };
        // Titled | closable | miniaturizable (| resizable when the editor can be); buffered backing.
        let window = send!(
            Id,
            send!(Id, class(b"NSWindow\0") as Id, b"alloc\0"),
            b"initWithContentRect:styleMask:backing:defer:\0",
            CGRect = frame,
            usize = if range.is_fixed() { 7 } else { 15 },
            usize = 2,
            i8 = 0
        );
        if window.is_null() {
            return 1;
        }
        if !range.is_fixed() {
            let pt = |(w, h): (u32, u32)| CGSize { w: f64::from(w), h: f64::from(h) };
            send!((), window, b"setContentMinSize:\0", CGSize = pt(range.min));
            send!((), window, b"setContentMaxSize:\0", CGSize = pt(range.max));
            if let Some(aspect) = range.aspect_ratio() {
                send!((), window, b"setContentAspectRatio:\0", CGSize = pt(aspect));
            }
        }
        send!((), window, b"setTitle:\0", Id = ns_string(title));
        send!((), window, b"setContentView:\0", Id = view.raw());
        send!((), window, b"setAcceptsMouseMovedEvents:\0", i8 = 1);
        send!((), window, b"center\0");
        send!((), window, b"makeKeyAndOrderFront:\0", Id = null_mut());
        send!((), app, b"activateIgnoringOtherApps:\0", i8 = 1);

        // Quit with Cmd-Q or with the window.
        let bar = send!(Id, send!(Id, class(b"NSMenu\0") as Id, b"alloc\0"), b"init\0");
        let bar_item = send!(Id, send!(Id, class(b"NSMenuItem\0") as Id, b"alloc\0"), b"init\0");
        send!((), bar, b"addItem:\0", Id = bar_item);
        let menu = send!(Id, send!(Id, class(b"NSMenu\0") as Id, b"alloc\0"), b"init\0");
        let quit = send!(
            Id,
            send!(Id, class(b"NSMenuItem\0") as Id, b"alloc\0"),
            b"initWithTitle:action:keyEquivalent:\0",
            Id = ns_string("Quit"),
            Sel = sel(b"terminate:\0"),
            Id = ns_string("q")
        );
        send!((), menu, b"addItem:\0", Id = quit);
        send!((), bar_item, b"setSubmenu:\0", Id = menu);
        send!((), app, b"setMainMenu:\0", Id = bar);
        let center = send!(Id, class(b"NSNotificationCenter\0") as Id, b"defaultCenter\0");
        send!(
            (),
            center,
            b"addObserver:selector:name:object:\0",
            Id = app,
            Sel = sel(b"terminate:\0"),
            Id = ns_string("NSWindowWillCloseNotification"),
            Id = window
        );
        send!((), app, b"run\0");
        drop(view);
        send!((), pool, b"drain\0");
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_map_to_modifiers() {
        assert_eq!(modifiers_from_flags(0), Modifiers::NONE);
        let all = modifiers_from_flags((1 << 17) | (1 << 18) | (1 << 19) | (1 << 20));
        assert!(all.shift && all.ctrl && all.alt && all.command);
        // Caps lock (1 << 16) and device bits are not modifiers here.
        assert_eq!(modifiers_from_flags((1 << 16) | 0x100), Modifiers::NONE);
        assert_eq!(modifiers_from_flags(1 << 20), Modifiers { command: true, ..Modifiers::NONE });
    }

    #[test]
    fn key_codes_map_to_keys() {
        assert_eq!(key_from_code(123, None), Some(Key::Left));
        assert_eq!(key_from_code(126, Some('\u{f700}')), Some(Key::Up));
        assert_eq!(key_from_code(36, Some('\r')), Some(Key::Return));
        assert_eq!(key_from_code(76, None), Some(Key::Return));
        assert_eq!(key_from_code(51, Some('\u{7f}')), Some(Key::Backspace));
        assert_eq!(key_from_code(117, None), Some(Key::Delete));
        assert_eq!(key_from_code(49, Some(' ')), Some(Key::Space));
        assert_eq!(key_from_code(0, Some('A')), Some(Key::Char('a')));
        assert_eq!(key_from_code(122, Some('\u{f704}')), None, "F1 types a private-use character");
        assert_eq!(key_from_code(0, None), None);
    }
}
