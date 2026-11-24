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
//! Repaint: after each tick and each input call the view asks the editor what
//! changed ([`Editor::damage`]) and invalidates only those rectangles
//! (`setNeedsDisplayInRect:`). An editor with nothing to repaint costs the
//! timer tick and nothing else; an editor that does not report damage (the
//! default) is invalidated whole, as before.
//!
//! Accessibility: the view is an accessibility group whose children are one
//! `NSAccessibilityElement` per node of [`Editor::accessibility_nodes`]
//! (a control with a name, a value as text and a role: slider, check box or
//! button). The elements hold no copy: each question (role, label, value,
//! frame, focus) asks the editor again, so the answer is always current, and the
//! frame is converted to screen coordinates on every ask, so a resize, a scale
//! change or a moved window needs no update. `accessibilityPerformIncrement`,
//! `Decrement` and `Press` go to [`Editor::accessibility_action`], and a
//! VoiceOver cursor move to [`Editor::accessibility_focus`]. After every tick
//! and input call the view compares the nodes with what it last saw and posts
//! `NSAccessibilityValueChangedNotification` for a value that changed (from the
//! host's automation too), `FocusedUIElementChanged` when keyboard focus moved,
//! and `LayoutChanged` when the controls or the size changed.
//!
//! Typing, tooltips, files: after every tick and input call the view polls the editor
//! for a text-field request ([`Editor::take_text_edit`]; a native `NSTextField` over the
//! control, the view as its delegate: Return and Enter commit, Escape cancels, and losing focus (a click
//! elsewhere, the window resigning key, the view leaving its window) commits valid text and
//! drops invalid text; a local key monitor takes Return, Enter and Escape for the field before a
//! host's own key handling can, and is removed whenever the field closes; no key
//! reaches the editor while it is open), re-registers one tooltip rectangle per
//! [`Editor::tooltips`] entry when the layout moved (the text is asked again when a tip
//! shows), and takes a file-dialog request ([`Editor::take_file_dialog`], an `NSOpenPanel`
//! sheet whose answer is delivered on a later tick). A view whose editor
//! [`Editor::wants_drops`] is a drag destination for file URLs.
//!
//! Main thread only.

#![allow(unsafe_code, clippy::missing_safety_doc)]

use openplugin_core::editor::{AccessAction, AccessNode, AccessRole, FileDialog, ParamMenu, ParamMenuAction, SizeRange, TextEdit};
use openplugin_core::editor::{Key, Modifiers};
use openplugin_core::Editor;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::ptr::null_mut;
use std::time::Instant;

type Id = *mut c_void;

/// An `NSView *` (for [`EditorView::raw`] and [`EditorView::attach`]).
pub type NSViewPtr = *mut c_void;
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
    fn sel_getName(sel: Sel) -> *const c_char;
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
unsafe extern "C" {
    static NSAccessibilitySliderRole: Id;
    static NSAccessibilityCheckBoxRole: Id;
    static NSAccessibilityButtonRole: Id;
    static NSAccessibilityGroupRole: Id;
    static NSAccessibilityValueChangedNotification: Id;
    static NSAccessibilityFocusedUIElementChangedNotification: Id;
    static NSAccessibilityLayoutChangedNotification: Id;
    fn NSAccessibilityPostNotification(element: Id, notification: Id);
    static NSPasteboardTypeFileURL: Id;
    static NSPasteboardURLReadingFileURLsOnlyKey: Id;
    fn NSBeep();
}

#[link(name = "System")]
unsafe extern "C" {
    static _NSConcreteGlobalBlock: [*const c_void; 32];
}

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
    /// A paint or an invalidation that came in while `busy`.
    repaint: bool,
    /// The view was dropped while `busy`: the state goes when the call ends.
    closing: bool,
    /// The view is changing its own frame.
    in_frame: bool,
    /// The backing scale of the last frame drawn (0 before the first).
    scale: f32,
    /// One accessibility element per node, created on demand (retained).
    ax_elements: Vec<Id>,
    /// What the nodes looked like when notifications were last posted.
    ax_seen: Vec<AxSeen>,
    /// The open text field (an autoreleased-on-close `NSTextField`, retained here).
    text_field: Option<Id>,
    /// The field must close (the editor's size changed under it); done outside the editor call.
    field_cancel: bool,
    /// The field is being opened: AppKit reports an end of editing while the field editor swaps in.
    opening: bool,
    /// The tooltip rectangles registered, by tooltip index, in view points.
    tip_key: Vec<(usize, [f64; 4])>,
    /// The view is registered as a drag destination.
    drags: bool,
    /// Answers to file dialogs, delivered on the next tick.
    file_results: Vec<(u32, Option<Vec<PathBuf>>)>,
    file_hook: Option<FileHook>,
    /// The parameter the SDK's menu was last shown for (the items act on it).
    menu_param: u32,
    menu_hook: Option<MenuHook>,
}

/// The items of the SDK's own parameter menu, in order: the item chosen is an
/// index into this list.
#[doc(hidden)]
pub const PARAM_MENU_ITEMS: [&str; 4] = ["Reset to default", "Copy value", "Paste value", "Type value"];

/// Stands in for the system menu (tests): returning true means the menu was
/// handled (nothing is shown); false lets the real `NSMenu` pop up.
#[doc(hidden)]
pub type MenuHook = Box<dyn FnMut(&ParamMenu) -> bool>;

/// Stands in for the system file panel (tests): `Some(answer)` resolves the
/// request, `None` lets the real panel open.
#[doc(hidden)]
pub type FileHook = Box<dyn FnMut(&FileDialog) -> Option<Option<Vec<PathBuf>>>>;

/// A node as the last notification pass saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AxSeen {
    value: String,
    focused: bool,
}

/// What changed between two passes over the nodes.
#[derive(Debug, PartialEq, Eq)]
struct AxDiff {
    /// The number of nodes changed.
    layout: bool,
    /// Nodes whose value text changed.
    values: Vec<usize>,
    /// The focused node changed: the new one (`None` for no focus).
    focus: Option<Option<usize>>,
}

fn ax_diff(prev: &[AxSeen], now: &[AccessNode]) -> AxDiff {
    let values = now.iter().enumerate().filter(|(i, n)| prev.get(*i).is_some_and(|p| p.value != n.value)).map(|(i, _)| i).collect();
    let before = prev.iter().position(|p| p.focused);
    let after = now.iter().position(|n| n.focused);
    AxDiff { layout: prev.len() != now.len(), values, focus: (before != after).then_some(after) }
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

/// The state, unless the editor is inside one of its calls (a host's modal
/// loop runs timers and queries there): nothing else may reach the editor then.
unsafe fn idle<'a>(this: Id) -> Option<&'a mut State> { unsafe {
    state(this).filter(|st| !st.busy)
}}

/// Run an editor call. A resize that arrived meanwhile is applied after it.
/// False, and no call, when the editor is already inside a call.
unsafe fn guarded(this: Id, call: impl FnOnce(&mut State)) -> bool { unsafe {
    let Some(st) = idle(this) else { return false };
    st.busy = true;
    call(st);
    st.busy = false;
    if st.closing {
        // The view was dropped inside the call. Its callers may still use `this`.
        drop(Box::from_raw(st as *mut State));
        send!(Id, this, b"autorelease\0");
        return true;
    }
    if let Some((w, h, frame)) = st.pending.take() {
        apply(this, st, w, h, frame);
    }
    if std::mem::take(&mut st.repaint) {
        redraw(this);
    }
    true
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
    if st.text_field.is_some() {
        st.field_cancel = true;
    }
    if !st.ax_elements.is_empty() {
        NSAccessibilityPostNotification(this, NSAccessibilityLayoutChangedNotification);
    }
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
    unsafe {
        if !guarded(this, |st| draw(this, st)) {
            if let Some(st) = state(this) {
                st.repaint = true;
            }
        }
    }
}

unsafe fn draw(this: Id, st: &mut State) {
    unsafe {
        let bounds = send_rect(this, b"bounds\0");
        let scale = backing_scale(this).clamp(1.0, 3.0);
        st.scale = scale;
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
    local_point(this, size, send!(CGPoint, event, b"locationInWindow\0"))
}}

/// A window point in editor points.
unsafe fn local_point(this: Id, size: (u32, u32), p: CGPoint) -> (f32, f32) { unsafe {
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
    // While the text field is open, nothing typed reaches the editor or the host.
    if state(this).is_some_and(|st| st.text_field.is_some()) {
        return;
    }
    let mut handled = false;
    guarded(this, |st| {
        let mods = sync_modifiers(st, event);
        if let Some(key) = event_key(event).filter(|_| st.editor.wants_keys()) {
            handled = if down { st.editor.key_down(key, mods) } else { st.editor.key_up(key, mods) };
        }
    });
    if handled {
        invalidate(this);
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
    unsafe { idle(this).is_some_and(|st| st.editor.wants_keys()) as i8 }
}

extern "C" fn become_first_responder(this: Id, _cmd: Sel) -> i8 {
    unsafe {
        guarded(this, |st| st.editor.focus_changed(true));
        invalidate(this);
    }
    1
}

extern "C" fn resign_first_responder(this: Id, _cmd: Sel) -> i8 {
    unsafe {
        guarded(this, |st| st.editor.focus_changed(false));
        invalidate(this);
    }
    1
}

unsafe fn redraw(this: Id) { unsafe {
    send!((), this, b"setNeedsDisplay:\0", i8 = 1);
}}

/// The view rectangle (whole points, a point more on every side for anti-aliasing and
/// rounding) that shows an editor rectangle, or `None` when it is not finite.
fn view_rect(r: &openplugin_core::editor::DamageRect, editor: (u32, u32), view: CGSize) -> Option<CGRect> {
    if !r.is_finite() || editor.0 == 0 || editor.1 == 0 {
        return None;
    }
    let (sx, sy) = (view.w / f64::from(editor.0), view.h / f64::from(editor.1));
    let x0 = ((f64::from(r.x) * sx).floor() - 1.0).clamp(0.0, view.w);
    let y0 = ((f64::from(r.y) * sy).floor() - 1.0).clamp(0.0, view.h);
    let x1 = (((f64::from(r.x) + f64::from(r.w)) * sx).ceil() + 1.0).clamp(0.0, view.w);
    let y1 = (((f64::from(r.y) + f64::from(r.h)) * sy).ceil() + 1.0).clamp(0.0, view.h);
    Some(CGRect { origin: CGPoint { x: x0, y: y0 }, size: CGSize { w: (x1 - x0).max(0.0), h: (y1 - y0).max(0.0) } })
}

/// Ask the editor what changed and invalidate that: nothing, some rectangles, or the whole view.
unsafe fn invalidate(this: Id) { unsafe {
    let Some(st) = state(this) else { return };
    if st.busy {
        st.repaint = true;
        return;
    }
    ax_sync(this);
    poll_requests(this);
    sync_tooltips(this);
    let mut damage = None;
    let mut size = (0, 0);
    guarded(this, |st| {
        damage = st.editor.damage();
        size = st.size;
    });
    let Some(rects) = damage else { return redraw(this) };
    if rects.is_empty() {
        return;
    }
    let view = send_rect(this, b"bounds\0").size;
    let mut cg = Vec::with_capacity(rects.len());
    for r in &rects {
        match view_rect(r, size, view) {
            Some(rect) => cg.push(rect),
            None => return redraw(this),
        }
    }
    for rect in cg {
        send!((), this, b"setNeedsDisplayInRect:\0", CGRect = rect);
    }
}}

/// The right button, or Control-click: the editor may take it for a parameter
/// menu (the host's or the SDK's); otherwise it goes on to the host.
extern "C" fn right_mouse_down(this: Id, _cmd: Sel, event: Id) {
    unsafe {
        close_text_field(this, true);
        let mut used = false;
        guarded(this, |st| {
            sync_modifiers(st, event);
            let (x, y) = local(this, st.size, event);
            used = st.editor.right_mouse_down(x, y);
        });
        if used {
            invalidate(this);
        } else {
            pass_on(this, b"rightMouseDown:\0", event);
        }
    }
}

extern "C" fn mouse_down(this: Id, _cmd: Sel, event: Id) {
    unsafe {
        // Control-click is a right click on a Mac.
        if modifiers_from_flags(send!(usize, event, b"modifierFlags\0")).ctrl {
            let mut used = false;
            guarded(this, |st| {
                sync_modifiers(st, event);
                let (x, y) = local(this, st.size, event);
                used = st.editor.right_mouse_down(x, y);
            });
            if used {
                invalidate(this);
                return;
            }
        }
        // A click on the view (the field takes its own) abandons the field.
        close_text_field(this, true);
        // A click gives the editor the keyboard, when it wants keys. Outside the
        // editor call: taking focus calls the editor again.
        if idle(this).is_some_and(|st| st.editor.wants_keys()) {
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
        invalidate(this);
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
        invalidate(this);
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
        let ran = guarded(this, |st| {
            if let Some(poll) = st.poll.as_mut() {
                poll(st.editor.as_mut());
            }
            let now = Instant::now();
            let dt = now.duration_since(st.last).as_secs_f32().min(0.1);
            st.last = now;
            st.editor.tick(dt);
            // An editor may change its own size (a poll hook resized it).
            follow(this, st, true);
            // Answers to file dialogs arrive here, never inside the call that asked.
            for (tag, paths) in std::mem::take(&mut st.file_results) {
                st.editor.file_dialog_done(tag, paths);
            }
        });
        if !ran {
            return;
        }
        ensure_drag_registration(this);
        // A window moved to a display of another scale gets a full frame.
        if state(this).is_some_and(|st| st.scale != 0.0 && st.scale != backing_scale(this).clamp(1.0, 3.0)) {
            redraw(this);
        } else {
            invalidate(this);
        }
    }
}

extern "C" fn yes(_this: Id, _cmd: Sel) -> i8 {
    1
}

extern "C" fn yes_event(_this: Id, _cmd: Sel, _event: Id) -> i8 {
    1
}

// ---- Typing a value, tooltips, files -------------------------------------

/// Requests the editor made during the last call: a text field, a file dialog.
/// Never called inside an editor call.
unsafe fn poll_requests(this: Id) { unsafe {
    if state(this).is_some_and(|st| st.field_cancel) {
        if let Some(st) = state(this) {
            st.field_cancel = false;
        }
        close_text_field(this, true);
    }
    let mut edit = None;
    let mut dialog = None;
    let mut menu = None;
    guarded(this, |st| {
        edit = st.editor.take_text_edit();
        dialog = st.editor.take_file_dialog();
        menu = st.editor.take_param_menu();
    });
    if let Some(menu) = menu {
        show_param_menu(this, menu);
    }
    if let Some(edit) = edit {
        open_text_field(this, edit);
    }
    if let Some(dialog) = dialog {
        open_file_dialog(this, dialog);
    }
}}

/// Show the SDK's own parameter menu at the click, or hand it to the test hook.
/// The items act on the parameter through [`menu_item`].
unsafe fn show_param_menu(this: Id, menu: ParamMenu) { unsafe {
    let Some(st) = state(this) else { return };
    st.menu_param = menu.param;
    if let Some(hook) = st.menu_hook.as_mut() {
        if hook(&menu) {
            return;
        }
    }
    let size = st.size;
    let ns = send!(Id, send!(Id, class(b"NSMenu\0") as Id, b"alloc\0"), b"initWithTitle:\0", Id = ns_string(&menu.label));
    if ns.is_null() {
        return;
    }
    send!((), ns, b"setAutoenablesItems:\0", i8 = 0);
    for (i, title) in PARAM_MENU_ITEMS.iter().enumerate() {
        let item = send!(
            Id,
            send!(Id, class(b"NSMenuItem\0") as Id, b"alloc\0"),
            b"initWithTitle:action:keyEquivalent:\0",
            Id = ns_string(title),
            Sel = sel(b"openpluginMenuItem:\0"),
            Id = ns_string("")
        );
        send!((), item, b"setTag:\0", isize = i as isize);
        send!((), item, b"setTarget:\0", Id = this);
        send!((), ns, b"addItem:\0", Id = item);
        send!((), item, b"release\0");
    }
    let bounds = send_rect(this, b"bounds\0");
    let sx = if size.0 > 0 { bounds.size.w / f64::from(size.0) } else { 1.0 };
    let sy = if size.1 > 0 { bounds.size.h / f64::from(size.1) } else { 1.0 };
    let at = CGPoint { x: f64::from(menu.x) * sx, y: f64::from(menu.y) * sy };
    // Blocks in the menu's own tracking loop; the chosen item runs `menu_item` inside it.
    send!(i8, ns, b"popUpMenuPositioningItem:atLocation:inView:\0", Id = null_mut(), CGPoint = at, Id = this);
    send!((), ns, b"release\0");
}}

const PASTEBOARD_TEXT: &str = "public.utf8-plain-text";

unsafe fn pasteboard_text() -> Option<String> { unsafe {
    let board = send!(Id, class(b"NSPasteboard\0") as Id, b"generalPasteboard\0");
    if board.is_null() {
        return None;
    }
    let s = send!(Id, board, b"stringForType:\0", Id = ns_string(PASTEBOARD_TEXT));
    if s.is_null() {
        return None;
    }
    let utf8 = send!(*const c_char, s, b"UTF8String\0");
    (!utf8.is_null()).then(|| CStr::from_ptr(utf8).to_string_lossy().into_owned())
}}

unsafe fn set_pasteboard_text(text: &str) { unsafe {
    let board = send!(Id, class(b"NSPasteboard\0") as Id, b"generalPasteboard\0");
    if board.is_null() {
        return;
    }
    send!(isize, board, b"clearContents\0");
    send!(i8, board, b"setString:forType:\0", Id = ns_string(text), Id = ns_string(PASTEBOARD_TEXT));
}}

/// Run item `index` of [`PARAM_MENU_ITEMS`] for the parameter the menu was
/// shown for. Paste reads the clipboard and copy writes it, unless `text` is
/// given (tests), which stands in for the clipboard both ways.
unsafe fn run_menu_item(this: Id, index: usize, text: Option<&str>) -> Option<String> { unsafe {
    let param = state(this)?.menu_param;
    let action = match index {
        0 => ParamMenuAction::Reset,
        1 => ParamMenuAction::Copy,
        2 => ParamMenuAction::Paste(match text {
            Some(t) => t.to_string(),
            None => pasteboard_text().unwrap_or_default(),
        }),
        3 => ParamMenuAction::Type,
        _ => return None,
    };
    let mut copied = None;
    guarded(this, |st| copied = st.editor.param_menu_action(param, action));
    if let (Some(t), None) = (copied.as_deref(), text) {
        set_pasteboard_text(t);
    }
    invalidate(this);
    copied
}}

/// The action of every item of the SDK's parameter menu; the tag is the index.
extern "C" fn menu_item(this: Id, _cmd: Sel, sender: Id) {
    unsafe {
        if sender.is_null() {
            return;
        }
        let tag = send!(isize, sender, b"tag\0");
        if tag >= 0 {
            run_menu_item(this, tag as usize, None);
        }
    }
}

/// Open the small native field over the control, with its text selected.
unsafe fn open_text_field(this: Id, edit: TextEdit) { unsafe {
    close_text_field(this, true);
    let Some(st) = state(this) else { return };
    let r = view_space(this, st.size, &edit.rect);
    let (w, h) = (r.size.w.max(72.0), 22.0);
    let frame = CGRect {
        origin: CGPoint { x: r.origin.x + (r.size.w - w) * 0.5, y: r.origin.y + (r.size.h - h) * 0.5 },
        size: CGSize { w, h },
    };
    let field = send!(Id, send!(Id, class(b"NSTextField\0") as Id, b"alloc\0"), b"initWithFrame:\0", CGRect = frame);
    if field.is_null() {
        return;
    }
    send!((), field, b"setStringValue:\0", Id = ns_string(&edit.text));
    send!((), field, b"setAlignment:\0", isize = 2);
    send!((), field, b"setFont:\0", Id = send!(Id, class(b"NSFont\0") as Id, b"systemFontOfSize:\0", f64 = 11.0));
    send!((), field, b"setAccessibilityLabel:\0", Id = ns_string(&edit.label));
    send!((), field, b"setDelegate:\0", Id = this);
    send!((), this, b"addSubview:\0", Id = field);
    st.text_field = Some(field);
    st.opening = true;
    let window = send!(Id, this, b"window\0");
    if !window.is_null() {
        send!(i8, window, b"makeFirstResponder:\0", Id = field);
        send!((), field, b"selectText:\0", Id = null_mut());
    }
    if let Some(st) = state(this) {
        st.opening = false;
    }
    install_field_hooks(this, window);
}}

/// Take the field away. `cancel` tells the editor nothing was committed.
unsafe fn close_text_field(this: Id, cancel: bool) { unsafe {
    let Some(st) = state(this) else { return };
    let Some(field) = st.text_field.take() else { return };
    remove_field_hooks(this);
    if cancel {
        guarded(this, |st| st.editor.text_edit_cancel());
    }
    send!((), field, b"setDelegate:\0", Id = null_mut());
    let window = send!(Id, this, b"window\0");
    if !window.is_null() {
        send!(i8, window, b"makeFirstResponder:\0", Id = this);
    }
    send!((), field, b"removeFromSuperview\0");
    // We may be inside the field's own delegate call: let it unwind first.
    send!(Id, field, b"autorelease\0");
    invalidate(this);
}}

/// Hand the field's text to the editor. Accepted: the field closes. Refused: it stays, turns
/// red and the system beeps. False when no field is open.
unsafe fn commit_text_field(this: Id) -> bool { unsafe {
    let Some(field) = state(this).and_then(|st| st.text_field) else { return false };
    let text = field_text(field);
    let mut done = false;
    guarded(this, |st| done = st.editor.text_edit_commit(&text));
    if done {
        close_text_field(this, false);
    } else {
        send!((), field, b"setTextColor:\0", Id = send!(Id, class(b"NSColor\0") as Id, b"redColor\0"));
        NSBeep();
    }
    true
}}

unsafe fn field_text(field: Id) -> String { unsafe {
    let utf8 = send!(*const c_char, send!(Id, field, b"stringValue\0"), b"UTF8String\0");
    if utf8.is_null() { String::new() } else { CStr::from_ptr(utf8).to_string_lossy().into_owned() }
}}

/// Focus left the field: valid text is committed, invalid text is discarded without a sound.
unsafe fn field_lost_focus(this: Id) { unsafe {
    let Some(field) = state(this).and_then(|st| st.text_field) else { return };
    let text = field_text(field);
    let mut done = false;
    guarded(this, |st| done = st.editor.text_edit_commit(&text));
    // A refused commit leaves the edit open at the editor: cancel closes it.
    close_text_field(this, !done);
}}

/// Return commits through the editor, Escape cancels; every other command is the field's own.
extern "C" fn do_command(this: Id, _cmd: Sel, _control: Id, _text_view: Id, command: Sel) -> i8 {
    unsafe {
        let name = CStr::from_ptr(sel_getName(command)).to_bytes();
        match name {
            b"insertNewline:" => commit_text_field(this) as i8,
            b"cancelOperation:" => {
                close_text_field(this, true);
                1
            }
            _ => 0,
        }
    }
}

/// The field lost focus without Return: commit what is valid.
extern "C" fn text_ended(this: Id, _cmd: Sel, _note: Id) {
    unsafe {
        if state(this).is_some_and(|st| !st.opening) {
            field_lost_focus(this);
        }
    }
}

/// The window the field sits in stopped being key.
extern "C" fn window_resigned_key(this: Id, _cmd: Sel, _note: Id) {
    unsafe {
        if state(this).is_some_and(|st| !st.opening) {
            field_lost_focus(this);
        }
    }
}

/// The view is about to leave its window (or change window): the field goes first,
/// while the window is still there.
extern "C" fn view_will_move_to_window(this: Id, _cmd: Sel, new_window: Id) {
    unsafe {
        if new_window != send!(Id, this, b"window\0") {
            field_lost_focus(this);
            remove_field_hooks(this);
        }
        let sup = ObjcSuper { receiver: this, super_class: class(b"NSView\0") };
        let f: unsafe extern "C" fn(*const ObjcSuper, Sel, Id) = std::mem::transmute(objc_msgSendSuper as unsafe extern "C" fn());
        f(&sup, sel(b"viewWillMoveToWindow:\0"), new_window);
    }
}

// ---- The key monitor for the type-in field ---------------------------------

/// What the field does with a key while it is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub enum FieldKey {
    Commit,
    Cancel,
    Pass,
}

/// Return (36) and keypad Enter (76) commit, Escape (53) cancels, but only when the field's own
/// editor holds the keyboard; every other key, and these keys elsewhere, pass unchanged.
#[doc(hidden)]
pub fn field_key(key_code: u16, field_editor_is_first_responder: bool) -> FieldKey {
    match (field_editor_is_first_responder, key_code) {
        (true, 36 | 76) => FieldKey::Commit,
        (true, 53) => FieldKey::Cancel,
        _ => FieldKey::Pass,
    }
}

#[repr(C)]
struct MonitorBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*const MonitorBlock, Id) -> Id,
    descriptor: *const BlockDescriptor,
}

// The block is read-only and holds no state: the views are found through `MONITORS`.
unsafe impl Sync for MonitorBlock {}

static MONITOR_BLOCK: MonitorBlock = MonitorBlock {
    isa: &raw const _NSConcreteGlobalBlock as *const c_void,
    flags: 1 << 28,
    reserved: 0,
    invoke: monitor_invoke,
    descriptor: &MONITOR_BLOCK_DESCRIPTOR,
};

static MONITOR_BLOCK_DESCRIPTOR: BlockDescriptor = BlockDescriptor { reserved: 0, size: std::mem::size_of::<MonitorBlock>() };

thread_local! {
    /// The views with an open field and the monitor object AppKit gave each (main thread only).
    static MONITORS: std::cell::RefCell<Vec<(usize, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// How many key monitors the type-in fields hold right now. Zero when no field is open.
#[doc(hidden)]
pub fn key_monitor_count() -> usize {
    MONITORS.with(|m| m.borrow().len())
}

/// The field editor of the view's open field, when it holds the keyboard in the event's window.
unsafe fn field_has_keyboard(view: Id, event: Id) -> bool { unsafe {
    let Some(field) = state(view).and_then(|st| st.text_field) else { return false };
    let window = send!(Id, view, b"window\0");
    // An event for another window is not ours. (An event that names no window, as a test
    // builds for a window that was never created on screen, is judged by the responder alone.)
    let target = send!(Id, event, b"window\0");
    if window.is_null() || (!target.is_null() && target != window) {
        return false;
    }
    let editor = send!(Id, field, b"currentEditor\0");
    !editor.is_null() && send!(Id, window, b"firstResponder\0") == editor
}}

/// The handler AppKit calls for every key down of the application: nil swallows the event.
unsafe extern "C" fn monitor_invoke(_block: *const MonitorBlock, event: Id) -> Id { unsafe {
    if event.is_null() {
        return event;
    }
    let views: Vec<usize> = MONITORS.with(|m| m.borrow().iter().map(|e| e.0).collect());
    for v in views {
        let view = v as Id;
        // Never re-enter an editor call.
        if state(view).is_none_or(|st| st.busy) {
            continue;
        }
        match field_key(send!(u16, event, b"keyCode\0"), field_has_keyboard(view, event)) {
            FieldKey::Commit => {
                commit_text_field(view);
                return null_mut();
            }
            FieldKey::Cancel => {
                close_text_field(view, true);
                return null_mut();
            }
            FieldKey::Pass => {}
        }
    }
    event
}}

/// The field is open: watch the keys, and the window losing key.
unsafe fn install_field_hooks(this: Id, window: Id) { unsafe {
    if MONITORS.with(|m| m.borrow().iter().any(|e| e.0 == this as usize)) || state(this).is_none_or(|st| st.text_field.is_none()) {
        return;
    }
    // NSEventMaskKeyDown is 1 << NSEventTypeKeyDown (10).
    let monitor = send!(Id, class(b"NSEvent\0") as Id, b"addLocalMonitorForEventsMatchingMask:handler:\0", u64 = 1u64 << 10, *const MonitorBlock = &MONITOR_BLOCK);
    if !monitor.is_null() {
        send!(Id, monitor, b"retain\0");
        MONITORS.with(|m| m.borrow_mut().push((this as usize, monitor as usize)));
    }
    if !window.is_null() {
        let center = send!(Id, class(b"NSNotificationCenter\0") as Id, b"defaultCenter\0");
        send!((), center, b"addObserver:selector:name:object:\0", Id = this, Sel = sel(b"openpluginWindowResignedKey:\0"), Id = ns_string("NSWindowDidResignKeyNotification"), Id = window);
    }
}}

/// Take the monitor and the observer away. Safe to call when neither is there.
unsafe fn remove_field_hooks(this: Id) { unsafe {
    let taken = MONITORS.with(|m| {
        let mut m = m.borrow_mut();
        m.iter().position(|e| e.0 == this as usize).map(|i| m.remove(i).1)
    });
    if let Some(monitor) = taken {
        send!((), class(b"NSEvent\0") as Id, b"removeMonitor:\0", Id = monitor as Id);
        send!((), monitor as Id, b"release\0");
    }
    let center = send!(Id, class(b"NSNotificationCenter\0") as Id, b"defaultCenter\0");
    send!((), center, b"removeObserver:\0", Id = this);
}}

/// Keep one tooltip rectangle per entry of [`Editor::tooltips`], moved with
/// the layout. Registered again only when a rectangle (or the view's size) changed.
unsafe fn sync_tooltips(this: Id) { unsafe {
    let Some(st) = state(this) else { return };
    let size = st.size;
    let key: Vec<(usize, [f64; 4])> = st
        .editor
        .tooltips()
        .iter()
        .enumerate()
        .filter(|(_, t)| t.rect.is_finite() && !t.rect.is_empty())
        .map(|(i, t)| {
            let r = view_space(this, size, &t.rect);
            (i, [r.origin.x, r.origin.y, r.size.w, r.size.h])
        })
        .collect();
    if key == st.tip_key {
        return;
    }
    send!((), this, b"removeAllToolTips\0");
    for (i, r) in &key {
        let rect = CGRect { origin: CGPoint { x: r[0], y: r[1] }, size: CGSize { w: r[2], h: r[3] } };
        send!(isize, this, b"addToolTipRect:owner:userData:\0", CGRect = rect, Id = this, *mut c_void = *i as *mut c_void);
    }
    st.tip_key = key;
}}

/// `view:stringForToolTip:point:userData:`: the text is asked of the editor each time a tip shows.
extern "C" fn tooltip_string(this: Id, _cmd: Sel, _view: Id, _tag: isize, _point: CGPoint, data: *mut c_void) -> Id {
    unsafe {
        let index = data as usize;
        match idle(this).and_then(|st| st.editor.tooltips().into_iter().nth(index)) {
            Some(t) => ns_string(&t.text),
            None => null_mut(),
        }
    }
}

/// Register as a drag destination for file URLs once the editor wants drops.
unsafe fn ensure_drag_registration(this: Id) { unsafe {
    let Some(st) = state(this) else { return };
    if st.drags || !st.editor.wants_drops() {
        return;
    }
    st.drags = true;
    let types = ns_array(&[NSPasteboardTypeFileURL]);
    send!((), this, b"registerForDraggedTypes:\0", Id = types);
}}

/// File paths in an `NSArray` of `NSURL`.
unsafe fn urls_to_paths(urls: Id) -> Vec<PathBuf> { unsafe {
    let mut out = Vec::new();
    if urls.is_null() {
        return out;
    }
    let n = send!(usize, urls, b"count\0");
    for i in 0..n {
        let url = send!(Id, urls, b"objectAtIndex:\0", usize = i);
        let path = send!(Id, url, b"path\0");
        if path.is_null() {
            continue;
        }
        let utf8 = send!(*const c_char, path, b"UTF8String\0");
        if !utf8.is_null() {
            out.push(PathBuf::from(CStr::from_ptr(utf8).to_string_lossy().into_owned()));
        }
    }
    out
}}

/// The file URLs on a dragging info's pasteboard.
unsafe fn drag_paths(info: Id) -> Vec<PathBuf> { unsafe {
    let pasteboard = send!(Id, info, b"draggingPasteboard\0");
    if pasteboard.is_null() {
        return Vec::new();
    }
    let classes = ns_array(&[class(b"NSURL\0") as Id]);
    let yes = send!(Id, class(b"NSNumber\0") as Id, b"numberWithBool:\0", i8 = 1);
    let options = send!(
        Id,
        class(b"NSDictionary\0") as Id,
        b"dictionaryWithObject:forKey:\0",
        Id = yes,
        Id = NSPasteboardURLReadingFileURLsOnlyKey
    );
    urls_to_paths(send!(Id, pasteboard, b"readObjectsForClasses:options:\0", Id = classes, Id = options))
}}

unsafe fn drag_position(this: Id, info: Id) -> (f32, f32) { unsafe {
    let size = state(this).map_or((1, 1), |st| st.size);
    local_point(this, size, send!(CGPoint, info, b"draggingLocation\0"))
}}

/// NSDragOperationCopy over a place the editor accepts, none elsewhere.
unsafe fn drag_over(this: Id, info: Id) -> usize { unsafe {
    if drag_paths(info).is_empty() {
        return 0;
    }
    let (x, y) = drag_position(this, info);
    let mut ok = false;
    guarded(this, |st| ok = st.editor.accepts_drop(x, y));
    ok as usize
}}

extern "C" fn dragging_entered(this: Id, _cmd: Sel, info: Id) -> usize {
    unsafe { drag_over(this, info) }
}

extern "C" fn dragging_updated(this: Id, _cmd: Sel, info: Id) -> usize {
    unsafe { drag_over(this, info) }
}

extern "C" fn dragging_exited(_this: Id, _cmd: Sel, _info: Id) {}

extern "C" fn prepare_for_drag(this: Id, _cmd: Sel, info: Id) -> i8 {
    unsafe { (drag_over(this, info) != 0) as i8 }
}

extern "C" fn perform_drag(this: Id, _cmd: Sel, info: Id) -> i8 {
    unsafe {
        let paths = drag_paths(info);
        if paths.is_empty() {
            return 0;
        }
        let (x, y) = drag_position(this, info);
        let mut taken = false;
        guarded(this, |st| taken = st.editor.files_dropped(&paths, x, y));
        invalidate(this);
        taken as i8
    }
}

/// The system open panel for a request (not shown).
unsafe fn make_open_panel(req: &FileDialog) -> Id { unsafe {
    let panel = send!(Id, class(b"NSOpenPanel\0") as Id, b"openPanel\0");
    if panel.is_null() {
        return panel;
    }
    send!((), panel, b"setAllowsMultipleSelection:\0", i8 = req.multiple as i8);
    send!((), panel, b"setCanChooseFiles:\0", i8 = 1);
    send!((), panel, b"setCanChooseDirectories:\0", i8 = 0);
    if !req.title.is_empty() {
        send!((), panel, b"setMessage:\0", Id = ns_string(&req.title));
    }
    let types: Vec<Id> = req.extensions.iter().map(|e| e.trim().trim_start_matches('.')).filter(|e| !e.is_empty()).map(|e| ns_string(e)).collect();
    if !types.is_empty() {
        send!((), panel, b"setAllowedFileTypes:\0", Id = ns_array(&types));
    }
    panel
}}

/// The completion block of a panel: a global block (it is never copied or
/// freed by the runtime) that carries the view, the panel and the tag after the
/// header, and frees itself when it has run.
#[repr(C)]
struct PanelBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut PanelBlock, isize),
    descriptor: *const BlockDescriptor,
    view: Id,
    panel: Id,
    tag: u32,
}

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

static PANEL_BLOCK_DESCRIPTOR: BlockDescriptor = BlockDescriptor { reserved: 0, size: std::mem::size_of::<PanelBlock>() };

/// `NSModalResponseOK` is 1; anything else is a cancel.
unsafe extern "C" fn panel_done(block: *mut PanelBlock, response: isize) { unsafe {
    let b = Box::from_raw(block);
    let paths = if response == 1 { Some(urls_to_paths(send!(Id, b.panel, b"URLs\0"))).filter(|p| !p.is_empty()) } else { None };
    // The view may be gone; its state says so.
    if let Some(st) = state(b.view) {
        st.file_results.push((b.tag, paths));
    }
    send!((), b.panel, b"release\0");
    send!((), b.view, b"release\0");
}}

unsafe fn make_panel_block(view: Id, panel: Id, tag: u32) -> *mut PanelBlock { unsafe {
    send!(Id, view, b"retain\0");
    send!(Id, panel, b"retain\0");
    Box::into_raw(Box::new(PanelBlock {
        isa: &raw const _NSConcreteGlobalBlock as *const c_void,
        flags: 1 << 28,
        reserved: 0,
        invoke: panel_done,
        descriptor: &PANEL_BLOCK_DESCRIPTOR,
        view,
        panel,
        tag,
    }))
}}

/// Show the panel as a sheet on the view's window, or on its own without one.
/// The answer is queued and delivered on a later tick.
unsafe fn open_file_dialog(this: Id, req: FileDialog) { unsafe {
    let Some(st) = state(this) else { return };
    if let Some(answer) = st.file_hook.as_mut().and_then(|hook| hook(&req)) {
        st.file_results.push((req.tag, answer));
        return;
    }
    let panel = make_open_panel(&req);
    if panel.is_null() {
        st.file_results.push((req.tag, None));
        return;
    }
    let block = make_panel_block(this, panel, req.tag);
    let window = send!(Id, this, b"window\0");
    if window.is_null() {
        send!((), panel, b"beginWithCompletionHandler:\0", *mut PanelBlock = block);
    } else {
        send!((), panel, b"beginSheetModalForWindow:completionHandler:\0", Id = window, *mut PanelBlock = block);
    }
}}

// ---- Accessibility -------------------------------------------------------

const AX_VIEW_IVAR: &[u8] = b"openpluginAxView\0";
const AX_INDEX_IVAR: &[u8] = b"openpluginAxIndex\0";

unsafe fn ivar(obj: Id, name: &[u8]) -> *mut c_void { unsafe {
    let mut p: *mut c_void = null_mut();
    object_getInstanceVariable(obj, name.as_ptr() as *const c_char, &mut p);
    p
}}

/// The element's view, its node index and the node as the editor describes it now.
unsafe fn ax_node(element: Id) -> Option<(Id, usize, AccessNode)> { unsafe {
    let view = ivar(element, AX_VIEW_IVAR);
    let index = ivar(element, AX_INDEX_IVAR) as usize;
    let st = idle(view)?;
    let node = st.editor.accessibility_nodes().into_iter().nth(index)?;
    Some((view, index, node))
}}

/// The view's elements for the editor's nodes as they are now (as they were
/// last, inside an editor call).
unsafe fn ax_children(this: Id) -> Vec<Id> { unsafe {
    let Some(st) = state(this) else { return Vec::new() };
    if st.busy {
        return st.ax_elements.clone();
    }
    let count = st.editor.accessibility_nodes().len();
    ax_elements(this, count)
}}

/// The view's elements, one per node of `count`, made or dropped to match.
unsafe fn ax_elements(this: Id, count: usize) -> Vec<Id> { unsafe {
    let Some(st) = state(this) else { return Vec::new() };
    while st.ax_elements.len() > count {
        if let Some(old) = st.ax_elements.pop() {
            object_setInstanceVariable(old, AX_VIEW_IVAR.as_ptr() as *const c_char, null_mut());
            send!((), old, b"release\0");
        }
    }
    let cls = element_class();
    while st.ax_elements.len() < count && !cls.is_null() {
        let element = send!(Id, send!(Id, cls as Id, b"alloc\0"), b"init\0");
        if element.is_null() {
            break;
        }
        object_setInstanceVariable(element, AX_VIEW_IVAR.as_ptr() as *const c_char, this);
        object_setInstanceVariable(element, AX_INDEX_IVAR.as_ptr() as *const c_char, st.ax_elements.len() as *mut c_void);
        st.ax_elements.push(element);
    }
    st.ax_elements.clone()
}}

/// Post the notifications for what changed in the nodes since the last pass.
unsafe fn ax_sync(this: Id) { unsafe {
    let Some(st) = state(this) else { return };
    // Elements are made when something asks for the view's children: until then nobody listens.
    if st.ax_elements.is_empty() {
        return;
    }
    let now = st.editor.accessibility_nodes();
    let diff = ax_diff(&st.ax_seen, &now);
    st.ax_seen = now.iter().map(|n| AxSeen { value: n.value.clone(), focused: n.focused }).collect();
    let elements = ax_elements(this, now.len());
    if diff.layout {
        NSAccessibilityPostNotification(this, NSAccessibilityLayoutChangedNotification);
    }
    for i in diff.values {
        if let Some(&e) = elements.get(i) {
            NSAccessibilityPostNotification(e, NSAccessibilityValueChangedNotification);
        }
    }
    if let Some(focus) = diff.focus {
        let target = focus.and_then(|i| elements.get(i).copied()).unwrap_or(this);
        NSAccessibilityPostNotification(target, NSAccessibilityFocusedUIElementChangedNotification);
    }
}}

/// `convertRect:toView:` style calls that return a rectangle.
unsafe fn send_rect_to(obj: Id, name: &[u8], rect: CGRect, other: Option<Id>) -> CGRect { unsafe {
    #[cfg(target_arch = "x86_64")]
    {
        let mut out = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w: 0.0, h: 0.0 } };
        match other {
            Some(o) => {
                let f: unsafe extern "C" fn(*mut CGRect, Id, Sel, CGRect, Id) = std::mem::transmute(objc_msgSend_stret as unsafe extern "C" fn());
                f(&mut out, obj, sel(name), rect, o);
            }
            None => {
                let f: unsafe extern "C" fn(*mut CGRect, Id, Sel, CGRect) = std::mem::transmute(objc_msgSend_stret as unsafe extern "C" fn());
                f(&mut out, obj, sel(name), rect);
            }
        }
        out
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        match other {
            Some(o) => {
                let f: unsafe extern "C" fn(Id, Sel, CGRect, Id) -> CGRect = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                f(obj, sel(name), rect, o)
            }
            None => {
                let f: unsafe extern "C" fn(Id, Sel, CGRect) -> CGRect = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                f(obj, sel(name), rect)
            }
        }
    }
}}

/// An editor rectangle in points as a rectangle of the view (the view may be
/// shown at another size than the editor).
unsafe fn view_space(this: Id, size: (u32, u32), r: &openplugin_core::editor::DamageRect) -> CGRect { unsafe {
    let b = send_rect(this, b"bounds\0");
    let sx = if size.0 > 0 { b.size.w / f64::from(size.0) } else { 1.0 };
    let sy = if size.1 > 0 { b.size.h / f64::from(size.1) } else { 1.0 };
    CGRect {
        origin: CGPoint { x: f64::from(r.x) * sx, y: f64::from(r.y) * sy },
        size: CGSize { w: f64::from(r.w) * sx, h: f64::from(r.h) * sy },
    }
}}

/// A view rectangle in screen coordinates (origin bottom left of the main
/// display), or in window coordinates while the view has no window.
unsafe fn screen_rect(this: Id, rect: CGRect) -> CGRect { unsafe {
    let in_window = send_rect_to(this, b"convertRect:toView:\0", rect, Some(null_mut()));
    let window = send!(Id, this, b"window\0");
    if window.is_null() {
        in_window
    } else {
        send_rect_to(window, b"convertRectToScreen:\0", in_window, None)
    }
}}

fn role_name(role: AccessRole) -> Id {
    unsafe {
        match role {
            AccessRole::Slider => NSAccessibilitySliderRole,
            AccessRole::Switch => NSAccessibilityCheckBoxRole,
            _ => NSAccessibilityButtonRole,
        }
    }
}

extern "C" fn ax_role(this: Id, _cmd: Sel) -> Id {
    unsafe { ax_node(this).map_or(null_mut(), |(_, _, n)| role_name(n.role)) }
}

extern "C" fn ax_label(this: Id, _cmd: Sel) -> Id {
    unsafe { ax_node(this).map_or(null_mut(), |(_, _, n)| ns_string(&n.label)) }
}

/// A slider's value is its text; a switch's is 0 or 1 (what a check box
/// reports); a button has none.
extern "C" fn ax_value(this: Id, _cmd: Sel) -> Id {
    unsafe {
        let Some((_, _, n)) = ax_node(this) else { return null_mut() };
        match n.role {
            AccessRole::Slider => ns_string(&n.value),
            AccessRole::Switch => send!(Id, class(b"NSNumber\0") as Id, b"numberWithInt:\0", i32 = n.on as i32),
            _ => null_mut(),
        }
    }
}

/// The parameter's text for every role ("On", "-6.0 dB"), which is what
/// VoiceOver speaks after the label.
extern "C" fn ax_value_description(this: Id, _cmd: Sel) -> Id {
    unsafe { ax_node(this).map_or(null_mut(), |(_, _, n)| ns_string(&n.value)) }
}

extern "C" fn ax_frame(this: Id, _cmd: Sel) -> CGRect {
    unsafe {
        let none = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w: 0.0, h: 0.0 } };
        let Some((view, _, node)) = ax_node(this) else { return none };
        let Some(st) = state(view) else { return none };
        let rect = view_space(view, st.size, &node.rect);
        screen_rect(view, rect)
    }
}

extern "C" fn ax_parent(this: Id, _cmd: Sel) -> Id {
    unsafe { ivar(this, AX_VIEW_IVAR) }
}

extern "C" fn ax_window(this: Id, _cmd: Sel) -> Id {
    unsafe {
        let view = ivar(this, AX_VIEW_IVAR);
        if view.is_null() { null_mut() } else { send!(Id, view, b"window\0") }
    }
}

extern "C" fn ax_enabled(this: Id, _cmd: Sel) -> i8 {
    unsafe { ax_node(this).is_some_and(|(_, _, n)| n.enabled) as i8 }
}

extern "C" fn ax_focused(this: Id, _cmd: Sel) -> i8 {
    unsafe { ax_node(this).is_some_and(|(_, _, n)| n.focused) as i8 }
}

/// VoiceOver's cursor arrives on the control: the view takes the keyboard (when
/// it can) and the editor moves its focus there.
extern "C" fn ax_set_focused(this: Id, _cmd: Sel, focused: i8) {
    unsafe {
        let Some((view, index, _)) = ax_node(this) else { return };
        if focused == 0 {
            return;
        }
        if idle(view).is_some_and(|st| st.editor.wants_keys()) {
            let window = send!(Id, view, b"window\0");
            if !window.is_null() {
                send!(i8, window, b"makeFirstResponder:\0", Id = view);
            }
        }
        guarded(view, |st| {
            st.editor.accessibility_focus(index);
        });
        invalidate(view);
    }
}

unsafe fn ax_act(element: Id, action: AccessAction) -> i8 { unsafe {
    let Some((view, index, node)) = ax_node(element) else { return 0 };
    if !node.enabled || state(view).is_none_or(|st| st.busy) {
        return 0;
    }
    let mut done = false;
    guarded(view, |st| done = st.editor.accessibility_action(index, action));
    invalidate(view);
    done as i8
}}

extern "C" fn ax_increment(this: Id, _cmd: Sel) -> i8 {
    unsafe { ax_act(this, AccessAction::Increment) }
}

extern "C" fn ax_decrement(this: Id, _cmd: Sel) -> i8 {
    unsafe { ax_act(this, AccessAction::Decrement) }
}

extern "C" fn ax_press(this: Id, _cmd: Sel) -> i8 {
    unsafe { ax_act(this, AccessAction::Press) }
}

unsafe fn ns_array(items: &[Id]) -> Id { unsafe {
    send!(Id, class(b"NSArray\0") as Id, b"arrayWithObjects:count:\0", *const Id = items.as_ptr(), usize = items.len())
}}

/// The view is a group when the editor has nodes; otherwise it is as opaque as before.
extern "C" fn view_is_ax_element(this: Id, _cmd: Sel) -> i8 {
    unsafe { idle(this).is_some_and(|st| !st.editor.accessibility_nodes().is_empty()) as i8 }
}

extern "C" fn view_ax_role(_this: Id, _cmd: Sel) -> Id {
    unsafe { NSAccessibilityGroupRole }
}

extern "C" fn view_ax_children(this: Id, _cmd: Sel) -> Id {
    unsafe { ns_array(&ax_children(this)) }
}

extern "C" fn view_ax_focused_element(this: Id, _cmd: Sel) -> Id {
    unsafe {
        let focus = idle(this).and_then(|st| st.editor.accessibility_nodes().iter().position(|n| n.focused));
        let elements = ax_children(this);
        focus.and_then(|i| elements.get(i).copied()).unwrap_or(this)
    }
}

/// The element under a point in screen coordinates (VoiceOver pointing, touch).
extern "C" fn view_ax_hit_test(this: Id, _cmd: Sel, point: CGPoint) -> Id {
    unsafe {
        let elements = ax_children(this);
        for &e in elements.iter().rev() {
            let f = ax_frame(e, sel(b"accessibilityFrame\0"));
            if point.x >= f.origin.x && point.x < f.origin.x + f.size.w && point.y >= f.origin.y && point.y < f.origin.y + f.size.h {
                return e;
            }
        }
        this
    }
}

unsafe fn element_class() -> Class { unsafe {
    let name = CString::new(format!("OpenPluginAXElement_{:x}", &MARKER as *const u8 as usize)).unwrap_or_default();
    let existing = objc_getClass(name.as_ptr());
    if !existing.is_null() {
        return existing;
    }
    let cls = objc_allocateClassPair(class(b"NSAccessibilityElement\0"), name.as_ptr(), 0);
    if cls.is_null() {
        return null_mut();
    }
    class_addIvar(cls, AX_VIEW_IVAR.as_ptr() as *const c_char, std::mem::size_of::<*mut c_void>(), 3, c"^v".as_ptr());
    class_addIvar(cls, AX_INDEX_IVAR.as_ptr() as *const c_char, std::mem::size_of::<*mut c_void>(), 3, c"^v".as_ptr());
    let add = |name: &[u8], imp: *const c_void, types: &[u8]| {
        class_addMethod(cls, sel(name), imp, types.as_ptr() as *const c_char);
    };
    add(b"accessibilityRole\0", ax_role as *const c_void, b"@@:\0");
    add(b"accessibilityLabel\0", ax_label as *const c_void, b"@@:\0");
    add(b"accessibilityValue\0", ax_value as *const c_void, b"@@:\0");
    add(b"accessibilityValueDescription\0", ax_value_description as *const c_void, b"@@:\0");
    add(b"accessibilityFrame\0", ax_frame as *const c_void, b"{CGRect={CGPoint=dd}{CGSize=dd}}@:\0");
    add(b"accessibilityParent\0", ax_parent as *const c_void, b"@@:\0");
    add(b"accessibilityWindow\0", ax_window as *const c_void, b"@@:\0");
    add(b"accessibilityTopLevelUIElement\0", ax_window as *const c_void, b"@@:\0");
    add(b"isAccessibilityElement\0", yes as *const c_void, b"c@:\0");
    add(b"isAccessibilityEnabled\0", ax_enabled as *const c_void, b"c@:\0");
    add(b"isAccessibilityFocused\0", ax_focused as *const c_void, b"c@:\0");
    add(b"setAccessibilityFocused:\0", ax_set_focused as *const c_void, b"v@:c\0");
    add(b"accessibilityPerformIncrement\0", ax_increment as *const c_void, b"c@:\0");
    add(b"accessibilityPerformDecrement\0", ax_decrement as *const c_void, b"c@:\0");
    add(b"accessibilityPerformPress\0", ax_press as *const c_void, b"c@:\0");
    objc_registerClassPair(cls);
    cls
}}

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
    add(b"rightMouseDown:\0", right_mouse_down as *const c_void, b"v@:@\0");
    add(b"openpluginMenuItem:\0", menu_item as *const c_void, b"v@:@\0");
    add(b"mouseDragged:\0", mouse_moved as *const c_void, b"v@:@\0");
    add(b"mouseMoved:\0", mouse_moved as *const c_void, b"v@:@\0");
    add(b"mouseUp:\0", mouse_up as *const c_void, b"v@:@\0");
    add(b"scrollWheel:\0", scroll_wheel as *const c_void, b"v@:@\0");
    add(b"openpluginTick:\0", tick as *const c_void, b"v@:@\0");
    add(b"view:stringForToolTip:point:userData:\0", tooltip_string as *const c_void, b"@@:@q{CGPoint=dd}^v\0");
    add(b"control:textView:doCommandBySelector:\0", do_command as *const c_void, b"c@:@@:\0");
    add(b"controlTextDidEndEditing:\0", text_ended as *const c_void, b"v@:@\0");
    add(b"openpluginWindowResignedKey:\0", window_resigned_key as *const c_void, b"v@:@\0");
    add(b"viewWillMoveToWindow:\0", view_will_move_to_window as *const c_void, b"v@:@\0");
    add(b"draggingEntered:\0", dragging_entered as *const c_void, b"Q@:@\0");
    add(b"draggingUpdated:\0", dragging_updated as *const c_void, b"Q@:@\0");
    add(b"draggingExited:\0", dragging_exited as *const c_void, b"v@:@\0");
    add(b"prepareForDragOperation:\0", prepare_for_drag as *const c_void, b"c@:@\0");
    add(b"performDragOperation:\0", perform_drag as *const c_void, b"c@:@\0");
    add(b"isAccessibilityElement\0", view_is_ax_element as *const c_void, b"c@:\0");
    add(b"accessibilityRole\0", view_ax_role as *const c_void, b"@@:\0");
    add(b"accessibilityChildren\0", view_ax_children as *const c_void, b"@@:\0");
    add(b"accessibilityFocusedUIElement\0", view_ax_focused_element as *const c_void, b"@@:\0");
    add(b"accessibilityHitTest:\0", view_ax_hit_test as *const c_void, b"@@:{CGPoint=dd}\0");
    objc_registerClassPair(cls);
    cls
}}

/// An editor in an `NSView`. Dropping it stops the timer, removes the view
/// from its parent, and drops the editor. Main thread only.
///
/// ```no_run
/// use openplugin_core::{Category, EditorHost, ParamInfo, PluginInfo, Unit};
/// use openplugin_dri::{cocoa::EditorView, DesignEditor};
///
/// static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];
/// static INFO: PluginInfo =
///     PluginInfo::new("com.example.gain", "Gain", "Example", "1.0.0", Category::Utility, &PARAMS);
/// const DESIGN: &str = "GoodLookinUI design version 1\n\
///     id,parameter,label,style,colour,x,y,width,height,font size\n\
///     gain,gain,Gain,Brit,#CE9435,10,10,70,76,10\n";
///
/// fn open(parent: openplugin_dri::cocoa::NSViewPtr) -> Option<EditorView> {
///     let editor = DesignEditor::from_csv(DESIGN, &INFO, EditorHost::new(|_| {}))?;
///     let view = EditorView::new(Box::new(editor), None)?;
///     // SAFETY: the host gave a live NSView.
///     if !unsafe { view.attach(parent) } { return None; }
///     Some(view)
/// }
/// # let _ = open;
/// ```
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
            let state = Box::into_raw(Box::new(State { editor, poll, size, last: Instant::now(), modifiers: Modifiers::NONE, busy: false, pending: None, repaint: false, closing: false, in_frame: false, scale: 0.0, ax_elements: Vec::new(), ax_seen: Vec::new(), text_field: None, field_cancel: false, opening: false, tip_key: Vec::new(), drags: false, file_results: Vec::new(), file_hook: None, menu_param: 0, menu_hook: None }));
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
            sync_tooltips(view);
            ensure_drag_registration(view);
            Some(EditorView { view, timer, state })
        }
    }

    /// The `NSView`. The view keeps its own reference until drop.
    pub fn raw(&self) -> NSViewPtr {
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
    pub unsafe fn attach(&self, parent: NSViewPtr) -> bool { unsafe {
        if parent.is_null() {
            return false;
        }
        send!((), parent, b"addSubview:\0", Id = self.view);
        true
    }}

    /// Hide or show the view (`-[NSView setHidden:]`). Main thread.
    pub fn set_hidden(&self, hidden: bool) {
        unsafe { send!((), self.view, b"setHidden:\0", i8 = hidden as i8) }
    }

    /// The hosted editor.
    pub fn editor(&mut self) -> Option<&mut (dyn Editor + 'static)> {
        unsafe { self.state.as_mut().map(|s| s.editor.as_mut()) }
    }

    /// The open text field (an `NSTextField`), if any.
    #[doc(hidden)]
    pub fn text_field(&self) -> Option<NSViewPtr> {
        unsafe { self.state.as_ref().and_then(|s| s.text_field) }
    }

    /// The tooltip rectangles in view points (x, y, width, height) with the
    /// text the view answers for each, asked through the same
    /// `view:stringForToolTip:point:userData:` method AppKit calls.
    #[doc(hidden)]
    pub fn tooltip_rects(&self) -> Vec<([f64; 4], String)> {
        unsafe {
            let Some(st) = self.state.as_ref() else { return Vec::new() };
            st.tip_key
                .iter()
                .map(|(i, r)| {
                    let f: unsafe extern "C" fn(Id, Sel, Id, isize, CGPoint, *mut c_void) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                    let s = f(self.view, sel(b"view:stringForToolTip:point:userData:\0"), self.view, 0, CGPoint { x: 0.0, y: 0.0 }, *i as *mut c_void);
                    let text = if s.is_null() { String::new() } else { CStr::from_ptr(send!(*const c_char, s, b"UTF8String\0")).to_string_lossy().into_owned() };
                    (*r, text)
                })
                .collect()
        }
    }

    /// Answer file dialog requests with `hook` instead of showing the system
    /// panel, for tests. See [`FileHook`].
    #[doc(hidden)]
    pub fn set_file_dialog_hook(&self, hook: Option<FileHook>) {
        if let Some(s) = unsafe { self.state.as_mut() } {
            s.file_hook = hook;
        }
    }

    /// The `NSOpenPanel` the view would show for a request, not shown (the
    /// caller owns nothing: it is autoreleased). For tests.
    #[doc(hidden)]
    pub fn open_panel_for(request: &FileDialog) -> NSViewPtr {
        unsafe { make_open_panel(request) }
    }

    /// Run the completion path of a real panel with `response` (1 is OK, 0
    /// cancel) on a panel that was never shown: the same block the system
    /// calls. For tests.
    #[doc(hidden)]
    pub fn simulate_panel_response(&self, request: &FileDialog, response: isize) {
        unsafe {
            let panel = make_open_panel(request);
            let block = make_panel_block(self.view, panel, request.tag);
            ((*block).invoke)(block, response);
        }
    }

    /// Answer the SDK's parameter menu with `hook` instead of popping up an
    /// `NSMenu`, for tests. See [`MenuHook`].
    #[doc(hidden)]
    pub fn set_menu_hook(&self, hook: Option<MenuHook>) {
        if let Some(s) = unsafe { self.state.as_mut() } {
            s.menu_hook = hook;
        }
    }

    /// Choose item `index` of [`PARAM_MENU_ITEMS`] as if from the menu last
    /// shown, with `text` standing in for the clipboard (what paste reads;
    /// copy returns what it would put there). For tests; the menu's own
    /// action, `openpluginMenuItem:`, does the same with the real clipboard.
    #[doc(hidden)]
    pub fn choose_menu_item(&self, index: usize, text: Option<&str>) -> Option<String> {
        unsafe { run_menu_item(self.view, index, text) }
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
            remove_field_hooks(self.view);
            if let Some(field) = self.state.as_mut().and_then(|s| s.text_field.take()) {
                send!((), field, b"setDelegate:\0", Id = null_mut());
                send!((), field, b"removeFromSuperview\0");
                send!(Id, field, b"autorelease\0");
            }
            // Elements the host still holds must not reach the view again.
            if let Some(st) = self.state.as_mut() {
                for e in st.ax_elements.drain(..) {
                    object_setInstanceVariable(e, AX_VIEW_IVAR.as_ptr() as *const c_char, null_mut());
                    send!((), e, b"release\0");
                }
            }
            // Detach the state first: the host may keep the view a while longer.
            object_setInstanceVariable(self.view, IVAR.as_ptr() as *const c_char, null_mut());
            send!((), self.view, b"removeFromSuperview\0");
            // Dropped from a host callback inside an editor call: `guarded` frees both when the call ends.
            if let Some(st) = self.state.as_mut().filter(|st| st.busy) {
                st.closing = true;
                return;
            }
            send!((), self.view, b"release\0");
            if !self.state.is_null() {
                drop(Box::from_raw(self.state));
            }
        }
    }
}

/// Whether the caller is on the process's main thread, where every AppKit call belongs.
pub fn on_main_thread() -> bool {
    unsafe extern "C" {
        fn pthread_main_np() -> i32;
    }
    unsafe { pthread_main_np() == 1 }
}

/// Show the editor in its own window and run the application until the
/// window closes or the user quits. Returns the exit code: 0, or 1 when the view
/// could not be made. Main thread; `poll`, when given, runs before every tick.
///
/// ```no_run
/// use openplugin_core::{Category, EditorHost, ParamInfo, PluginInfo, Unit};
/// use openplugin_dri::DesignEditor;
///
/// static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];
/// static INFO: PluginInfo =
///     PluginInfo::new("com.example.gain", "Gain", "Example", "1.0.0", Category::Utility, &PARAMS);
/// const DESIGN: &str = "GoodLookinUI design version 1\n\
///     id,parameter,label,style,colour,x,y,width,height,font size\n\
///     gain,gain,Gain,Brit,#CE9435,10,10,70,76,10\n";
///
/// let editor = DesignEditor::from_csv(DESIGN, &INFO, EditorHost::new(|_| {})).unwrap();
/// std::process::exit(openplugin_dri::cocoa::run_window("Gain", Box::new(editor), None));
/// ```
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
    fn damage_maps_to_whole_view_points_with_a_margin() {
        use openplugin_core::editor::DamageRect;
        let view = CGSize { w: 240.0, h: 160.0 };
        let r = view_rect(&DamageRect::new(10.5, 20.0, 30.0, 40.25), (240, 160), view).unwrap();
        assert_eq!((r.origin.x, r.origin.y, r.size.w, r.size.h), (9.0, 19.0, 33.0, 43.0));
        // Cut to the view.
        let r = view_rect(&DamageRect::new(-50.0, 150.0, 400.0, 400.0), (240, 160), view).unwrap();
        assert_eq!((r.origin.x, r.origin.y, r.size.w, r.size.h), (0.0, 149.0, 240.0, 11.0));
        // A view shown at twice the editor's size.
        let r = view_rect(&DamageRect::new(10.0, 10.0, 10.0, 10.0), (120, 80), view).unwrap();
        assert_eq!((r.origin.x, r.origin.y, r.size.w, r.size.h), (19.0, 19.0, 22.0, 22.0));
        assert!(view_rect(&DamageRect::new(f32::NAN, 0.0, 1.0, 1.0), (240, 160), view).is_none());
        assert!(view_rect(&DamageRect::new(0.0, 0.0, 1.0, 1.0), (0, 0), view).is_none());
    }

    #[test]
    fn diff_reports_values_focus_and_layout() {
        let node = |value: &str, focused: bool| AccessNode {
            label: "x".into(),
            value: value.into(),
            role: AccessRole::Slider,
            on: false,
            rect: openplugin_core::editor::DamageRect::new(0.0, 0.0, 1.0, 1.0),
            focused,
            enabled: true,
        };
        let seen = |value: &str, focused: bool| AxSeen { value: value.into(), focused };
        let prev = [seen("1", false), seen("2", false)];
        let d = ax_diff(&prev, &[node("1", false), node("2", false)]);
        assert_eq!(d, AxDiff { layout: false, values: vec![], focus: None });
        let d = ax_diff(&prev, &[node("1", false), node("3", true)]);
        assert_eq!(d, AxDiff { layout: false, values: vec![1], focus: Some(Some(1)) });
        let d = ax_diff(&[seen("1", true)], &[node("1", false)]);
        assert_eq!(d.focus, Some(None));
        let d = ax_diff(&[], &[node("1", false)]);
        assert!(d.layout && d.values.is_empty());
    }

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
