//! The editor as a screen reader reads it on Windows: the UI Automation tree
//! of the HWND view, walked with the system's own client (`CUIAutomation`,
//! COM declared by hand), as `a11y.rs` reads the macOS tree.
//!
//! The view sits in a window on the main thread, which pumps messages and has
//! a COM apartment, as a host's window thread has. A second thread is the
//! screen reader: it finds the view's element from its window, reads the
//! children (control type, name, value, range, toggle state, place on the
//! screen), sets a value, toggles the switch and moves focus, and checks the
//! gestures that reach a recording `EditorHost`; then a value changes from the
//! host's side, the editor is resized, and the view goes while an element is
//! still held. No harness, as the other editor tests.

#[cfg(not(windows))]
fn main() {
    println!("editor a11y (win32): skipped: Windows only (0 tests run)");
}

#[cfg(windows)]
fn main() {
    win::editor_tree();
}

#[cfg(windows)]
mod win {
    use example_editor::{layout, GainEditor, HEIGHT, WIDTH};
    use openplugin::core::editor::{Key, Modifiers};
    use openplugin::core::{EditorHost, Gesture};
    use openplugin::dri::win32::testing;
    use openplugin::dri::win32::{EditorView, Hwnd, TopWindow};
    use std::ffi::c_void;
    use std::ptr::null_mut;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex};

    type Com = *mut c_void;
    type Hresult = i32;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid(u32, u16, u16, [u8; 8]);

    const CLSID_UIAUTOMATION: Guid = Guid(0xff48_dba4, 0x60ef, 0x4201, [0xaa, 0x87, 0x54, 0x10, 0x3e, 0xef, 0x59, 0x4e]);
    const IID_UIAUTOMATION: Guid = Guid(0x30cb_e57d, 0xd9d0, 0x452a, [0xab, 0x13, 0x7a, 0xc5, 0xac, 0x48, 0x25, 0xee]);
    const IID_VALUE_PATTERN: Guid = Guid(0xa94c_d8b1, 0x0844, 0x4cd6, [0x9d, 0x2d, 0x64, 0x05, 0x37, 0xab, 0x39, 0xe9]);
    const IID_RANGE_PATTERN: Guid = Guid(0x5921_3f4f, 0x7346, 0x49e5, [0xb1, 0x20, 0x80, 0x55, 0x59, 0x87, 0xa1, 0x48]);
    const IID_TOGGLE_PATTERN: Guid = Guid(0x94cf_8058, 0x9b8d, 0x4ab9, [0x8b, 0xfd, 0x4c, 0xd0, 0xa3, 0x3c, 0x8c, 0x70]);
    const IID_INVOKE_PATTERN: Guid = Guid(0xfb37_7fbe, 0x8ea6, 0x46d5, [0x9c, 0x73, 0x64, 0x99, 0x64, 0x2d, 0x30, 0x59]);

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Variant {
        vt: u16,
        reserved: [u16; 3],
        data: [usize; 2],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, model: u32) -> Hresult;
        fn CoCreateInstance(clsid: *const Guid, outer: Com, context: u32, iid: *const Guid, out: *mut Com) -> Hresult;
    }

    #[link(name = "oleaut32")]
    unsafe extern "system" {
        fn SysStringLen(text: *const u16) -> u32;
        fn VariantClear(v: *mut Variant) -> Hresult;
        fn SafeArrayAccessData(array: *mut c_void, data: *mut *mut c_void) -> Hresult;
        fn SafeArrayUnaccessData(array: *mut c_void) -> Hresult;
    }

    unsafe fn slot(obj: Com, index: usize) -> *const c_void {
        unsafe { *(*(obj as *const *const *const c_void)).add(index) }
    }

    /// `com!(object, vtable index, fn(argument types) -> return type, arguments)`.
    macro_rules! com {
        ($obj:expr, $index:expr, fn($($t:ty),*) -> $r:ty $(, $a:expr)*) => {{
            let obj: Com = $obj;
            let f: unsafe extern "system" fn(Com $(, $t)*) -> $r = std::mem::transmute(slot(obj, $index));
            f(obj $(, $a)*)
        }};
    }

    unsafe fn release(obj: Com) {
        if !obj.is_null() {
            unsafe { com!(obj, 2, fn() -> u32) };
        }
    }

    // Property, pattern and control type ids (`UIAutomationClient.h`).
    const PROP_BOUNDS: i32 = 30001;
    const PROP_CONTROL_TYPE: i32 = 30003;
    const PROP_NAME: i32 = 30005;
    const PROP_HAS_FOCUS: i32 = 30008;
    const PROP_IS_ENABLED: i32 = 30010;
    const PROP_VALUE: i32 = 30045;
    const PATTERN_INVOKE: i32 = 10000;
    const PATTERN_VALUE: i32 = 10002;
    const PATTERN_RANGE: i32 = 10003;
    const PATTERN_TOGGLE: i32 = 10015;
    const TYPE_CHECK_BOX: i32 = 50002;
    const TYPE_SLIDER: i32 = 50015;
    const TYPE_GROUP: i32 = 50026;

    /// `IUIAutomationElement::GetCurrentPropertyValue`.
    unsafe fn property(element: Com, id: i32) -> Result<Variant, Hresult> {
        unsafe {
            let mut v = Variant { vt: 0, reserved: [0; 3], data: [0; 2] };
            let hr = com!(element, 10, fn(i32, *mut Variant) -> Hresult, id, &mut v);
            if hr < 0 { Err(hr) } else { Ok(v) }
        }
    }

    unsafe fn text(element: Com, id: i32) -> String {
        unsafe {
            let Ok(mut v) = property(element, id) else { return "<gone>".into() };
            let out = if v.vt == 8 && v.data[0] != 0 {
                let p = v.data[0] as *const u16;
                String::from_utf16_lossy(std::slice::from_raw_parts(p, SysStringLen(p) as usize))
            } else {
                String::new()
            };
            VariantClear(&mut v);
            out
        }
    }

    unsafe fn int(element: Com, id: i32) -> i32 {
        unsafe {
            match property(element, id) {
                Ok(v) if v.vt == 3 => *(v.data.as_ptr() as *const i32),
                _ => -1,
            }
        }
    }

    unsafe fn flag(element: Com, id: i32) -> bool {
        unsafe { matches!(property(element, id), Ok(v) if v.vt == 11 && *(v.data.as_ptr() as *const i16) != 0) }
    }

    /// The element's rectangle on the screen: left, top, width, height.
    unsafe fn bounds(element: Com) -> [f64; 4] {
        unsafe {
            let Ok(mut v) = property(element, PROP_BOUNDS) else { return [0.0; 4] };
            let mut out = [0.0; 4];
            // VT_ARRAY | VT_R8
            if v.vt == 0x2005 && v.data[0] != 0 {
                let mut data = null_mut();
                if SafeArrayAccessData(v.data[0] as *mut c_void, &mut data) >= 0 {
                    out.copy_from_slice(std::slice::from_raw_parts(data as *const f64, 4));
                    SafeArrayUnaccessData(v.data[0] as *mut c_void);
                }
            }
            VariantClear(&mut v);
            out
        }
    }

    /// `IUIAutomationElement::GetCurrentPatternAs`; null when the element has no such pattern.
    unsafe fn pattern(element: Com, id: i32, iid: &Guid) -> Com {
        unsafe {
            let mut out = null_mut();
            com!(element, 14, fn(i32, *const Guid, *mut Com) -> Hresult, id, iid, &mut out);
            out
        }
    }

    /// The element's children in the tree.
    unsafe fn children(automation: Com, element: Com) -> Vec<Com> {
        unsafe {
            let mut all = null_mut();
            assert!(com!(automation, 21, fn(*mut Com) -> Hresult, &mut all) >= 0, "a condition");
            let mut array = null_mut();
            // TreeScope_Children
            let hr = com!(element, 6, fn(i32, Com, *mut Com) -> Hresult, 2, all, &mut array);
            release(all);
            assert!(hr >= 0 && !array.is_null(), "FindAll: {hr:#x}");
            let mut count = 0i32;
            com!(array, 3, fn(*mut i32) -> Hresult, &mut count);
            let out = (0..count)
                .map(|i| {
                    let mut child = null_mut();
                    com!(array, 4, fn(i32, *mut Com) -> Hresult, i, &mut child);
                    child
                })
                .collect();
            release(array);
            out
        }
    }

    /// The numbers of a range value pattern: value, minimum, maximum, small change.
    unsafe fn range(element: Com) -> Option<[f64; 4]> {
        unsafe {
            let p = pattern(element, PATTERN_RANGE, &IID_RANGE_PATTERN);
            if p.is_null() {
                return None;
            }
            let mut out = [0.0f64; 4];
            // get_CurrentValue 4, get_CurrentMinimum 7, get_CurrentMaximum 6, get_CurrentSmallChange 9.
            for (i, index) in [4usize, 7, 6, 9].into_iter().enumerate() {
                com!(p, index, fn(*mut f64) -> Hresult, &mut out[i]);
            }
            release(p);
            Some(out)
        }
    }

    unsafe fn set_range(element: Com, value: f64) -> Hresult {
        unsafe {
            let p = pattern(element, PATTERN_RANGE, &IID_RANGE_PATTERN);
            if p.is_null() {
                return -1;
            }
            let hr = com!(p, 3, fn(f64) -> Hresult, value);
            release(p);
            hr
        }
    }

    /// The toggle state (0 off, 1 on), or `None` without the pattern.
    unsafe fn toggle_state(element: Com) -> Option<i32> {
        unsafe {
            let p = pattern(element, PATTERN_TOGGLE, &IID_TOGGLE_PATTERN);
            if p.is_null() {
                return None;
            }
            let mut state = -1;
            com!(p, 4, fn(*mut i32) -> Hresult, &mut state);
            release(p);
            Some(state)
        }
    }

    unsafe fn toggle(element: Com) -> Hresult {
        unsafe {
            let p = pattern(element, PATTERN_TOGGLE, &IID_TOGGLE_PATTERN);
            if p.is_null() {
                return -1;
            }
            let hr = com!(p, 3, fn() -> Hresult);
            release(p);
            hr
        }
    }

    fn near(a: [f64; 4], b: [f64; 4]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= 2.0)
    }

    /// What the reader's thread asks the view's thread to do.
    enum Ask {
        Param(usize, f32),
        ShiftTab,
        Resize(u32, u32),
        Place,
        DropView,
    }

    /// The view's side of the conversation: asks in, the view's place on the screen and its scale out.
    struct Link {
        ask: Sender<Ask>,
        answer: Receiver<(i32, i32, f32)>,
    }

    impl Link {
        fn ask(&self, what: Ask) -> (i32, i32, f32) {
            self.ask.send(what).expect("the view's thread");
            self.answer.recv().expect("the view's thread answers")
        }
    }

    type Gestures = Arc<Mutex<Vec<String>>>;

    fn take(gestures: &Gestures) -> Vec<String> {
        std::mem::take(&mut *gestures.lock().unwrap())
    }

    /// The screen reader: everything here is on its own thread.
    fn read(view: usize, link: Link, gestures: Gestures) {
        unsafe {
            // COINIT_MULTITHREADED
            assert!(CoInitializeEx(null_mut(), 0) >= 0);
            let mut automation = null_mut();
            // CLSCTX_INPROC_SERVER
            let hr = CoCreateInstance(&CLSID_UIAUTOMATION, null_mut(), 1, &IID_UIAUTOMATION, &mut automation);
            assert!(hr >= 0 && !automation.is_null(), "UI Automation is available: {hr:#x}");
            let mut root = null_mut();
            let hr = com!(automation, 6, fn(Hwnd, *mut Com) -> Hresult, view as Hwnd, &mut root);
            assert!(hr >= 0 && !root.is_null(), "the view's window has an element: {hr:#x}");

            // The view is one group; its children are the two controls.
            assert_eq!(int(root, PROP_CONTROL_TYPE), TYPE_GROUP);
            let kids = children(automation, root);
            assert_eq!(kids.len(), 2, "two controls");
            let (gain, warm) = (kids[0], kids[1]);

            // Name, value and control type.
            assert_eq!((int(gain, PROP_CONTROL_TYPE), text(gain, PROP_NAME).as_str(), text(gain, PROP_VALUE).as_str()), (TYPE_SLIDER, "Gain", "0.0 dB"));
            assert!(flag(gain, PROP_IS_ENABLED) && !flag(gain, PROP_HAS_FOCUS));
            assert_eq!((int(warm, PROP_CONTROL_TYPE), text(warm, PROP_NAME).as_str(), text(warm, PROP_VALUE).as_str()), (TYPE_CHECK_BOX, "Warm", "Off"));
            assert_eq!(toggle_state(warm), Some(0));

            // The patterns follow the kind of control.
            let [value, min, max, step] = range(gain).expect("a knob has a range");
            assert_eq!((value, min, max), (0.0, -24.0, 24.0));
            assert!((step - 2.4).abs() < 1e-6, "5% of 48 dB: {step}");
            assert!(range(warm).is_none() && toggle_state(gain).is_none(), "a switch has no range, a knob no toggle");
            for element in [gain, warm] {
                let p = pattern(element, PATTERN_INVOKE, &IID_INVOKE_PATTERN);
                assert!(p.is_null(), "neither is a button");
                let p = pattern(element, PATTERN_VALUE, &IID_VALUE_PATTERN);
                assert!(!p.is_null(), "both have a value as text");
                let mut read_only = 0i32;
                com!(p, 5, fn(*mut i32) -> Hresult, &mut read_only);
                assert!(read_only != 0, "the text is read, not written");
                release(p);
            }

            // Places are screen rectangles in pixels: the view's place plus the editor's at its scale.
            let (ox, oy, scale) = link.ask(Ask::Place);
            let expect = |r: (f32, f32, f32, f32)| {
                [f64::from(ox) + f64::from(r.0 * scale), f64::from(oy) + f64::from(r.1 * scale), f64::from(r.2 * scale), f64::from(r.3 * scale)]
            };
            let lay = layout(WIDTH, HEIGHT);
            assert!(near(bounds(gain), expect(lay.knob)), "{:?} vs {:?}", bounds(gain), expect(lay.knob));
            assert!(near(bounds(warm), expect(lay.switch)), "{:?} vs {:?}", bounds(warm), expect(lay.switch));
            // Pointing at a control finds its element, where this desktop can be pointed at.
            let b = bounds(warm);
            let mut hit = null_mut();
            let at = Point { x: (b[0] + b[2] * 0.5) as i32, y: (b[1] + b[3] * 0.5) as i32 };
            if com!(automation, 7, fn(Point, *mut Com) -> Hresult, at, &mut hit) >= 0 && !hit.is_null() && text(hit, PROP_NAME) == "Warm" {
                println!("  pointing at the switch finds it");
            } else {
                println!("  pointing was not checked (another window is in front, or the desktop is locked)");
            }
            release(hit);

            // A value set by the reader is one whole gesture.
            take(&gestures);
            assert!(set_range(gain, 2.4) >= 0);
            assert_eq!(take(&gestures), ["begin 0", "perform 0 2.400", "end 0"]);
            assert_eq!(text(gain, PROP_VALUE), "2.4 dB");
            assert_eq!(range(gain).unwrap()[0], f64::from(2.4f32));
            assert!(set_range(gain, 0.0) >= 0);
            assert_eq!(take(&gestures), ["begin 0", "perform 0 0.000", "end 0"]);
            assert_eq!(text(gain, PROP_VALUE), "0.0 dB");
            // Outside the range: refused, and nothing reaches the host.
            assert!(set_range(gain, 100.0) < 0);
            assert!(take(&gestures).is_empty());

            // Toggle flips the switch like a click: begin, perform, end.
            assert!(toggle(warm) >= 0);
            assert_eq!(take(&gestures), ["begin 1", "perform 1 1.000", "end 1"]);
            assert_eq!((toggle_state(warm), text(warm, PROP_VALUE).as_str()), (Some(1), "On"));
            assert!(toggle(warm) >= 0);
            assert_eq!(take(&gestures), ["begin 1", "perform 1 0.000", "end 1"]);
            assert_eq!(toggle_state(warm), Some(0));

            // Automation: a value from the host reaches the element, with no gesture.
            link.ask(Ask::Param(0, -12.0));
            assert_eq!(text(gain, PROP_VALUE), "-12.0 dB");
            assert_eq!(range(gain).unwrap()[0], -12.0);
            link.ask(Ask::Param(1, 1.0));
            assert_eq!((toggle_state(warm), text(warm, PROP_VALUE).as_str()), (Some(1), "On"));
            assert!(take(&gestures).is_empty());

            // At the end of the range, setting the end again does nothing.
            link.ask(Ask::Param(0, 24.0));
            assert!(set_range(gain, 24.0) >= 0);
            assert!(take(&gestures).is_empty());

            // The reader's cursor takes the keyboard to the switch, and the tree says so.
            assert!(com!(warm, 3, fn() -> Hresult) >= 0, "SetFocus");
            assert!(flag(warm, PROP_HAS_FOCUS) && !flag(gain, PROP_HAS_FOCUS));
            // Keyboard focus moves the other way too: Shift-Tab goes back to the knob.
            link.ask(Ask::ShiftTab);
            assert!(flag(gain, PROP_HAS_FOCUS) && !flag(warm, PROP_HAS_FOCUS));

            // Resize: the elements move with the editor.
            let (ox, oy, scale) = link.ask(Ask::Resize(360, 240));
            let expect = |r: (f32, f32, f32, f32)| {
                [f64::from(ox) + f64::from(r.0 * scale), f64::from(oy) + f64::from(r.1 * scale), f64::from(r.2 * scale), f64::from(r.3 * scale)]
            };
            let lay2 = layout(360, 240);
            assert!(near(bounds(gain), expect(lay2.knob)), "{:?} vs {:?}", bounds(gain), expect(lay2.knob));
            assert!(near(bounds(warm), expect(lay2.switch)));

            // Elements the reader still holds go quiet when the view is gone.
            link.ask(Ask::DropView);
            take(&gestures);
            assert_ne!(text(gain, PROP_NAME), "Gain", "the element is gone");
            assert!(set_range(gain, 1.0) < 0);
            assert!(toggle(warm) < 0);
            assert!(take(&gestures).is_empty());

            for kid in kids {
                release(kid);
            }
            release(root);
            release(automation);
        }
    }

    fn place(view: &EditorView) -> (i32, i32, f32) {
        let (x, y) = unsafe { testing::client_origin(view.raw()) };
        (x, y, view.scale())
    }

    pub fn editor_tree() {
        if testing::parent_window("probe", 0, 0, 8, 8).is_null() {
            println!("editor a11y (win32): skipped: no desktop to make windows on");
            return;
        }
        unsafe {
            // COINIT_APARTMENTTHREADED: the window thread of a host has an apartment.
            assert!(CoInitializeEx(null_mut(), 2) >= 0);
        }
        let gestures: Gestures = Arc::default();
        let sink = gestures.clone();
        let host = EditorHost::new(move |g| {
            sink.lock().unwrap().push(match g {
                Gesture::Begin(i) => format!("begin {i}"),
                Gesture::Perform(i, v) => format!("perform {i} {v:.3}"),
                Gesture::End(i) => format!("end {i}"),
            })
        });
        let view = EditorView::new(Box::new(GainEditor::new(host)), None).expect("a view");
        let top = TopWindow::new("OpenPlugin a11y", &view, None).expect("a window");
        top.set_visible(true);
        unsafe { testing::move_to_front(top.raw(), 200, 200) };
        testing::pump_until(200, || false);

        let (ask_tx, ask_rx) = channel::<Ask>();
        let (answer_tx, answer_rx) = channel();
        let hwnd = view.raw() as usize;
        let reader = {
            let gestures = gestures.clone();
            std::thread::spawn(move || read(hwnd, Link { ask: ask_tx, answer: answer_rx }, gestures))
        };
        let mut view = Some(view);
        let finished = testing::pump_until(120_000, || {
            while let Ok(ask) = ask_rx.try_recv() {
                let mut at = (0, 0, 0.0);
                match ask {
                    Ask::Param(index, value) => view.as_mut().unwrap().editor().unwrap().param_changed(index, value),
                    Ask::ShiftTab => {
                        let shift = Modifiers { shift: true, ..Modifiers::NONE };
                        assert!(view.as_mut().unwrap().editor().unwrap().key_down(Key::Tab, shift));
                    }
                    Ask::Resize(w, h) => assert_eq!(view.as_ref().unwrap().resize(w, h), (w, h)),
                    Ask::Place => {}
                    Ask::DropView => drop(view.take()),
                }
                if let Some(v) = view.as_ref() {
                    at = place(v);
                }
                let _ = answer_tx.send(at);
            }
            reader.is_finished()
        });
        assert!(finished, "the reader finished");
        if let Err(panic) = reader.join() {
            std::panic::resume_unwind(panic);
        }
        drop(top);
        println!("a11y (win32 editor): 1 passed");
    }
}
