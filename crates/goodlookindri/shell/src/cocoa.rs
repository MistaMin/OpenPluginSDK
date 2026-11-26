//! Cocoa window through the Objective-C runtime, with no Objective-C or Rust bindings crate.
//! Only calls that exist on macOS 10.6 are used unconditionally; newer ones (the backing scale)
//! are checked with `respondsToSelector:`. Thread-local storage is avoided on purpose, because
//! 10.6 has none: the state lives in a plain static used only on the main thread.

#![allow(non_upper_case_globals, clippy::missing_safety_doc)]

use crate::core::{Core, Event, Key};
use std::ffi::{c_void, CStr};
use std::os::raw::c_char;
use std::ptr::null_mut;

type Id = *mut c_void;
type Sel = *mut c_void;
type Class = *mut c_void;

#[cfg(target_pointer_width = "64")]
type CGFloat = f64;
#[cfg(not(target_pointer_width = "64"))]
type CGFloat = f32;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint { x: CGFloat, y: CGFloat }
#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize { w: CGFloat, h: CGFloat }
#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect { origin: CGPoint, size: CGSize }

#[cfg(target_pointer_width = "64")]
const RECT_ENC: &[u8] = b"{CGRect={CGPoint=dd}{CGSize=dd}}\0";
#[cfg(not(target_pointer_width = "64"))]
const RECT_ENC: &[u8] = b"{CGRect={CGPoint=ff}{CGSize=ff}}\0";

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Class;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    fn objc_msgSend_stret();
    fn objc_allocateClassPair(sup: Class, name: *const c_char, extra: usize) -> Class;
    fn class_addMethod(cls: Class, sel: Sel, imp: *const c_void, types: *const c_char) -> i8;
    fn objc_registerClassPair(cls: Class);
}
#[link(name = "AppKit", kind = "framework")]
extern "C" {}
#[link(name = "Foundation", kind = "framework")]
extern "C" {}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    fn CGColorSpaceRelease(cs: *mut c_void);
    fn CGDataProviderCreateWithData(info: *mut c_void, data: *const c_void, size: usize, release: *const c_void) -> *mut c_void;
    fn CGDataProviderRelease(p: *mut c_void);
    fn CGImageCreate(w: usize, h: usize, bpc: usize, bpp: usize, bpr: usize, cs: *mut c_void, info: u32, provider: *mut c_void, decode: *const CGFloat, interp: u8, intent: i32) -> *mut c_void;
    fn CGImageRelease(i: *mut c_void);
    fn CGContextDrawImage(ctx: *mut c_void, rect: CGRect, img: *mut c_void);
    fn CGContextSaveGState(ctx: *mut c_void);
    fn CGContextRestoreGState(ctx: *mut c_void);
    fn CGContextTranslateCTM(ctx: *mut c_void, tx: CGFloat, ty: CGFloat);
    fn CGContextScaleCTM(ctx: *mut c_void, sx: CGFloat, sy: CGFloat);
}

fn sel(name: &[u8]) -> Sel { unsafe { sel_registerName(name.as_ptr() as *const c_char) } }
fn class(name: &[u8]) -> Class { unsafe { objc_getClass(name.as_ptr() as *const c_char) } }

/// `send!(Ret, receiver, b"selector:\0", ArgType = value, ...)`
macro_rules! send {
    ($ret:ty, $obj:expr, $sel:expr $(, $t:ty = $a:expr)*) => {{
        let f: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $ret =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f($obj, sel($sel) $(, $a)*)
    }};
}

/// Methods returning a `CGRect` use the struct-return entry point on Intel.
unsafe fn send_rect(obj: Id, name: &[u8]) -> CGRect {
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    {
        let f: unsafe extern "C" fn(*mut CGRect, Id, Sel) = std::mem::transmute(objc_msgSend_stret as unsafe extern "C" fn());
        let mut r = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w: 0.0, h: 0.0 } };
        f(&mut r, obj, sel(name));
        r
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        let f: unsafe extern "C" fn(Id, Sel) -> CGRect = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(obj, sel(name))
    }
}

unsafe fn nsstring(s: &str) -> Id {
    let c = std::ffi::CString::new(s).unwrap_or_default();
    send!(Id, class(b"NSString\0") as Id, b"stringWithUTF8String:\0", *const c_char = c.as_ptr())
}

// Main thread only.
static mut CORE: *mut Core = null_mut();

unsafe fn core<'a>() -> Option<&'a mut Core> {
    let p = CORE;
    if p.is_null() { None } else { Some(&mut *p) }
}

unsafe fn backing_scale(view: Id) -> f32 {
    #[cfg(target_pointer_width = "64")]
    {
        let win = send!(Id, view, b"window\0");
        if !win.is_null() && send!(i8, win, b"respondsToSelector:\0", Sel = sel(b"backingScaleFactor\0")) != 0 {
            let s = send!(f64, win, b"backingScaleFactor\0");
            if s >= 1.0 { return s as f32; }
        }
    }
    let _ = view;
    1.0
}

unsafe fn sync_size(view: Id) -> (CGRect, f32) {
    let b = send_rect(view, b"bounds\0");
    let scale = backing_scale(view);
    if let Some(c) = core() { c.resize(b.size.w as f32, b.size.h as f32, scale); }
    (b, scale)
}

extern "C" fn draw_rect(this: Id, _cmd: Sel, _dirty: CGRect) {
    unsafe {
        let (bounds, _) = sync_size(this);
        let c = match core() { Some(c) => c, None => return };
        let fb = c.render();
        let ctx_obj = send!(Id, class(b"NSGraphicsContext\0") as Id, b"currentContext\0");
        if ctx_obj.is_null() { return; }
        let ctx = send!(*mut c_void, ctx_obj, b"graphicsPort\0");
        let cs = CGColorSpaceCreateDeviceRGB();
        let provider = CGDataProviderCreateWithData(null_mut(), fb.pixels.as_ptr() as *const c_void, fb.pixels.len() * 4, std::ptr::null());
        // kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little: memory is B,G,R,X, which is our u32.
        let img = CGImageCreate(fb.width, fb.height, 8, 32, fb.width * 4, cs, 6 | (2 << 12), provider, std::ptr::null(), 0, 0);
        if !img.is_null() {
            CGContextSaveGState(ctx);
            // The view is flipped, so flip back to keep the first row on top.
            CGContextTranslateCTM(ctx, 0.0, bounds.size.h);
            CGContextScaleCTM(ctx, 1.0, -1.0);
            CGContextDrawImage(ctx, CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: bounds.size }, img);
            CGContextRestoreGState(ctx);
            CGImageRelease(img);
        }
        CGDataProviderRelease(provider);
        CGColorSpaceRelease(cs);
    }
}

unsafe fn mouse_event(this: Id, event: Id, kind: u8) {
    let p = send!(CGPoint, event, b"locationInWindow\0");
    let h = send_rect(this, b"bounds\0").size.h;
    let (x, y) = (p.x as f32, (h - p.y) as f32);
    if let Some(c) = core() {
        let e = match kind {
            0 => Event::MouseMove { x, y },
            1 => Event::MouseDown { x, y },
            _ => Event::MouseUp { x, y },
        };
        if c.event(e) { send!((), this, b"setNeedsDisplay:\0", i8 = 1); }
    }
}
extern "C" fn mouse_moved(this: Id, _c: Sel, e: Id) { unsafe { mouse_event(this, e, 0) } }
extern "C" fn mouse_down(this: Id, _c: Sel, e: Id) { unsafe { mouse_event(this, e, 1) } }
extern "C" fn mouse_up(this: Id, _c: Sel, e: Id) { unsafe { mouse_event(this, e, 2) } }
/// `scrollWheel:` deltaY is positive when content moves down. The shell's `dy` is the reverse.
extern "C" fn scroll_wheel(this: Id, _c: Sel, e: Id) {
    unsafe {
        let dy = send!(f64, e, b"deltaY\0");
        let p = send!(CGPoint, e, b"locationInWindow\0");
        let h = send_rect(this, b"bounds\0").size.h;
        if let Some(c) = core() {
            let ev = Event::MouseWheel { x: p.x as f32, y: (h - p.y) as f32, dy: -dy as f32 };
            if c.event(ev) { send!((), this, b"setNeedsDisplay:\0", i8 = 1); }
        }
    }
}

/// Key codes from HIToolbox/Events.h. Shift is bit 17 of `modifierFlags`.
extern "C" fn key_down(this: Id, _c: Sel, e: Id) {
    unsafe {
        let code = send!(u16, e, b"keyCode\0");
        let shift = send!(usize, e, b"modifierFlags\0") & (1 << 17) != 0;
        let key = match code {
            48 => Key::Tab,
            36 | 76 => Key::Enter,
            49 => Key::Space,
            126 => Key::Up,
            125 => Key::Down,
            53 => Key::Escape,
            _ => return,
        };
        if let Some(c) = core() {
            if c.event(Event::Key { key, shift }) { send!((), this, b"setNeedsDisplay:\0", i8 = 1); }
        }
    }
}

extern "C" fn is_flipped(_this: Id, _c: Sel) -> i8 { 1 }
extern "C" fn yes_responder(_this: Id, _c: Sel) -> i8 { 1 }
extern "C" fn terminate_after_last(_this: Id, _c: Sel, _app: Id) -> i8 { 1 }
extern "C" fn window_will_close(_this: Id, _c: Sel, _n: Id) {
    unsafe { CORE = null_mut(); }
}
extern "C" fn tick(this: Id, _c: Sel, _timer: Id) {
    unsafe {
        if let Some(c) = core() {
            if let Some(code) = c.smoke_result() { std::process::exit(code); }
            if c.tick() { send!((), this, b"setNeedsDisplay:\0", i8 = 1); }
        }
    }
}

unsafe fn make_view_class() -> Class {
    let cls = objc_allocateClassPair(class(b"NSView\0"), b"GLDriShellView\0".as_ptr() as *const c_char, 0);
    let add = |name: &[u8], imp: *const c_void, types: &[u8]| {
        class_addMethod(cls, sel(name), imp, types.as_ptr() as *const c_char);
    };
    let mut draw_types = b"v@:".to_vec();
    draw_types.extend_from_slice(RECT_ENC);
    add(b"drawRect:\0", draw_rect as *const c_void, &draw_types);
    add(b"isFlipped\0", is_flipped as *const c_void, b"c@:\0");
    add(b"acceptsFirstResponder\0", yes_responder as *const c_void, b"c@:\0");
    add(b"mouseMoved:\0", mouse_moved as *const c_void, b"v@:@\0");
    add(b"mouseDragged:\0", mouse_moved as *const c_void, b"v@:@\0");
    add(b"mouseDown:\0", mouse_down as *const c_void, b"v@:@\0");
    add(b"mouseUp:\0", mouse_up as *const c_void, b"v@:@\0");
    add(b"scrollWheel:\0", scroll_wheel as *const c_void, b"v@:@\0");
    add(b"keyDown:\0", key_down as *const c_void, b"v@:@\0");
    add(b"tick:\0", tick as *const c_void, b"v@:@\0");
    add(b"windowWillClose:\0", window_will_close as *const c_void, b"v@:@\0");
    add(b"applicationShouldTerminateAfterLastWindowClosed:\0", terminate_after_last as *const c_void, b"c@:@\0");
    objc_registerClassPair(cls);
    cls
}

pub(crate) fn run(mut core_state: Core) -> i32 {
    unsafe {
        let pool = send!(Id, send!(Id, class(b"NSAutoreleasePool\0") as Id, b"alloc\0"), b"init\0");
        let app = send!(Id, class(b"NSApplication\0") as Id, b"sharedApplication\0");
        send!((), app, b"setActivationPolicy:\0", isize = 0);

        // Menu bar with Quit, so Cmd-Q works.
        let bar = send!(Id, send!(Id, class(b"NSMenu\0") as Id, b"alloc\0"), b"init\0");
        let bar_item = send!(Id, send!(Id, class(b"NSMenuItem\0") as Id, b"alloc\0"), b"init\0");
        send!((), bar, b"addItem:\0", Id = bar_item);
        let app_menu = send!(Id, send!(Id, class(b"NSMenu\0") as Id, b"alloc\0"), b"init\0");
        let quit = send!(Id, send!(Id, class(b"NSMenuItem\0") as Id, b"alloc\0"), b"initWithTitle:action:keyEquivalent:\0",
            Id = nsstring("Quit"), Sel = sel(b"terminate:\0"), Id = nsstring("q"));
        send!((), app_menu, b"addItem:\0", Id = quit);
        send!((), bar_item, b"setSubmenu:\0", Id = app_menu);
        send!((), app, b"setMainMenu:\0", Id = bar);

        let (w, h) = (core_state.cfg.width as CGFloat, core_state.cfg.height as CGFloat);
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w, h } };
        let title = nsstring(&core_state.cfg.title);
        CORE = &mut core_state;

        let cls = make_view_class();
        let view = send!(Id, send!(Id, cls as Id, b"alloc\0"), b"initWithFrame:\0", CGRect = frame);
        // titled | closable | miniaturizable | resizable, buffered backing.
        let window = send!(Id, send!(Id, class(b"NSWindow\0") as Id, b"alloc\0"), b"initWithContentRect:styleMask:backing:defer:\0",
            CGRect = frame, usize = 15, usize = 2, i8 = 0);
        if window.is_null() || view.is_null() {
            CORE = null_mut();
            eprintln!("goodlookindri-shell: could not create the window");
            return 1;
        }
        send!((), window, b"setContentView:\0", Id = view);
        send!((), window, b"setTitle:\0", Id = title);
        send!((), window, b"setDelegate:\0", Id = view);
        send!((), window, b"setAcceptsMouseMovedEvents:\0", i8 = 1);
        send!((), window, b"center\0");
        send!((), window, b"makeKeyAndOrderFront:\0", Id = null_mut());
        send!((), window, b"makeFirstResponder:\0", Id = view);
        send!((), app, b"setDelegate:\0", Id = view);
        send!((), app, b"activateIgnoringOtherApps:\0", i8 = 1);
        send!(Id, class(b"NSTimer\0") as Id, b"scheduledTimerWithTimeInterval:target:selector:userInfo:repeats:\0",
            f64 = 0.016, Id = view, Sel = sel(b"tick:\0"), Id = null_mut(), i8 = 1);

        send!((), app, b"run\0");
        send!((), pool, b"drain\0");
        CORE = null_mut();
        let _ = CStr::from_bytes_with_nul(b"\0");
        0
    }
}
