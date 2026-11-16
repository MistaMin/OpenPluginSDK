//! The Windows view that hosts an [`Editor`]: a child `HWND` of a window class
//! of its own, a 30 Hz timer, and a GDI blit of the frame the editor renders
//! (the frame is a top-down 32-bit `0x00RRGGBB` bitmap, which is what a
//! `BI_RGB` DIB is). Pure Rust; it links user32, gdi32, kernel32, shell32,
//! comdlg32 and comctl32 through hand-written declarations and nothing else.
//!
//! [`EditorView`] goes inside a host's parent window (CLAP `clap.gui` with the
//! `win32` API, the VST3 and AAX shells through `op_editor_*`).
//! [`TopWindow`] is a window with a title bar that holds the view (a floating
//! CLAP editor), and [`run_window`] runs one as an application (standalone).
//!
//! Window class names are global to the process, and a host may load several
//! plugins built on this SDK. The class names therefore carry the address of
//! a static in this binary, so every loaded binary registers its own classes
//! and never runs another binary's window procedure. The classes are
//! unregistered when the binary's last view goes, so the binary can unload.
//!
//! Size: the editor works in points; the window is `points x scale` pixels.
//! The scale is the one the host gave ([`EditorView::set_scale`], CLAP
//! `set_scale`, VST3 `setContentScaleFactor`) or, when it gave none, the DPI
//! of the window (`GetDpiForWindow / 96`, followed through
//! `WM_DPICHANGED_AFTERPARENT`). [`EditorView::resize`] takes points,
//! [`EditorView::set_pixel_size`] pixels, and a host that sizes the child
//! window itself (`WM_SIZE`) is followed too; all three go to
//! [`Editor::resize`] with a size inside the editor's [`SizeRange`]. Mouse
//! positions are always in editor points. A resize that arrives while the
//! editor is inside one of its own calls waits until that call ends, and a
//! timer tick or a paint that arrives then (a host's modal menu pumps
//! messages) is skipped and made up afterwards.
//!
//! Repaint: after each tick and each input call the view asks the editor what
//! changed ([`Editor::damage`]) and invalidates only those rectangles.
//!
//! Typing, menus, tooltips, files: a text-field request
//! ([`Editor::take_text_edit`]) opens a native `EDIT` control over the control
//! (Return commits, Escape cancels, losing focus commits valid text and drops
//! invalid text); the SDK's parameter menu is a popup menu; tooltips are a
//! `tooltips_class32` control whose text is asked of the editor when a tip
//! shows; a file request is the system's open dialog, answered on a later
//! tick; an editor that [`Editor::wants_drops`] takes files dropped from
//! Explorer.
//!
//! Not here yet: accessibility (UI Automation providers for
//! [`Editor::accessibility_nodes`]).
//!
//! One thread only: the thread that made the view runs its messages.

#![allow(unsafe_code, clippy::missing_safety_doc)]

use openplugin_core::editor::{DamageRect, FileDialog, Key, Modifiers, ParamMenu, ParamMenuAction, SizeRange, TextEdit};
use openplugin_core::Editor;
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::time::Instant;

/// A window handle (`HWND`).
pub type Hwnd = *mut c_void;
type Handle = *mut c_void;
type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

#[repr(C)]
struct WndClassExW {
    size: u32,
    style: u32,
    proc_: Option<WndProc>,
    cls_extra: i32,
    wnd_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
    icon_small: Handle,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct PaintStruct {
    dc: Handle,
    erase: i32,
    paint: Rect,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: Point,
}

#[repr(C)]
struct MinMaxInfo {
    reserved: Point,
    max_size: Point,
    max_position: Point,
    min_track: Point,
    max_track: Point,
}

#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_ppm: i32,
    y_ppm: i32,
    clr_used: u32,
    clr_important: u32,
}

#[repr(C)]
struct NmHdr {
    from: Hwnd,
    id: usize,
    code: u32,
}

#[repr(C)]
struct NmTtDispInfoW {
    hdr: NmHdr,
    text: *mut u16,
    buffer: [u16; 80],
    instance: Handle,
    flags: u32,
    lparam: isize,
}

#[repr(C)]
struct ToolInfoW {
    size: u32,
    flags: u32,
    hwnd: Hwnd,
    id: usize,
    rect: Rect,
    instance: Handle,
    text: *mut u16,
    lparam: isize,
    reserved: *mut c_void,
}

#[repr(C)]
struct InitCommonControlsExInfo {
    size: u32,
    icc: u32,
}

#[repr(C)]
struct OpenFileNameW {
    size: u32,
    owner: Hwnd,
    instance: Handle,
    filter: *const u16,
    custom_filter: *mut u16,
    max_custom_filter: u32,
    filter_index: u32,
    file: *mut u16,
    max_file: u32,
    file_title: *mut u16,
    max_file_title: u32,
    initial_dir: *const u16,
    title: *const u16,
    flags: u32,
    file_offset: u16,
    file_extension: u16,
    def_ext: *const u16,
    cust_data: isize,
    hook: *const c_void,
    template_name: *const u16,
    reserved_ptr: *mut c_void,
    reserved: u32,
    flags_ex: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    fn UnregisterClassW(name: *const u16, instance: Handle) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: Handle,
        instance: Handle,
        param: *mut c_void,
    ) -> Hwnd;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn IsWindow(hwnd: Hwnd) -> i32;
    fn IsWindowVisible(hwnd: Hwnd) -> i32;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize;
    fn CallWindowProcW(prev: isize, hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize;
    #[cfg_attr(target_pointer_width = "32", link_name = "GetWindowLongW")]
    fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    #[cfg_attr(target_pointer_width = "32", link_name = "SetWindowLongW")]
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    fn SetTimer(hwnd: Hwnd, id: usize, ms: u32, func: *const c_void) -> usize;
    fn KillTimer(hwnd: Hwnd, id: usize) -> i32;
    fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: i32) -> i32;
    fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Handle;
    fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn SetWindowPos(hwnd: Hwnd, after: Hwnd, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn SetParent(child: Hwnd, parent: Hwnd) -> Hwnd;
    fn GetParent(hwnd: Hwnd) -> Hwnd;
    fn SetCapture(hwnd: Hwnd) -> Hwnd;
    fn ReleaseCapture() -> i32;
    fn SetFocus(hwnd: Hwnd) -> Hwnd;
    fn GetFocus() -> Hwnd;
    fn GetKeyState(vk: i32) -> i16;
    fn MapVirtualKeyW(code: u32, map: u32) -> u32;
    fn ScreenToClient(hwnd: Hwnd, point: *mut Point) -> i32;
    fn ClientToScreen(hwnd: Hwnd, point: *mut Point) -> i32;
    fn GetDpiForWindow(hwnd: Hwnd) -> u32;
    fn GetDpiForSystem() -> u32;
    fn SetProcessDpiAwarenessContext(context: isize) -> i32;
    fn AdjustWindowRectExForDpi(rect: *mut Rect, style: u32, menu: i32, ex_style: u32, dpi: u32) -> i32;
    fn SendMessageW(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize;
    fn CreatePopupMenu() -> Handle;
    fn AppendMenuW(menu: Handle, flags: u32, id: usize, text: *const u16) -> i32;
    fn TrackPopupMenu(menu: Handle, flags: u32, x: i32, y: i32, reserved: i32, hwnd: Hwnd, rect: *const Rect) -> i32;
    fn DestroyMenu(menu: Handle) -> i32;
    fn OpenClipboard(owner: Hwnd) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(format: u32) -> Handle;
    fn SetClipboardData(format: u32, data: Handle) -> Handle;
    fn MessageBeep(kind: u32) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn PeekMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostQuitMessage(code: i32);
    fn MsgWaitForMultipleObjects(count: u32, handles: *const Handle, all: i32, ms: u32, mask: u32) -> u32;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
    fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
    fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max: i32) -> i32;
    fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
    fn GetDC(hwnd: Hwnd) -> Handle;
    fn ReleaseDC(hwnd: Hwnd, dc: Handle) -> i32;
    fn PrintWindow(hwnd: Hwnd, dc: Handle, flags: u32) -> i32;
    fn PostMessageW(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> i32;
    fn SetCursorPos(x: i32, y: i32) -> i32;
    fn FindWindowExW(parent: Hwnd, after: Hwnd, class: *const u16, title: *const u16) -> Hwnd;
    fn GetWindowThreadProcessId(hwnd: Hwnd, process: *mut u32) -> u32;
    fn GetClassNameW(hwnd: Hwnd, text: *mut u16, max: i32) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    #[allow(clippy::too_many_arguments)]
    fn SetDIBitsToDevice(
        dc: Handle,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        src_x: i32,
        src_y: i32,
        start_scan: u32,
        lines: u32,
        bits: *const c_void,
        info: *const BitmapInfoHeader,
        usage: u32,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn StretchDIBits(
        dc: Handle,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        src_x: i32,
        src_y: i32,
        src_w: i32,
        src_h: i32,
        bits: *const c_void,
        info: *const BitmapInfoHeader,
        usage: u32,
        rop: u32,
    ) -> i32;
    fn SetStretchBltMode(dc: Handle, mode: i32) -> i32;
    fn SetBrushOrgEx(dc: Handle, x: i32, y: i32, old: *mut Point) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike_out: u32,
        charset: u32,
        out_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch_and_family: u32,
        face: *const u16,
    ) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
    fn SetTextColor(dc: Handle, color: u32) -> u32;
    fn SetBkColor(dc: Handle, color: u32) -> u32;
    fn GetStockObject(index: i32) -> Handle;
    fn CreateCompatibleDC(dc: Handle) -> Handle;
    fn DeleteDC(dc: Handle) -> i32;
    fn CreateDIBSection(dc: Handle, info: *const BitmapInfoHeader, usage: u32, bits: *mut *mut c_void, section: Handle, offset: u32) -> Handle;
    fn SelectObject(dc: Handle, object: Handle) -> Handle;
    #[allow(clippy::too_many_arguments)]
    fn BitBlt(dc: Handle, x: i32, y: i32, w: i32, h: i32, src: Handle, src_x: i32, src_y: i32, rop: u32) -> i32;
    fn GdiFlush() -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut Handle) -> i32;
    fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
    fn GlobalLock(mem: Handle) -> *mut c_void;
    fn GlobalUnlock(mem: Handle) -> i32;
    fn GlobalFree(mem: Handle) -> Handle;
    fn GetCurrentProcessId() -> u32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn DragAcceptFiles(hwnd: Hwnd, accept: i32);
    fn DragQueryFileW(drop: Handle, index: u32, file: *mut u16, max: u32) -> u32;
    fn DragQueryPoint(drop: Handle, point: *mut Point) -> i32;
    fn DragFinish(drop: Handle);
}

#[link(name = "comdlg32")]
unsafe extern "system" {
    fn GetOpenFileNameW(ofn: *mut OpenFileNameW) -> i32;
}

#[link(name = "comctl32")]
unsafe extern "system" {
    fn InitCommonControlsEx(info: *const InitCommonControlsExInfo) -> i32;
}

const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_PAINT: u32 = 0x000F;
const WM_CLOSE: u32 = 0x0010;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_GETMINMAXINFO: u32 = 0x0024;
const WM_SETFONT: u32 = 0x0030;
const WM_NOTIFY: u32 = 0x004E;
const WM_NCDESTROY: u32 = 0x0082;
const WM_GETDLGCODE: u32 = 0x0087;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_TIMER: u32 = 0x0113;
const WM_CTLCOLOREDIT: u32 = 0x0133;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_CAPTURECHANGED: u32 = 0x0215;
const WM_DROPFILES: u32 = 0x0233;
const WM_DPICHANGED: u32 = 0x02E0;
const WM_DPICHANGED_AFTERPARENT: u32 = 0x02E3;
const WM_PRINTCLIENT: u32 = 0x0318;
const EM_SETSEL: u32 = 0x00B1;

const GWLP_USERDATA: i32 = -21;
const GWLP_WNDPROC: i32 = -4;
const GWLP_HWNDPARENT: i32 = -8;
const GWL_STYLE: i32 = -16;
const GWL_EXSTYLE: i32 = -20;

const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CLIPCHILDREN: u32 = 0x0200_0000;
const WS_POPUP: u32 = 0x8000_0000;
const WS_BORDER: u32 = 0x0080_0000;
const WS_CAPTION: u32 = 0x00C0_0000;
const WS_SYSMENU: u32 = 0x0008_0000;
const WS_THICKFRAME: u32 = 0x0004_0000;
const WS_MINIMIZEBOX: u32 = 0x0002_0000;
const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
const ES_CENTER: u32 = 0x0001;
const ES_AUTOHSCROLL: u32 = 0x0080;
const CS_DBLCLKS: u32 = 0x0008;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const CW_USEDEFAULT: i32 = i32::MIN;
const TIMER_ID: usize = 0x4f50;
const DLGC_WANTALLKEYS: isize = 0x0004;
const CF_UNICODETEXT: u32 = 13;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().filter(|c| *c != 0).chain(std::iter::once(0)).collect()
}

fn from_wide(text: &[u16]) -> String {
    let end = text.iter().position(|c| *c == 0).unwrap_or(text.len());
    String::from_utf16_lossy(&text[..end])
}

/// Its address names this binary: in the class names, and to find the module.
static ANCHOR: u8 = 0;

fn anchor() -> usize {
    &ANCHOR as *const u8 as usize
}

fn module() -> Handle {
    let mut handle = null_mut();
    // FROM_ADDRESS | UNCHANGED_REFCOUNT
    unsafe { GetModuleHandleExW(0x4 | 0x2, anchor() as *const u16, &mut handle) };
    handle
}

fn view_class() -> Vec<u16> {
    wide(&format!("OpenPluginEditor_{:x}", anchor()))
}

fn top_class() -> Vec<u16> {
    wide(&format!("OpenPluginWindow_{:x}", anchor()))
}

/// Views and top windows alive in this binary; the classes live while it is not zero.
static USERS: AtomicUsize = AtomicUsize::new(0);

fn acquire_classes() {
    if USERS.fetch_add(1, Ordering::SeqCst) != 0 {
        return;
    }
    unsafe {
        let cursor = LoadCursorW(null_mut(), 32512 as *const u16);
        for (name, proc_, style) in [(view_class(), view_proc as WndProc, CS_DBLCLKS), (top_class(), top_proc as WndProc, 0)] {
            let class = WndClassExW {
                size: std::mem::size_of::<WndClassExW>() as u32,
                style,
                proc_: Some(proc_),
                cls_extra: 0,
                wnd_extra: 0,
                instance: module(),
                icon: null_mut(),
                cursor,
                background: null_mut(),
                menu_name: null(),
                class_name: name.as_ptr(),
                icon_small: null_mut(),
            };
            // Fails only if the class is still there from before, which is as good.
            RegisterClassExW(&class);
        }
    }
}

fn release_classes() {
    if USERS.fetch_sub(1, Ordering::SeqCst) != 1 {
        return;
    }
    unsafe {
        UnregisterClassW(view_class().as_ptr(), module());
        UnregisterClassW(top_class().as_ptr(), module());
    }
}

/// Called before every tick with the editor, so a format bridge can pass it
/// the values that changed in the host since the last tick.
pub type Poll = Box<dyn FnMut(&mut dyn Editor)>;

/// The items of the SDK's own parameter menu, in order: the item chosen is an
/// index into this list.
pub const PARAM_MENU_ITEMS: [&str; 4] = ["Reset to default", "Copy value", "Paste value", "Type value"];

/// Stands in for the system menu (tests): returning true means the menu was
/// handled (nothing is shown); false lets the real popup menu open.
pub type MenuHook = Box<dyn FnMut(&ParamMenu) -> bool>;

/// Stands in for the system file dialog (tests): `Some(answer)` resolves the
/// request, `None` lets the real dialog open.
pub type FileHook = Box<dyn FnMut(&FileDialog) -> Option<Option<Vec<PathBuf>>>>;

struct State {
    /// The child window; null before [`EditorView::attach`] and after the
    /// host destroyed the parent.
    hwnd: Hwnd,
    editor: Box<dyn Editor>,
    poll: Option<Poll>,
    /// The editor's size in points, which mouse positions are mapped to.
    size: (u32, u32),
    last: Instant,
    modifiers: Modifiers,
    /// The editor is inside one of its calls.
    busy: bool,
    /// A resize that came in while `busy`: width, height (points), and whether
    /// the window must follow (it must not when the host sized the window).
    pending: Option<(u32, u32, bool)>,
    /// A paint or an invalidation that came in while `busy`.
    repaint: bool,
    /// The view is changing its own window size.
    in_frame: bool,
    /// The scale the host set, if it set one.
    host_scale: Option<f32>,
    /// Pixels per point in use.
    scale: f32,
    hidden: bool,
    /// The left button went down here and has the mouse captured.
    down: bool,
    /// The last mouse position, in points.
    mouse: (f32, f32),
    /// The open text field (an `EDIT` control).
    text_field: Option<Hwnd>,
    field_font: Handle,
    /// The field's text was refused: it is drawn in red.
    field_invalid: bool,
    /// The field must close (the editor's size changed under it); done outside the editor call.
    field_cancel: bool,
    /// The field is being opened: focus moves, which is not the user leaving it.
    opening: bool,
    /// The tooltip control, made when the editor first has tooltips.
    tip: Hwnd,
    /// The tooltip rectangles registered, by tooltip index, in pixels.
    tip_key: Vec<(usize, Rect)>,
    /// The text of the tip showing; the control reads it after the callback returns.
    tip_text: Vec<u16>,
    /// The window accepts dropped files.
    drags: bool,
    /// Answers to file dialogs, delivered on the next tick.
    file_results: Vec<(u32, Option<Vec<PathBuf>>)>,
    file_hook: Option<FileHook>,
    /// The parameter the SDK's menu was last shown for.
    menu_param: u32,
    menu_hook: Option<MenuHook>,
    /// A [`TopWindow`] that keeps its client area the size of the view.
    top: Hwnd,
}

unsafe fn state_of(hwnd: Hwnd) -> *mut State {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State }
}

/// The view `hwnd` is still the window of state `sp` (a modal loop may have
/// run in which the host closed the editor).
unsafe fn alive(hwnd: Hwnd, sp: *mut State) -> bool {
    unsafe { !hwnd.is_null() && IsWindow(hwnd) != 0 && state_of(hwnd) == sp }
}

fn to_pixels(points: (u32, u32), scale: f32) -> (u32, u32) {
    let px = |v: u32| ((v as f32 * scale).round() as u32).max(1);
    (px(points.0), px(points.1))
}

fn to_points(pixels: (u32, u32), scale: f32) -> (u32, u32) {
    let pt = |v: u32| ((v as f32 / scale).round() as u32).max(1);
    (pt(pixels.0), pt(pixels.1))
}

fn valid_scale(scale: f32) -> Option<f32> {
    (scale.is_finite() && scale > 0.0).then(|| scale.clamp(0.25, 8.0))
}

unsafe fn window_scale(hwnd: Hwnd) -> f32 {
    let dpi = unsafe { if hwnd.is_null() { GetDpiForSystem() } else { GetDpiForWindow(hwnd) } };
    if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }
}

unsafe fn client_size(hwnd: Hwnd) -> (i32, i32) {
    let mut rc = Rect::default();
    unsafe { GetClientRect(hwnd, &mut rc) };
    (rc.right - rc.left, rc.bottom - rc.top)
}

/// Run an editor call. A resize that arrived meanwhile is applied after it.
/// False, and no call, when the editor is already inside a call.
unsafe fn guarded(sp: *mut State, call: impl FnOnce(&mut State)) -> bool {
    unsafe {
        let st = &mut *sp;
        if st.busy {
            return false;
        }
        st.busy = true;
        call(st);
        st.busy = false;
        if let Some((w, h, frame)) = st.pending.take() {
            apply(st, w, h, frame);
        }
        if std::mem::take(&mut st.repaint) {
            redraw(st.hwnd);
        }
        true
    }
}

/// Give the editor a size inside its range, and follow the size it ends up with.
unsafe fn apply(st: &mut State, width: u32, height: u32, frame: bool) {
    let (w, h) = st.editor.size_range().clamp(width, height);
    st.editor.resize(w, h);
    unsafe { follow(st, frame) };
}

/// Take the editor's size if it changed, and make the window that size.
unsafe fn follow(st: &mut State, frame: bool) {
    let now = st.editor.size();
    if now == st.size || now.0 == 0 || now.1 == 0 {
        return;
    }
    st.size = now;
    if st.text_field.is_some() {
        st.field_cancel = true;
    }
    unsafe {
        if frame {
            set_window_pixels(st);
        }
        redraw(st.hwnd);
    }
}

/// Make the window `points x scale` pixels, and a [`TopWindow`] around it fit.
unsafe fn set_window_pixels(st: &mut State) {
    if st.hwnd.is_null() {
        return;
    }
    let (w, h) = to_pixels(st.size, st.scale);
    unsafe {
        st.in_frame = true;
        SetWindowPos(st.hwnd, null_mut(), 0, 0, w as i32, h as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        st.in_frame = false;
        if !st.top.is_null() && IsWindow(st.top) != 0 {
            fit_top(st.top, (w as i32, h as i32));
        }
    }
}

/// A size in points from outside: now, or after the editor's current call.
unsafe fn resize_to(sp: *mut State, width: u32, height: u32, frame: bool) {
    unsafe {
        let st = &mut *sp;
        if st.busy {
            st.pending = Some((width, height, frame));
        } else {
            apply(st, width, height, frame);
        }
    }
}

/// The scale changed (the host said so, or the window is on another display).
unsafe fn rescale(sp: *mut State) {
    unsafe {
        let st = &mut *sp;
        let now = match st.host_scale {
            Some(s) => s,
            None => window_scale(st.hwnd),
        };
        if now == st.scale {
            return;
        }
        st.scale = now;
        if st.text_field.is_some() {
            st.field_cancel = true;
        }
        st.tip_key.clear();
        set_window_pixels(st);
        redraw(st.hwnd);
    }
}

unsafe fn redraw(hwnd: Hwnd) {
    // A null window would mean every window on the desktop.
    if !hwnd.is_null() {
        unsafe { InvalidateRect(hwnd, null(), 0) };
    }
}

/// Draw the editor's frame over the client area of the window, into `target`
/// or, when that is null, into the window's paint context.
unsafe fn paint(sp: *mut State, target: Handle) {
    unsafe {
        let hwnd = (*sp).hwnd;
        let mut ps: PaintStruct = std::mem::zeroed();
        let dc = if target.is_null() { BeginPaint(hwnd, &mut ps) } else { target };
        if !dc.is_null() {
            let (cw, ch) = client_size(hwnd);
            let drawn = guarded(sp, |st| {
                if cw <= 0 || ch <= 0 || st.size.0 == 0 {
                    return;
                }
                // The scale that makes the frame exactly the client area, so it is not stretched.
                let scale = (cw as f32 / st.size.0 as f32).clamp(0.25, 8.0);
                let frame = st.editor.render(scale);
                if frame.width == 0 || frame.height == 0 || frame.pixels.len() < frame.width * frame.height {
                    return;
                }
                let info = bitmap_info(frame.width as i32, frame.height as i32);
                let bits = frame.pixels.as_ptr() as *const c_void;
                if frame.width as i32 == cw && frame.height as i32 == ch {
                    SetDIBitsToDevice(dc, 0, 0, cw as u32, ch as u32, 0, 0, 0, ch as u32, bits, &info, 0);
                } else {
                    SetStretchBltMode(dc, 4); // HALFTONE
                    SetBrushOrgEx(dc, 0, 0, null_mut());
                    StretchDIBits(dc, 0, 0, cw, ch, 0, 0, frame.width as i32, frame.height as i32, bits, &info, 0, 0x00CC_0020);
                }
            });
            if !drawn {
                (*sp).repaint = true;
            }
        }
        if target.is_null() {
            EndPaint(hwnd, &ps);
        }
    }
}

/// A top-down 32-bit `BI_RGB` bitmap: each pixel a `0x00RRGGBB` word.
fn bitmap_info(width: i32, height: i32) -> BitmapInfoHeader {
    BitmapInfoHeader {
        size: std::mem::size_of::<BitmapInfoHeader>() as u32,
        width,
        height: -height,
        planes: 1,
        bit_count: 32,
        compression: 0,
        size_image: 0,
        x_ppm: 0,
        y_ppm: 0,
        clr_used: 0,
        clr_important: 0,
    }
}

fn lparam_point(l: isize) -> (i32, i32) {
    (i32::from(l as u16 as i16), i32::from((l >> 16) as u16 as i16))
}

/// A client position in pixels as editor points (a host may show the view at another size).
fn client_to_points(p: (i32, i32), client: (i32, i32), size: (u32, u32)) -> (f32, f32) {
    let sx = if client.0 > 0 { size.0 as f32 / client.0 as f32 } else { 1.0 };
    let sy = if client.1 > 0 { size.1 as f32 / client.1 as f32 } else { 1.0 };
    (p.0 as f32 * sx, p.1 as f32 * sy)
}

unsafe fn mouse_point(st: &State, l: isize) -> (f32, f32) {
    client_to_points(lparam_point(l), unsafe { client_size(st.hwnd) }, st.size)
}

/// The window rectangle in pixels that shows an editor rectangle in points.
unsafe fn view_space(st: &State, r: &DamageRect) -> Rect {
    let (cw, ch) = unsafe { client_size(st.hwnd) };
    let sx = if st.size.0 > 0 { cw as f32 / st.size.0 as f32 } else { 1.0 };
    let sy = if st.size.1 > 0 { ch as f32 / st.size.1 as f32 } else { 1.0 };
    Rect {
        left: (r.x * sx).round() as i32,
        top: (r.y * sy).round() as i32,
        right: ((r.x + r.w) * sx).round() as i32,
        bottom: ((r.y + r.h) * sy).round() as i32,
    }
}

/// The pixel rectangle (a pixel more on every side for anti-aliasing and rounding) that
/// shows an editor rectangle, or `None` when it is not finite.
fn pixel_rect(r: &DamageRect, editor: (u32, u32), client: (i32, i32)) -> Option<Rect> {
    if !r.is_finite() || editor.0 == 0 || editor.1 == 0 {
        return None;
    }
    let (cw, ch) = (f64::from(client.0), f64::from(client.1));
    let (sx, sy) = (cw / f64::from(editor.0), ch / f64::from(editor.1));
    let left = ((f64::from(r.x) * sx).floor() - 1.0).clamp(0.0, cw);
    let top = ((f64::from(r.y) * sy).floor() - 1.0).clamp(0.0, ch);
    let right = (((f64::from(r.x) + f64::from(r.w)) * sx).ceil() + 1.0).clamp(0.0, cw);
    let bottom = (((f64::from(r.y) + f64::from(r.h)) * sy).ceil() + 1.0).clamp(0.0, ch);
    Some(Rect { left: left as i32, top: top as i32, right: right.max(left) as i32, bottom: bottom.max(top) as i32 })
}

fn modifiers_from_keys(down: impl Fn(i32) -> bool) -> Modifiers {
    Modifiers { shift: down(0x10), ctrl: down(0x11), alt: down(0x12), command: down(0x5B) || down(0x5C) }
}

/// Tell the editor about the modifier keys when they differ from the last
/// ones it heard. Returns the current modifiers.
fn sync_modifiers(st: &mut State) -> Modifiers {
    let now = modifiers_from_keys(|vk| unsafe { GetKeyState(vk) } < 0);
    if now != st.modifiers {
        st.modifiers = now;
        st.editor.modifiers_changed(now);
    }
    now
}

/// A key from a virtual-key code or, failing that, the character the key
/// types without modifiers (`MapVirtualKeyW`), lower case.
fn key_from_vk(vk: u32, typed: Option<char>) -> Option<Key> {
    Some(match vk {
        0x25 => Key::Left,
        0x27 => Key::Right,
        0x28 => Key::Down,
        0x26 => Key::Up,
        0x21 => Key::PageUp,
        0x22 => Key::PageDown,
        0x24 => Key::Home,
        0x23 => Key::End,
        0x09 => Key::Tab,
        0x0D => Key::Return,
        0x1B => Key::Escape,
        0x2E => Key::Delete,
        0x08 => Key::Backspace,
        0x20 => Key::Space,
        _ => {
            let c = typed?;
            if c.is_control() {
                return None;
            }
            Key::Char(c.to_lowercase().next().unwrap_or(c))
        }
    })
}

fn typed_char(vk: u32) -> Option<char> {
    // MAPVK_VK_TO_CHAR; the top bit marks a dead key.
    let code = unsafe { MapVirtualKeyW(vk, 2) } & 0x7fff_ffff;
    if code == 0 { None } else { char::from_u32(code) }
}

unsafe fn key_event(sp: *mut State, msg: u32, w: usize, l: isize, down: bool) -> isize {
    unsafe {
        let hwnd = (*sp).hwnd;
        // While the text field is open, nothing typed reaches the editor or the host.
        if (*sp).text_field.is_some() {
            return 0;
        }
        let vk = w as u32;
        let key = key_from_vk(vk, typed_char(vk));
        let mut handled = false;
        guarded(sp, |st| {
            let mods = sync_modifiers(st);
            if let Some(key) = key.filter(|_| st.editor.wants_keys()) {
                handled = if down { st.editor.key_down(key, mods) } else { st.editor.key_up(key, mods) };
            }
        });
        if handled {
            invalidate(sp);
            return 0;
        }
        // A key the editor did not use goes to the parent, so the host can keep its
        // transport keys; the system keys (Alt, F10) keep their default handling.
        let parent = GetParent(hwnd);
        if (msg == WM_KEYDOWN || msg == WM_KEYUP) && !parent.is_null() {
            SendMessageW(parent, msg, w, l)
        } else {
            DefWindowProcW(hwnd, msg, w, l)
        }
    }
}

/// Ask the editor what changed and invalidate that: nothing, some rectangles, or the whole view.
unsafe fn invalidate(sp: *mut State) {
    unsafe {
        let hwnd = (*sp).hwnd;
        if hwnd.is_null() {
            return;
        }
        poll_requests(sp);
        // A menu or a dialog ran its own message loop: the editor may be gone.
        if !alive(hwnd, sp) {
            return;
        }
        sync_tooltips(sp);
        let mut damage = None;
        let mut size = (0, 0);
        if !guarded(sp, |st| {
            damage = st.editor.damage();
            size = st.size;
        }) {
            (*sp).repaint = true;
            return;
        }
        let Some(rects) = damage else { return redraw(hwnd) };
        if rects.is_empty() {
            return;
        }
        let client = client_size(hwnd);
        let mut out = Vec::with_capacity(rects.len());
        for r in &rects {
            match pixel_rect(r, size, client) {
                Some(rect) => out.push(rect),
                None => return redraw(hwnd),
            }
        }
        for rect in out {
            InvalidateRect(hwnd, &rect, 0);
        }
    }
}

unsafe fn tick(sp: *mut State) {
    unsafe {
        let hwnd = (*sp).hwnd;
        let ran = guarded(sp, |st| {
            if let Some(poll) = st.poll.as_mut() {
                poll(st.editor.as_mut());
            }
            let now = Instant::now();
            let dt = now.duration_since(st.last).as_secs_f32().min(0.1);
            st.last = now;
            st.editor.tick(dt);
            // An editor may change its own size (a poll hook resized it).
            follow(st, true);
            // Answers to file dialogs arrive here, never inside the call that asked.
            for (tag, paths) in std::mem::take(&mut st.file_results) {
                st.editor.file_dialog_done(tag, paths);
            }
        });
        if !ran {
            return;
        }
        // A window moved to a display of another scale, in a host that sent no message.
        if (*sp).host_scale.is_none() && window_scale(hwnd) != (*sp).scale {
            rescale(sp);
        }
        sync_drops(sp);
        invalidate(sp);
    }
}

unsafe fn sync_drops(sp: *mut State) {
    unsafe {
        let st = &mut *sp;
        if st.busy || st.hwnd.is_null() {
            return;
        }
        let want = st.editor.wants_drops();
        if want != st.drags {
            DragAcceptFiles(st.hwnd, i32::from(want));
            st.drags = want;
        }
    }
}

unsafe fn files_dropped(sp: *mut State, drop: Handle) {
    unsafe {
        let count = DragQueryFileW(drop, u32::MAX, null_mut(), 0);
        let mut paths = Vec::with_capacity(count as usize);
        for i in 0..count {
            let len = DragQueryFileW(drop, i, null_mut(), 0) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(drop, i, buf.as_mut_ptr(), buf.len() as u32);
            paths.push(PathBuf::from(from_wide(&buf)));
        }
        let mut pt = Point::default();
        DragQueryPoint(drop, &mut pt);
        DragFinish(drop);
        guarded(sp, |st| {
            let (x, y) = client_to_points((pt.x, pt.y), client_size(st.hwnd), st.size);
            if !paths.is_empty() && st.editor.accepts_drop(x, y) {
                st.editor.files_dropped(&paths, x, y);
            }
        });
        invalidate(sp);
    }
}

// ---- Typing a value, menus, tooltips, files --------------------------------

/// Requests the editor made during the last call: a menu, a text field, a
/// file dialog. Never called inside an editor call.
unsafe fn poll_requests(sp: *mut State) {
    unsafe {
        let hwnd = (*sp).hwnd;
        if std::mem::take(&mut (*sp).field_cancel) {
            close_text_field(sp, true);
        }
        let mut edit = None;
        let mut dialog = None;
        let mut menu = None;
        guarded(sp, |st| {
            edit = st.editor.take_text_edit();
            dialog = st.editor.take_file_dialog();
            menu = st.editor.take_param_menu();
        });
        if let Some(menu) = menu {
            show_param_menu(sp, menu);
            if !alive(hwnd, sp) {
                return;
            }
        }
        if let Some(edit) = edit {
            open_text_field(sp, edit);
        }
        if let Some(dialog) = dialog {
            open_file_dialog(sp, dialog);
        }
    }
}

/// Show the SDK's own parameter menu at the click, or hand it to the test hook.
unsafe fn show_param_menu(sp: *mut State, menu: ParamMenu) {
    unsafe {
        let st = &mut *sp;
        let hwnd = st.hwnd;
        st.menu_param = menu.param;
        if let Some(hook) = st.menu_hook.as_mut() {
            if hook(&menu) {
                return;
            }
        }
        let popup = CreatePopupMenu();
        if popup.is_null() {
            return;
        }
        for (i, title) in PARAM_MENU_ITEMS.iter().enumerate() {
            AppendMenuW(popup, 0, i + 1, wide(title).as_ptr());
        }
        let at = view_space(st, &DamageRect::new(menu.x, menu.y, 0.0, 0.0));
        let mut pt = Point { x: at.left, y: at.top };
        ClientToScreen(hwnd, &mut pt);
        // TPM_RETURNCMD | TPM_RIGHTBUTTON: blocks in the menu's own loop and returns the item.
        let chosen = TrackPopupMenu(popup, 0x0100 | 0x0002, pt.x, pt.y, 0, hwnd, null());
        DestroyMenu(popup);
        if chosen > 0 && alive(hwnd, sp) {
            run_menu_item(sp, chosen as usize - 1, None);
        }
    }
}

unsafe fn clipboard_text(owner: Hwnd) -> Option<String> {
    unsafe {
        if OpenClipboard(owner) == 0 {
            return None;
        }
        let mut out = None;
        let data = GetClipboardData(CF_UNICODETEXT);
        if !data.is_null() {
            let p = GlobalLock(data) as *const u16;
            if !p.is_null() {
                let mut len = 0;
                while *p.add(len) != 0 {
                    len += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
                GlobalUnlock(data);
            }
        }
        CloseClipboard();
        out
    }
}

unsafe fn set_clipboard_text(owner: Hwnd, text: &str) {
    unsafe {
        let w = wide(text);
        if OpenClipboard(owner) == 0 {
            return;
        }
        EmptyClipboard();
        // GMEM_MOVEABLE
        let mem = GlobalAlloc(0x0002, w.len() * 2);
        if !mem.is_null() {
            let p = GlobalLock(mem) as *mut u16;
            if p.is_null() {
                GlobalFree(mem);
            } else {
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                GlobalUnlock(mem);
                // The clipboard owns the memory once it takes it.
                if SetClipboardData(CF_UNICODETEXT, mem).is_null() {
                    GlobalFree(mem);
                }
            }
        }
        CloseClipboard();
    }
}

/// Run item `index` of [`PARAM_MENU_ITEMS`] for the parameter the menu was
/// shown for. Paste reads the clipboard and copy writes it, unless `text` is
/// given (tests), which stands in for the clipboard both ways.
unsafe fn run_menu_item(sp: *mut State, index: usize, text: Option<&str>) -> Option<String> {
    unsafe {
        let param = (*sp).menu_param;
        let hwnd = (*sp).hwnd;
        let action = match index {
            0 => ParamMenuAction::Reset,
            1 => ParamMenuAction::Copy,
            2 => ParamMenuAction::Paste(match text {
                Some(t) => t.to_string(),
                None => clipboard_text(hwnd).unwrap_or_default(),
            }),
            3 => ParamMenuAction::Type,
            _ => return None,
        };
        let mut copied = None;
        guarded(sp, |st| copied = st.editor.param_menu_action(param, action));
        if let (Some(t), None) = (copied.as_deref(), text) {
            set_clipboard_text(hwnd, t);
        }
        invalidate(sp);
        copied
    }
}

/// The `EDIT` class's own window procedure, read when a field is first subclassed.
static EDIT_PROC: AtomicIsize = AtomicIsize::new(0);

/// Open the small native field over the control, with its text selected.
unsafe fn open_text_field(sp: *mut State, edit: TextEdit) {
    unsafe {
        close_text_field(sp, true);
        let st = &mut *sp;
        if st.hwnd.is_null() {
            return;
        }
        let r = view_space(st, &edit.rect);
        let scale = st.scale.max(0.25);
        let w = (r.right - r.left).max((72.0 * scale) as i32);
        let h = (22.0 * scale) as i32;
        let x = r.left + ((r.right - r.left) - w) / 2;
        let y = r.top + ((r.bottom - r.top) - h) / 2;
        let field = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            wide(&edit.text).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_BORDER | ES_CENTER | ES_AUTOHSCROLL,
            x,
            y,
            w,
            h,
            st.hwnd,
            null_mut(),
            null_mut(),
            null_mut(),
        );
        if field.is_null() {
            guarded(sp, |st| st.editor.text_edit_cancel());
            return;
        }
        // DEFAULT_CHARSET, CLEARTYPE_QUALITY.
        st.field_font = CreateFontW(-((12.0 * scale).round() as i32), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, wide("Segoe UI").as_ptr());
        if !st.field_font.is_null() {
            SendMessageW(field, WM_SETFONT, st.field_font as usize, 1);
        }
        let old = SetWindowLongPtrW(field, GWLP_WNDPROC, field_proc as WndProc as usize as isize);
        EDIT_PROC.store(old, Ordering::SeqCst);
        st.text_field = Some(field);
        st.field_invalid = false;
        st.opening = true;
        SetFocus(field);
        SendMessageW(field, EM_SETSEL, 0, -1);
        if alive_field(sp, field) {
            (*sp).opening = false;
        }
    }
}

unsafe fn alive_field(sp: *mut State, field: Hwnd) -> bool {
    unsafe { IsWindow(field) != 0 && state_of(GetParent(field)) == sp && (*sp).text_field == Some(field) }
}

/// Take the field away. `cancel` tells the editor nothing was committed.
unsafe fn close_text_field(sp: *mut State, cancel: bool) {
    unsafe {
        let st = &mut *sp;
        let Some(field) = st.text_field.take() else { return };
        let font = std::mem::replace(&mut st.field_font, null_mut());
        st.field_invalid = false;
        if cancel {
            guarded(sp, |st| st.editor.text_edit_cancel());
        }
        let hwnd = (*sp).hwnd;
        if GetFocus() == field && !hwnd.is_null() {
            SetFocus(hwnd);
        }
        DestroyWindow(field);
        if !font.is_null() {
            DeleteObject(font);
        }
        invalidate(sp);
    }
}

unsafe fn field_text(field: Hwnd) -> String {
    unsafe {
        let len = GetWindowTextLengthW(field).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        GetWindowTextW(field, buf.as_mut_ptr(), buf.len() as i32);
        from_wide(&buf)
    }
}

/// Hand the field's text to the editor. Accepted: the field closes. Refused: it stays, turns
/// red and the system beeps. False when no field is open.
unsafe fn commit_text_field(sp: *mut State) -> bool {
    unsafe {
        let Some(field) = (*sp).text_field else { return false };
        let text = field_text(field);
        let mut done = false;
        guarded(sp, |st| done = st.editor.text_edit_commit(&text));
        if done {
            close_text_field(sp, false);
        } else {
            (*sp).field_invalid = true;
            InvalidateRect(field, null(), 1);
            MessageBeep(0);
        }
        true
    }
}

/// Focus left the field: valid text is committed, invalid text is discarded without a sound.
unsafe fn field_lost_focus(sp: *mut State) {
    unsafe {
        let Some(field) = (*sp).text_field else { return };
        let text = field_text(field);
        let mut done = false;
        guarded(sp, |st| done = st.editor.text_edit_commit(&text));
        // A refused commit leaves the edit open at the editor: cancel closes it.
        close_text_field(sp, !done);
    }
}

/// The field's window procedure: Return commits through the editor, Escape
/// cancels, losing focus commits; everything else is the `EDIT` control's own.
unsafe extern "system" fn field_proc(field: Hwnd, msg: u32, w: usize, l: isize) -> isize {
    unsafe {
        let old = EDIT_PROC.load(Ordering::SeqCst);
        let own = |msg, w, l| if old != 0 { CallWindowProcW(old, field, msg, w, l) } else { DefWindowProcW(field, msg, w, l) };
        let sp = state_of(GetParent(field));
        if sp.is_null() || (*sp).text_field != Some(field) {
            return own(msg, w, l);
        }
        match msg {
            WM_GETDLGCODE => own(msg, w, l) | DLGC_WANTALLKEYS,
            WM_KEYDOWN if w == 0x0D => {
                commit_text_field(sp);
                0
            }
            WM_KEYDOWN if w == 0x1B => {
                close_text_field(sp, true);
                0
            }
            // Return, Escape and Tab type nothing (the control would beep).
            WM_CHAR if w == 0x0D || w == 0x1B || w == 0x09 => 0,
            WM_KILLFOCUS => {
                let r = own(msg, w, l);
                if alive_field(sp, field) && !(*sp).opening {
                    field_lost_focus(sp);
                }
                r
            }
            _ => own(msg, w, l),
        }
    }
}

const TTM_ADDTOOLW: u32 = 0x0400 + 50;
const TTM_DELTOOLW: u32 = 0x0400 + 51;
const TTM_SETMAXTIPWIDTH: u32 = 0x0400 + 24;
const TTN_GETDISPINFOW: u32 = (-530i32) as u32;

fn tool_info(hwnd: Hwnd, id: usize, rect: Rect) -> ToolInfoW {
    ToolInfoW {
        // Without the last field: the size every version of the control takes.
        size: (std::mem::size_of::<ToolInfoW>() - std::mem::size_of::<*mut c_void>()) as u32,
        flags: 0x0010, // TTF_SUBCLASS
        hwnd,
        id,
        rect,
        instance: null_mut(),
        text: usize::MAX as *mut u16, // LPSTR_TEXTCALLBACK
        lparam: 0,
        reserved: null_mut(),
    }
}

/// Register one tooltip rectangle per [`Editor::tooltips`] entry when the
/// layout moved. The text is asked again when a tip shows.
unsafe fn sync_tooltips(sp: *mut State) {
    unsafe {
        let st = &mut *sp;
        if st.busy || st.hwnd.is_null() {
            return;
        }
        let key: Vec<(usize, Rect)> = st
            .editor
            .tooltips()
            .iter()
            .enumerate()
            .filter(|(_, t)| t.rect.is_finite() && !t.rect.is_empty())
            .map(|(i, t)| (i, view_space(st, &t.rect)))
            .collect();
        if key == st.tip_key {
            return;
        }
        if st.tip.is_null() {
            if key.is_empty() {
                return;
            }
            // ICC_BAR_CLASSES: the tooltip class.
            InitCommonControlsEx(&InitCommonControlsExInfo { size: 8, icc: 0x0004 });
            // WS_EX_TOPMOST; TTS_ALWAYSTIP | TTS_NOPREFIX.
            st.tip = CreateWindowExW(
                0x0008,
                wide("tooltips_class32").as_ptr(),
                null(),
                WS_POPUP | 0x01 | 0x02,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                st.hwnd,
                null_mut(),
                null_mut(),
                null_mut(),
            );
            if st.tip.is_null() {
                return;
            }
            SendMessageW(st.tip, TTM_SETMAXTIPWIDTH, 0, (320.0 * st.scale) as isize);
        }
        for (i, r) in &st.tip_key {
            let info = tool_info(st.hwnd, *i, *r);
            SendMessageW(st.tip, TTM_DELTOOLW, 0, &info as *const ToolInfoW as isize);
        }
        for (i, r) in &key {
            let info = tool_info(st.hwnd, *i, *r);
            SendMessageW(st.tip, TTM_ADDTOOLW, 0, &info as *const ToolInfoW as isize);
        }
        st.tip_key = key;
    }
}

/// The text of tooltip `index`, asked of the editor.
unsafe fn tooltip_text(sp: *mut State, index: usize) -> String {
    unsafe {
        let st = &*sp;
        if st.busy {
            return String::new();
        }
        st.editor.tooltips().get(index).map(|t| t.text.clone()).unwrap_or_default()
    }
}

/// The filter string of the open dialog: pairs of a name and a pattern.
fn file_filter(extensions: &[String]) -> Vec<u16> {
    let mut out = String::new();
    let clean: Vec<String> = extensions.iter().map(|e| e.trim_start_matches('.').replace(['\0', ';', '*'], "")).filter(|e| !e.is_empty()).collect();
    if !clean.is_empty() {
        let patterns = clean.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(";");
        out.push_str(&format!("Files ({patterns})\0{patterns}\0"));
    }
    out.push_str("All files (*.*)\0*.*\0");
    out.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The paths in the buffer an open dialog filled: one path, or a folder
/// followed by file names (several files chosen), ended by an empty string.
fn parse_file_list(buffer: &[u16]) -> Vec<PathBuf> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (i, c) in buffer.iter().enumerate() {
        if *c == 0 {
            if i == start {
                break;
            }
            parts.push(String::from_utf16_lossy(&buffer[start..i]));
            start = i + 1;
        }
    }
    match parts.len() {
        0 => Vec::new(),
        1 => vec![PathBuf::from(&parts[0])],
        _ => parts[1..].iter().map(|name| PathBuf::from(&parts[0]).join(name)).collect(),
    }
}

/// Show the system's open dialog (it runs its own message loop), or ask the
/// test hook. The answer is delivered on the next tick.
unsafe fn open_file_dialog(sp: *mut State, req: FileDialog) {
    unsafe {
        let st = &mut *sp;
        let hwnd = st.hwnd;
        if let Some(hook) = st.file_hook.as_mut() {
            if let Some(answer) = hook(&req) {
                st.file_results.push((req.tag, answer));
                return;
            }
        }
        let filter = file_filter(&req.extensions);
        let title = wide(&req.title);
        let mut file = vec![0u16; 32768];
        let mut ofn: OpenFileNameW = std::mem::zeroed();
        ofn.size = std::mem::size_of::<OpenFileNameW>() as u32;
        ofn.owner = hwnd;
        ofn.filter = filter.as_ptr();
        ofn.filter_index = 1;
        ofn.file = file.as_mut_ptr();
        ofn.max_file = file.len() as u32;
        ofn.title = if req.title.is_empty() { null() } else { title.as_ptr() };
        // EXPLORER | FILEMUSTEXIST | PATHMUSTEXIST | HIDEREADONLY | NOCHANGEDIR (| ALLOWMULTISELECT)
        ofn.flags = 0x0008_0000 | 0x1000 | 0x0800 | 0x0004 | 0x0008 | if req.multiple { 0x0200 } else { 0 };
        let ok = GetOpenFileNameW(&mut ofn) != 0;
        if !alive(hwnd, sp) {
            return;
        }
        let paths = if ok { Some(parse_file_list(&file)).filter(|p| !p.is_empty()) } else { None };
        (*sp).file_results.push((req.tag, paths));
    }
}

// ---- The view's window procedure -------------------------------------------

unsafe extern "system" fn view_proc(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize {
    // A panic must not unwind into the system's dispatcher.
    match std::panic::catch_unwind(|| unsafe { view_message(hwnd, msg, w, l) }) {
        Ok(result) => result,
        Err(_) => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

unsafe fn view_message(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize {
    unsafe {
        let sp = state_of(hwnd);
        if sp.is_null() {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        match msg {
            WM_NCDESTROY => {
                // The host destroyed the parent: the window is gone, the editor stays
                // until the view is dropped.
                let st = &mut *sp;
                st.hwnd = null_mut();
                st.tip = null_mut();
                st.tip_key.clear();
                st.text_field = None;
                st.drags = false;
                st.down = false;
                if !st.field_font.is_null() {
                    DeleteObject(std::mem::replace(&mut st.field_font, null_mut()));
                }
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DefWindowProcW(hwnd, msg, w, l)
            }
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                paint(sp, null_mut());
                0
            }
            WM_PRINTCLIENT => {
                paint(sp, w as Handle);
                // That frame took the editor's damage with it: the screen gets a whole one.
                redraw(hwnd);
                0
            }
            WM_TIMER if w == TIMER_ID => {
                tick(sp);
                0
            }
            WM_SIZE => {
                let st = &mut *sp;
                let (pw, ph) = ((l as u32) & 0xffff, ((l as u32) >> 16) & 0xffff);
                if !st.in_frame && pw > 0 && ph > 0 {
                    let (pw_pt, ph_pt) = to_points((pw, ph), st.scale);
                    if (pw_pt, ph_pt) != st.size {
                        resize_to(sp, pw_pt, ph_pt, false);
                    }
                    redraw(hwnd);
                }
                0
            }
            WM_DPICHANGED_AFTERPARENT => {
                rescale(sp);
                0
            }
            WM_MOUSEMOVE => {
                guarded(sp, |st| {
                    sync_modifiers(st);
                    let (x, y) = mouse_point(st, l);
                    st.mouse = (x, y);
                    st.editor.mouse_move(x, y);
                });
                0
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                // A click on the view (the field takes its own) abandons the field.
                close_text_field(sp, true);
                // A click gives the editor the keyboard, when it wants keys. Outside the
                // editor call: taking focus calls the editor again.
                if !(*sp).busy && (*sp).editor.wants_keys() && GetFocus() != hwnd {
                    SetFocus(hwnd);
                }
                if !alive(hwnd, sp) {
                    return 0;
                }
                (*sp).down = true;
                SetCapture(hwnd);
                guarded(sp, |st| {
                    sync_modifiers(st);
                    let (x, y) = mouse_point(st, l);
                    st.mouse = (x, y);
                    st.editor.mouse_down(x, y, if msg == WM_LBUTTONDBLCLK { 2 } else { 1 });
                });
                invalidate(sp);
                0
            }
            WM_LBUTTONUP => {
                if std::mem::take(&mut (*sp).down) {
                    ReleaseCapture();
                }
                guarded(sp, |st| {
                    sync_modifiers(st);
                    let (x, y) = mouse_point(st, l);
                    st.mouse = (x, y);
                    st.editor.mouse_up(x, y);
                });
                invalidate(sp);
                0
            }
            WM_CAPTURECHANGED => {
                // Another window took the mouse in the middle of a drag: the drag ends here.
                if std::mem::take(&mut (*sp).down) {
                    guarded(sp, |st| {
                        let (x, y) = st.mouse;
                        st.editor.mouse_up(x, y);
                    });
                    invalidate(sp);
                }
                0
            }
            WM_RBUTTONDOWN => {
                // The editor may take it for a parameter menu (the host's or the SDK's);
                // otherwise it goes on to the host.
                close_text_field(sp, true);
                let mut used = false;
                guarded(sp, |st| {
                    sync_modifiers(st);
                    let (x, y) = mouse_point(st, l);
                    used = st.editor.right_mouse_down(x, y);
                });
                if !alive(hwnd, sp) {
                    return 0;
                }
                if used {
                    invalidate(sp);
                    0
                } else {
                    DefWindowProcW(hwnd, msg, w, l)
                }
            }
            WM_MOUSEWHEEL => {
                // The position is in screen pixels; a notch of the wheel is 120.
                let (sx, sy) = lparam_point(l);
                let mut pt = Point { x: sx, y: sy };
                ScreenToClient(hwnd, &mut pt);
                let delta = f32::from((w >> 16) as u16 as i16) / 120.0;
                guarded(sp, |st| {
                    sync_modifiers(st);
                    let (x, y) = client_to_points((pt.x, pt.y), client_size(st.hwnd), st.size);
                    st.editor.wheel(x, y, delta);
                });
                0
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => key_event(sp, msg, w, l, true),
            WM_KEYUP | WM_SYSKEYUP => key_event(sp, msg, w, l, false),
            WM_SETFOCUS | WM_KILLFOCUS => {
                guarded(sp, |st| st.editor.focus_changed(msg == WM_SETFOCUS));
                if alive(hwnd, sp) {
                    invalidate(sp);
                }
                0
            }
            WM_GETDLGCODE => {
                if !(*sp).busy && (*sp).editor.wants_keys() {
                    DLGC_WANTALLKEYS
                } else {
                    DefWindowProcW(hwnd, msg, w, l)
                }
            }
            WM_CTLCOLOREDIT if (*sp).field_invalid && (*sp).text_field == Some(l as Hwnd) => {
                SetTextColor(w as Handle, 0x0000_00FF);
                SetBkColor(w as Handle, 0x00FF_FFFF);
                GetStockObject(0) as isize // WHITE_BRUSH
            }
            WM_NOTIFY => {
                let hdr = l as *const NmHdr;
                if !hdr.is_null() && !(*sp).tip.is_null() && (*hdr).from == (*sp).tip && (*hdr).code == TTN_GETDISPINFOW {
                    let info = l as *mut NmTtDispInfoW;
                    (*sp).tip_text = wide(&tooltip_text(sp, (*hdr).id));
                    (*info).text = (*sp).tip_text.as_mut_ptr();
                    return 0;
                }
                DefWindowProcW(hwnd, msg, w, l)
            }
            WM_DROPFILES => {
                files_dropped(sp, w as Handle);
                0
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

/// An editor in a child window. Dropping it stops the timer, destroys the
/// window, and drops the editor. Use it on one thread only.
pub struct EditorView {
    state: *mut State,
}

impl EditorView {
    /// `poll` runs before every tick (see [`Poll`]). The window is made by
    /// [`Self::attach`], since a child window needs its parent to exist.
    pub fn new(editor: Box<dyn Editor>, poll: Option<Poll>) -> Option<EditorView> {
        acquire_classes();
        let size = editor.size();
        let state = Box::into_raw(Box::new(State {
            hwnd: null_mut(),
            editor,
            poll,
            size,
            last: Instant::now(),
            modifiers: Modifiers::NONE,
            busy: false,
            pending: None,
            repaint: false,
            in_frame: false,
            host_scale: None,
            scale: unsafe { window_scale(null_mut()) },
            hidden: false,
            down: false,
            mouse: (0.0, 0.0),
            text_field: None,
            field_font: null_mut(),
            field_invalid: false,
            field_cancel: false,
            opening: false,
            tip: null_mut(),
            tip_key: Vec::new(),
            tip_text: Vec::new(),
            drags: false,
            file_results: Vec::new(),
            file_hook: None,
            menu_param: 0,
            menu_hook: None,
            top: null_mut(),
        }));
        Some(EditorView { state })
    }

    #[allow(clippy::mut_from_ref)]
    fn st(&self) -> &mut State {
        // The view is used on one thread, and the state lives as long as the view.
        unsafe { &mut *self.state }
    }

    /// The child `HWND`; null before [`Self::attach`]. The view destroys it on drop.
    pub fn raw(&self) -> Hwnd {
        self.st().hwnd
    }

    /// The editor's current size in points.
    pub fn size(&self) -> (u32, u32) {
        self.st().size
    }

    /// The sizes the editor can take, in points.
    pub fn size_range(&self) -> SizeRange {
        self.st().editor.size_range()
    }

    /// Pixels per point: the host's scale, or the window's DPI over 96.
    pub fn scale(&self) -> f32 {
        self.st().scale
    }

    /// Use the host's scale from now on (CLAP `set_scale`, VST3
    /// `setContentScaleFactor`); the window becomes `points x scale` pixels.
    /// False for a scale that is not a positive number.
    pub fn set_scale(&self, scale: f32) -> bool {
        let Some(scale) = valid_scale(scale) else { return false };
        self.st().host_scale = Some(scale);
        unsafe { rescale(self.state) };
        true
    }

    /// The window's size in pixels: the editor's size at the current scale.
    pub fn pixel_size(&self) -> (u32, u32) {
        to_pixels(self.st().size, self.st().scale)
    }

    /// A size in points as pixels at the current scale.
    pub fn to_pixels(&self, width: u32, height: u32) -> (u32, u32) {
        to_pixels((width, height), self.st().scale)
    }

    /// A size in pixels as points at the current scale.
    pub fn to_points(&self, width: u32, height: u32) -> (u32, u32) {
        to_points((width, height), self.st().scale)
    }

    /// A point of the editor (points, origin top left) in the window's client
    /// pixels. Before the view has a window: the point at the current scale.
    pub fn to_client(&self, x: f32, y: f32) -> (i32, i32) {
        let st = self.st();
        if st.hwnd.is_null() {
            return ((x * st.scale).round() as i32, (y * st.scale).round() as i32);
        }
        let r = unsafe { view_space(st, &DamageRect::new(x, y, 0.0, 0.0)) };
        (r.left, r.top)
    }

    /// A point of the editor in screen pixels (where a host pops up a menu
    /// for a floating editor). `None` before the view has a window.
    pub fn to_screen(&self, x: f32, y: f32) -> Option<(i32, i32)> {
        let hwnd = self.st().hwnd;
        if hwnd.is_null() {
            return None;
        }
        let (cx, cy) = self.to_client(x, y);
        let mut pt = Point { x: cx, y: cy };
        unsafe { ClientToScreen(hwnd, &mut pt) };
        Some((pt.x, pt.y))
    }

    /// Move the editor and the window to `width` x `height` points, brought
    /// into the editor's range. Returns the size the editor takes. If the
    /// editor is inside one of its own calls, the change is made when that
    /// call ends. A fixed editor stays as it is.
    pub fn resize(&self, width: u32, height: u32) -> (u32, u32) {
        let (w, h) = self.size_range().clamp(width, height);
        unsafe { resize_to(self.state, w, h, true) };
        (w, h)
    }

    /// Make the window exactly `width` x `height` pixels (CLAP `set_size`,
    /// VST3 `onSize`) and give the editor the nearest size in points its
    /// range allows. Returns that size in points.
    pub fn set_pixel_size(&self, width: u32, height: u32) -> (u32, u32) {
        let st = self.st();
        let (pw, ph) = to_points((width, height), st.scale);
        let (w, h) = st.editor.size_range().clamp(pw, ph);
        unsafe {
            if st.hwnd.is_null() {
                resize_to(self.state, w, h, true);
            } else {
                // The window procedure hears the new size and resizes the editor.
                SetWindowPos(st.hwnd, null_mut(), 0, 0, width.max(1) as i32, height.max(1) as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        (w, h)
    }

    /// Make the child window inside a host's parent window, or move it to
    /// another parent. It is `points x scale` pixels at the parent's top left.
    ///
    /// # Safety
    /// `parent` is null or a window of this thread's process.
    pub unsafe fn attach(&self, parent: Hwnd) -> bool {
        unsafe {
            if parent.is_null() || IsWindow(parent) == 0 {
                return false;
            }
            let st = self.st();
            if !st.hwnd.is_null() {
                SetParent(st.hwnd, parent);
                rescale(self.state);
                return true;
            }
            st.scale = st.host_scale.unwrap_or_else(|| window_scale(parent));
            let (w, h) = to_pixels(st.size, st.scale);
            let style = WS_CHILD | WS_CLIPCHILDREN | if st.hidden { 0 } else { WS_VISIBLE };
            let hwnd = CreateWindowExW(0, view_class().as_ptr(), null(), style, 0, 0, w as i32, h as i32, parent, null_mut(), module(), null_mut());
            if hwnd.is_null() {
                return false;
            }
            st.hwnd = hwnd;
            st.last = Instant::now();
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, self.state as isize);
            SetTimer(hwnd, TIMER_ID, 33, null());
            sync_tooltips(self.state);
            sync_drops(self.state);
            true
        }
    }

    pub fn set_hidden(&self, hidden: bool) {
        let st = self.st();
        st.hidden = hidden;
        if !st.hwnd.is_null() {
            // SW_HIDE, SW_SHOWNA
            unsafe { ShowWindow(st.hwnd, if hidden { 0 } else { 8 }) };
        }
    }

    /// The hosted editor.
    pub fn editor(&mut self) -> Option<&mut (dyn Editor + 'static)> {
        unsafe { self.state.as_mut().map(|s| s.editor.as_mut()) }
    }

    /// The open text field (an `EDIT` control), if any.
    pub fn text_field(&self) -> Option<Hwnd> {
        self.st().text_field
    }

    /// The tooltip rectangles in window pixels (left, top, right, bottom) with
    /// the text the view answers for each, asked the way the tooltip control asks.
    pub fn tooltip_rects(&self) -> Vec<([i32; 4], String)> {
        let keys = self.st().tip_key.clone();
        keys.iter().map(|(i, r)| ([r.left, r.top, r.right, r.bottom], unsafe { tooltip_text(self.state, *i) })).collect()
    }

    /// What the tooltip control itself holds: how many tools it has, and the
    /// text it gets for each when it asks the view (`TTM_GETTEXTW`, which sends
    /// the same `TTN_GETDISPINFOW` a showing tip sends). Empty before the view
    /// has a window or tooltips. For tests.
    pub fn tooltip_control_texts(&self) -> Vec<String> {
        let st = self.st();
        if st.tip.is_null() {
            return Vec::new();
        }
        let keys = st.tip_key.clone();
        let (tip, hwnd) = (st.tip, st.hwnd);
        unsafe {
            // TTM_GETTOOLCOUNT
            let count = SendMessageW(tip, 0x0400 + 13, 0, 0) as usize;
            keys.iter()
                .take(count)
                .map(|(i, r)| {
                    let mut buf = vec![0u16; 1024];
                    let mut info = tool_info(hwnd, *i, *r);
                    info.text = buf.as_mut_ptr();
                    // TTM_GETTEXTW; wParam is the room in the buffer.
                    SendMessageW(tip, 0x0400 + 56, buf.len() - 1, &mut info as *mut ToolInfoW as isize);
                    from_wide(&buf)
                })
                .collect()
        }
    }

    /// Answer file dialog requests with `hook` instead of showing the system
    /// dialog, for tests. See [`FileHook`].
    pub fn set_file_dialog_hook(&self, hook: Option<FileHook>) {
        self.st().file_hook = hook;
    }

    /// Answer the SDK's parameter menu with `hook` instead of showing a popup
    /// menu, for tests. See [`MenuHook`].
    pub fn set_menu_hook(&self, hook: Option<MenuHook>) {
        self.st().menu_hook = hook;
    }

    /// Choose item `index` of [`PARAM_MENU_ITEMS`] as if from the menu last
    /// shown, with `text` standing in for the clipboard (what paste reads;
    /// copy returns what it would put there). For tests.
    pub fn choose_menu_item(&self, index: usize, text: Option<&str>) -> Option<String> {
        unsafe { run_menu_item(self.state, index, text) }
    }

    /// Replace the poll hook.
    pub fn set_poll(&mut self, poll: Option<Poll>) {
        self.st().poll = poll;
    }
}

impl Drop for EditorView {
    fn drop(&mut self) {
        unsafe {
            let st = &mut *self.state;
            let hwnd = std::mem::replace(&mut st.hwnd, null_mut());
            if !hwnd.is_null() {
                KillTimer(hwnd, TIMER_ID);
                if st.down {
                    st.down = false;
                    ReleaseCapture();
                }
                // Detach the state first: nothing the window still hears reaches the editor.
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                // The field and the tooltip control are the window's own and go with it.
                DestroyWindow(hwnd);
            }
            if !st.field_font.is_null() {
                DeleteObject(st.field_font);
            }
            drop(Box::from_raw(self.state));
        }
        release_classes();
    }
}

// ---- A window of its own -----------------------------------------------------

struct Top {
    /// The view's window, a child of this one.
    child: Hwnd,
    /// Called when the user closes the window; without it the window is destroyed.
    on_close: Option<Box<dyn FnMut()>>,
    /// Closing the window ends the message loop ([`run_window`]).
    quit: bool,
}

/// Make the window's client area `client` pixels.
unsafe fn fit_top(top: Hwnd, client: (i32, i32)) {
    unsafe {
        let (w, h) = outer_size(top, client);
        SetWindowPos(top, null_mut(), 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// The window size whose client area is `client` pixels.
unsafe fn outer_size(top: Hwnd, client: (i32, i32)) -> (i32, i32) {
    unsafe {
        let mut rc = Rect { left: 0, top: 0, right: client.0, bottom: client.1 };
        let style = GetWindowLongPtrW(top, GWL_STYLE) as u32;
        let ex_style = GetWindowLongPtrW(top, GWL_EXSTYLE) as u32;
        AdjustWindowRectExForDpi(&mut rc, style, 0, ex_style, GetDpiForWindow(top).max(96));
        (rc.right - rc.left, rc.bottom - rc.top)
    }
}

unsafe extern "system" fn top_proc(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize {
    match std::panic::catch_unwind(|| unsafe { top_message(hwnd, msg, w, l) }) {
        Ok(result) => result,
        Err(_) => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

unsafe fn top_message(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize {
    unsafe {
        let tp = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Top;
        if tp.is_null() {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        let child = (*tp).child;
        let child_state = if IsWindow(child) != 0 && GetParent(child) == hwnd { state_of(child) } else { null_mut() };
        match msg {
            WM_SIZE => {
                // The view fills the window; it hears the size and resizes the editor.
                let (cw, ch) = (((l as u32) & 0xffff) as i32, (((l as u32) >> 16) & 0xffff) as i32);
                if !child_state.is_null() && cw > 0 && ch > 0 && !(*child_state).in_frame {
                    SetWindowPos(child, null_mut(), 0, 0, cw, ch, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
                }
                0
            }
            WM_GETMINMAXINFO => {
                if !child_state.is_null() && !(*child_state).busy {
                    let st = &*child_state;
                    let range = st.editor.size_range();
                    if !range.is_fixed() {
                        let info = l as *mut MinMaxInfo;
                        let min = to_pixels(range.min, st.scale);
                        let max = to_pixels(range.max, st.scale);
                        let (min_w, min_h) = outer_size(hwnd, (min.0 as i32, min.1 as i32));
                        let (max_w, max_h) = outer_size(hwnd, (max.0 as i32, max.1 as i32));
                        (*info).min_track = Point { x: min_w, y: min_h };
                        (*info).max_track = Point { x: max_w, y: max_h };
                        return 0;
                    }
                }
                DefWindowProcW(hwnd, msg, w, l)
            }
            WM_DPICHANGED => {
                // The system proposes a rectangle on the new display; the view then
                // hears WM_DPICHANGED_AFTERPARENT and sizes itself and this window.
                let rc = l as *const Rect;
                if !rc.is_null() {
                    let r = *rc;
                    SetWindowPos(hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                }
                if !child_state.is_null() {
                    rescale(child_state);
                }
                0
            }
            WM_SETFOCUS => {
                if !child_state.is_null() {
                    SetFocus(child);
                }
                0
            }
            WM_CLOSE => {
                // The callback may destroy this window (the host closes the editor),
                // so it is taken out while it runs.
                match (*tp).on_close.take() {
                    Some(mut close) => {
                        close();
                        if IsWindow(hwnd) != 0 && GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Top == tp {
                            (*tp).on_close = Some(close);
                        }
                    }
                    None => {
                        DestroyWindow(hwnd);
                    }
                }
                0
            }
            WM_DESTROY => {
                if (*tp).quit {
                    PostQuitMessage(0);
                }
                0
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(tp));
                DefWindowProcW(hwnd, msg, w, l)
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

/// A window with a title bar whose client area is an [`EditorView`]: a
/// floating CLAP editor, the standalone window. It is resizable when the
/// editor is, within the editor's range. It starts hidden. Dropping it
/// destroys the window (and the view's child window with it; drop order
/// between the two does not matter).
pub struct TopWindow {
    hwnd: Hwnd,
}

impl TopWindow {
    /// `on_close` runs when the user closes the window (the window stays; the
    /// owner decides). Without it, closing destroys the window.
    pub fn new(title: &str, view: &EditorView, on_close: Option<Box<dyn FnMut()>>) -> Option<TopWindow> {
        Self::make(title, view, on_close, false)
    }

    fn make(title: &str, view: &EditorView, on_close: Option<Box<dyn FnMut()>>, quit: bool) -> Option<TopWindow> {
        acquire_classes();
        unsafe {
            let fixed = view.size_range().is_fixed();
            let style = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN | if fixed { 0 } else { WS_THICKFRAME | WS_MAXIMIZEBOX };
            let hwnd = CreateWindowExW(
                0,
                top_class().as_ptr(),
                wide(title).as_ptr(),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                null_mut(),
                null_mut(),
                module(),
                null_mut(),
            );
            if hwnd.is_null() {
                release_classes();
                return None;
            }
            let top = TopWindow { hwnd };
            if !view.attach(hwnd) {
                return None;
            }
            view.st().top = hwnd;
            let state = Box::into_raw(Box::new(Top { child: view.raw(), on_close, quit }));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
            let (w, h) = view.pixel_size();
            fit_top(hwnd, (w as i32, h as i32));
            Some(top)
        }
    }

    /// The window's `HWND`.
    pub fn raw(&self) -> Hwnd {
        self.hwnd
    }

    pub fn set_visible(&self, visible: bool) {
        // SW_HIDE, SW_SHOW
        unsafe { ShowWindow(self.hwnd, if visible { 5 } else { 0 }) };
    }

    pub fn set_title(&self, title: &str) {
        unsafe { SetWindowTextW(self.hwnd, wide(title).as_ptr()) };
    }

    /// Keep the window above `owner` (CLAP `set_transient`).
    ///
    /// # Safety
    /// `owner` is null or a window.
    pub unsafe fn set_owner(&self, owner: Hwnd) {
        unsafe { SetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT, owner as isize) };
    }
}

impl Drop for TopWindow {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(self.hwnd) != 0 {
                DestroyWindow(self.hwnd);
            }
        }
        release_classes();
    }
}

/// Make this process aware of each display's DPI (an application does this
/// once, before its first window; a plugin never does: the host decides).
pub fn set_process_dpi_aware() {
    // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
    unsafe { SetProcessDpiAwarenessContext(-4) };
}

/// Show the editor in its own window and run the message loop until the
/// window closes. Returns the exit code.
pub fn run_window(title: &str, editor: Box<dyn Editor>, poll: Option<Poll>) -> i32 {
    set_process_dpi_aware();
    let Some(view) = EditorView::new(editor, poll) else { return 1 };
    let Some(top) = TopWindow::make(title, &view, None, true) else { return 1 };
    top.set_visible(true);
    unsafe {
        let mut msg: Msg = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    drop(view);
    drop(top);
    0
}

/// What a test of real windows needs: a parent window, a message pump that
/// waits for a condition, and a picture of what a window drew.
pub mod testing {
    use super::*;

    /// Pixels of a window, top row first, each a `0x00RRGGBB` word.
    pub struct Bitmap {
        pub width: usize,
        pub height: usize,
        pub pixels: Vec<u32>,
    }

    impl Bitmap {
        pub fn pixel(&self, x: usize, y: usize) -> u32 {
            self.pixels[y * self.width + x] & 0x00ff_ffff
        }

        /// The file as a 32-bit BMP.
        pub fn save_bmp(&self, path: &std::path::Path) -> std::io::Result<()> {
            let bytes = self.pixels.len() * 4;
            let mut out = Vec::with_capacity(54 + bytes);
            out.extend_from_slice(b"BM");
            out.extend_from_slice(&((54 + bytes) as u32).to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&54u32.to_le_bytes());
            out.extend_from_slice(&40u32.to_le_bytes());
            out.extend_from_slice(&(self.width as i32).to_le_bytes());
            out.extend_from_slice(&(-(self.height as i32)).to_le_bytes());
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&32u16.to_le_bytes());
            out.extend_from_slice(&[0u8; 24]);
            for p in &self.pixels {
                out.extend_from_slice(&(p | 0xff00_0000).to_le_bytes());
            }
            std::fs::write(path, out)
        }
    }

    /// A visible window without a frame, `width` x `height` pixels, above the other windows, to parent a view in.
    pub fn parent_window(title: &str, x: i32, y: i32, width: u32, height: u32) -> Hwnd {
        unsafe {
            // WS_EX_TOPMOST | WS_EX_NOACTIVATE: on top, so the screen shows it, without taking the keyboard.
            let hwnd = CreateWindowExW(0x0008 | 0x0800_0000, wide("STATIC").as_ptr(), wide(title).as_ptr(), WS_POPUP | WS_CLIPCHILDREN, x, y, width as i32, height as i32, null_mut(), null_mut(), null_mut(), null_mut());
            if !hwnd.is_null() {
                // Twice: a process started hidden has its first show command replaced. SW_SHOWNOACTIVATE.
                ShowWindow(hwnd, 4);
                ShowWindow(hwnd, 4);
            }
            hwnd
        }
    }

    /// # Safety
    /// `hwnd` is a window this thread made.
    pub unsafe fn destroy_window(hwnd: Hwnd) {
        unsafe { DestroyWindow(hwnd) };
    }

    /// # Safety
    /// `hwnd` is null or a window.
    pub unsafe fn is_window(hwnd: Hwnd) -> bool {
        unsafe { !hwnd.is_null() && IsWindow(hwnd) != 0 }
    }

    /// `SendMessageW`.
    ///
    /// # Safety
    /// As the message requires.
    pub unsafe fn send(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize {
        unsafe { SendMessageW(hwnd, msg, w, l) }
    }

    /// The `lParam` of a mouse message at a client position in pixels.
    pub fn mouse_lparam(x: i32, y: i32) -> isize {
        (((y as u16 as u32) << 16) | (x as u16 as u32)) as isize
    }

    /// The window's client size in pixels.
    ///
    /// # Safety
    /// `hwnd` is a window.
    pub unsafe fn client_size(hwnd: Hwnd) -> (u32, u32) {
        let (w, h) = unsafe { super::client_size(hwnd) };
        (w.max(0) as u32, h.max(0) as u32)
    }

    /// Run this thread's messages until `done` says so. False if `timeout_ms` passed first.
    pub fn pump_until(timeout_ms: u32, mut done: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        unsafe {
            let mut msg: Msg = std::mem::zeroed();
            loop {
                // PM_REMOVE
                while PeekMessageW(&mut msg, null_mut(), 0, 0, 1) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                if done() {
                    return true;
                }
                if start.elapsed().as_millis() >= u128::from(timeout_ms) {
                    return false;
                }
                // QS_ALLINPUT
                MsgWaitForMultipleObjects(0, null(), 0, 10, 0x04FF);
            }
        }
    }

    unsafe fn grab(width: i32, height: i32, draw: impl FnOnce(Handle) -> bool) -> Option<Bitmap> {
        unsafe {
            if width <= 0 || height <= 0 {
                return None;
            }
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return None;
            }
            let info = bitmap_info(width, height);
            let mut bits: *mut c_void = null_mut();
            let section = CreateDIBSection(dc, &info, 0, &mut bits, null_mut(), 0);
            let mut out = None;
            if !section.is_null() && !bits.is_null() {
                let old = SelectObject(dc, section);
                if draw(dc) {
                    GdiFlush();
                    let pixels = std::slice::from_raw_parts(bits as *const u32, (width * height) as usize).to_vec();
                    out = Some(Bitmap { width: width as usize, height: height as usize, pixels });
                }
                SelectObject(dc, old);
            }
            if !section.is_null() {
                DeleteObject(section);
            }
            DeleteDC(dc);
            out
        }
    }

    /// What the window draws in its client area, asked of the window itself
    /// (`PrintWindow`): works for a window that is covered or off screen.
    ///
    /// # Safety
    /// `hwnd` is a window.
    pub unsafe fn capture(hwnd: Hwnd) -> Option<Bitmap> {
        unsafe {
            let (w, h) = super::client_size(hwnd);
            // PW_CLIENTONLY: the window is asked to draw now (WM_PRINTCLIENT), so the
            // picture never depends on what the compositor last showed.
            // A window that cannot show (a process with no interactive desktop) is not
            // printed by the system: it is asked directly. PRF_CLIENT.
            let out = grab(w, h, |dc| {
                if IsWindowVisible(hwnd) != 0 {
                    PrintWindow(hwnd, dc, 0x1) != 0
                } else {
                    SendMessageW(hwnd, WM_PRINTCLIENT, dc as usize, 0x4);
                    true
                }
            });
            // Printing validates the window; the screen still needs its own paint.
            InvalidateRect(hwnd, null(), 0);
            out
        }
    }

    /// What is on the screen where the window is: the real desktop pixels,
    /// which exist only in a logged-in desktop session.
    ///
    /// # Safety
    /// `hwnd` is a window.
    pub unsafe fn capture_screen(hwnd: Hwnd) -> Option<Bitmap> {
        unsafe {
            let mut rc = Rect::default();
            if GetWindowRect(hwnd, &mut rc) == 0 {
                return None;
            }
            let screen = GetDC(null_mut());
            if screen.is_null() {
                return None;
            }
            // SRCCOPY | CAPTUREBLT
            let out = grab(rc.right - rc.left, rc.bottom - rc.top, |dc| BitBlt(dc, 0, 0, rc.right - rc.left, rc.bottom - rc.top, screen, rc.left, rc.top, 0x00CC_0020 | 0x4000_0000) != 0);
            ReleaseDC(null_mut(), screen);
            out
        }
    }

    /// Put the real pointer at a screen position. False where there is no
    /// pointer to move (a locked desktop, a process with no desktop).
    pub fn set_pointer(x: i32, y: i32) -> bool {
        unsafe { SetCursorPos(x, y) != 0 }
    }

    /// `PostMessageW`: the message waits in the window's queue, so a modal loop
    /// the window's thread is inside (a menu, a dialog) is the one that reads it.
    ///
    /// # Safety
    /// As the message requires.
    pub unsafe fn post(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> bool {
        unsafe { PostMessageW(hwnd, msg, w, l) != 0 }
    }

    /// The child windows of `parent` (the top-level windows, for a null
    /// `parent`) that belong to this process, of a class (any, for `None`).
    ///
    /// # Safety
    /// `parent` is null or a window.
    pub unsafe fn windows(parent: Hwnd, class: Option<&str>) -> Vec<Hwnd> {
        unsafe {
            let class = class.map(wide);
            let name = class.as_ref().map_or(null(), |c| c.as_ptr());
            let mut out = Vec::new();
            let mut at = null_mut();
            loop {
                at = FindWindowExW(parent, at, name, null());
                if at.is_null() {
                    return out;
                }
                let mut process = 0u32;
                GetWindowThreadProcessId(at, &mut process);
                if process == GetCurrentProcessId() {
                    out.push(at);
                }
            }
        }
    }

    /// Every window below `parent`, at any depth.
    ///
    /// # Safety
    /// `parent` is a window.
    pub unsafe fn descendants(parent: Hwnd) -> Vec<Hwnd> {
        unsafe {
            let mut out = Vec::new();
            for child in windows(parent, None) {
                out.push(child);
                out.extend(descendants(child));
            }
            out
        }
    }

    /// # Safety
    /// `hwnd` is a window.
    pub unsafe fn class_name(hwnd: Hwnd) -> String {
        let mut buf = [0u16; 128];
        let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), 128) }.max(0) as usize;
        String::from_utf16_lossy(&buf[..n])
    }

    /// The window's text (a title, a field's contents).
    ///
    /// # Safety
    /// `hwnd` is a window.
    pub unsafe fn text(hwnd: Hwnd) -> String {
        unsafe { field_text(hwnd) }
    }

    /// Replace the window's text (`WM_SETTEXT`, which works across threads).
    ///
    /// # Safety
    /// `hwnd` is a window.
    pub unsafe fn set_text(hwnd: Hwnd, text: &str) {
        unsafe { SendMessageW(hwnd, 0x000C, 0, wide(text).as_ptr() as isize) };
    }

    /// # Safety
    /// `hwnd` is null or a window.
    pub unsafe fn is_visible(hwnd: Hwnd) -> bool {
        unsafe { !hwnd.is_null() && IsWindowVisible(hwnd) != 0 }
    }

    /// The whole window as the compositor has it, frame and all
    /// (`PrintWindow` with `PW_RENDERFULLCONTENT`); for windows that are not
    /// ours to ask (a menu, a system dialog).
    ///
    /// # Safety
    /// `hwnd` is a window.
    pub unsafe fn capture_window(hwnd: Hwnd) -> Option<Bitmap> {
        unsafe {
            let mut rc = Rect::default();
            if GetWindowRect(hwnd, &mut rc) == 0 {
                return None;
            }
            grab(rc.right - rc.left, rc.bottom - rc.top, |dc| PrintWindow(hwnd, dc, 0x2) != 0)
        }
    }

    /// Where the top left of the window's client area is on the screen, in pixels.
    ///
    /// # Safety
    /// `hwnd` is a window.
    pub unsafe fn client_origin(hwnd: Hwnd) -> (i32, i32) {
        let mut pt = Point::default();
        unsafe { ClientToScreen(hwnd, &mut pt) };
        (pt.x, pt.y)
    }

    /// Put a window at a screen position, above the others.
    ///
    /// # Safety
    /// `hwnd` is a window.
    pub unsafe fn move_to_front(hwnd: Hwnd, x: i32, y: i32) {
        // HWND_TOPMOST
        unsafe { SetWindowPos(hwnd, -1isize as Hwnd, x, y, 0, 0, SWP_NOSIZE) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_and_pixels_round_trip_at_every_common_scale() {
        for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0] {
            for points in [(240u32, 160u32), (180, 120), (481, 323), (1, 1)] {
                let px = to_pixels(points, scale);
                assert_eq!(to_points(px, scale), points, "{points:?} at {scale}");
            }
        }
        assert_eq!(to_pixels((240, 160), 1.5), (360, 240));
        assert_eq!(to_points((300, 200), 1.25), (240, 160));
        assert_eq!(to_pixels((0, 0), 2.0), (1, 1), "a window is never empty");
    }

    #[test]
    fn scales_are_checked() {
        assert_eq!(valid_scale(1.5), Some(1.5));
        assert_eq!(valid_scale(0.0), None);
        assert_eq!(valid_scale(-2.0), None);
        assert_eq!(valid_scale(f32::NAN), None);
        assert_eq!(valid_scale(100.0), Some(8.0));
    }

    #[test]
    fn damage_maps_to_whole_pixels_with_a_margin() {
        let r = pixel_rect(&DamageRect::new(10.5, 20.0, 30.0, 40.25), (240, 160), (240, 160)).unwrap();
        assert_eq!(r, Rect { left: 9, top: 19, right: 42, bottom: 62 });
        // Cut to the window.
        let r = pixel_rect(&DamageRect::new(-50.0, 150.0, 400.0, 400.0), (240, 160), (240, 160)).unwrap();
        assert_eq!(r, Rect { left: 0, top: 149, right: 240, bottom: 160 });
        // A window at 150 %.
        let r = pixel_rect(&DamageRect::new(10.0, 10.0, 10.0, 10.0), (240, 160), (360, 240)).unwrap();
        assert_eq!(r, Rect { left: 14, top: 14, right: 31, bottom: 31 });
        assert!(pixel_rect(&DamageRect::new(f32::NAN, 0.0, 1.0, 1.0), (240, 160), (240, 160)).is_none());
        assert!(pixel_rect(&DamageRect::new(0.0, 0.0, 1.0, 1.0), (0, 0), (240, 160)).is_none());
    }

    #[test]
    fn mouse_positions_are_signed_and_in_points() {
        assert_eq!(lparam_point(testing::mouse_lparam(100, 50)), (100, 50));
        // A drag outside the window, up and to the left.
        assert_eq!(lparam_point(testing::mouse_lparam(-3, -20)), (-3, -20));
        assert_eq!(client_to_points((180, 120), (360, 240), (240, 160)), (120.0, 80.0));
        assert_eq!(client_to_points((10, 10), (0, 0), (240, 160)), (10.0, 10.0));
    }

    #[test]
    fn virtual_keys_map_to_keys() {
        assert_eq!(key_from_vk(0x25, None), Some(Key::Left));
        assert_eq!(key_from_vk(0x26, None), Some(Key::Up));
        assert_eq!(key_from_vk(0x27, None), Some(Key::Right));
        assert_eq!(key_from_vk(0x28, None), Some(Key::Down));
        assert_eq!(key_from_vk(0x0D, Some('\r')), Some(Key::Return));
        assert_eq!(key_from_vk(0x08, Some('\u{8}')), Some(Key::Backspace));
        assert_eq!(key_from_vk(0x2E, None), Some(Key::Delete));
        assert_eq!(key_from_vk(0x20, Some(' ')), Some(Key::Space));
        assert_eq!(key_from_vk(0x09, Some('\t')), Some(Key::Tab));
        assert_eq!(key_from_vk(0x1B, Some('\u{1b}')), Some(Key::Escape));
        assert_eq!(key_from_vk(0x41, Some('A')), Some(Key::Char('a')));
        assert_eq!(key_from_vk(0x70, None), None, "F1 types nothing");
        assert_eq!(key_from_vk(0x10, None), None, "Shift alone is not a key");
    }

    #[test]
    fn key_states_map_to_modifiers() {
        assert_eq!(modifiers_from_keys(|_| false), Modifiers::NONE);
        let all = modifiers_from_keys(|_| true);
        assert!(all.shift && all.ctrl && all.alt && all.command);
        assert_eq!(modifiers_from_keys(|vk| vk == 0x5C), Modifiers { command: true, ..Modifiers::NONE });
        assert_eq!(modifiers_from_keys(|vk| vk == 0x10), Modifiers { shift: true, ..Modifiers::NONE });
    }

    #[test]
    fn open_dialog_buffers_parse_to_paths() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert_eq!(parse_file_list(&w("C:\\a\\one.wav\0\0")), vec![PathBuf::from("C:\\a\\one.wav")]);
        assert_eq!(parse_file_list(&w("C:\\a\0one.wav\0two.wav\0\0")), vec![PathBuf::from("C:\\a\\one.wav"), PathBuf::from("C:\\a\\two.wav")]);
        assert!(parse_file_list(&w("\0\0")).is_empty());
        assert!(parse_file_list(&[]).is_empty());
    }

    #[test]
    fn file_filters_list_the_extensions_then_every_file() {
        let text = |v: Vec<u16>| String::from_utf16_lossy(&v);
        assert_eq!(text(file_filter(&[])), "All files (*.*)\0*.*\0\0");
        assert_eq!(text(file_filter(&["wav".into(), ".AIFF".into()])), "Files (*.wav;*.AIFF)\0*.wav;*.AIFF\0All files (*.*)\0*.*\0\0");
    }

    #[test]
    fn a_view_without_a_window_keeps_its_editor_and_sizes() {
        struct Probe((u32, u32));
        impl Editor for Probe {
            fn size(&self) -> (u32, u32) {
                self.0
            }
            fn size_range(&self) -> SizeRange {
                SizeRange::between((100, 50), (400, 200))
            }
            fn resize(&mut self, w: u32, h: u32) {
                self.0 = (w, h);
            }
            fn param_changed(&mut self, _: usize, _: f32) {}
            fn render(&mut self, _: f32) -> openplugin_core::editor::Frame<'_> {
                openplugin_core::editor::Frame { pixels: &[], width: 0, height: 0 }
            }
        }
        let mut view = EditorView::new(Box::new(Probe((200, 100))), None).unwrap();
        assert!(view.raw().is_null());
        assert_eq!(view.size(), (200, 100));
        assert!(view.set_scale(1.5));
        assert_eq!(view.pixel_size(), (300, 150));
        assert_eq!(view.resize(1000, 10), (400, 50));
        assert_eq!(view.size(), (400, 50));
        assert_eq!(view.set_pixel_size(300, 150), (200, 100));
        assert_eq!(view.size(), (200, 100));
        assert!(!view.set_scale(0.0));
        assert!(!unsafe { view.attach(null_mut()) });
        assert!(view.editor().is_some());
        assert!(view.text_field().is_none() && view.tooltip_rects().is_empty());
    }
}
