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

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    mac::shell_editor_clicks_and_draws();
    mac::clap_gui_lifecycle();
    mac::lv2_ui_lifecycle();
    println!("cocoa: 3 passed");
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
    extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_msgSend();
    }
    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}

    fn sel(name: &CStr) -> Sel {
        unsafe { sel_registerName(name.as_ptr()) }
    }
    fn class(name: &CStr) -> Id {
        unsafe { objc_getClass(name.as_ptr()) }
    }

    macro_rules! send {
        ($ret:ty, $obj:expr, $sel:expr $(, $t:ty = $a:expr)*) => {{
            let f: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $ret =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            f($obj, sel($sel) $(, $a)*)
        }};
    }

    extern "C" {
        fn op_editor_size(width: *mut u32, height: *mut u32) -> u32;
        fn op_editor_create(host: *const OpEditorHost) -> Id;
        fn op_editor_view(editor: Id) -> Id;
        fn op_editor_attach(editor: Id, parent: Id) -> u32;
        fn op_editor_bind(editor: Id, instance: Id);
        fn op_editor_set_plain(editor: Id, index: u32, plain: f32);
        fn op_editor_destroy(editor: Id);
        fn op_shell_create(rate: f64, max_block: u32, channels: u32) -> Id;
        fn op_shell_destroy(instance: Id);
    }

    unsafe fn new_view(w: f64, h: f64) -> Id {
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w, h } };
        send!(Id, send!(Id, class(c"NSView"), c"alloc"), c"initWithFrame:", CGRect = frame)
    }

    /// Pump the main run loop so timers fire.
    unsafe fn run_loop(seconds: f64) {
        let date = send!(Id, class(c"NSDate"), c"dateWithTimeIntervalSinceNow:", f64 = seconds);
        let run_loop = send!(Id, class(c"NSRunLoop"), c"currentRunLoop");
        send!((), run_loop, c"runUntilDate:", Id = date);
    }

    /// A left mouse button event at a point in window coordinates (y up).
    unsafe fn mouse(kind: usize, x: f64, y: f64) -> Id {
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
            isize = 1,
            f32 = 1.0
        )
    }

    /// The view's pixel at a point in editor coordinates (y down), as 0xRRGGBB.
    unsafe fn pixel(view: Id, x: f64, y: f64) -> u32 {
        let bounds = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { w: WIDTH as f64, h: HEIGHT as f64 } };
        let rep = send!(Id, view, c"bitmapImageRepForCachingDisplayInRect:", CGRect = bounds);
        send!((), view, c"cacheDisplayInRect:toBitmapImageRep:", CGRect = bounds, Id = rep);
        let scale = send!(isize, rep, c"pixelsWide") as f64 / WIDTH as f64;
        let color = send!(Id, rep, c"colorAtX:y:", isize = (x * scale) as isize, isize = (y * scale) as isize);
        let srgb = send!(Id, class(c"NSColorSpace"), c"sRGBColorSpace");
        let color = send!(Id, color, c"colorUsingColorSpace:", Id = srgb);
        let c = |name: &CStr| (send!(f64, color, name) * 255.0).round() as u32;
        (c(c"redComponent") << 16) | (c(c"greenComponent") << 8) | c(c"blueComponent")
    }

    fn near(a: u32, b: u32) -> bool {
        (0..3).all(|i| (((a >> (i * 8)) & 255) as i32 - ((b >> (i * 8)) & 255) as i32).abs() <= 3)
    }

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

            // +24 dB from the host fills the fader to the top.
            op_editor_set_plain(editor, 0, 24.0);
            assert!(near(pixel(view, 40.0, 25.0), 0x40a0e0), "fader top {:06x}", pixel(view, 40.0, 25.0));
            // -24 dB empties it; the top row is the editor's first row.
            op_editor_set_plain(editor, 0, -24.0);
            assert!(near(pixel(view, 40.0, 25.0), 0x383c44), "fader top {:06x}", pixel(view, 40.0, 25.0));
            assert!(near(pixel(view, 5.0, 5.0), 0x202428), "background {:06x}", pixel(view, 5.0, 5.0));
            assert!(near(pixel(view, 120.0, 35.0), 0x505560), "switch off");

            // Click the Warm switch: editor y 35 is window y 160 - 35 (y up).
            let down = mouse(1, 120.0, HEIGHT as f64 - 35.0);
            let up = mouse(2, 120.0, HEIGHT as f64 - 35.0);
            send!((), view, c"mouseDown:", Id = down);
            send!((), view, c"mouseUp:", Id = up);
            let log = LOG.with(|l| l.borrow().clone());
            assert_eq!(log, vec!["begin 1", "perform 1 1", "end 1"], "gestures");
            assert!(near(pixel(view, 120.0, 35.0), 0xe08030), "switch on {:06x}", pixel(view, 120.0, 35.0));

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
    unsafe extern "C" fn host_extension(_h: *const clap_host, id: *const c_char) -> *const c_void {
        if CStr::from_ptr(id).to_bytes_with_nul() == EXT_PARAMS {
            &HOST_PARAMS as *const clap_host_params as *const c_void
        } else {
            std::ptr::null()
        }
    }
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
            assert!(!gui.can_resize.unwrap()(plugin));
            assert!(gui.set_size.unwrap()(plugin, WIDTH, HEIGHT));
            assert!(!gui.set_size.unwrap()(plugin, WIDTH + 1, HEIGHT));
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
            unsafe extern "C" fn one_get(l: *const clap_input_events, _i: u32) -> *const clap_event_header {
                &(*((*l).ctx as *const One)).event.header
            }
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
            (*params).flush.unwrap()(plugin, &input, std::ptr::null());
            run_loop(0.2);
            assert!(near(pixel(view, 40.0, 25.0), 0x40a0e0), "polled +24 dB: {:06x}", pixel(view, 40.0, 25.0));

            // A click on the switch asks the host to flush, and the plane has it.
            let before = FLUSHES.load(Ordering::Relaxed);
            send!((), view, c"mouseDown:", Id = mouse(1, 120.0, HEIGHT as f64 - 35.0));
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
            unsafe extern "C" fn write(_c: *mut c_void, port: u32, size: u32, protocol: u32, buffer: *const c_void) {
                assert_eq!((port, size, protocol), (AUDIO_PORTS + 1, 4, 0));
                assert_eq!(*(buffer as *const f32), 1.0);
                WRITES.fetch_add(1, Ordering::Relaxed);
            }
            let mut widget: Id = std::ptr::null_mut();
            let handle = (ui.instantiate)(ui, desc.uri, c"".as_ptr(), Some(write), std::ptr::null_mut(), &mut widget, list.as_ptr());
            assert!(!handle.is_null());
            assert!(!widget.is_null());
            assert_eq!(send!(Id, widget, c"superview"), parent);
            let full = 24.0f32;
            (ui.port_event)(handle, AUDIO_PORTS, 4, 0, &full as *const f32 as *const c_void);
            assert!(near(pixel(widget, 40.0, 25.0), 0x40a0e0), "port_event reached the editor");
            send!((), widget, c"mouseDown:", Id = mouse(1, 120.0, HEIGHT as f64 - 35.0));
            assert_eq!(WRITES.load(Ordering::Relaxed), 1, "the click wrote the Warm port");
            (ui.cleanup)(handle);
            assert_eq!(send!(usize, send!(Id, parent, c"subviews"), c"count"), 0);
            (desc.cleanup)(instance);
            send!((), parent, c"release");
        }
        println!("lv2 ui: ok");
    }
}
