//! Win32 window using GDI only. No GPU, no manifest-gated APIs: this runs on Windows 7 SP1
//! through Windows 11. DPI is system-wide (`SetProcessDPIAware`); per-monitor awareness is added
//! later by looking the newer calls up at run time.

#![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms)]

use crate::core::{Core, Event, Key};
use std::ffi::c_void;
use std::ptr::{null, null_mut};

type HWND = *mut c_void;
type HDC = *mut c_void;
type HINSTANCE = *mut c_void;
type WNDPROC = unsafe extern "system" fn(HWND, u32, usize, isize) -> isize;

#[repr(C)]
struct POINT { x: i32, y: i32 }
#[repr(C)]
struct RECT { left: i32, top: i32, right: i32, bottom: i32 }
#[repr(C)]
struct MSG { hwnd: HWND, message: u32, wparam: usize, lparam: isize, time: u32, pt: POINT }
#[repr(C)]
struct WNDCLASSEXW {
    cbSize: u32, style: u32, lpfnWndProc: Option<WNDPROC>, cbClsExtra: i32, cbWndExtra: i32,
    hInstance: HINSTANCE, hIcon: *mut c_void, hCursor: *mut c_void, hbrBackground: *mut c_void,
    lpszMenuName: *const u16, lpszClassName: *const u16, hIconSm: *mut c_void,
}
#[repr(C)]
struct PAINTSTRUCT { hdc: HDC, fErase: i32, rcPaint: RECT, fRestore: i32, fIncUpdate: i32, rgbReserved: [u8; 32] }
#[repr(C)]
struct BITMAPINFOHEADER {
    biSize: u32, biWidth: i32, biHeight: i32, biPlanes: u16, biBitCount: u16, biCompression: u32,
    biSizeImage: u32, biXPelsPerMeter: i32, biYPelsPerMeter: i32, biClrUsed: u32, biClrImportant: u32,
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassExW(c: *const WNDCLASSEXW) -> u16;
    fn CreateWindowExW(ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: HWND, menu: *mut c_void, inst: HINSTANCE, param: *mut c_void) -> HWND;
    fn DefWindowProcW(h: HWND, m: u32, w: usize, l: isize) -> isize;
    fn GetMessageW(m: *mut MSG, h: HWND, min: u32, max: u32) -> i32;
    fn TranslateMessage(m: *const MSG) -> i32;
    fn DispatchMessageW(m: *const MSG) -> isize;
    fn PostQuitMessage(code: i32);
    fn ShowWindow(h: HWND, cmd: i32) -> i32;
    fn UpdateWindow(h: HWND) -> i32;
    fn LoadCursorW(inst: HINSTANCE, name: usize) -> *mut c_void;
    fn AdjustWindowRect(r: *mut RECT, style: u32, menu: i32) -> i32;
    fn BeginPaint(h: HWND, p: *mut PAINTSTRUCT) -> HDC;
    fn EndPaint(h: HWND, p: *const PAINTSTRUCT) -> i32;
    fn InvalidateRect(h: HWND, r: *const RECT, erase: i32) -> i32;
    fn SetTimer(h: HWND, id: usize, ms: u32, f: *const c_void) -> usize;
    fn SetCapture(h: HWND) -> HWND;
    fn ReleaseCapture() -> i32;
    fn SetProcessDPIAware() -> i32;
    fn GetDC(h: HWND) -> HDC;
    fn ReleaseDC(h: HWND, dc: HDC) -> i32;
    fn ScreenToClient(h: HWND, p: *mut POINT) -> i32;
    fn SetFocus(h: HWND) -> HWND;
    fn GetKeyState(vk: i32) -> i16;
}
#[link(name = "gdi32")]
extern "system" {
    fn GetDeviceCaps(dc: HDC, index: i32) -> i32;
    fn StretchDIBits(dc: HDC, xd: i32, yd: i32, wd: i32, hd: i32, xs: i32, ys: i32, ws: i32, hs: i32, bits: *const c_void, info: *const BITMAPINFOHEADER, usage: u32, rop: u32) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
}

const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_TIMER: u32 = 0x0113;
const WM_KEYDOWN: u32 = 0x0100;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const CS_HREDRAW_VREDRAW: u32 = 0x0003;
const SW_SHOW: i32 = 5;
const LOGPIXELSX: i32 = 88;
const CW_USEDEFAULT: i32 = i32::MIN;
const TIMER_ID: usize = 1;

// Touched only from the thread that runs the message loop.
static mut CORE: *mut Core = null_mut();
static mut SCALE: f32 = 1.0;

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }

unsafe fn core<'a>() -> Option<&'a mut Core> {
    let p = CORE;
    if p.is_null() { None } else { Some(&mut *p) }
}

fn mouse(lparam: isize) -> (f32, f32) {
    let (x, y) = (lparam as u32 as u16 as i16, (lparam as u32 >> 16) as u16 as i16);
    let s = unsafe { SCALE };
    (x as f32 / s, y as f32 / s)
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_ERASEBKGND => return 1,
        WM_SIZE => {
            if let Some(c) = core() {
                let (w, h) = (lparam as u32 & 0xffff, (lparam as u32 >> 16) & 0xffff);
                if w > 0 && h > 0 {
                    let s = SCALE;
                    c.resize(w as f32 / s, h as f32 / s, s);
                }
            }
            return 0;
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            if let Some(c) = core() {
                let fb = c.render();
                let info = BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: fb.width as i32,
                    biHeight: -(fb.height as i32),
                    biPlanes: 1, biBitCount: 32, biCompression: 0, biSizeImage: 0,
                    biXPelsPerMeter: 0, biYPelsPerMeter: 0, biClrUsed: 0, biClrImportant: 0,
                };
                StretchDIBits(dc, 0, 0, fb.width as i32, fb.height as i32, 0, 0, fb.width as i32, fb.height as i32,
                    fb.pixels.as_ptr() as *const c_void, &info, 0, 0x00CC_0020);
            }
            EndPaint(hwnd, &ps);
            return 0;
        }
        WM_TIMER => {
            if let Some(c) = core() {
                if let Some(code) = c.smoke_result() { std::process::exit(code); }
                if c.tick() { InvalidateRect(hwnd, null(), 0); }
            }
            return 0;
        }
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP => {
            let (x, y) = mouse(lparam);
            if msg == WM_LBUTTONDOWN { SetCapture(hwnd); }
            if msg == WM_LBUTTONUP { ReleaseCapture(); }
            if let Some(c) = core() {
                let e = match msg {
                    WM_MOUSEMOVE => Event::MouseMove { x, y },
                    WM_LBUTTONDOWN => Event::MouseDown { x, y },
                    _ => Event::MouseUp { x, y },
                };
                if c.event(e) { InvalidateRect(hwnd, null(), 0); }
            }
            return 0;
        }
        WM_MOUSEWHEEL => {
            // The wheel reports screen coordinates; WHEEL_DELTA is 120 per notch, positive away from the user.
            let mut pt = POINT { x: lparam as u32 as u16 as i16 as i32, y: (lparam as u32 >> 16) as u16 as i16 as i32 };
            ScreenToClient(hwnd, &mut pt);
            let notches = ((wparam >> 16) as u16 as i16) as f32 / 120.0;
            let s = SCALE;
            if let Some(c) = core() {
                // Shell wheel events are positive when content moves up, the opposite of the wheel.
                let e = Event::MouseWheel { x: pt.x as f32 / s, y: pt.y as f32 / s, dy: -notches };
                if c.event(e) { InvalidateRect(hwnd, null(), 0); }
            }
            return 0;
        }
        WM_KEYDOWN => {
            let key = match wparam {
                0x09 => Some(Key::Tab),
                0x0D => Some(Key::Enter),
                0x20 => Some(Key::Space),
                0x26 => Some(Key::Up),
                0x28 => Some(Key::Down),
                0x1B => Some(Key::Escape),
                _ => None,
            };
            if let (Some(key), Some(c)) = (key, core()) {
                let shift = GetKeyState(0x10) < 0;
                if c.event(Event::Key { key, shift }) { InvalidateRect(hwnd, null(), 0); }
                return 0;
            }
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            return 0;
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

pub(crate) fn run(mut core: Core) -> i32 {
    unsafe {
        SetProcessDPIAware();
        let dc = GetDC(null_mut());
        let dpi = if dc.is_null() { 96 } else { GetDeviceCaps(dc, LOGPIXELSX) };
        if !dc.is_null() { ReleaseDC(null_mut(), dc); }
        let scale = if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 };
        SCALE = scale;

        let inst = GetModuleHandleW(null());
        let class = wide("GoodLookinDriShell");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, style: CS_HREDRAW_VREDRAW,
            lpfnWndProc: Some(wnd_proc), cbClsExtra: 0, cbWndExtra: 0, hInstance: inst,
            hIcon: null_mut(), hCursor: LoadCursorW(null_mut(), 32512), hbrBackground: null_mut(),
            lpszMenuName: null(), lpszClassName: class.as_ptr(), hIconSm: null_mut(),
        };
        if RegisterClassExW(&wc) == 0 {
            eprintln!("goodlookindri-shell: RegisterClassExW failed");
            return 1;
        }
        let (cw, ch) = ((core.cfg.width as f32 * scale).round() as i32, (core.cfg.height as f32 * scale).round() as i32);
        let mut r = RECT { left: 0, top: 0, right: cw, bottom: ch };
        AdjustWindowRect(&mut r, WS_OVERLAPPEDWINDOW, 0);
        core.resize(core.cfg.width as f32, core.cfg.height as f32, scale);
        let title = wide(&core.cfg.title);
        CORE = &mut core;
        let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), WS_OVERLAPPEDWINDOW, CW_USEDEFAULT, CW_USEDEFAULT,
            r.right - r.left, r.bottom - r.top, null_mut(), null_mut(), inst, null_mut());
        if hwnd.is_null() {
            CORE = null_mut();
            eprintln!("goodlookindri-shell: CreateWindowExW failed");
            return 1;
        }
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        SetFocus(hwnd);
        SetTimer(hwnd, TIMER_ID, 16, null());
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        CORE = null_mut();
        msg.wparam as i32
    }
}
