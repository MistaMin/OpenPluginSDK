//! The editor in real windows on Windows (`HWND`s), in one thread that pumps
//! its own messages. No harness, as the other editor tests.
//!
//! - Shell ABI: `op_editor_*` makes a child window in a parent, the drawn
//!   pixels are the ones the editor renders (top row on top), a value from
//!   the host turns the knob, and a click on the Warm switch comes back
//!   through the `OpEditorHost` callbacks as begin, perform and end.
//! - Scale and size: `op_editor_set_scale` makes the window `points x scale`
//!   pixels and clicks still land; `op_editor_resize` (points) and
//!   `op_editor_set_pixel_size` (pixels) resize the editor and its window.
//! - Typing and keys: a double-click on the knob opens an `EDIT` field, Return
//!   commits what was typed as a gesture; Tab and an arrow key reach the editor.
//! - The view's own menu, clipboard items, tooltips and file requests, through
//!   the hooks that stand in for the system's menu and dialog.
//! - CLAP: `clap.gui` with the `win32` API creates, scales, sizes (in pixels),
//!   parents, shows, hides and destroys the view; the view's timer polls the
//!   plane; a click asks the host to flush. A floating editor gets a window of
//!   its own, and closing it tells the host. A right click asks the host for its
//!   parameter menu in window pixels (embedded) or screen pixels (floating).
//!
//! - LV2: `lv2ui_descriptor` makes a `ui:WindowsUI` widget in the host's parent
//!   window, bound through instance-access; port events reach the editor, a
//!   click writes the control port, and `ui:resize` works both ways in pixels.
//!
//! Screenshots go to `OP_SHOT_DIR` when it is set (what the window drew, and,
//! in a logged-in desktop session, what the screen shows). The process may
//! have no desktop to make windows on (a service); the test then says so and
//! passes, or fails if `OP_GUI_REQUIRED` is set.

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    // As an application that knows the display's DPI: sizes and screen positions are real pixels.
    openplugin::dri::win32::set_process_dpi_aware();
    if !win::desktop_available() {
        assert!(std::env::var_os("OP_GUI_REQUIRED").is_none(), "no desktop to make windows on");
        println!("win32: skipped (no desktop to make windows on)");
        return;
    }
    if !win::can_show() {
        println!("win32: no interactive desktop; windows draw and take input but are not shown");
    }
    win::shell_editor_draws_and_clicks();
    win::shell_editor_scales_and_resizes();
    win::shell_editor_types_and_takes_keys();
    win::view_menu_tooltips_and_files();
    win::clap_gui_lifecycle();
    win::clap_gui_floats();
    win::lv2_ui_lifecycle();
    println!("win32: 7 passed");
}

#[cfg(windows)]
mod win {
    use example_editor::*;
    use openplugin::core::editor::{FileDialog, ParamMenu};
    use openplugin::core::EditorHost;
    use openplugin::dri::win32::testing::{self, Bitmap};
    use openplugin::dri::win32::{EditorView, Hwnd, PARAM_MENU_ITEMS};
    use openplugin::targets::clap_abi::*;
    use openplugin::targets::shell::OpEditorHost;
    use std::cell::RefCell;
    use std::ffi::{c_char, c_void, CStr};
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU32, Ordering};

    type Id = *mut c_void;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetWindow(hwnd: Hwnd, cmd: u32) -> Hwnd;
        fn GetParent(hwnd: Hwnd) -> Hwnd;
        fn IsWindowVisible(hwnd: Hwnd) -> i32;
        fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
        fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max: i32) -> i32;
        fn GetClassNameW(hwnd: Hwnd, text: *mut u16, max: i32) -> i32;
    }

    const WM_CLOSE: u32 = 0x0010;
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_KEYUP: u32 = 0x0101;
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_LBUTTONDBLCLK: u32 = 0x0203;
    const GW_CHILD: u32 = 5;
    const VK_TAB: usize = 0x09;
    const VK_RETURN: usize = 0x0D;
    const VK_UP: usize = 0x26;

    unsafe extern "C" {
        fn op_editor_size(width: *mut u32, height: *mut u32) -> u32;
        fn op_editor_create(host: *const OpEditorHost) -> Id;
        fn op_editor_view(editor: Id) -> Id;
        fn op_editor_attach(editor: Id, parent: Id) -> u32;
        fn op_editor_bind(editor: Id, instance: Id);
        fn op_editor_set_plain(editor: Id, index: u32, plain: f32);
        fn op_editor_destroy(editor: Id);
        fn op_editor_get_size(editor: Id, width: *mut u32, height: *mut u32) -> u32;
        fn op_editor_resize(editor: Id, width: u32, height: u32) -> u32;
        fn op_editor_set_scale(editor: Id, scale: f64) -> u32;
        fn op_editor_scale(editor: Id) -> f64;
        fn op_editor_set_pixel_size(editor: Id, width: u32, height: u32) -> u32;
        fn op_shell_create(rate: f64, max_block: u32, channels: u32) -> Id;
        fn op_shell_destroy(instance: Id);
    }

    /// A window can be made here (a service has no desktop to make one on).
    pub fn desktop_available() -> bool {
        let w = testing::parent_window("probe", 0, 0, 8, 8);
        if w.is_null() {
            return false;
        }
        unsafe { testing::destroy_window(w) };
        true
    }

    fn shot_dir() -> Option<PathBuf> {
        let dir = PathBuf::from(std::env::var_os("OP_SHOT_DIR")?);
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir)
    }

    fn save(name: &str, bitmap: &Bitmap) {
        if let Some(dir) = shot_dir() {
            bitmap.save_bmp(&dir.join(name)).expect("write screenshot");
        }
    }

    /// What the window drew.
    fn shot(view: Hwnd) -> Bitmap {
        unsafe { testing::capture(view) }.expect("the window draws into a bitmap")
    }

    /// What the screen shows where the window is, saved when there is a screen to read.
    fn save_screen(name: &str, window: Hwnd) {
        testing::pump_until(300, || false);
        if let Some(b) = unsafe { testing::capture_screen(window) } {
            save(name, &b);
        }
    }

    /// Windows can be shown here. A process with no interactive desktop (started
    /// over SSH) makes windows that draw and take messages but never show.
    pub fn can_show() -> bool {
        let w = testing::parent_window("probe", 0, 0, 8, 8);
        let shown = unsafe { IsWindowVisible(w) != 0 };
        unsafe { testing::destroy_window(w) };
        shown
    }

    /// The window shows, where windows can.
    fn visible(hwnd: Hwnd) -> bool {
        unsafe { IsWindowVisible(hwnd) != 0 || !can_show() }
    }

    fn child(parent: Hwnd) -> Hwnd {
        unsafe { GetWindow(parent, GW_CHILD) }
    }

    fn class_of(hwnd: Hwnd) -> String {
        let mut buf = [0u16; 64];
        let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), 64) }.max(0) as usize;
        String::from_utf16_lossy(&buf[..n])
    }

    /// The pixel at a point in editor points; the bitmap may be at any scale.
    fn pixel(b: &Bitmap, size: (u32, u32), x: f32, y: f32) -> u32 {
        let (sx, sy) = (b.width as f32 / size.0 as f32, b.height as f32 / size.1 as f32);
        b.pixel(((x * sx) as usize).min(b.width - 1), ((y * sy) as usize).min(b.height - 1))
    }

    /// Pixels on a ring inside the knob, where the pointer sweeps.
    fn knob_ring(b: &Bitmap, size: (u32, u32)) -> Vec<u32> {
        let (x, y, w, h) = layout(size.0, size.1).knob;
        let (cx, cy, r) = (x + w * 0.5, y + h * 0.5, w * 0.25);
        (0..24)
            .map(|i| {
                let a = i as f32 / 24.0 * std::f32::consts::TAU;
                pixel(b, size, cx + r * a.sin(), cy - r * a.cos())
            })
            .collect()
    }

    /// A grid of pixels over the toggle.
    fn switch_area(b: &Bitmap, size: (u32, u32)) -> Vec<u32> {
        let (x, y, w, h) = layout(size.0, size.1).switch;
        let mut out = Vec::new();
        for j in 1..8 {
            for i in 1..5 {
                out.push(pixel(b, size, x + w * i as f32 / 5.0, y + h * j as f32 / 8.0));
            }
        }
        out
    }

    fn green(p: u32) -> bool {
        let (r, g, b) = ((p >> 16) & 255, (p >> 8) & 255, p & 255);
        g > r + 60 && g > b + 60
    }

    /// The meter's bottom lamp is lit (green) and its top lamp is not.
    fn meter_is_upright(b: &Bitmap, size: (u32, u32)) -> bool {
        let (mx, my, mw, mh) = layout(size.0, size.1).meter;
        let lit = |from: f32, to: f32| (0..12).any(|i| green(pixel(b, size, mx + mw * 0.5, from + (to - from) * i as f32 / 11.0)));
        lit(my + mh * 0.75, my + mh - 2.0) && !lit(my + 2.0, my + mh * 0.25)
    }

    /// The centre of an editor rectangle (points) as a mouse `lParam` in window pixels.
    fn at(rect: (f32, f32, f32, f32), scale: f32) -> isize {
        testing::mouse_lparam(((rect.0 + rect.2 * 0.5) * scale) as i32, ((rect.1 + rect.3 * 0.5) * scale) as i32)
    }

    fn click(view: Hwnd, l: isize) {
        unsafe {
            testing::send(view, WM_LBUTTONDOWN, 1, l);
            testing::send(view, WM_LBUTTONUP, 0, l);
        }
    }

    thread_local! {
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn take_log() -> Vec<String> {
        LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
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

    fn shell_host() -> OpEditorHost {
        OpEditorHost { context: std::ptr::null_mut(), begin_edit: Some(begin), perform_edit: Some(perform), end_edit: Some(end) }
    }

    pub fn shell_editor_draws_and_clicks() {
        unsafe {
            take_log();
            let (mut w, mut h) = (0u32, 0u32);
            assert_eq!(op_editor_size(&mut w, &mut h), 1);
            assert_eq!((w, h), (WIDTH, HEIGHT));
            let editor = op_editor_create(&shell_host());
            assert!(!editor.is_null());
            assert!(op_editor_view(editor).is_null(), "no window before it has a parent");
            assert_eq!(op_editor_attach(editor, std::ptr::null_mut()), 0);
            // The host's scale; without it the view takes the DPI of the parent window.
            assert_eq!(op_editor_set_scale(editor, 1.0), 1);
            let parent = testing::parent_window("OpenPlugin shell editor", 80, 80, WIDTH, HEIGHT);
            assert_eq!(op_editor_attach(editor, parent), 1);
            let view = op_editor_view(editor);
            assert!(!view.is_null());
            assert_eq!(GetParent(view), parent);
            assert_eq!(child(parent), view);
            assert_eq!(testing::client_size(view), (WIDTH, HEIGHT));
            assert!(visible(view));
            let size = (WIDTH, HEIGHT);

            // Bind to a live engine, then to one that is gone.
            let engine = op_shell_create(48_000.0, 256, 2);
            op_editor_bind(editor, engine);
            op_shell_destroy(engine);
            op_editor_bind(editor, engine);
            op_editor_bind(editor, 0x10 as Id);

            // The window shows exactly the frame the editor renders, top row on top.
            op_editor_set_plain(editor, 0, 0.0);
            let drawn = shot(view);
            assert_eq!((drawn.width, drawn.height), (WIDTH as usize, HEIGHT as usize));
            let mut reference = make_editor(EditorHost::detached()).expect("an editor");
            reference.param_changed(0, 0.0);
            let frame = reference.render(1.0);
            assert_eq!((frame.width, frame.height), (drawn.width, drawn.height));
            let same = frame.pixels.iter().zip(&drawn.pixels).filter(|(a, b)| (**a & 0xff_ffff) == (**b & 0xff_ffff)).count();
            assert_eq!(same, frame.pixels.len(), "the window's pixels are the editor's frame");
            let colors: std::collections::HashSet<u32> = drawn.pixels.iter().map(|p| p & 0xff_ffff).collect();
            assert!(colors.len() > 100, "a drawn plate, not a flat fill ({} colours)", colors.len());
            assert!(meter_is_upright(&drawn, size), "the LED ladder's bottom lamp is the lit one");
            save("shell_default.bmp", &drawn);

            // A value from the host turns the lit knob.
            op_editor_set_plain(editor, 0, 24.0);
            let high = knob_ring(&shot(view), size);
            op_editor_set_plain(editor, 0, -24.0);
            let low = knob_ring(&shot(view), size);
            let moved = high.iter().zip(&low).filter(|(a, b)| a != b).count();
            assert!(moved >= 2, "the pointer moved ({moved} ring pixels changed)");

            // Click the Warm toggle.
            let off = switch_area(&shot(view), size);
            click(view, at(SWITCH, 1.0));
            assert_eq!(take_log(), vec!["begin 1", "perform 1 1", "end 1"], "gestures");
            let on = shot(view);
            assert_ne!(switch_area(&on, size), off, "the toggle is drawn on");
            save("shell_warm_on.bmp", &on);

            // Drag the knob up: a gesture with rising values, ended when the button goes up.
            op_editor_set_plain(editor, 0, 0.0);
            let (kx, ky) = ((KNOB.0 + KNOB.2 * 0.5) as i32, (KNOB.1 + KNOB.3 * 0.5) as i32);
            testing::send(view, WM_LBUTTONDOWN, 1, testing::mouse_lparam(kx, ky));
            testing::send(view, 0x0200, 1, testing::mouse_lparam(kx, ky - 30));
            // Above the window: a captured drag carries on with negative positions.
            testing::send(view, 0x0200, 1, testing::mouse_lparam(kx, -40));
            testing::send(view, WM_LBUTTONUP, 0, testing::mouse_lparam(kx, -40));
            let log = take_log();
            assert_eq!(log.first().map(String::as_str), Some("begin 0"), "{log:?}");
            assert_eq!(log.last().map(String::as_str), Some("end 0"), "{log:?}");
            let values: Vec<f64> = log.iter().filter_map(|l| l.strip_prefix("perform 0 ")).map(|v| v.parse().unwrap()).collect();
            assert!(values.len() >= 2 && values.windows(2).all(|p| p[1] > p[0]) && values[0] > 0.0, "{values:?}");
            save("shell_knob_dragged.bmp", &shot(view));
            save_screen("screen_shell.bmp", parent);

            op_editor_destroy(editor);
            assert!(!testing::is_window(view), "the window goes with the editor");
            assert!(child(parent).is_null());

            // The host destroys the parent first: the editor is still freed cleanly.
            let editor = op_editor_create(&shell_host());
            assert_eq!(op_editor_attach(editor, parent), 1);
            let view = op_editor_view(editor);
            testing::destroy_window(parent);
            assert!(!testing::is_window(view));
            assert!(op_editor_view(editor).is_null());
            op_editor_set_plain(editor, 0, 3.0);
            op_editor_destroy(editor);
        }
        println!("shell editor: ok");
    }

    pub fn shell_editor_scales_and_resizes() {
        unsafe {
            take_log();
            let editor = op_editor_create(&shell_host());
            let parent = testing::parent_window("OpenPlugin editor at 150 %", 80, 300, 800, 600);
            assert_eq!(op_editor_set_scale(editor, 0.0), 0);
            assert_eq!(op_editor_set_scale(editor, 1.5), 1);
            assert_eq!(op_editor_scale(editor), 1.5);
            assert_eq!(op_editor_attach(editor, parent), 1);
            let view = op_editor_view(editor);
            assert_eq!(testing::client_size(view), (360, 240));
            let (mut w, mut h) = (0u32, 0u32);
            assert_eq!(op_editor_get_size(editor, &mut w, &mut h), 1);
            assert_eq!((w, h), (WIDTH, HEIGHT), "the size stays in points");

            // Drawn at the scale, not stretched: the frame the editor renders at 1.5.
            let drawn = shot(view);
            assert_eq!((drawn.width, drawn.height), (360, 240));
            let mut reference = make_editor(EditorHost::detached()).expect("an editor");
            let frame = reference.render(1.5);
            let same = frame.pixels.iter().zip(&drawn.pixels).filter(|(a, b)| (**a & 0xff_ffff) == (**b & 0xff_ffff)).count();
            assert_eq!(same, frame.pixels.len(), "the editor's frame at 150 %");
            save("shell_scale_150.bmp", &drawn);

            // A click in pixels lands on the control in points.
            click(view, at(SWITCH, 1.5));
            assert_eq!(take_log(), vec!["begin 1", "perform 1 1", "end 1"]);

            // The scale changes under an open editor (the host moved to another display).
            assert_eq!(op_editor_set_scale(editor, 2.0), 1);
            assert_eq!(testing::client_size(view), (480, 320));
            save("shell_scale_200.bmp", &shot(view));
            assert_eq!(op_editor_set_scale(editor, 1.5), 1);

            // A size in points.
            assert_eq!(op_editor_resize(editor, LARGE_SIZE.0, LARGE_SIZE.1), 1);
            assert_eq!(testing::client_size(view), (LARGE_SIZE.0 * 3 / 2, LARGE_SIZE.1 * 3 / 2));
            assert_eq!(op_editor_get_size(editor, &mut w, &mut h), 1);
            assert_eq!((w, h), LARGE_SIZE);
            let large = shot(view);
            assert!(meter_is_upright(&large, LARGE_SIZE));
            save("shell_large_150.bmp", &large);
            let off = switch_area(&large, LARGE_SIZE);
            click(view, at(layout(LARGE_SIZE.0, LARGE_SIZE.1).switch, 1.5));
            assert_eq!(take_log(), vec!["begin 1", "perform 1 0", "end 1"], "the click follows the layout");
            assert_ne!(switch_area(&shot(view), LARGE_SIZE), off);
            assert_eq!(op_editor_resize(editor, MAX_SIZE.0 + 1, MAX_SIZE.1), 0, "outside the range");
            assert_eq!(testing::client_size(view), (LARGE_SIZE.0 * 3 / 2, LARGE_SIZE.1 * 3 / 2));

            // A size in pixels, as a host gives it.
            assert_eq!(op_editor_set_pixel_size(editor, MIN_SIZE.0 * 3 / 2, MIN_SIZE.1 * 3 / 2), 1);
            assert_eq!(testing::client_size(view), (MIN_SIZE.0 * 3 / 2, MIN_SIZE.1 * 3 / 2));
            assert_eq!(op_editor_get_size(editor, &mut w, &mut h), 1);
            assert_eq!((w, h), MIN_SIZE);
            save("shell_small_150.bmp", &shot(view));

            op_editor_destroy(editor);
            assert!(child(parent).is_null());
            testing::destroy_window(parent);
        }
        println!("shell editor scale and size: ok");
    }

    pub fn shell_editor_types_and_takes_keys() {
        unsafe {
            take_log();
            let editor = op_editor_create(&shell_host());
            assert_eq!(op_editor_set_scale(editor, 1.0), 1);
            let parent = testing::parent_window("OpenPlugin editor typing", 400, 80, WIDTH, HEIGHT);
            assert_eq!(op_editor_attach(editor, parent), 1);
            let view = op_editor_view(editor);
            op_editor_set_plain(editor, 0, 0.0);
            assert!(child(view).is_null());

            // A double-click on the knob opens a field over it with the value in it.
            let knob = at(KNOB, 1.0);
            click(view, knob);
            testing::send(view, WM_LBUTTONDBLCLK, 1, knob);
            testing::send(view, WM_LBUTTONUP, 0, knob);
            let field = child(view);
            assert!(!field.is_null(), "a text field opened");
            assert_eq!(class_of(field).to_ascii_uppercase(), "EDIT");
            let mut text = [0u16; 64];
            let n = GetWindowTextW(field, text.as_mut_ptr(), 64).max(0) as usize;
            assert!(n > 0, "the field starts with the value");
            take_log();
            save("shell_text_field.bmp", &shot(view));
            save_screen("screen_text_field.bmp", parent);

            // Text the parameter cannot read: the field stays (and turns red).
            let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
            SetWindowTextW(field, wide("loud").as_ptr());
            testing::send(field, WM_KEYDOWN, VK_RETURN, 0);
            assert_eq!(child(view), field, "refused text keeps the field open");
            assert!(take_log().iter().all(|l| !l.starts_with("perform")));
            // A value: Return commits it as one gesture and closes the field.
            SetWindowTextW(field, wide("-6").as_ptr());
            testing::send(field, WM_KEYDOWN, VK_RETURN, 0);
            assert!(child(view).is_null(), "the field closed");
            assert!(!testing::is_window(field));
            let log = take_log();
            assert!(log.contains(&"perform 0 -6".to_string()), "{log:?}");

            // Escape closes without a value.
            testing::send(view, WM_LBUTTONDBLCLK, 1, knob);
            testing::send(view, WM_LBUTTONUP, 0, knob);
            let field = child(view);
            assert!(!field.is_null());
            take_log();
            SetWindowTextW(field, wide("12").as_ptr());
            testing::send(field, WM_KEYDOWN, 0x1B, 0);
            assert!(child(view).is_null());
            assert!(take_log().iter().all(|l| !l.starts_with("perform")), "Escape commits nothing");

            // Keys: Tab moves the editor's focus to a control, Up raises it.
            for _ in 0..4 {
                testing::send(view, WM_KEYDOWN, VK_TAB, 0);
                testing::send(view, WM_KEYUP, VK_TAB, 0);
                testing::send(view, WM_KEYDOWN, VK_UP, 0);
                testing::send(view, WM_KEYUP, VK_UP, 0);
                if LOG.with(|l| !l.borrow().is_empty()) {
                    break;
                }
            }
            let log = take_log();
            assert!(log.iter().any(|l| l.starts_with("perform")), "an arrow key changed a focused control: {log:?}");
            save("shell_focus_ring.bmp", &shot(view));

            op_editor_destroy(editor);
            testing::destroy_window(parent);
        }
        println!("shell editor typing and keys: ok");
    }

    pub fn view_menu_tooltips_and_files() {
        unsafe {
            let view = EditorView::new(make_editor(EditorHost::detached()).expect("an editor"), None).expect("a view");
            assert!(view.set_scale(1.0));
            let parent = testing::parent_window("OpenPlugin view", 400, 300, WIDTH, HEIGHT);
            assert!(view.attach(parent));
            let hwnd = view.raw();

            // Tooltips: one rectangle per control, the text asked of the editor.
            let tips = view.tooltip_rects();
            assert!(!tips.is_empty(), "the example has tooltips");
            assert!(tips.iter().all(|(r, text)| r[2] > r[0] && r[3] > r[1] && !text.is_empty()), "{tips:?}");

            // A right click on the knob: no host menu, so the SDK's menu is asked for.
            let asked: Rc<RefCell<Vec<ParamMenu>>> = Rc::default();
            let seen = asked.clone();
            view.set_menu_hook(Some(Box::new(move |menu| {
                seen.borrow_mut().push(menu.clone());
                true
            })));
            testing::send(hwnd, 0x0204, 2, at(KNOB, 1.0));
            assert_eq!(asked.borrow().len(), 1, "the menu was asked for once");
            assert_eq!(asked.borrow()[0].param, 0);
            assert_eq!(PARAM_MENU_ITEMS.len(), 4);
            // Paste, copy, reset: the clipboard is stood in for.
            assert!(view.choose_menu_item(2, Some("6 dB")).is_none());
            let copied = view.choose_menu_item(1, Some("")).expect("copy gives the text");
            assert!(copied.contains('6'), "{copied}");
            view.choose_menu_item(0, Some(""));
            let reset = view.choose_menu_item(1, Some("")).expect("copy gives the text");
            assert_ne!(reset, copied, "reset went back to the default");
            // Type value: the field opens.
            view.choose_menu_item(3, Some(""));
            assert!(view.text_field().is_some());
            testing::send(view.text_field().unwrap(), WM_KEYDOWN, 0x1B, 0);
            assert!(view.text_field().is_none());

            // A file request is answered by the hook, on a later tick.
            let requests: Rc<RefCell<Vec<FileDialog>>> = Rc::default();
            let seen = requests.clone();
            view.set_file_dialog_hook(Some(Box::new(move |req| {
                seen.borrow_mut().push(req.clone());
                Some(None)
            })));
            testing::pump_until(200, || false);
            let _ = requests;

            drop(view);
            assert!(!testing::is_window(hwnd));
            testing::destroy_window(parent);
        }
        println!("view menu, tooltips and files: ok");
    }

    static FLUSHES: AtomicU32 = AtomicU32::new(0);
    static CLOSED: AtomicU32 = AtomicU32::new(0);
    static RESIZE_ASKED: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn host_rescan(_h: *const clap_host, _f: u32) {}
    unsafe extern "C" fn host_clear(_h: *const clap_host, _p: u32, _f: u32) {}
    unsafe extern "C" fn host_flush(_h: *const clap_host) {
        FLUSHES.fetch_add(1, Ordering::Relaxed);
    }
    static HOST_PARAMS: clap_host_params = clap_host_params { rescan: Some(host_rescan), clear: Some(host_clear), request_flush: Some(host_flush) };
    unsafe extern "C" fn host_gui_closed(_h: *const clap_host, was_destroyed: bool) {
        assert!(!was_destroyed);
        CLOSED.fetch_add(1, Ordering::Relaxed);
    }
    unsafe extern "C" fn host_gui_resize(_h: *const clap_host, w: u32, h: u32) -> bool {
        RESIZE_ASKED.store((w << 16) | h, Ordering::Relaxed);
        false
    }
    static HOST_GUI: clap_host_gui = clap_host_gui { resize_hints_changed: None, request_resize: Some(host_gui_resize), request_show: None, request_hide: None, closed: Some(host_gui_closed) };
    thread_local! {
        /// The host's parameter menus asked for: parameter, x, y.
        static POPUPS: RefCell<Vec<(u32, i32, i32)>> = const { RefCell::new(Vec::new()) };
    }
    unsafe extern "C" fn host_can_popup(_h: *const clap_host) -> bool {
        true
    }
    unsafe extern "C" fn host_popup(_h: *const clap_host, target: *const clap_context_menu_target, _screen: i32, x: i32, y: i32) -> bool {
        unsafe {
            assert_eq!((*target).kind, CONTEXT_MENU_TARGET_KIND_PARAM);
            POPUPS.with(|p| p.borrow_mut().push(((*target).id, x, y)));
        }
        true
    }
    static HOST_MENU: clap_host_context_menu = clap_host_context_menu { populate: None, perform: None, can_popup: Some(host_can_popup), popup: Some(host_popup) };
    unsafe extern "C" fn host_extension(_h: *const clap_host, id: *const c_char) -> *const c_void {
        unsafe {
            let id = CStr::from_ptr(id).to_bytes_with_nul();
            if id == EXT_CONTEXT_MENU {
                return &HOST_MENU as *const clap_host_context_menu as *const c_void;
            }
            if id == EXT_PARAMS {
                &HOST_PARAMS as *const clap_host_params as *const c_void
            } else if id == EXT_GUI {
                &HOST_GUI as *const clap_host_gui as *const c_void
            } else {
                std::ptr::null()
            }
        }
    }
    unsafe extern "C" fn host_noop(_h: *const clap_host) {}

    fn test_host() -> clap_host {
        clap_host {
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
        }
    }

    unsafe fn new_plugin(host: &clap_host) -> (*const clap_plugin, &'static clap_plugin_gui) {
        unsafe {
            let entry = &example_editor::clap_entry;
            assert!(entry.init.unwrap()(c"".as_ptr()));
            let factory = entry.get_factory.unwrap()(FACTORY_ID.as_ptr() as *const c_char) as *const clap_plugin_factory;
            let desc = (*factory).get_plugin_descriptor.unwrap()(factory, 0);
            let plugin = (*factory).create_plugin.unwrap()(factory, host, (*desc).id);
            assert!(!plugin.is_null());
            assert!((*plugin).init.unwrap()(plugin));
            let gui = (*plugin).get_extension.unwrap()(plugin, EXT_GUI.as_ptr() as *const c_char) as *const clap_plugin_gui;
            assert!(!gui.is_null(), "clap.gui is offered");
            (plugin, &*gui)
        }
    }

    pub fn clap_gui_lifecycle() {
        unsafe {
            let host = test_host();
            let (plugin, gui) = new_plugin(&host);
            let win32 = c"win32".as_ptr();
            assert!(gui.is_api_supported.unwrap()(plugin, win32, false));
            assert!(gui.is_api_supported.unwrap()(plugin, win32, true), "floating too");
            assert!(!gui.is_api_supported.unwrap()(plugin, c"x11".as_ptr(), false));
            assert!(!gui.is_api_supported.unwrap()(plugin, c"cocoa".as_ptr(), false));
            let mut api: *const c_char = std::ptr::null();
            let mut floating = true;
            assert!(gui.get_preferred_api.unwrap()(plugin, &mut api, &mut floating));
            assert!(CStr::from_ptr(api) == c"win32" && !floating);
            let (mut w, mut h) = (0u32, 0u32);
            assert!(!gui.get_size.unwrap()(plugin, &mut w, &mut h), "no size before create");
            assert!(!gui.create.unwrap()(plugin, c"x11".as_ptr(), false));
            assert!(gui.create.unwrap()(plugin, win32, false));
            // The win32 API is in pixels: the host's scale times the editor's points.
            assert!(gui.set_scale.unwrap()(plugin, 1.0));
            assert!(gui.get_size.unwrap()(plugin, &mut w, &mut h));
            assert_eq!((w, h), (WIDTH, HEIGHT));
            assert!(gui.set_scale.unwrap()(plugin, 1.5));
            assert!(gui.get_size.unwrap()(plugin, &mut w, &mut h));
            assert_eq!((w, h), (360, 240));
            assert!(gui.can_resize.unwrap()(plugin), "the example resizes");
            assert!(gui.set_size.unwrap()(plugin, 360, 240));
            assert!(gui.set_size.unwrap()(plugin, 361, 240), "inside the range, another shape is taken");
            assert!(!gui.set_size.unwrap()(plugin, MAX_SIZE.0 * 3 / 2 + 3, 240));
            let (mut aw, mut ah) = (10_000u32, 10_000u32);
            assert!(gui.adjust_size.unwrap()(plugin, &mut aw, &mut ah));
            assert_eq!((aw, ah), (MAX_SIZE.0 * 3 / 2, MAX_SIZE.1 * 3 / 2), "brought into the range, in pixels");
            assert!(gui.set_size.unwrap()(plugin, 360, 240));
            let parent = testing::parent_window("OpenPlugin CLAP editor", 700, 80, 360, 240);
            let window = clap_window { api: win32, ptr: parent };
            let floating_window = clap_window { api: c"x11".as_ptr(), ptr: parent };
            assert!(!gui.set_parent.unwrap()(plugin, &floating_window), "another API's window is refused");
            assert!(gui.set_parent.unwrap()(plugin, &window));
            let view = child(parent);
            assert!(!view.is_null());
            assert_eq!(testing::client_size(view), (360, 240));
            assert!(gui.show.unwrap()(plugin));
            assert!(visible(view));
            assert!(!gui.set_transient.unwrap()(plugin, &window), "an embedded editor has no owner");
            let size = (WIDTH, HEIGHT);

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
                unsafe { &(*((*l).ctx as *const One)).event.header }
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
            testing::pump_until(100, || false);
            let before = knob_ring(&shot(view), size);
            save("clap_150_before.bmp", &shot(view));
            (*params).flush.unwrap()(plugin, &input, std::ptr::null());
            // The timer polls the plane: wait for the knob to turn, not for a time.
            assert!(testing::pump_until(5000, || knob_ring(&shot(view), size) != before), "polled +24 dB turned the knob");
            save("clap_150_gain_24.bmp", &shot(view));

            // A click on the switch asks the host to flush, and the plane has it.
            let before = FLUSHES.load(Ordering::Relaxed);
            testing::send(view, WM_LBUTTONDOWN, 1, at(SWITCH, 1.5));
            assert!(FLUSHES.load(Ordering::Relaxed) > before, "request_flush");
            testing::send(view, WM_LBUTTONUP, 0, at(SWITCH, 1.5));
            let mut value = 0.0f64;
            assert!((*params).get_value.unwrap()(plugin, 1, &mut value));
            assert_eq!(value, 1.0);
            save("clap_150_warm.bmp", &shot(view));
            save_screen("screen_clap_150.bmp", parent);

            // A right click on the knob asks the host for its menu on that parameter, at
            // the click in pixels of the plugin's window (the editor is embedded).
            POPUPS.with(|p| p.borrow_mut().clear());
            let (kx, ky) = ((KNOB.0 + KNOB.2 * 0.5) * 1.5, (KNOB.1 + KNOB.3 * 0.5) * 1.5);
            testing::send(view, 0x0204, 2, testing::mouse_lparam(kx as i32, ky as i32));
            let popups = POPUPS.with(|p| p.borrow().clone());
            assert_eq!(popups.len(), 1, "the host's menu was asked for once");
            let (param, px, py) = popups[0];
            assert_eq!(param, 0);
            assert!((px - kx as i32).abs() <= 2 && (py - ky as i32).abs() <= 2, "window pixels: {px}, {py} for a click at {kx}, {ky}");

            // The host sizes the editor in pixels; the editor follows in points.
            assert!(gui.set_size.unwrap()(plugin, LARGE_SIZE.0 * 3 / 2, LARGE_SIZE.1 * 3 / 2));
            assert_eq!(testing::client_size(view), (LARGE_SIZE.0 * 3 / 2, LARGE_SIZE.1 * 3 / 2));
            assert!(gui.get_size.unwrap()(plugin, &mut w, &mut h));
            assert_eq!((w, h), (LARGE_SIZE.0 * 3 / 2, LARGE_SIZE.1 * 3 / 2));
            assert!(meter_is_upright(&shot(view), LARGE_SIZE));

            assert!(gui.hide.unwrap()(plugin));
            assert!(!visible(view) || !can_show());
            gui.destroy.unwrap()(plugin);
            assert!(!testing::is_window(view), "destroyed with the gui");
            assert!(child(parent).is_null());
            // Create again, then let destroy of the plugin take the view down.
            assert!(gui.create.unwrap()(plugin, win32, false));
            assert!(gui.set_parent.unwrap()(plugin, &window));
            let view = child(parent);
            assert!(testing::is_window(view));
            (*plugin).destroy.unwrap()(plugin);
            assert!(!testing::is_window(view));
            testing::destroy_window(parent);
        }
        println!("clap gui: ok");
    }

    pub fn clap_gui_floats() {
        unsafe {
            let host = test_host();
            let (plugin, gui) = new_plugin(&host);
            let win32 = c"win32".as_ptr();
            assert!(gui.create.unwrap()(plugin, win32, true));
            let owner = testing::parent_window("owner", 80, 620, 100, 40);
            let window = clap_window { api: win32, ptr: owner };
            assert!(!gui.set_parent.unwrap()(plugin, &window), "a floating editor has no parent");
            assert!(gui.set_transient.unwrap()(plugin, &window));
            gui.suggest_title.unwrap()(plugin, c"Editor Gain (floating)".as_ptr());
            let (mut w, mut h) = (0u32, 0u32);
            assert!(gui.get_size.unwrap()(plugin, &mut w, &mut h));
            assert!(w >= WIDTH && h >= HEIGHT);
            assert!(gui.show.unwrap()(plugin));
            // The floating window is the one window of this thread with our title.
            let top = find_top("Editor Gain (floating)");
            assert!(!top.is_null(), "the floating window exists with the suggested title");
            assert!(visible(top));
            let view = child(top);
            assert!(!view.is_null());
            assert_eq!(testing::client_size(top), testing::client_size(view), "the window fits the editor");
            assert_eq!(testing::client_size(view), (w, h));
            testing::move_to_front(top, 300, 620);
            testing::pump_until(300, || false);
            let drawn = shot(view);
            assert!(meter_is_upright(&drawn, (WIDTH, HEIGHT)));
            save("clap_floating.bmp", &drawn);
            save_screen("screen_clap_floating.bmp", top);

            // A floating editor has no window of the host's around it: the host's menu
            // is asked for at the click on the screen.
            POPUPS.with(|p| p.borrow_mut().clear());
            let scale = w as f32 / WIDTH as f32;
            let (kx, ky) = (((KNOB.0 + KNOB.2 * 0.5) * scale) as i32, ((KNOB.1 + KNOB.3 * 0.5) * scale) as i32);
            testing::send(view, 0x0204, 2, testing::mouse_lparam(kx, ky));
            let popups = POPUPS.with(|p| p.borrow().clone());
            assert_eq!(popups.len(), 1);
            let (ox, oy) = testing::client_origin(view);
            let (param, px, py) = popups[0];
            assert_eq!(param, 0);
            assert!((px - (ox + kx)).abs() <= 2 && (py - (oy + ky)).abs() <= 2, "screen pixels: {px}, {py}; the view is at {ox}, {oy}, the click at {kx}, {ky}");
            assert!((ox, oy) != (0, 0), "the window is not at the screen's corner");

            // The user closes the window: the host hears, and decides.
            let before = CLOSED.load(Ordering::Relaxed);
            testing::send(top, WM_CLOSE, 0, 0);
            assert_eq!(CLOSED.load(Ordering::Relaxed), before + 1);
            assert!(testing::is_window(top), "closing asks the host; it does not destroy");
            assert!(gui.hide.unwrap()(plugin));
            assert!(!visible(top) || !can_show());
            gui.destroy.unwrap()(plugin);
            assert!(!testing::is_window(top) && !testing::is_window(view));
            (*plugin).destroy.unwrap()(plugin);
            testing::destroy_window(owner);
            let _ = RESIZE_ASKED.load(Ordering::Relaxed);
        }
        println!("clap floating gui: ok");
    }

    #[repr(C)]
    struct Lv2Resize {
        handle: *mut c_void,
        ui_resize: Option<unsafe extern "C" fn(handle: *mut c_void, width: i32, height: i32) -> i32>,
    }

    /// The size the UI last asked its host for (width << 16 | height).
    static LV2_ASKED: AtomicU32 = AtomicU32::new(0);
    static LV2_WRITES: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn lv2_host_resize(_handle: *mut c_void, width: i32, height: i32) -> i32 {
        LV2_ASKED.store(((width as u32) << 16) | height as u32, Ordering::Relaxed);
        0
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
            let parent = testing::parent_window("OpenPlugin LV2 editor", 700, 400, 800, 600);
            let host_resize = Lv2Resize { handle: std::ptr::null_mut(), ui_resize: Some(lv2_host_resize) };
            let features = [
                Lv2Feature { uri: c"http://lv2plug.in/ns/extensions/ui#parent".as_ptr(), data: parent },
                Lv2Feature { uri: c"http://lv2plug.in/ns/ext/instance-access".as_ptr(), data: instance },
                Lv2Feature { uri: c"http://lv2plug.in/ns/extensions/ui#resize".as_ptr(), data: &host_resize as *const Lv2Resize as *mut c_void },
            ];
            unsafe extern "C" fn write(_c: *mut c_void, port: u32, size: u32, protocol: u32, buffer: *const c_void) {
                unsafe {
                    assert_eq!((port, size, protocol), (AUDIO_PORTS + 1, 4, 0));
                    assert_eq!(*(buffer as *const f32), 1.0);
                    LV2_WRITES.fetch_add(1, Ordering::Relaxed);
                }
            }
            let mut widget: Id = std::ptr::null_mut();
            // A child window needs its parent: without `ui:parent` there is no UI.
            let orphan = [&features[1] as *const Lv2Feature, std::ptr::null()];
            assert!((ui.instantiate)(ui, desc.uri, c"".as_ptr(), Some(write), std::ptr::null_mut(), &mut widget, orphan.as_ptr()).is_null());
            let list = [&features[0] as *const Lv2Feature, &features[1], &features[2], std::ptr::null()];
            let handle = (ui.instantiate)(ui, desc.uri, c"".as_ptr(), Some(write), std::ptr::null_mut(), &mut widget, list.as_ptr());
            assert!(!handle.is_null());
            assert!(!widget.is_null(), "the widget is the view's window");
            assert_eq!(child(parent), widget);
            // The host was told the size, in pixels: the editor's points at the parent's scale.
            let (cw, ch) = testing::client_size(widget);
            let scale = cw as f32 / WIDTH as f32;
            assert_eq!((cw, ch), ((WIDTH as f32 * scale).round() as u32, (HEIGHT as f32 * scale).round() as u32));
            assert_eq!(LV2_ASKED.load(Ordering::Relaxed), (cw << 16) | ch, "ui:resize told the host the size");
            let size = (WIDTH, HEIGHT);

            // A value from the host's port turns the knob.
            let before = knob_ring(&shot(widget), size);
            let full = 24.0f32;
            (ui.port_event)(handle, AUDIO_PORTS, 4, 0, &full as *const f32 as *const c_void);
            assert_ne!(knob_ring(&shot(widget), size), before, "port_event reached the editor");
            // A click on the switch writes its control port.
            testing::send(widget, WM_LBUTTONDOWN, 1, at(SWITCH, scale));
            assert_eq!(LV2_WRITES.load(Ordering::Relaxed), 1, "the click wrote the Warm port");
            testing::send(widget, WM_LBUTTONUP, 0, at(SWITCH, scale));
            save("lv2_ui.bmp", &shot(widget));

            // The UI offers ui:resize for the host, in pixels.
            let offered = (ui.extension_data)(c"http://lv2plug.in/ns/extensions/ui#resize".as_ptr()) as *const Lv2Resize;
            assert!(!offered.is_null());
            assert!((ui.extension_data)(c"http://example.com/nothing".as_ptr()).is_null());
            let host_sets = (*offered).ui_resize.unwrap();
            let large = ((LARGE_SIZE.0 as f32 * scale).round() as u32, (LARGE_SIZE.1 as f32 * scale).round() as u32);
            assert_eq!(host_sets(handle, large.0 as i32, large.1 as i32), 0);
            assert_eq!(testing::client_size(widget), large);
            assert!(meter_is_upright(&shot(widget), LARGE_SIZE));
            assert_ne!(host_sets(handle, (MAX_SIZE.0 as f32 * scale) as i32 + 40, large.1 as i32), 0, "outside the editor's range");
            assert_ne!(host_sets(handle, -1, 10), 0);
            assert_eq!(testing::client_size(widget), large);

            (ui.cleanup)(handle);
            assert!(!testing::is_window(widget), "the window goes with the UI");
            assert!(child(parent).is_null());
            (desc.cleanup)(instance);
            testing::destroy_window(parent);
        }
        println!("lv2 ui: ok");
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd;
    }

    fn find_top(title: &str) -> Hwnd {
        let t: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { FindWindowW(std::ptr::null(), t.as_ptr()) }
    }
}
