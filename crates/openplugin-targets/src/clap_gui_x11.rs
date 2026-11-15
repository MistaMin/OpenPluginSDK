//! `clap.gui` on Linux: the editor in an X11 child window, embedded in the
//! host's window (`CLAP_WINDOW_API_X11`, never floating). The structure
//! follows the macOS module in `clap.rs`; the differences are the window
//! handle, the units of a size and who runs the event loop.
//!
//! Event loop. A Linux host runs no event loop for the view's own X
//! connection, so the view is driven from the host's: `create` registers a
//! timer of [`x11::TICK_INTERVAL_MS`] with `clap.timer-support`, and, when the
//! host offers `clap.posix-fd-support`, the view's X descriptor for reading.
//! Both calls land in [`x11::EditorView::pump`]. A host without timer support
//! cannot drive the view, so `create` fails there instead of showing a window
//! that never draws. The timer and the descriptor are unregistered whenever
//! the gui goes (`destroy`, a second `create`, the plugin's `destroy`).
//!
//! Size. CLAP sizes on X11 are physical pixels; the view and the editor work
//! in points. Every size crossing the ABI goes through [`pixels_per_point`],
//! which is 1.0 until the view says what scale it renders at. `set_scale`
//! returns false: the view, not the host, decides its scale.
//!
//! Resizing follows the macOS module: `can_resize`, `get_resize_hints`,
//! `adjust_size` and `set_size` follow the editor's `SizeRange`, and the
//! editor asks the host for a size through `clap_host_gui::request_resize`.
//! An X11 child window does not resize its parent, so when the editor changes
//! its own size during a pump, the host is asked for that size as well.
//!
//! Main thread only, like every `clap.gui`, timer and descriptor call.

use super::*;
use openplugin_core::editor::SizeRange;
use openplugin_core::{Editor, ValueMirror};
use openplugin_dri::x11;
use std::cell::Cell;
use std::ffi::c_ulong;
use std::os::fd::RawFd;
use std::rc::Rc;

/// What the gui needs from its view: [`x11::EditorView`], or a stand-in in tests.
pub(super) trait View {
    /// The editor's size in points.
    fn size(&self) -> (u32, u32);
    fn size_range(&self) -> SizeRange;
    fn resize(&mut self, width: u32, height: u32) -> (u32, u32);
    fn attach(&mut self, parent: x11::Window) -> bool;
    fn set_hidden(&mut self, hidden: bool);
    fn editor(&mut self) -> Option<&mut (dyn Editor + 'static)>;
    /// The descriptor to watch, or negative for none.
    fn fd(&self) -> RawFd;
    fn pump(&mut self);
}

impl View for x11::EditorView {
    fn size(&self) -> (u32, u32) {
        x11::EditorView::size(self)
    }
    fn size_range(&self) -> SizeRange {
        x11::EditorView::size_range(self)
    }
    fn resize(&mut self, width: u32, height: u32) -> (u32, u32) {
        x11::EditorView::resize(self, width, height)
    }
    fn attach(&mut self, parent: x11::Window) -> bool {
        x11::EditorView::attach(self, parent)
    }
    fn set_hidden(&mut self, hidden: bool) {
        x11::EditorView::set_hidden(self, hidden)
    }
    fn editor(&mut self) -> Option<&mut (dyn Editor + 'static)> {
        x11::EditorView::editor(self)
    }
    fn fd(&self) -> RawFd {
        x11::EditorView::fd(self)
    }
    fn pump(&mut self) {
        x11::EditorView::pump(self)
    }
}

fn make_x11_view(editor: Box<dyn Editor>, poll: x11::Poll) -> Option<Box<dyn View>> {
    x11::EditorView::new(editor, Some(poll)).map(|view| Box::new(view) as Box<dyn View>)
}

/// Physical pixels per editor point, for every size `clap.gui` passes on X11.
///
/// SCALE: always 1.0 for now. `x11::EditorView` does not yet report the scale
/// it renders at; once it does, return that here and nothing else changes.
fn pixels_per_point(_view: &dyn View) -> f64 {
    1.0
}

fn to_pixels(scale: f64, (w, h): (u32, u32)) -> (u32, u32) {
    let px = |v: u32| (f64::from(v) * scale).round().clamp(0.0, f64::from(u32::MAX)) as u32;
    (px(w), px(h))
}

fn to_points(scale: f64, (w, h): (u32, u32)) -> (u32, u32) {
    let pt = |v: u32| (f64::from(v) / scale).round().clamp(0.0, f64::from(u32::MAX)) as u32;
    (pt(w), pt(h))
}

pub(super) struct Gui {
    host: *const clap_host,
    host_gui: *const clap_host_gui,
    timers: *const clap_host_timer_support,
    timer: u32,
    fds: *const clap_host_posix_fd_support,
    /// The descriptor the host watches for us, when it took it.
    fd: Option<RawFd>,
    /// [`pixels_per_point`], shared with the editor's resize request.
    scale: Rc<Cell<f64>>,
    /// The size in points the host last gave or was asked for.
    known: (u32, u32),
    /// Inside `view.pump()`: a `set_size` the host makes from a request made
    /// there waits in `pending` until the pump returns.
    pumping: bool,
    pending: Option<(u32, u32)>,
    pub(super) view: Box<dyn View>,
}

impl Drop for Gui {
    /// The host stops calling before the view closes its connection.
    fn drop(&mut self) {
        if let (Some(fd), Some(fds)) = (self.fd.take(), unsafe { self.fds.as_ref() }) {
            if let Some(unregister) = fds.unregister_fd {
                unsafe { unregister(self.host, fd) };
            }
        }
        if let Some(timers) = unsafe { self.timers.as_ref() } {
            if let Some(unregister) = timers.unregister_timer {
                unsafe { unregister(self.host, self.timer) };
            }
        }
    }
}

fn is_x11(api: *const c_char, floating: bool) -> bool {
    !api.is_null() && !floating && unsafe { CStr::from_ptr(api) }.to_bytes_with_nul() == WINDOW_API_X11
}

unsafe extern "C" fn is_api_supported(_plugin: *const clap_plugin, api: *const c_char, floating: bool) -> bool {
    guarded(std::ptr::null(), false, || is_x11(api, floating))
}

unsafe extern "C" fn get_preferred_api(_plugin: *const clap_plugin, api: *mut *const c_char, floating: *mut bool) -> bool {
    guarded(std::ptr::null(), false, || {
        if api.is_null() || floating.is_null() {
            return false;
        }
        unsafe {
            *api = WINDOW_API_X11.as_ptr() as *const c_char;
            *floating = false;
        }
        true
    })
}

fn request_resize(host: *const clap_host, host_gui: *const clap_host_gui, width: u32, height: u32) -> bool {
    let Some(gui) = (unsafe { host_gui.as_ref() }) else { return false };
    match gui.request_resize {
        Some(f) => unsafe { f(host, width, height) },
        None => false,
    }
}

/// `create` with the editor factory and the view maker given, so tests can
/// pass their own.
pub(super) fn create_with(
    plugin: *const clap_plugin,
    api: *const c_char,
    floating: bool,
    factory: Option<EditorFactory>,
    make: impl FnOnce(Box<dyn Editor>, x11::Poll) -> Option<Box<dyn View>>,
) -> bool {
    if !is_x11(api, floating) {
        return false;
    }
    let Some(instance) = instance(plugin) else { return false };
    let Some(factory) = factory else { return false };
    instance.gui = None;
    let host = instance.host;
    // Without the host's timer nothing would ever pump the view.
    let timers = host_call(host, EXT_TIMER_SUPPORT) as *const clap_host_timer_support;
    let Some(register_timer) = (unsafe { timers.as_ref() }).and_then(|t| t.unregister_timer.and(t.register_timer)) else {
        return false;
    };
    let host_gui = host_call(host, EXT_GUI) as *const clap_host_gui;
    let scale = Rc::new(Cell::new(1.0));
    let request = {
        let scale = scale.clone();
        move |w: u32, h: u32| -> bool {
            let (pw, ph) = to_pixels(scale.get(), (w, h));
            request_resize(host, host_gui, pw, ph)
        }
    };
    let Some(mut editor) = factory(editor_host_for(instance).with_resize(request)) else { return false };
    editor.bind(instance.editor_state.clone());
    if let Some(info) = instance.track_info.as_ref() {
        editor.track_info(info);
    }
    let plane = instance.plane.clone();
    let mut mirror = ValueMirror::new(plane.len());
    // The current values before the first frame, then whatever changes.
    mirror.update(editor.as_mut(), |i| plane.load(i));
    let poll: x11::Poll = Box::new(move |editor: &mut dyn Editor| mirror.update(editor, |i| plane.load(i)));
    let Some(view) = make(editor, poll) else { return false };
    let mut timer = CLAP_INVALID_ID;
    if !unsafe { register_timer(host, x11::TICK_INTERVAL_MS, &mut timer) } {
        return false;
    }
    scale.set(pixels_per_point(view.as_ref()));
    let known = view.size();
    let mut gui = Gui {
        host,
        host_gui,
        timers,
        timer,
        fds: std::ptr::null(),
        fd: None,
        scale,
        known,
        pumping: false,
        pending: None,
        view,
    };
    // A host that watches descriptors gets input to the view at once; the
    // timer alone still drives it at 30 Hz.
    let fd = gui.view.fd();
    let fds = host_call(host, EXT_POSIX_FD_SUPPORT) as *const clap_host_posix_fd_support;
    if let Some(ext) = unsafe { fds.as_ref() } {
        if let (Some(register), Some(_)) = (ext.register_fd, ext.unregister_fd) {
            if fd >= 0 && unsafe { register(host, fd, POSIX_FD_READ) } {
                gui.fds = fds;
                gui.fd = Some(fd);
            }
        }
    }
    instance.gui = Some(gui);
    true
}

unsafe extern "C" fn create(plugin: *const clap_plugin, api: *const c_char, floating: bool) -> bool {
    guarded(std::ptr::null(), false, || create_with(plugin, api, floating, editor_factory(), make_x11_view))
}

unsafe extern "C" fn destroy(plugin: *const clap_plugin) {
    guarded(std::ptr::null(), (), || {
        if let Some(instance) = instance(plugin) {
            instance.gui = None;
        }
    })
}

/// Handle the view's events and tick, then tell the host about a size the
/// editor took by itself.
fn pump(plugin: *const clap_plugin) {
    let Some(inst) = instance(plugin) else { return };
    let host = inst.host;
    let Some(gui) = inst.gui.as_mut() else { return };
    if gui.pumping {
        return;
    }
    gui.pumping = true;
    gui.view.pump();
    gui.pumping = false;
    if let Some((w, h)) = gui.pending.take() {
        gui.view.resize(w, h);
        gui.known = gui.view.size();
    }
    gui.scale.set(pixels_per_point(gui.view.as_ref()));
    let now = gui.view.size();
    if now == gui.known || now.0 == 0 || now.1 == 0 {
        return;
    }
    gui.known = now;
    let (w, h) = to_pixels(gui.scale.get(), now);
    let host_gui = gui.host_gui;
    // No borrow of the gui is held: the host may answer with `set_size` at once.
    request_resize(host, host_gui, w, h);
}

unsafe extern "C" fn on_timer(plugin: *const clap_plugin, timer_id: u32) {
    guarded(std::ptr::null(), (), || {
        let ours = instance(plugin).and_then(|inst| inst.gui.as_ref()).is_some_and(|gui| gui.timer == timer_id);
        if ours {
            pump(plugin);
        }
    })
}

unsafe extern "C" fn on_fd(plugin: *const clap_plugin, fd: i32, _flags: u32) {
    guarded(std::ptr::null(), (), || {
        let ours = instance(plugin).and_then(|inst| inst.gui.as_ref()).is_some_and(|gui| gui.fd == Some(fd));
        if ours {
            pump(plugin);
        }
    })
}

static TIMER_SUPPORT: clap_plugin_timer_support = clap_plugin_timer_support { on_timer: Some(on_timer) };
static POSIX_FD_SUPPORT: clap_plugin_posix_fd_support = clap_plugin_posix_fd_support { on_fd: Some(on_fd) };

/// `clap.timer-support` and `clap.posix-fd-support`, offered with `clap.gui`.
pub(super) fn driver_extension(id: &[u8]) -> Option<*const c_void> {
    editor_factory()?;
    if id == EXT_TIMER_SUPPORT {
        return Some(&TIMER_SUPPORT as *const clap_plugin_timer_support as *const c_void);
    }
    if id == EXT_POSIX_FD_SUPPORT {
        return Some(&POSIX_FD_SUPPORT as *const clap_plugin_posix_fd_support as *const c_void);
    }
    None
}

unsafe extern "C" fn set_scale(_plugin: *const clap_plugin, _scale: f64) -> bool {
    guarded(std::ptr::null(), false, || {
        // The view decides the scale it renders at (see `pixels_per_point`).
        false
    })
}

fn gui_of<'a>(plugin: *const clap_plugin) -> Option<&'a mut Gui> {
    instance(plugin).and_then(|inst| inst.gui.as_mut())
}

unsafe extern "C" fn get_size(plugin: *const clap_plugin, width: *mut u32, height: *mut u32) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(gui) = gui_of(plugin) else { return false };
        if width.is_null() || height.is_null() {
            return false;
        }
        let (w, h) = to_pixels(gui.scale.get(), gui.view.size());
        unsafe {
            *width = w;
            *height = h;
        }
        true
    })
}

unsafe extern "C" fn can_resize(plugin: *const clap_plugin) -> bool {
    guarded(std::ptr::null(), false, || gui_of(plugin).is_some_and(|gui| !gui.view.size_range().is_fixed()))
}

unsafe extern "C" fn get_resize_hints(plugin: *const clap_plugin, hints: *mut clap_gui_resize_hints) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(r) = gui_of(plugin).map(|gui| gui.view.size_range()) else { return false };
        if hints.is_null() || r.is_fixed() {
            return false;
        }
        let (aw, ah) = r.aspect_ratio().unwrap_or((0, 0));
        unsafe {
            *hints = clap_gui_resize_hints {
                can_resize_horizontally: r.can_resize_horizontally(),
                can_resize_vertically: r.can_resize_vertically(),
                preserve_aspect_ratio: aw > 0,
                aspect_ratio_width: aw,
                aspect_ratio_height: ah,
            };
        }
        true
    })
}

/// The size nearest to the one proposed that the editor takes, in pixels.
unsafe extern "C" fn adjust_size(plugin: *const clap_plugin, width: *mut u32, height: *mut u32) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(gui) = gui_of(plugin) else { return false };
        if width.is_null() || height.is_null() {
            return false;
        }
        let scale = gui.scale.get();
        unsafe {
            let (w, h) = to_points(scale, (*width, *height));
            let (w, h) = to_pixels(scale, gui.view.size_range().clamp(w, h));
            *width = w;
            *height = h;
        }
        true
    })
}

/// Take a size inside the editor's bounds, as on macOS. A size the host sets
/// while answering a request made inside a pump is applied when the pump ends.
unsafe extern "C" fn set_size(plugin: *const clap_plugin, width: u32, height: u32) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(gui) = gui_of(plugin) else { return false };
        let (w, h) = to_points(gui.scale.get(), (width, height));
        if !gui.view.size_range().contains(w, h) {
            return false;
        }
        if gui.pumping {
            gui.pending = Some((w, h));
            return true;
        }
        gui.view.resize(w, h);
        gui.known = gui.view.size();
        true
    })
}

// `clap_window` holds a union of handles; on X11 it is an `unsigned long`.
// Linux targets are ILP32 or LP64, so that is the size of the pointer member
// declared in `clap_abi`, and reading the pointer reads the whole XID.
const _: () = assert!(std::mem::size_of::<c_ulong>() == std::mem::size_of::<*mut c_void>());

/// The parent's XID from a `clap_window`, if it is an X11 window that fits an XID.
fn x11_parent(window: &clap_window) -> Option<x11::Window> {
    if !window.api.is_null() && unsafe { CStr::from_ptr(window.api) }.to_bytes_with_nul() != WINDOW_API_X11 {
        return None;
    }
    let xid = window.ptr as usize as c_ulong;
    x11::Window::try_from(xid).ok().filter(|&xid| xid != 0)
}

unsafe extern "C" fn set_parent(plugin: *const clap_plugin, window: *const clap_window) -> bool {
    guarded(std::ptr::null(), false, || {
        let Some(gui) = gui_of(plugin) else { return false };
        let Some(window) = (unsafe { window.as_ref() }) else { return false };
        match x11_parent(window) {
            Some(parent) => gui.view.attach(parent),
            None => false,
        }
    })
}

unsafe extern "C" fn set_transient(_plugin: *const clap_plugin, _window: *const clap_window) -> bool {
    false
}

unsafe extern "C" fn suggest_title(_plugin: *const clap_plugin, _title: *const c_char) {}

fn set_hidden(plugin: *const clap_plugin, hidden: bool) -> bool {
    match gui_of(plugin) {
        Some(gui) => {
            gui.view.set_hidden(hidden);
            true
        }
        None => false,
    }
}

unsafe extern "C" fn show(plugin: *const clap_plugin) -> bool {
    guarded(std::ptr::null(), false, || set_hidden(plugin, false))
}

unsafe extern "C" fn hide(plugin: *const clap_plugin) -> bool {
    guarded(std::ptr::null(), false, || set_hidden(plugin, true))
}

pub(super) static GUI: clap_plugin_gui = clap_plugin_gui {
    is_api_supported: Some(is_api_supported),
    get_preferred_api: Some(get_preferred_api),
    create: Some(create),
    destroy: Some(destroy),
    set_scale: Some(set_scale),
    get_size: Some(get_size),
    can_resize: Some(can_resize),
    get_resize_hints: Some(get_resize_hints),
    adjust_size: Some(adjust_size),
    set_size: Some(set_size),
    set_parent: Some(set_parent),
    set_transient: Some(set_transient),
    suggest_title: Some(suggest_title),
    show: Some(show),
    hide: Some(hide),
};

#[cfg(test)]
mod tests {
    //! A fake host with timer, descriptor and gui support that records every
    //! call, and a fake view, so the gui runs without an X server.
    use super::*;
    use openplugin_core::{Category, EditorHost, Frame, ParamInfo, PluginInfo, Unit};
    use std::cell::RefCell;

    struct Quiet;

    static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];

    static INFO: PluginInfo = PluginInfo {
        id: "com.opengrid.test.x11gui",
        name: "X11 Gui",
        vendor: "OpenGrid",
        url: "https://github.com/MistaMin/OpenPluginSDK",
        email: "contact@mistamin.com",
        version: "0.1.0",
        category: Category::Utility,
        params: &PARAMS,
        is_instrument: false,
        has_editor: true,
    };

    impl Plugin for Quiet {
        fn info(&self) -> &'static PluginInfo { &INFO }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, _: &str, _: f32) -> bool { false }
        fn param(&self, _: &str) -> Option<f32> { None }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Call {
        RegisterTimer(u32),
        UnregisterTimer(u32),
        RegisterFd(i32, u32),
        UnregisterFd(i32),
        RequestResize(u32, u32),
    }

    struct FakeHost {
        offer_timer: bool,
        timer_ok: bool,
        offer_fd: bool,
        calls: Mutex<Vec<Call>>,
    }

    const TIMER_ID: u32 = 7;
    const FAKE_FD: i32 = 42;

    impl FakeHost {
        fn new(offer_timer: bool, offer_fd: bool) -> Box<Self> {
            Box::new(Self { offer_timer, timer_ok: true, offer_fd, calls: Mutex::new(Vec::new()) })
        }

        fn clap_host(&self) -> clap_host {
            clap_host {
                clap_version: CLAP_VERSION,
                host_data: self as *const FakeHost as *mut c_void,
                name: c"fake".as_ptr(),
                vendor: c"fake".as_ptr(),
                url: c"".as_ptr(),
                version: c"1".as_ptr(),
                get_extension: Some(fake_extension),
                request_restart: None,
                request_process: None,
                request_callback: None,
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        fn clear(&self) {
            self.calls.lock().unwrap().clear();
        }

        /// Every registered timer and descriptor was unregistered.
        fn balanced(&self) -> bool {
            let calls = self.calls();
            let count = |c: Call| calls.iter().filter(|&&x| x == c).count();
            count(Call::RegisterTimer(x11::TICK_INTERVAL_MS)) == count(Call::UnregisterTimer(TIMER_ID))
                && count(Call::RegisterFd(FAKE_FD, POSIX_FD_READ)) == count(Call::UnregisterFd(FAKE_FD))
        }
    }

    fn fake<'a>(host: *const clap_host) -> &'a FakeHost {
        unsafe { &*((*host).host_data as *const FakeHost) }
    }

    unsafe extern "C" fn register_timer(host: *const clap_host, period: u32, id: *mut u32) -> bool {
        let this = fake(host);
        this.calls.lock().unwrap().push(Call::RegisterTimer(period));
        if this.timer_ok {
            unsafe { *id = TIMER_ID };
        }
        this.timer_ok
    }

    unsafe extern "C" fn unregister_timer(host: *const clap_host, id: u32) -> bool {
        fake(host).calls.lock().unwrap().push(Call::UnregisterTimer(id));
        true
    }

    unsafe extern "C" fn register_fd(host: *const clap_host, fd: i32, flags: u32) -> bool {
        fake(host).calls.lock().unwrap().push(Call::RegisterFd(fd, flags));
        true
    }

    unsafe extern "C" fn unregister_fd(host: *const clap_host, fd: i32) -> bool {
        fake(host).calls.lock().unwrap().push(Call::UnregisterFd(fd));
        true
    }

    unsafe extern "C" fn host_request_resize(host: *const clap_host, w: u32, h: u32) -> bool {
        fake(host).calls.lock().unwrap().push(Call::RequestResize(w, h));
        true
    }

    static HOST_TIMERS: clap_host_timer_support =
        clap_host_timer_support { register_timer: Some(register_timer), unregister_timer: Some(unregister_timer) };
    static HOST_FDS: clap_host_posix_fd_support =
        clap_host_posix_fd_support { register_fd: Some(register_fd), modify_fd: None, unregister_fd: Some(unregister_fd) };
    static HOST_GUI: clap_host_gui = clap_host_gui {
        resize_hints_changed: None,
        request_resize: Some(host_request_resize),
        request_show: None,
        request_hide: None,
        closed: None,
    };

    unsafe extern "C" fn fake_extension(host: *const clap_host, id: *const c_char) -> *const c_void {
        let this = fake(host);
        let id = unsafe { CStr::from_ptr(id) }.to_bytes_with_nul();
        if this.offer_timer && id == EXT_TIMER_SUPPORT {
            return &HOST_TIMERS as *const clap_host_timer_support as *const c_void;
        }
        if this.offer_fd && id == EXT_POSIX_FD_SUPPORT {
            return &HOST_FDS as *const clap_host_posix_fd_support as *const c_void;
        }
        if id == EXT_GUI {
            return &HOST_GUI as *const clap_host_gui as *const c_void;
        }
        std::ptr::null()
    }

    /// What the fake editor and view saw, per test thread.
    #[derive(Default)]
    struct Probe {
        pumps: u32,
        params: Vec<(usize, f32)>,
        attached: Option<u32>,
        hidden: Option<bool>,
        /// The editor takes this size by itself at the next tick.
        grow_to: Option<(u32, u32)>,
        made: u32,
    }

    thread_local! {
        static PROBE: RefCell<Probe> = RefCell::new(Probe::default());
    }

    fn probe<R>(f: impl FnOnce(&mut Probe) -> R) -> R {
        PROBE.with(|p| f(&mut p.borrow_mut()))
    }

    struct TestEditor {
        size: (u32, u32),
        pixels: Vec<u32>,
    }

    impl Editor for TestEditor {
        fn size(&self) -> (u32, u32) {
            self.size
        }
        fn size_range(&self) -> SizeRange {
            SizeRange::between((200, 100), (800, 400))
        }
        fn resize(&mut self, width: u32, height: u32) {
            self.size = (width, height);
        }
        fn param_changed(&mut self, index: usize, plain: f32) {
            probe(|p| p.params.push((index, plain)));
        }
        fn tick(&mut self, _dt: f32) {
            if let Some(size) = probe(|p| p.grow_to.take()) {
                self.size = size;
            }
        }
        fn render(&mut self, _scale: f32) -> Frame<'_> {
            Frame { pixels: &self.pixels, width: 0, height: 0 }
        }
    }

    fn test_factory(_host: EditorHost) -> Option<Box<dyn Editor>> {
        Some(Box::new(TestEditor { size: (400, 200), pixels: Vec::new() }))
    }

    struct FakeView {
        editor: Box<dyn Editor>,
        poll: x11::Poll,
    }

    impl View for FakeView {
        fn size(&self) -> (u32, u32) {
            self.editor.size()
        }
        fn size_range(&self) -> SizeRange {
            self.editor.size_range()
        }
        fn resize(&mut self, width: u32, height: u32) -> (u32, u32) {
            let (w, h) = self.size_range().clamp(width, height);
            self.editor.resize(w, h);
            self.editor.size()
        }
        fn attach(&mut self, parent: x11::Window) -> bool {
            probe(|p| p.attached = Some(parent));
            true
        }
        fn set_hidden(&mut self, hidden: bool) {
            probe(|p| p.hidden = Some(hidden));
        }
        fn editor(&mut self) -> Option<&mut (dyn Editor + 'static)> {
            Some(self.editor.as_mut())
        }
        fn fd(&self) -> RawFd {
            FAKE_FD
        }
        fn pump(&mut self) {
            probe(|p| p.pumps += 1);
            (self.poll)(self.editor.as_mut());
            self.editor.tick(0.033);
        }
    }

    fn fake_view(editor: Box<dyn Editor>, poll: x11::Poll) -> Option<Box<dyn View>> {
        probe(|p| p.made += 1);
        Some(Box::new(FakeView { editor, poll }))
    }

    fn no_view(_editor: Box<dyn Editor>, _poll: x11::Poll) -> Option<Box<dyn View>> {
        probe(|p| p.made += 1);
        None
    }

    const X11: *const c_char = WINDOW_API_X11.as_ptr() as *const c_char;

    fn create_fake(plugin: *const clap_plugin) -> bool {
        create_with(plugin, X11, false, Some(test_factory), fake_view)
    }

    fn has_gui(plugin: *const clap_plugin) -> bool {
        instance(plugin).is_some_and(|inst| inst.gui.is_some())
    }

    #[test]
    fn x11_is_the_only_api() {
        let adapter = ClapAdapter::new(Quiet);
        let plugin = adapter.as_plugin();
        unsafe {
            assert!(is_api_supported(plugin, X11, false));
            assert!(!is_api_supported(plugin, X11, true));
            for api in [c"cocoa", c"wayland", c"win32", c""] {
                assert!(!is_api_supported(plugin, api.as_ptr(), false));
                assert!(!is_api_supported(plugin, api.as_ptr(), true));
            }
            assert!(!is_api_supported(plugin, std::ptr::null(), false));
            let mut api: *const c_char = std::ptr::null();
            let mut floating = true;
            assert!(get_preferred_api(plugin, &mut api, &mut floating));
            assert_eq!(CStr::from_ptr(api).to_bytes(), b"x11");
            assert!(!floating);
            assert!(!get_preferred_api(plugin, std::ptr::null_mut(), &mut floating));
        }
        // Wrong API or floating: no editor is made at all.
        assert!(!create_with(plugin, c"wayland".as_ptr(), false, Some(test_factory), fake_view));
        assert!(!create_with(plugin, X11, true, Some(test_factory), fake_view));
        assert_eq!(probe(|p| p.made), 0);
    }

    #[test]
    fn nothing_is_offered_without_an_editor_factory() {
        // This test binary never installs an editor.
        let host = FakeHost::new(true, true);
        let ch = host.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Quiet, &ch) };
        let plugin = adapter.as_plugin();
        for id in [EXT_GUI, EXT_TIMER_SUPPORT, EXT_POSIX_FD_SUPPORT] {
            assert!(unsafe { plugin_extension(plugin, id.as_ptr() as *const c_char) }.is_null());
        }
        assert!(!unsafe { create(plugin, X11, false) });
        assert!(host.calls().is_empty());
    }

    #[test]
    fn create_fails_cleanly_when_the_view_cannot_be_made() {
        let host = FakeHost::new(true, true);
        let ch = host.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Quiet, &ch) };
        assert!(!create_with(adapter.as_plugin(), X11, false, Some(test_factory), no_view));
        assert_eq!(probe(|p| p.made), 1);
        assert!(!has_gui(adapter.as_plugin()));
        assert!(host.calls().is_empty());
    }

    #[test]
    fn create_fails_without_host_timer_support() {
        let host = FakeHost::new(false, true);
        let ch = host.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Quiet, &ch) };
        assert!(!create_fake(adapter.as_plugin()));
        assert_eq!(probe(|p| p.made), 0, "no view is made for a host that cannot drive it");
        assert!(!has_gui(adapter.as_plugin()));
        assert!(host.calls().is_empty());
        // No host at all.
        let bare = ClapAdapter::new(Quiet);
        assert!(!create_fake(bare.as_plugin()));
    }

    #[test]
    fn create_fails_when_the_host_refuses_the_timer() {
        let mut host = FakeHost::new(true, true);
        host.timer_ok = false;
        let ch = host.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Quiet, &ch) };
        assert!(!create_fake(adapter.as_plugin()));
        assert!(!has_gui(adapter.as_plugin()));
        assert_eq!(host.calls(), vec![Call::RegisterTimer(x11::TICK_INTERVAL_MS)]);
    }

    #[test]
    fn timer_and_fd_are_registered_and_always_unregistered() {
        let host = FakeHost::new(true, true);
        let ch = host.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Quiet, &ch) };
        let plugin = adapter.as_plugin();
        assert!(create_fake(plugin));
        assert_eq!(host.calls(), vec![Call::RegisterTimer(x11::TICK_INTERVAL_MS), Call::RegisterFd(FAKE_FD, POSIX_FD_READ)]);

        // destroy: the descriptor first, then the timer.
        unsafe { destroy(plugin) };
        assert!(!has_gui(plugin));
        assert_eq!(&host.calls()[2..], &[Call::UnregisterFd(FAKE_FD), Call::UnregisterTimer(TIMER_ID)]);
        unsafe { destroy(plugin) };
        assert_eq!(host.calls().len(), 4, "a second destroy unregisters nothing");

        // A second create replaces the first gui and unregisters its calls.
        assert!(create_fake(plugin));
        assert!(create_fake(plugin));
        let calls = host.calls();
        assert_eq!(calls.len(), 10);
        assert_eq!(calls[6..8], [Call::UnregisterFd(FAKE_FD), Call::UnregisterTimer(TIMER_ID)]);

        // The plugin's own destroy with a gui open.
        drop(adapter);
        assert!(host.balanced(), "{:?}", host.calls());
    }

    #[test]
    fn a_host_without_fd_support_drives_the_view_by_timer_alone() {
        let host = FakeHost::new(true, false);
        let ch = host.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Quiet, &ch) };
        let plugin = adapter.as_plugin();
        assert!(create_fake(plugin));
        assert_eq!(host.calls(), vec![Call::RegisterTimer(x11::TICK_INTERVAL_MS)]);
        unsafe { on_fd(plugin, FAKE_FD, POSIX_FD_READ) };
        assert_eq!(probe(|p| p.pumps), 0, "a descriptor the host never took is not ours");
        unsafe { on_timer(plugin, TIMER_ID) };
        assert_eq!(probe(|p| p.pumps), 1);
        unsafe { destroy(plugin) };
        assert_eq!(host.calls(), vec![Call::RegisterTimer(x11::TICK_INTERVAL_MS), Call::UnregisterTimer(TIMER_ID)]);
    }

    #[test]
    fn timer_and_fd_pump_the_view_and_the_poll_brings_host_values() {
        let host = FakeHost::new(true, true);
        let ch = host.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Quiet, &ch) };
        let plugin = adapter.as_plugin();
        // Offered through get_extension only with a factory; called directly here.
        assert!(create_fake(plugin));
        probe(|p| p.params.clear());
        unsafe {
            on_timer(plugin, TIMER_ID + 1);
            on_fd(plugin, FAKE_FD + 1, POSIX_FD_READ);
        }
        assert_eq!(probe(|p| p.pumps), 0, "someone else's timer or descriptor");
        unsafe { on_timer(plugin, TIMER_ID) };
        unsafe { on_fd(plugin, FAKE_FD, POSIX_FD_READ) };
        assert_eq!(probe(|p| p.pumps), 2);

        // A value the host set reaches the editor at the next pump.
        assert!(adapter.plane().unwrap().store(0, 6.0));
        unsafe { on_timer(plugin, TIMER_ID) };
        assert_eq!(probe(|p| p.params.clone()), vec![(0, 6.0)]);
        unsafe { destroy(plugin) };
        unsafe { on_timer(plugin, TIMER_ID) };
        assert_eq!(probe(|p| p.pumps), 3, "no gui, no pump");
    }

    #[test]
    fn sizes_follow_the_editor_range() {
        let host = FakeHost::new(true, true);
        let ch = host.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Quiet, &ch) };
        let plugin = adapter.as_plugin();
        let (mut w, mut h) = (0u32, 0u32);
        unsafe {
            assert!(!get_size(plugin, &mut w, &mut h), "no gui yet");
            assert!(!can_resize(plugin));
        }
        assert!(create_fake(plugin));
        host.clear();
        unsafe {
            assert!(get_size(plugin, &mut w, &mut h));
            assert_eq!((w, h), (400, 200));
            assert!(can_resize(plugin));
            let mut hints = clap_gui_resize_hints {
                can_resize_horizontally: false,
                can_resize_vertically: false,
                preserve_aspect_ratio: true,
                aspect_ratio_width: 9,
                aspect_ratio_height: 9,
            };
            assert!(get_resize_hints(plugin, &mut hints));
            assert!(hints.can_resize_horizontally && hints.can_resize_vertically && !hints.preserve_aspect_ratio);
            let (mut aw, mut ah) = (5000u32, 10u32);
            assert!(adjust_size(plugin, &mut aw, &mut ah));
            assert_eq!((aw, ah), (800, 100));
            assert!(!set_size(plugin, 900, 300));
            assert!(set_size(plugin, 600, 300));
            assert!(get_size(plugin, &mut w, &mut h));
            assert_eq!((w, h), (600, 300));
            assert!(!set_scale(plugin, 2.0));
        }
        // A size the host set is not asked back.
        unsafe { on_timer(plugin, TIMER_ID) };
        assert!(host.calls().is_empty(), "{:?}", host.calls());
    }

    #[test]
    fn a_size_the_editor_takes_itself_is_asked_of_the_host() {
        let host = FakeHost::new(true, true);
        let ch = host.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Quiet, &ch) };
        let plugin = adapter.as_plugin();
        assert!(create_fake(plugin));
        host.clear();
        probe(|p| p.grow_to = Some((500, 250)));
        unsafe { on_timer(plugin, TIMER_ID) };
        assert_eq!(host.calls(), vec![Call::RequestResize(500, 250)]);
        unsafe { on_timer(plugin, TIMER_ID) };
        assert_eq!(host.calls().len(), 1, "asked once");
    }

    #[test]
    fn a_set_size_during_a_pump_waits_for_the_pump() {
        let host = FakeHost::new(true, true);
        let ch = host.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Quiet, &ch) };
        let plugin = adapter.as_plugin();
        assert!(create_fake(plugin));
        gui_of(plugin).unwrap().pumping = true;
        assert!(unsafe { set_size(plugin, 300, 150) });
        assert_eq!(gui_of(plugin).unwrap().view.size(), (400, 200));
        gui_of(plugin).unwrap().pumping = false;
        unsafe { on_timer(plugin, TIMER_ID) };
        assert_eq!(gui_of(plugin).unwrap().view.size(), (300, 150));
        assert!(!host.calls().iter().any(|c| matches!(c, Call::RequestResize(..))));
    }

    #[test]
    fn set_parent_takes_an_x11_window_and_show_hide_reach_the_view() {
        let host = FakeHost::new(true, true);
        let ch = host.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Quiet, &ch) };
        let plugin = adapter.as_plugin();
        let window = |api: *const c_char, xid: usize| clap_window { api, ptr: xid as *mut c_void };
        unsafe {
            assert!(!set_parent(plugin, &window(X11, 0x0340_0007)), "no gui yet");
            assert!(!show(plugin));
        }
        assert!(create_fake(plugin));
        unsafe {
            assert!(!set_parent(plugin, std::ptr::null()));
            assert!(!set_parent(plugin, &window(X11, 0)));
            assert!(!set_parent(plugin, &window(c"cocoa".as_ptr(), 0x0340_0007)));
            assert_eq!(probe(|p| p.attached), None);
            assert!(set_parent(plugin, &window(X11, 0x0340_0007)));
            assert_eq!(probe(|p| p.attached), Some(0x0340_0007));
            assert!(set_parent(plugin, &window(std::ptr::null(), 0x0500_0001)));
            assert_eq!(probe(|p| p.attached), Some(0x0500_0001));
            if std::mem::size_of::<usize>() > 4 {
                assert!(!set_parent(plugin, &window(X11, 1usize << 33)), "not an XID");
            }
            assert!(show(plugin));
            assert_eq!(probe(|p| p.hidden), Some(false));
            assert!(hide(plugin));
            assert_eq!(probe(|p| p.hidden), Some(true));
            assert!(!set_transient(plugin, &window(X11, 1)));
        }
    }

    #[test]
    fn the_real_view_either_fails_cleanly_or_registers_and_unregisters() {
        // Today's x11 view has no backend and is never made; once it is, a
        // machine with a display makes it and the calls must still balance.
        let host = FakeHost::new(true, true);
        let ch = host.clap_host();
        let adapter = unsafe { ClapAdapter::with_host(Quiet, &ch) };
        let plugin = adapter.as_plugin();
        if create_with(plugin, X11, false, Some(test_factory), make_x11_view) {
            assert_eq!(host.calls()[0], Call::RegisterTimer(x11::TICK_INTERVAL_MS));
            unsafe { destroy(plugin) };
            let calls = host.calls();
            assert_eq!(calls.last(), Some(&Call::UnregisterTimer(TIMER_ID)));
        } else {
            assert!(!has_gui(plugin));
            assert!(host.calls().is_empty());
        }
    }

    #[test]
    fn scale_conversions_round_trip() {
        assert_eq!(to_pixels(1.0, (400, 200)), (400, 200));
        assert_eq!(to_points(1.0, (400, 200)), (400, 200));
        assert_eq!(to_pixels(2.0, (400, 201)), (800, 402));
        assert_eq!(to_points(2.0, (800, 402)), (400, 201));
        assert_eq!(to_points(1.5, (601, 301)), (401, 201));
    }

    /// Layouts of the CLAP 1.2 headers.
    #[test]
    fn timer_and_fd_structs_match_the_host_abi() {
        let ptr = std::mem::size_of::<usize>();
        assert_eq!(std::mem::size_of::<clap_plugin_timer_support>(), ptr);
        assert_eq!(std::mem::size_of::<clap_host_timer_support>(), 2 * ptr);
        assert_eq!(std::mem::size_of::<clap_plugin_posix_fd_support>(), ptr);
        assert_eq!(std::mem::size_of::<clap_host_posix_fd_support>(), 3 * ptr);
        assert_eq!(std::mem::size_of::<clap_window>(), 2 * ptr);
    }
}
