//! The system's own UI on Windows, as the HWND view opens it, with nothing
//! standing in: the popup menu on a parameter, the tooltip control, the open
//! dialog, and the window of a standalone editor.
//!
//! The view's thread is inside the menu's or the dialog's own message loop
//! while they are open, so a second thread drives: it posts the mouse and key
//! messages a user's input becomes, waits for the system's window to appear,
//! takes its picture and answers it.
//!
//! It needs a desktop that shows windows, and it opens a menu and a dialog on
//! it, so it runs only when `OP_SYSTEM_UI` is set (pictures go to
//! `OP_SHOT_DIR`). No harness, as the other editor tests.

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    if std::env::var_os("OP_SYSTEM_UI").is_none() {
        println!("win32_system_ui: skipped (set OP_SYSTEM_UI to open the system's menu and dialog on this desktop)");
        return;
    }
    win::run();
}

#[cfg(windows)]
mod win {
    use example_editor::*;
    use openplugin::core::editor::{DamageRect, FileDialog, Frame};
    use openplugin::core::{Editor, EditorHost};
    use openplugin::dri::win32::testing::{self, Bitmap};
    use openplugin::dri::win32::{set_process_dpi_aware, EditorView, Hwnd, TopWindow, PARAM_MENU_ITEMS};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const WM_COMMAND: u32 = 0x0111;
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_KEYUP: u32 = 0x0101;
    const WM_MOUSEMOVE: u32 = 0x0200;
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_RBUTTONDOWN: u32 = 0x0204;
    const VK_RETURN: usize = 0x0D;
    const VK_ESCAPE: usize = 0x1B;
    const VK_DOWN: usize = 0x28;

    /// An editor that asks for a file when it is clicked and tells what it was given.
    struct Asker {
        pixels: Vec<u32>,
        ask: bool,
        answers: Arc<Mutex<Vec<Option<Vec<PathBuf>>>>>,
    }

    impl Editor for Asker {
        fn size(&self) -> (u32, u32) {
            (200, 80)
        }
        fn param_changed(&mut self, _: usize, _: f32) {}
        fn render(&mut self, scale: f32) -> Frame<'_> {
            let (w, h) = ((200.0 * scale).round() as usize, (80.0 * scale).round() as usize);
            self.pixels.clear();
            self.pixels.resize(w * h, 0x0030_5080);
            Frame::new(&self.pixels, w, h)
        }
        fn damage(&mut self) -> Option<Vec<DamageRect>> {
            Some(Vec::new())
        }
        fn mouse_down(&mut self, _: f32, _: f32, _: u32) {
            self.ask = true;
        }
        fn take_file_dialog(&mut self) -> Option<FileDialog> {
            std::mem::take(&mut self.ask).then(|| FileDialog { tag: 7, title: "Choose an audio file".into(), extensions: vec!["wav".into(), "aiff".into()], multiple: false })
        }
        fn file_dialog_done(&mut self, tag: u32, paths: Option<Vec<PathBuf>>) {
            assert_eq!(tag, 7);
            self.answers.lock().unwrap().push(paths);
        }
    }

    fn save(name: &str, bitmap: &Bitmap) {
        if let Some(dir) = std::env::var_os("OP_SHOT_DIR").map(PathBuf::from) {
            std::fs::create_dir_all(&dir).expect("the screenshot folder");
            bitmap.save_bmp(&dir.join(name)).expect("write screenshot");
        }
    }

    /// Wait for `get` to give something, for at most `ms`.
    fn wait<T>(ms: u64, mut get: impl FnMut() -> Option<T>) -> Option<T> {
        let start = Instant::now();
        loop {
            if let Some(v) = get() {
                return Some(v);
            }
            if start.elapsed() > Duration::from_millis(ms) {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn colours(b: &Bitmap) -> usize {
        b.pixels.iter().map(|p| p & 0xff_ffff).collect::<std::collections::HashSet<u32>>().len()
    }

    fn window(h: usize) -> Hwnd {
        h as Hwnd
    }

    /// A visible top-level window of this process of a class, with a title if one is asked for.
    fn top_window(class: &str, title: Option<&str>) -> Option<usize> {
        unsafe { testing::windows(std::ptr::null_mut(), Some(class)) }
            .into_iter()
            .find(|w| unsafe { testing::is_visible(*w) && title.is_none_or(|t| testing::text(*w) == t) })
            .map(|w| w as usize)
    }

    /// What the second thread found, in order.
    #[derive(Default)]
    struct Report {
        lines: Vec<String>,
        failures: Vec<String>,
    }

    fn drive(view: usize, asker: usize, file: PathBuf, answers: Arc<Mutex<Vec<Option<Vec<PathBuf>>>>>) -> Report {
        let mut r = Report::default();
        unsafe {
            let (cw, _) = testing::client_size(window(view));
            let scale = cw as f32 / WIDTH as f32;
            let knob = testing::mouse_lparam(((KNOB.0 + KNOB.2 * 0.5) * scale) as i32, ((KNOB.1 + KNOB.3 * 0.5) * scale) as i32);
            r.lines.push(format!("the editor is {cw} pixels wide (scale {scale})"));

            // ---- The parameter menu: a real popup menu.
            testing::post(window(view), WM_RBUTTONDOWN, 2, knob);
            match wait(5000, || top_window("#32768", None)) {
                None => r.failures.push("no popup menu appeared after a right click on the knob".into()),
                Some(menu) => {
                    std::thread::sleep(Duration::from_millis(300));
                    match testing::capture_window(window(menu)) {
                        Some(b) => {
                            r.lines.push(format!("popup menu: {} x {} pixels, {} colours", b.width, b.height, colours(&b)));
                            if colours(&b) < 3 {
                                r.failures.push("the popup menu's picture is flat".into());
                            }
                            save("sysui_menu.bmp", &b);
                        }
                        None => r.failures.push("the popup menu could not be captured".into()),
                    }
                    // Down to the last item, "Type value", and Return: the keys go to the menu's loop.
                    for _ in 0..PARAM_MENU_ITEMS.len() {
                        testing::post(window(view), WM_KEYDOWN, VK_DOWN, 0);
                        testing::post(window(view), WM_KEYUP, VK_DOWN, 0);
                    }
                    testing::post(window(view), WM_KEYDOWN, VK_RETURN, 0);
                    let field = wait(5000, || testing::windows(window(view), Some("EDIT")).first().map(|w| *w as usize));
                    match field {
                        None => r.failures.push("choosing \"Type value\" in the menu opened no text field".into()),
                        Some(field) => {
                            r.lines.push(format!("\"Type value\" chosen in the menu opened a field holding {:?}", testing::text(window(field))));
                            if wait(3000, || top_window("#32768", None).is_none().then_some(())).is_none() {
                                r.failures.push("the menu stayed open".into());
                            }
                            if let Some(b) = testing::capture(window(view)) {
                                save("sysui_type_value.bmp", &b);
                            }
                            testing::post(window(field), WM_KEYDOWN, VK_ESCAPE, 0);
                            if wait(3000, || testing::windows(window(view), Some("EDIT")).is_empty().then_some(())).is_none() {
                                r.failures.push("Escape did not close the field".into());
                            }
                        }
                    }
                }
            }

            // ---- A tooltip: the pointer rests on the knob.
            let (ox, oy) = testing::client_origin(window(view));
            let (kx, ky) = (((KNOB.0 + KNOB.2 * 0.5) * scale) as i32, ((KNOB.1 + KNOB.3 * 0.5) * scale) as i32);
            let moved = testing::set_pointer(ox + kx, oy + ky);
            r.lines.push(format!("the real pointer {} to the knob", if moved { "moved" } else { "could not be moved" }));
            testing::post(window(view), WM_MOUSEMOVE, 0, knob);
            let tips = testing::windows(std::ptr::null_mut(), Some("tooltips_class32"));
            r.lines.push(format!("tooltip controls made: {}", tips.len()));
            if tips.is_empty() {
                r.failures.push("the view made no tooltip control".into());
            }
            match wait(4000, || tips.iter().find(|t| testing::is_visible(**t)).map(|t| *t as usize)) {
                Some(tip) => {
                    let text = testing::text(window(tip));
                    r.lines.push(format!("a tooltip showed: {text:?}"));
                    if let Some(b) = testing::capture_window(window(tip)) {
                        save("sysui_tooltip.bmp", &b);
                    }
                }
                // The control shows a tip only where the real pointer is; a locked or
                // remote desktop has none to move there.
                None => r.lines.push("no tooltip showed (the real pointer is not over the control)".into()),
            }

            // ---- The open dialog: the system's own.
            let click = testing::mouse_lparam(20, 20);
            testing::post(window(asker), WM_LBUTTONDOWN, 1, click);
            testing::post(window(asker), WM_LBUTTONUP, 0, click);
            match wait(15000, || top_window("#32770", Some("Choose an audio file"))) {
                None => r.failures.push("no open dialog appeared".into()),
                Some(dialog) => {
                    std::thread::sleep(Duration::from_millis(1500));
                    match testing::capture_window(window(dialog)) {
                        Some(b) => {
                            r.lines.push(format!("open dialog {:?}: {} x {} pixels, {} colours", testing::text(window(dialog)), b.width, b.height, colours(&b)));
                            save("sysui_open_dialog.bmp", &b);
                        }
                        None => r.failures.push("the open dialog could not be captured".into()),
                    }
                    // Type the file's path in the name field and press Open.
                    let fields: Vec<Hwnd> = testing::descendants(window(dialog)).into_iter().filter(|w| testing::class_name(*w).eq_ignore_ascii_case("Edit")).collect();
                    r.lines.push(format!("the dialog has {} text field(s)", fields.len()));
                    match fields.first() {
                        Some(field) => {
                            testing::set_text(*field, &file.to_string_lossy());
                            testing::post(window(dialog), WM_COMMAND, 1, 0); // IDOK
                        }
                        None => {
                            testing::post(window(dialog), WM_COMMAND, 2, 0); // IDCANCEL
                        }
                    }
                    match wait(10000, || answers.lock().unwrap().first().cloned()) {
                        Some(Some(paths)) => {
                            r.lines.push(format!("the editor was given {paths:?}"));
                            if paths != vec![file.clone()] {
                                r.failures.push(format!("the dialog answered {paths:?}, not {file:?}"));
                            }
                        }
                        Some(None) => r.failures.push("the dialog was cancelled instead of answered".into()),
                        None => {
                            r.failures.push("the editor heard no answer from the dialog".into());
                            testing::post(window(dialog), WM_COMMAND, 2, 0);
                        }
                    }
                }
            }
        }
        r
    }

    pub fn run() {
        set_process_dpi_aware();
        // The file the dialog is answered with.
        let file = std::env::temp_dir().join("openplugin system ui test.wav");
        std::fs::write(&file, b"RIFF").expect("a file to choose");

        let view = EditorView::new(make_editor(EditorHost::detached()).expect("an editor"), None).expect("a view");
        let top = TopWindow::new("OpenPlugin system UI", &view, None).expect("a window");
        top.set_visible(true);
        unsafe { testing::move_to_front(top.raw(), 120, 120) };

        let answers: Arc<Mutex<Vec<Option<Vec<PathBuf>>>>> = Arc::default();
        let asker = EditorView::new(Box::new(Asker { pixels: Vec::new(), ask: false, answers: answers.clone() }), None).expect("a view");
        let asker_top = TopWindow::new("OpenPlugin file request", &asker, None).expect("a window");
        asker_top.set_visible(true);
        unsafe { testing::move_to_front(asker_top.raw(), 700, 120) };
        assert!(testing::pump_until(2000, || unsafe { testing::is_visible(view.raw()) && testing::is_visible(asker.raw()) }), "the windows show on this desktop");
        if let Some(b) = unsafe { testing::capture_window(top.raw()) } {
            save("sysui_window.bmp", &b);
        }

        // The tooltip control holds one tool per tooltip and gets each text from the view.
        let asked = view.tooltip_control_texts();
        let own: Vec<String> = view.tooltip_rects().into_iter().map(|(_, text)| text).collect();
        println!("  the tooltip control's tools: {asked:?}");
        assert!(!asked.is_empty() && asked == own, "the control holds {asked:?}, the editor has {own:?}");

        let done = Arc::new(AtomicBool::new(false));
        let (view_hwnd, asker_hwnd) = (view.raw() as usize, asker.raw() as usize);
        let driver = {
            let (done, file, answers) = (done.clone(), file.clone(), answers.clone());
            std::thread::spawn(move || {
                let report = drive(view_hwnd, asker_hwnd, file, answers);
                done.store(true, Ordering::SeqCst);
                report
            })
        };
        let finished = testing::pump_until(120_000, || done.load(Ordering::SeqCst));
        assert!(finished, "the driver finished");
        let report = driver.join().expect("the driver thread");
        for line in &report.lines {
            println!("  {line}");
        }
        let _ = std::fs::remove_file(&file);
        drop(view);
        drop(asker);
        assert!(report.failures.is_empty(), "{:#?}", report.failures);
        println!("win32_system_ui: ok");
    }
}
