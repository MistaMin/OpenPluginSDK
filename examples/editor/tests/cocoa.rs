//! The editor in real `NSView`s, on the main thread (this test has no
//! harness, so `main` is the process's main thread, as AppKit needs).
//!
//! - Shell ABI: `op_editor_*` makes a view, a click on the Warm switch comes
//!   back through the `OpEditorHost` callbacks as begin, perform and end, and
//!   the drawn pixels are where the editor put them (top row on top).
//! - CLAP: `clap.gui` creates, sizes, parents, shows, hides and destroys the
//!   view; the view's timer polls the plane; editor gestures reach the host
//!   as a `request_flush`.
//! - LV2: `lv2ui_descriptor` makes the widget, bound through instance-access.
//! - Resizing, in all three: the host gives the editor another size (shell
//!   `op_editor_resize`, CLAP `adjust_size` and `set_size`, LV2 `ui:resize`),
//!   and the view's frame, the drawn pixels and the click positions follow it.
//!   The editor also asks its host for a size (a double-click on the plate),
//!   through `op_editor_set_request_resize`, `clap_host_gui::request_resize`
//!   (the host answers inside the call) and the host's `ui:resize` feature.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    mac::shell_editor_clicks_and_draws();
    mac::clap_gui_lifecycle();
    mac::lv2_ui_lifecycle();
    mac::shell_editor_resizes();
    mac::clap_gui_resizes();
    mac::lv2_ui_resizes();
    println!("cocoa: 6 passed");
}

#[cfg(target_os = "macos")]
mod mac {
    use example_editor::*;
    use openplugin::targets::clap_abi::*;
    use openplugin::targets::shell::OpEditorHost;
    use std::cell::RefCell;
    use std::ffi::{c_char, c_void, CStr};
    use std::sync::atomic::{AtomicU32, Ordering};

    type Id = *mut c_void;
    type Sel = *mut c_void;

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
    fn class(name: &CStr) -> Id {
        unsafe { objc_getClass(name.as_ptr()) }
    }

    macro_rules! send {
        ($ret:ty, $obj:expr_2021, $sel:expr_2021 $(, $t:ty = $a:expr_2021)*) => {{
            let f: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $ret =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            f($obj, sel($sel) $(, $a)*)
        }};
    }

    unsafe extern "C" {
        fn op_editor_size(width: *mut u32, height: *mut u32) -> u32;
        fn op_editor_create(host: *const OpEditorHost) -> Id;
        fn op_editor_view(editor: Id) -> Id;
        fn op_editor_attach(editor: Id, parent: Id) -> u32;
        fn op_editor_bind(editor: Id, instance: Id);
        fn op_editor_set_plain(editor: Id, index: u32, plain: f32);
        fn op_editor_destroy(editor: Id);
        fn op_editor_size_range(min_w: *mut u32, min_h: *mut u32, max_w: *mut u32, max_h: *mut u32, aspect_w: *mut u32, aspect_h: *mut u32) -> u32;
        fn op_editor_get_size(editor: Id, width: *mut u32, height: *mut u32) -> u32;
        fn op_editor_adjust_size(editor: Id, width: *mut u32, height: *mut u32) -> u32;
        fn op_editor_resize(editor: Id, width: u32, height: u32) -> u32;
        fn op_editor_set_request_resize(editor: Id, context: Id, request: Option<unsafe extern "C" fn(Id, u32, u32) -> u32>);
        fn op_shell_create(rate: f64, max_block: u32, channels: u32) -> Id;
        fn op_shell_destroy(instance: Id);
    }

    unsafe fn new_view(w: f64, h: f64) -> Id { unsafe {
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w, h } };
        send!(Id, send!(Id, class(c"NSView"), c"alloc"), c"initWithFrame:", CGRect = frame)
    }}

    /// Pump the main run loop so timers fire.
    unsafe fn run_loop(seconds: f64) { unsafe {
        let date = send!(Id, class(c"NSDate"), c"dateWithTimeIntervalSinceNow:", f64 = seconds);
        let run_loop = send!(Id, class(c"NSRunLoop"), c"currentRunLoop");
        send!((), run_loop, c"runUntilDate:", Id = date);
    }}

    /// The view's size in points, as its bounds say.
    unsafe fn bounds_size(view: Id) -> (u32, u32) { unsafe {
        #[cfg(target_arch = "x86_64")]
        let rect = {
            let f: unsafe extern "C" fn(*mut CGRect, Id, Sel) = std::mem::transmute(objc_msgSend_stret as unsafe extern "C" fn());
            let mut rect = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w: 0.0, h: 0.0 } };
            f(&mut rect, view, sel(c"bounds"));
            rect
        };
        #[cfg(not(target_arch = "x86_64"))]
        let rect = {
            let f: unsafe extern "C" fn(Id, Sel) -> CGRect = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            f(view, sel(c"bounds"))
        };
        (rect.size.w.round() as u32, rect.size.h.round() as u32)
    }}

    /// Give a view a size, as a host does with the parent it hands out.
    unsafe fn set_size(view: Id, (w, h): (u32, u32)) { unsafe {
        send!((), view, c"setFrameSize:", CGSize = CGSize { w: w as f64, h: h as f64 });
    }}

    /// A left mouse button event at a point in window coordinates (y up).
    unsafe fn mouse(kind: usize, x: f64, y: f64) -> Id { unsafe { mouse_n(kind, x, y, 1) }}

    unsafe fn mouse_n(kind: usize, x: f64, y: f64, clicks: isize) -> Id { unsafe {
        send!(
            Id,
            class(c"NSEvent"),
            c"mouseEventWithType:location:modifierFlags:timestamp:windowNumber:context:eventNumber:clickCount:pressure:",
            usize = kind,
            CGPoint = CGPoint { x, y },
            usize = 0,
            f64 = 0.0,
            isize = 0,
            Id = std::ptr::null_mut(),
            isize = 0,
            isize = clicks,
            f32 = 1.0
        )
    }}

    /// The view's pixel at a point in editor coordinates (y down), as 0xRRGGBB.
    unsafe fn pixel(view: Id, x: f64, y: f64) -> u32 { unsafe {
        let (width, height) = bounds_size(view);
        let bounds = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w: width as f64, h: height as f64 } };
        let rep = send!(Id, view, c"bitmapImageRepForCachingDisplayInRect:", CGRect = bounds);
        send!((), view, c"cacheDisplayInRect:toBitmapImageRep:", CGRect = bounds, Id = rep);
        let scale = send!(isize, rep, c"pixelsWide") as f64 / width as f64;
        let color = send!(Id, rep, c"colorAtX:y:", isize = (x * scale) as isize, isize = (y * scale) as isize);
        let srgb = send!(Id, class(c"NSColorSpace"), c"sRGBColorSpace");
        let color = send!(Id, color, c"colorUsingColorSpace:", Id = srgb);
        let c = |name: &CStr| (send!(f64, color, name) * 255.0).round() as u32;
        (c(c"redComponent") << 16) | (c(c"greenComponent") << 8) | c(c"blueComponent")
    }}

    /// Pixels on a ring inside the knob, where the pointer sweeps.
    unsafe fn knob_ring(view: Id) -> Vec<u32> { unsafe {
        let (width, height) = bounds_size(view);
        let (x, y, w, h) = layout(width, height).knob;
        let (cx, cy, r) = (x + w * 0.5, y + h * 0.5, w * 0.25);
        (0..24)
            .map(|i| {
                let a = i as f32 / 24.0 * std::f32::consts::TAU;
                pixel(view, (cx + r * a.sin()) as f64, (cy - r * a.cos()) as f64)
            })
            .collect()
    }}

    /// A grid of pixels over the toggle.
    unsafe fn switch_area(view: Id) -> Vec<u32> { unsafe {
        let (width, height) = bounds_size(view);
        let (x, y, w, h) = layout(width, height).switch;
        let mut out = Vec::new();
        for j in 1..8 {
            for i in 1..5 {
                out.push(pixel(view, (x + w * i as f32 / 5.0) as f64, (y + h * j as f32 / 8.0) as f64));
            }
        }
        out
    }}

    fn green(p: u32) -> bool {
        let (r, g, b) = ((p >> 16) & 255, (p >> 8) & 255, p & 255);
        g > r + 60 && g > b + 60
    }

    /// The point to click the toggle at, in window coordinates (y up).
    fn switch_click() -> (f64, f64) {
        switch_click_at((WIDTH, HEIGHT))
    }

    /// The same for an editor of another size (the view's height flips y).
    fn switch_click_at(size: (u32, u32)) -> (f64, f64) {
        let (x, y, w, h) = layout(size.0, size.1).switch;
        point_at(size, (x + w * 0.5, y + h * 0.5))
    }

    /// An editor point (y down) as a window point (y up) for a view of `size`.
    fn point_at(size: (u32, u32), (x, y): (f32, f32)) -> (f64, f64) {
        (x as f64, size.1 as f64 - y as f64)
    }

    /// The meter's bottom lamp is lit (green) and its top lamp is not.
    unsafe fn meter_is_upright(view: Id) -> bool { unsafe {
        let (width, height) = bounds_size(view);
        let (mx, my, mw, mh) = layout(width, height).meter;
        // A lamp can sit between two sample rows at a small size: scan the end quarters.
        let lit = |from: f32, to: f32| {
            (0..12).any(|i| green(pixel(view, (mx + mw * 0.5) as f64, (from + (to - from) * i as f32 / 11.0) as f64)))
        };
        lit(my + mh * 0.75, my + mh - 2.0) && !lit(my + 2.0, my + mh * 0.25)
    }}

    thread_local! {
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "C" fn begin(_c: *mut c_void, i: u32) {
        LOG.with(|l| l.borrow_mut().push(format!("begin {i}")));
    }
    unsafe extern "C" fn perform(_c: *mut c_void, i: u32, v: f64) {
        LOG.with(|l| l.borrow_mut().push(format!("perform {i} {v}")));
    }
    unsafe extern "C" fn end(_c: *mut c_void, i: u32) {
        LOG.with(|l| l.borrow_mut().push(format!("end {i}")));
    }

    pub fn shell_editor_clicks_and_draws() {
        unsafe {
            let (mut w, mut h) = (0u32, 0u32);
            assert_eq!(op_editor_size(&mut w, &mut h), 1);
            assert_eq!((w, h), (WIDTH, HEIGHT));
            let host = OpEditorHost { context: std::ptr::null_mut(), begin_edit: Some(begin), perform_edit: Some(perform), end_edit: Some(end) };
            let editor = op_editor_create(&host);
            assert!(!editor.is_null());
            let view = op_editor_view(editor);
            assert!(!view.is_null());
            let parent = new_view(WIDTH as f64, HEIGHT as f64);
            assert_eq!(op_editor_attach(editor, parent), 1);
            assert_eq!(send!(Id, view, c"superview"), parent);

            // Bind to a live engine, then to one that is gone.
            let engine = op_shell_create(48_000.0, 256, 2);
            op_editor_bind(editor, engine);
            op_shell_destroy(engine);
            op_editor_bind(editor, engine);
            op_editor_bind(editor, 0x10 as Id);

            // A value from the host turns the lit knob.
            op_editor_set_plain(editor, 0, 24.0);
            let high = knob_ring(view);
            op_editor_set_plain(editor, 0, -24.0);
            let low = knob_ring(view);
            let moved = high.iter().zip(&low).filter(|(a, b)| a != b).count();
            assert!(moved >= 2, "the pointer moved ({moved} ring pixels changed)");
            // The image is upright: the LED ladder's bottom lamp is the lit one.
            let (mx, my, mw, mh) = METER;
            let bottom = pixel(view, (mx + mw * 0.5) as f64, (my + mh - 4.0) as f64);
            let top = pixel(view, (mx + mw * 0.5) as f64, (my + 4.0) as f64);
            assert!(green(bottom) && !green(top), "bottom {bottom:06x}, top {top:06x}");

            // Click the Warm toggle.
            let off = switch_area(view);
            let (cx, cy) = switch_click();
            send!((), view, c"mouseDown:", Id = mouse(1, cx, cy));
            send!((), view, c"mouseUp:", Id = mouse(2, cx, cy));
            let log = LOG.with(|l| l.borrow().clone());
            assert_eq!(log, vec!["begin 1", "perform 1 1", "end 1"], "gestures");
            assert_ne!(switch_area(view), off, "the toggle is drawn on");

            op_editor_destroy(editor);
            assert!(send!(Id, parent, c"subviews").is_null() || send!(usize, send!(Id, parent, c"subviews"), c"count") == 0);
            send!((), parent, c"release");
        }
        println!("shell editor: ok");
    }

    static FLUSHES: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn host_rescan(_h: *const clap_host, _f: u32) {}
    unsafe extern "C" fn host_clear(_h: *const clap_host, _p: u32, _f: u32) {}
    unsafe extern "C" fn host_flush(_h: *const clap_host) {
        FLUSHES.fetch_add(1, Ordering::Relaxed);
    }
    static HOST_PARAMS: clap_host_params = clap_host_params { rescan: Some(host_rescan), clear: Some(host_clear), request_flush: Some(host_flush) };
    unsafe extern "C" fn host_extension(_h: *const clap_host, id: *const c_char) -> *const c_void { unsafe {
        if CStr::from_ptr(id).to_bytes_with_nul() == EXT_PARAMS {
            &HOST_PARAMS as *const clap_host_params as *const c_void
        } else {
            std::ptr::null()
        }
    }}
    unsafe extern "C" fn host_noop(_h: *const clap_host) {}

    pub fn clap_gui_lifecycle() {
        unsafe {
            let host = clap_host {
                clap_version: CLAP_VERSION,
                host_data: std::ptr::null_mut(),
                name: c"test".as_ptr(),
                vendor: c"test".as_ptr(),
                url: c"".as_ptr(),
                version: c"1".as_ptr(),
                get_extension: Some(host_extension),
                request_restart: Some(host_noop),
                request_process: Some(host_noop),
                request_callback: Some(host_noop),
            };
            let entry = &example_editor::clap_entry;
            assert!(entry.init.unwrap()(c"".as_ptr()));
            let factory = entry.get_factory.unwrap()(FACTORY_ID.as_ptr() as *const c_char) as *const clap_plugin_factory;
            let desc = (*factory).get_plugin_descriptor.unwrap()(factory, 0);
            let plugin = (*factory).create_plugin.unwrap()(factory, &host, (*desc).id);
            assert!(!plugin.is_null());
            assert!((*plugin).init.unwrap()(plugin));
            let gui = (*plugin).get_extension.unwrap()(plugin, EXT_GUI.as_ptr() as *const c_char) as *const clap_plugin_gui;
            assert!(!gui.is_null(), "clap.gui is offered");
            let gui = &*gui;
            let cocoa = WINDOW_API_COCOA.as_ptr() as *const c_char;
            assert!(gui.is_api_supported.unwrap()(plugin, cocoa, false));
            assert!(!gui.is_api_supported.unwrap()(plugin, cocoa, true));
            assert!(!gui.is_api_supported.unwrap()(plugin, c"x11".as_ptr(), false));
            let (mut w, mut h) = (0u32, 0u32);
            assert!(!gui.get_size.unwrap()(plugin, &mut w, &mut h), "no size before create");
            assert!(gui.create.unwrap()(plugin, cocoa, false));
            assert!(gui.get_size.unwrap()(plugin, &mut w, &mut h));
            assert_eq!((w, h), (WIDTH, HEIGHT));
            assert!(gui.can_resize.unwrap()(plugin), "the example resizes");
            assert!(gui.set_size.unwrap()(plugin, WIDTH, HEIGHT));
            assert!(gui.set_size.unwrap()(plugin, WIDTH + 1, HEIGHT), "inside the range, another shape is taken");
            assert!(!gui.set_size.unwrap()(plugin, MAX_SIZE.0 + 1, HEIGHT));
            assert!(gui.set_size.unwrap()(plugin, WIDTH, HEIGHT));
            let parent = new_view(WIDTH as f64, HEIGHT as f64);
            let window = clap_window { api: cocoa, ptr: parent };
            assert!(gui.set_parent.unwrap()(plugin, &window));
            let subviews = send!(Id, parent, c"subviews");
            assert_eq!(send!(usize, subviews, c"count"), 1);
            let view = send!(Id, subviews, c"objectAtIndex:", usize = 0);
            assert!(gui.show.unwrap()(plugin));

            // Values the host sets reach the editor through the view's poll.
            let params = (*plugin).get_extension.unwrap()(plugin, EXT_PARAMS.as_ptr() as *const c_char) as *const clap_plugin_params;
            #[repr(C)]
            struct One {
                event: clap_event_param_value,
            }
            unsafe extern "C" fn one_size(_l: *const clap_input_events) -> u32 {
                1
            }
            unsafe extern "C" fn one_get(l: *const clap_input_events, _i: u32) -> *const clap_event_header { unsafe {
                &(*((*l).ctx as *const One)).event.header
            }}
            let one = One {
                event: clap_event_param_value {
                    header: clap_event_header { size: std::mem::size_of::<clap_event_param_value>() as u32, time: 0, space_id: 0, type_: EVENT_PARAM_VALUE, flags: 0 },
                    param_id: 0,
                    cookie: std::ptr::null_mut(),
                    note_id: -1,
                    port_index: -1,
                    channel: -1,
                    key: -1,
                    value: 24.0,
                },
            };
            let input = clap_input_events { ctx: &one as *const One as *mut c_void, size: Some(one_size), get: Some(one_get) };
            run_loop(0.1);
            let before = knob_ring(view);
            (*params).flush.unwrap()(plugin, &input, std::ptr::null());
            run_loop(0.2);
            assert_ne!(knob_ring(view), before, "polled +24 dB turned the knob");

            // A click on the switch asks the host to flush, and the plane has it.
            let before = FLUSHES.load(Ordering::Relaxed);
            let (cx, cy) = switch_click();
            send!((), view, c"mouseDown:", Id = mouse(1, cx, cy));
            assert!(FLUSHES.load(Ordering::Relaxed) > before, "request_flush");
            let mut value = 0.0f64;
            assert!((*params).get_value.unwrap()(plugin, 1, &mut value));
            assert_eq!(value, 1.0);

            assert!(gui.hide.unwrap()(plugin));
            gui.destroy.unwrap()(plugin);
            assert_eq!(send!(usize, send!(Id, parent, c"subviews"), c"count"), 0, "removed on destroy");
            // Create again, then let destroy of the plugin take the view down.
            assert!(gui.create.unwrap()(plugin, cocoa, false));
            assert!(gui.set_parent.unwrap()(plugin, &window));
            (*plugin).destroy.unwrap()(plugin);
            assert_eq!(send!(usize, send!(Id, parent, c"subviews"), c"count"), 0);
            send!((), parent, c"release");
        }
        println!("clap gui: ok");
    }

    pub fn lv2_ui_lifecycle() {
        use openplugin::targets::lv2::{Lv2Feature, AUDIO_PORTS};
        unsafe {
            let desc = &*example_editor::lv2_descriptor(0);
            let instance = (desc.instantiate)(desc, 48_000.0, std::ptr::null(), std::ptr::null());
            assert!(!instance.is_null());
            let ui = &*example_editor::lv2ui_descriptor(0);
            assert!(CStr::from_ptr(ui.uri).to_string_lossy().ends_with("#ui"));
            assert!(example_editor::lv2ui_descriptor(1).is_null());
            let parent = new_view(WIDTH as f64, HEIGHT as f64);
            let features = [
                Lv2Feature { uri: c"http://lv2plug.in/ns/extensions/ui#parent".as_ptr(), data: parent },
                Lv2Feature { uri: c"http://lv2plug.in/ns/ext/instance-access".as_ptr(), data: instance },
            ];
            let list = [&features[0] as *const Lv2Feature, &features[1], std::ptr::null()];
            static WRITES: AtomicU32 = AtomicU32::new(0);
            unsafe extern "C" fn write(_c: *mut c_void, port: u32, size: u32, protocol: u32, buffer: *const c_void) { unsafe {
                assert_eq!((port, size, protocol), (AUDIO_PORTS + 1, 4, 0));
                assert_eq!(*(buffer as *const f32), 1.0);
                WRITES.fetch_add(1, Ordering::Relaxed);
            }}
            let mut widget: Id = std::ptr::null_mut();
            let handle = (ui.instantiate)(ui, desc.uri, c"".as_ptr(), Some(write), std::ptr::null_mut(), &mut widget, list.as_ptr());
            assert!(!handle.is_null());
            assert!(!widget.is_null());
            assert_eq!(send!(Id, widget, c"superview"), parent);
            let before = knob_ring(widget);
            let full = 24.0f32;
            (ui.port_event)(handle, AUDIO_PORTS, 4, 0, &full as *const f32 as *const c_void);
            assert_ne!(knob_ring(widget), before, "port_event reached the editor");
            let (cx, cy) = switch_click();
            send!((), widget, c"mouseDown:", Id = mouse(1, cx, cy));
            assert_eq!(WRITES.load(Ordering::Relaxed), 1, "the click wrote the Warm port");
            (ui.cleanup)(handle);
            assert_eq!(send!(usize, send!(Id, parent, c"subviews"), c"count"), 0);
            (desc.cleanup)(instance);
            send!((), parent, c"release");
        }
        println!("lv2 ui: ok");
    }

    /// Check the view at `size`: bounds, the lit meter where this size puts
    /// it, and a click at the switch writing one gesture set. Returns nothing;
    /// panics with the context on a mismatch.
    unsafe fn check_drawn_at(view: Id, size: (u32, u32), what: &str) { unsafe {
        assert_eq!(bounds_size(view), size, "{what}: the view's size");
        assert!(meter_is_upright(view), "{what}: the meter is drawn where this size puts it");
        // The plate reaches the far corner at this size, not a stretched or stale image.
        let corner = pixel(view, size.0 as f64 - 3.0, size.1 as f64 - 3.0);
        let near = pixel(view, size.0 as f64 - 8.0, size.1 as f64 - 8.0);
        assert!(!green(corner) && !green(near), "{what}: corner");
    }}

    thread_local! {
        static ASKED: RefCell<Vec<(u32, u32)>> = const { RefCell::new(Vec::new()) };
        /// The shell test's editor, so the request callback can answer at once.
        static SHELL_EDITOR: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    unsafe extern "C" fn shell_request(_context: Id, w: u32, h: u32) -> u32 {
        ASKED.with(|a| a.borrow_mut().push((w, h)));
        // The shell's host agrees and resizes inside the call: the view must wait for the click to end.
        let editor = SHELL_EDITOR.with(|e| e.get()) as Id;
        unsafe { op_editor_resize(editor, w, h) }
    }

    pub fn shell_editor_resizes() {
        unsafe {
            let (mut min_w, mut min_h, mut max_w, mut max_h, mut aw, mut ah) = (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
            assert_eq!(op_editor_size_range(&mut min_w, &mut min_h, &mut max_w, &mut max_h, &mut aw, &mut ah), 1);
            assert_eq!(((min_w, min_h), (max_w, max_h), (aw, ah)), (MIN_SIZE, MAX_SIZE, (WIDTH, HEIGHT)));
            assert_eq!(op_editor_size_range(std::ptr::null_mut(), &mut min_h, &mut max_w, &mut max_h, &mut aw, &mut ah), 0);

            let host = OpEditorHost { context: std::ptr::null_mut(), begin_edit: Some(begin), perform_edit: Some(perform), end_edit: Some(end) };
            let editor = op_editor_create(&host);
            assert!(!editor.is_null());
            let view = op_editor_view(editor);
            let parent = new_view(WIDTH as f64, HEIGHT as f64);
            assert_eq!(op_editor_attach(editor, parent), 1);
            let (mut w, mut h) = (0u32, 0u32);
            assert_eq!(op_editor_get_size(editor, &mut w, &mut h), 1);
            assert_eq!((w, h), (WIDTH, HEIGHT));

            // Proposed sizes are brought into the range and the editor is not changed.
            let (mut w, mut h) = (5000u32, 5000u32);
            assert_eq!(op_editor_adjust_size(editor, &mut w, &mut h), 1);
            assert_eq!((w, h), MAX_SIZE);
            assert_eq!(op_editor_get_size(editor, &mut w, &mut h), 1);
            assert_eq!((w, h), (WIDTH, HEIGHT));
            assert_eq!(bounds_size(view), (WIDTH, HEIGHT));

            // A size outside the range is refused and changes nothing.
            assert_eq!(op_editor_resize(editor, 100, 100), 0);
            assert_eq!(op_editor_resize(editor, MAX_SIZE.0 + 1, MAX_SIZE.1), 0);
            assert_eq!(bounds_size(view), (WIDTH, HEIGHT));

            let size = (300, 200);
            assert_eq!(op_editor_resize(editor, size.0, size.1), 1);
            set_size(parent, size);
            run_loop(0.1);
            assert_eq!(op_editor_get_size(editor, &mut w, &mut h), 1);
            assert_eq!((w, h), size);
            check_drawn_at(view, size, "shell 300x200");

            // The click lands on the scaled switch. The old position is bare plate.
            LOG.with(|l| l.borrow_mut().clear());
            let off = switch_area(view);
            // The opening-size switch centre is bare plate at this size.
            let (ox, oy) = point_at(size, (SWITCH.0 + SWITCH.2 * 0.5, SWITCH.1 + SWITCH.3 * 0.5));
            send!((), view, c"mouseDown:", Id = mouse(1, ox, oy));
            send!((), view, c"mouseUp:", Id = mouse(2, ox, oy));
            assert!(LOG.with(|l| l.borrow().is_empty()), "a click at the old switch position hits nothing");
            let (cx, cy) = switch_click_at(size);
            send!((), view, c"mouseDown:", Id = mouse(1, cx, cy));
            send!((), view, c"mouseUp:", Id = mouse(2, cx, cy));
            assert_eq!(LOG.with(|l| l.borrow().clone()), vec!["begin 1", "perform 1 1", "end 1"], "the click on the scaled switch");
            assert_ne!(switch_area(view), off, "the scaled toggle is drawn on");

            // The editor asks for a size; the shell host answers inside the click.
            ASKED.with(|a| a.borrow_mut().clear());
            SHELL_EDITOR.with(|e| e.set(editor as usize));
            op_editor_set_request_resize(editor, std::ptr::null_mut(), Some(shell_request));
            let bare = point_at(size, (10.0, 10.0));
            send!((), view, c"mouseDown:", Id = mouse_n(1, bare.0, bare.1, 2));
            assert_eq!(ASKED.with(|a| a.borrow().clone()), vec![LARGE_SIZE]);
            assert_eq!(op_editor_get_size(editor, &mut w, &mut h), 1);
            assert_eq!((w, h), LARGE_SIZE);
            set_size(parent, LARGE_SIZE);
            run_loop(0.1);
            check_drawn_at(view, LARGE_SIZE, "shell 360x240");
            // Without a callback a request is refused and nothing changes.
            op_editor_set_request_resize(editor, std::ptr::null_mut(), None);
            let bare = point_at(LARGE_SIZE, (10.0, 10.0));
            send!((), view, c"mouseDown:", Id = mouse_n(1, bare.0, bare.1, 2));
            assert_eq!(op_editor_get_size(editor, &mut w, &mut h), 1);
            assert_eq!((w, h), LARGE_SIZE);

            op_editor_destroy(editor);
            send!((), parent, c"release");
        }
        println!("shell resize: ok");
    }

    thread_local! {
        static GUI_PLUGIN: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    }

    unsafe extern "C" fn host_hints_changed(_h: *const clap_host) {}
    /// The host agrees and calls `set_size` inside the request, as CLAP hosts may.
    unsafe extern "C" fn host_request_resize(_h: *const clap_host, w: u32, h: u32) -> bool { unsafe {
        ASKED.with(|a| a.borrow_mut().push((w, h)));
        let (plugin, gui) = GUI_PLUGIN.with(|p| p.get());
        if plugin == 0 {
            return false;
        }
        let gui = &*(gui as *const clap_plugin_gui);
        gui.set_size.unwrap()(plugin as *const clap_plugin, w, h)
    }}
    unsafe extern "C" fn host_show_hide(_h: *const clap_host) -> bool {
        true
    }
    unsafe extern "C" fn host_closed(_h: *const clap_host, _destroyed: bool) {}
    static HOST_GUI: clap_host_gui = clap_host_gui {
        resize_hints_changed: Some(host_hints_changed),
        request_resize: Some(host_request_resize),
        request_show: Some(host_show_hide),
        request_hide: Some(host_show_hide),
        closed: Some(host_closed),
    };
    unsafe extern "C" fn host_extension_gui(_h: *const clap_host, id: *const c_char) -> *const c_void { unsafe {
        match CStr::from_ptr(id).to_bytes_with_nul() {
            EXT_PARAMS => &HOST_PARAMS as *const clap_host_params as *const c_void,
            EXT_GUI => &HOST_GUI as *const clap_host_gui as *const c_void,
            _ => std::ptr::null(),
        }
    }}

    pub fn clap_gui_resizes() {
        unsafe {
            let host = Box::leak(Box::new(clap_host {
                clap_version: CLAP_VERSION,
                host_data: std::ptr::null_mut(),
                name: c"test".as_ptr(),
                vendor: c"test".as_ptr(),
                url: c"".as_ptr(),
                version: c"1".as_ptr(),
                get_extension: Some(host_extension_gui),
                request_restart: Some(host_noop),
                request_process: Some(host_noop),
                request_callback: Some(host_noop),
            }));
            let entry = &example_editor::clap_entry;
            assert!(entry.init.unwrap()(c"".as_ptr()));
            let factory = entry.get_factory.unwrap()(FACTORY_ID.as_ptr() as *const c_char) as *const clap_plugin_factory;
            let desc = (*factory).get_plugin_descriptor.unwrap()(factory, 0);
            let plugin = (*factory).create_plugin.unwrap()(factory, host, (*desc).id);
            assert!((*plugin).init.unwrap()(plugin));
            let gui = &*((*plugin).get_extension.unwrap()(plugin, EXT_GUI.as_ptr() as *const c_char) as *const clap_plugin_gui);
            let cocoa = WINDOW_API_COCOA.as_ptr() as *const c_char;
            assert!(gui.create.unwrap()(plugin, cocoa, false));
            let parent = new_view(WIDTH as f64, HEIGHT as f64);
            let window = clap_window { api: cocoa, ptr: parent };
            assert!(gui.set_parent.unwrap()(plugin, &window));
            let view = send!(Id, send!(Id, parent, c"subviews"), c"objectAtIndex:", usize = 0);
            assert!(gui.show.unwrap()(plugin));
            let params = (*plugin).get_extension.unwrap()(plugin, EXT_PARAMS.as_ptr() as *const c_char) as *const clap_plugin_params;

            // What the host may ask.
            assert!(gui.can_resize.unwrap()(plugin));
            let mut hints = std::mem::zeroed::<clap_gui_resize_hints>();
            assert!(gui.get_resize_hints.unwrap()(plugin, &mut hints));
            assert!(hints.can_resize_horizontally && hints.can_resize_vertically && hints.preserve_aspect_ratio);
            assert_eq!((hints.aspect_ratio_width, hints.aspect_ratio_height), (WIDTH, HEIGHT));
            let adjust = |w: u32, h: u32| {
                let (mut w, mut h) = (w, h);
                assert!(gui.adjust_size.unwrap()(plugin, &mut w, &mut h));
                (w, h)
            };
            assert_eq!(adjust(5000, 5000), MAX_SIZE);
            assert_eq!(adjust(10, 10), MIN_SIZE);
            assert_eq!(adjust(WIDTH, HEIGHT), (WIDTH, HEIGHT));
            let (w, h) = adjust(300, 100);
            assert!(MIN_SIZE.0 <= w && w <= MAX_SIZE.0 && (w as i64 * 2 - h as i64 * 3).abs() <= 2, "{w}x{h} keeps 3:2");
            assert_eq!(adjust(w, h), (w, h), "an adjusted size adjusts to itself");

            // set_size follows the range.
            let size = (360, 240);
            assert!(gui.set_size.unwrap()(plugin, size.0, size.1));
            let (mut gw, mut gh) = (0u32, 0u32);
            assert!(gui.get_size.unwrap()(plugin, &mut gw, &mut gh));
            assert_eq!((gw, gh), size);
            assert!(!gui.set_size.unwrap()(plugin, 5000, 5000));
            assert!(!gui.set_size.unwrap()(plugin, MIN_SIZE.0 - 1, MIN_SIZE.1));
            assert!(gui.get_size.unwrap()(plugin, &mut gw, &mut gh));
            assert_eq!((gw, gh), size, "a refused size changes nothing");
            set_size(parent, size);
            run_loop(0.1);
            check_drawn_at(view, size, "clap 360x240");

            // The scaled controls: the switch click goes through the plane, the old spot does not.
            let before = FLUSHES.load(Ordering::Relaxed);
            let old = point_at(size, (SWITCH.0 + SWITCH.2 * 0.5, SWITCH.1 + SWITCH.3 * 0.5));
            send!((), view, c"mouseDown:", Id = mouse(1, old.0, old.1));
            send!((), view, c"mouseUp:", Id = mouse(2, old.0, old.1));
            assert_eq!(FLUSHES.load(Ordering::Relaxed), before, "the old switch position is bare plate now");
            let off = switch_area(view);
            let (cx, cy) = switch_click_at(size);
            send!((), view, c"mouseDown:", Id = mouse(1, cx, cy));
            send!((), view, c"mouseUp:", Id = mouse(2, cx, cy));
            assert!(FLUSHES.load(Ordering::Relaxed) > before, "request_flush");
            let mut value = 0.0f64;
            assert!((*params).get_value.unwrap()(plugin, 1, &mut value));
            assert_eq!(value, 1.0);
            assert_ne!(switch_area(view), off);

            // A smaller size too, then back to the opening size.
            assert!(gui.set_size.unwrap()(plugin, MIN_SIZE.0, MIN_SIZE.1));
            set_size(parent, MIN_SIZE);
            run_loop(0.1);
            check_drawn_at(view, MIN_SIZE, "clap min");
            assert!(gui.set_size.unwrap()(plugin, WIDTH, HEIGHT));
            set_size(parent, (WIDTH, HEIGHT));
            run_loop(0.1);
            check_drawn_at(view, (WIDTH, HEIGHT), "clap opening size");

            // The editor asks the host (a double-click on the plate); the host answers with
            // set_size inside the request, and the view takes it after the click.
            ASKED.with(|a| a.borrow_mut().clear());
            GUI_PLUGIN.with(|p| p.set((plugin as usize, gui as *const clap_plugin_gui as usize)));
            let bare = point_at((WIDTH, HEIGHT), (10.0, 10.0));
            send!((), view, c"mouseDown:", Id = mouse_n(1, bare.0, bare.1, 2));
            send!((), view, c"mouseUp:", Id = mouse_n(2, bare.0, bare.1, 2));
            GUI_PLUGIN.with(|p| p.set((0, 0)));
            assert_eq!(ASKED.with(|a| a.borrow().clone()), vec![LARGE_SIZE], "clap_host_gui::request_resize");
            assert!(gui.get_size.unwrap()(plugin, &mut gw, &mut gh));
            assert_eq!((gw, gh), LARGE_SIZE);
            set_size(parent, LARGE_SIZE);
            run_loop(0.1);
            check_drawn_at(view, LARGE_SIZE, "clap after the editor's request");

            gui.destroy.unwrap()(plugin);
            (*plugin).destroy.unwrap()(plugin);
            send!((), parent, c"release");
        }
        println!("clap resize: ok");
    }

    #[repr(C)]
    struct Resize {
        handle: *mut c_void,
        ui_resize: Option<unsafe extern "C" fn(handle: *mut c_void, width: i32, height: i32) -> i32>,
    }

    unsafe extern "C" fn lv2_host_resize(_handle: *mut c_void, width: i32, height: i32) -> i32 {
        ASKED.with(|a| a.borrow_mut().push((width as u32, height as u32)));
        0
    }

    pub fn lv2_ui_resizes() {
        use openplugin::targets::lv2::{Lv2Feature, AUDIO_PORTS};
        unsafe {
            let desc = &*example_editor::lv2_descriptor(0);
            let instance = (desc.instantiate)(desc, 48_000.0, std::ptr::null(), std::ptr::null());
            let ui = &*example_editor::lv2ui_descriptor(0);
            let parent = new_view(WIDTH as f64, HEIGHT as f64);
            let host_resize = Resize { handle: std::ptr::null_mut(), ui_resize: Some(lv2_host_resize) };
            let features = [
                Lv2Feature { uri: c"http://lv2plug.in/ns/extensions/ui#parent".as_ptr(), data: parent },
                Lv2Feature { uri: c"http://lv2plug.in/ns/ext/instance-access".as_ptr(), data: instance },
                Lv2Feature { uri: c"http://lv2plug.in/ns/extensions/ui#resize".as_ptr(), data: &host_resize as *const Resize as *mut c_void },
            ];
            let list = [&features[0] as *const Lv2Feature, &features[1], &features[2], std::ptr::null()];
            static WRITES: AtomicU32 = AtomicU32::new(0);
            unsafe extern "C" fn write(_c: *mut c_void, port: u32, size: u32, protocol: u32, buffer: *const c_void) { unsafe {
                assert_eq!((port, size, protocol), (AUDIO_PORTS + 1, 4, 0));
                assert_eq!(*(buffer as *const f32), 1.0);
                WRITES.fetch_add(1, Ordering::Relaxed);
            }}
            ASKED.with(|a| a.borrow_mut().clear());
            let mut widget: Id = std::ptr::null_mut();
            let handle = (ui.instantiate)(ui, desc.uri, c"".as_ptr(), Some(write), std::ptr::null_mut(), &mut widget, list.as_ptr());
            assert!(!handle.is_null());
            assert_eq!(ASKED.with(|a| a.borrow().clone()), vec![(WIDTH, HEIGHT)], "the opening size is reported to the host");

            // The UI offers ui:resize for the host.
            let offered = (ui.extension_data)(c"http://lv2plug.in/ns/extensions/ui#resize".as_ptr()) as *const Resize;
            assert!(!offered.is_null());
            assert!((ui.extension_data)(c"http://example.com/nothing".as_ptr()).is_null());
            let host_sets = (*offered).ui_resize.unwrap();
            assert_ne!(host_sets(handle, 100, 100), 0, "below the minimum");
            assert_ne!(host_sets(handle, 5000, 5000), 0, "above the maximum");
            assert_ne!(host_sets(handle, -1, 100), 0);
            assert_eq!(bounds_size(widget), (WIDTH, HEIGHT));
            let size = (300, 200);
            assert_eq!(host_sets(handle, size.0 as i32, size.1 as i32), 0);
            set_size(parent, size);
            run_loop(0.1);
            check_drawn_at(widget, size, "lv2 300x200");

            // A port event still reaches the scaled editor, and the click lands on the scaled switch.
            let before = knob_ring(widget);
            let full = 24.0f32;
            (ui.port_event)(handle, AUDIO_PORTS, 4, 0, &full as *const f32 as *const c_void);
            assert_ne!(knob_ring(widget), before, "port_event reached the resized editor");
            let old = point_at(size, (SWITCH.0 + SWITCH.2 * 0.5, SWITCH.1 + SWITCH.3 * 0.5));
            send!((), widget, c"mouseDown:", Id = mouse(1, old.0, old.1));
            assert_eq!(WRITES.load(Ordering::Relaxed), 0, "the old switch position is bare plate now");
            let (cx, cy) = switch_click_at(size);
            send!((), widget, c"mouseDown:", Id = mouse(1, cx, cy));
            assert_eq!(WRITES.load(Ordering::Relaxed), 1, "the click wrote the Warm port");

            // The editor asks the host; the host's ui:resize agrees; the view follows on its next tick.
            ASKED.with(|a| a.borrow_mut().clear());
            let bare = point_at(size, (10.0, 10.0));
            send!((), widget, c"mouseDown:", Id = mouse_n(1, bare.0, bare.1, 2));
            assert_eq!(ASKED.with(|a| a.borrow().clone()), vec![LARGE_SIZE], "the host's ui:resize feature");
            run_loop(0.2);
            assert_eq!(bounds_size(widget), LARGE_SIZE, "the view took the size the host agreed to");
            check_drawn_at(widget, LARGE_SIZE, "lv2 after the editor's request");

            (ui.cleanup)(handle);
            (desc.cleanup)(instance);
            send!((), parent, c"release");
        }
        println!("lv2 resize: ok");
    }
}
