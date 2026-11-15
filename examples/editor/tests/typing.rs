//! Typing a value, tooltips and files, on the main thread (no harness, so
//! `main` is the process's main thread, as AppKit needs). Nothing is shown to
//! the user: the editor sits in a borderless window that is never ordered in.
//!
//! - Typing: a double-click event on the knob (and a Return key event on the
//!   focused knob) opens the native text field over it with the formatted
//!   value; Return, heard as the field's delegate hears it, commits through
//!   `ParamInfo::parse` as begin, perform, end at a recording `EditorHost`;
//!   Escape and focus loss cancel; invalid text keeps the field open; keys
//!   typed while it is open reach neither the editor nor the host;
//!   Option-double-click resets.
//! - Tooltips: one tooltip rectangle per control, at the control's place, with
//!   the parameter's name and value, moving with the layout.
//! - Files: a simulated drag (the view's drag destination methods called with a
//!   pasteboard that holds file URLs) reaches the editor with a hit position
//!   and the paths; a file dialog request is answered through the test hook
//!   and through the real completion block of an `NSOpenPanel` that is never shown.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod a11y_support;

#[cfg(target_os = "macos")]
fn main() {
    mac::typing_a_value();
    mac::tooltips_follow_layout_and_values();
    mac::dropped_files_reach_the_editor();
    mac::file_dialog_requests();
    println!("typing (editor): 4 passed");
}

#[cfg(target_os = "macos")]
mod mac {
    use super::a11y_support::*;
    use example_editor::{layout, make_editor, HEIGHT, WIDTH};
    use openplugin::core::editor::FileDialog;
    use openplugin::dri::cocoa::Id;
    use openplugin::core::{Editor, EditorHost, Frame};
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    const ALT: usize = 1 << 19;

    fn centre(r: (f32, f32, f32, f32)) -> (f32, f32) {
        (r.0 + r.2 * 0.5, r.1 + r.3 * 0.5)
    }

    pub fn typing_a_value() {
        unsafe {
            let mut rig = Rig::new(make_editor);
            let lay = layout(WIDTH, HEIGHT);
            let (kx, ky) = centre(lay.knob);
            let kids = children(rig.raw());
            let gain = kids[0];

            // A single click is a begin and an end, and no field.
            rig.click(kx, ky, 1, 0);
            assert_eq!(rig.gestures(), ["begin 0", "end 0"]);
            assert!(!rig.has_field());

            // A double-click opens the field, prefilled with ParamInfo::format.
            rig.click(kx, ky, 2, 0);
            run_loop(0.05);
            assert!(rig.has_field());
            assert_eq!(rig.field_text(), "0.0 dB");
            assert!(rig.gestures().iter().all(|g| g != "perform 0 0.000"), "opening sends no value");
            rig.gestures();

            // Typing and Return commits: one whole gesture, through parse.
            rig.set_field_text(c"-6 dB");
            assert!(rig.field_command(c"insertNewline:"));
            assert!(!rig.has_field());
            assert_eq!(rig.gestures(), ["begin 0", "perform 0 -6.000", "end 0"]);
            assert_eq!(seen(gain).value, "-6.0 dB");

            // Escape cancels: nothing is sent, the value stays.
            rig.click(kx, ky, 2, 0);
            assert!(rig.has_field());
            assert_eq!(rig.field_text(), "-6.0 dB");
            rig.gestures();
            rig.set_field_text(c"12");
            assert!(rig.field_command(c"cancelOperation:"));
            assert!(!rig.has_field() && rig.gestures().is_empty());
            assert_eq!(seen(gain).value, "-6.0 dB");

            // So does losing focus.
            rig.click(kx, ky, 2, 0);
            rig.gestures();
            assert!(rig.has_field());
            rig.set_field_text(c"12");
            rig.field_end_editing();
            assert!(!rig.has_field() && rig.gestures().is_empty());

            // Invalid text is refused: the field stays open and nothing is sent.
            rig.click(kx, ky, 2, 0);
            rig.gestures();
            rig.set_field_text(c"banana");
            assert!(rig.field_command(c"insertNewline:"));
            assert!(rig.has_field() && rig.gestures().is_empty());
            assert_eq!(rig.field_text(), "banana");
            assert_eq!(seen(gain).value, "-6.0 dB");

            // Keys typed into the open field reach neither the editor nor the host:
            // Up (126) would raise the focused knob, and Backspace (51) would reset it.
            rig.key(126, c"\u{f700}", 0);
            rig.key(51, c"\u{7f}", 0);
            assert!(rig.gestures().is_empty() && rig.has_field());
            assert_eq!(seen(gain).value, "-6.0 dB");
            // A correction and Return commit (units are optional, the value is clamped).
            rig.set_field_text(c"99");
            assert!(rig.field_command(c"insertNewline:"));
            assert_eq!(rig.gestures(), ["begin 0", "perform 0 24.000", "end 0"]);

            // With the field closed the same keys act again: the knob has focus.
            rig.key(51, c"\u{7f}", 0);
            assert_eq!(rig.gestures(), ["begin 0", "perform 0 0.000", "end 0"]);

            // Return on the focused knob opens the field too.
            rig.key(36, c"\r", 0);
            assert!(rig.has_field());
            assert_eq!(rig.field_text(), "0.0 dB");
            // A click elsewhere on the view abandons it.
            rig.click(5.0, 5.0, 1, 0);
            assert!(!rig.has_field());
            rig.gestures();

            // Option-double-click resets instead of typing.
            rig.editor().param_changed(0, 9.0);
            rig.click(kx, ky, 2, ALT);
            assert!(!rig.has_field());
            let g = rig.gestures();
            assert!(g.contains(&"perform 0 0.000".to_string()), "{g:?}");

            // A resize closes an open field (the control moved under it).
            rig.click(kx, ky, 2, 0);
            assert!(rig.has_field());
            rig.view.as_ref().unwrap().resize(360, 240);
            run_loop(0.1);
            assert!(!rig.has_field());
        }
    }

    pub fn tooltips_follow_layout_and_values() {
        unsafe {
            let mut rig = Rig::new(make_editor);
            let tips = |rig: &Rig| rig.view.as_ref().unwrap().tooltip_rects();
            let want = |l: &example_editor::Layout| [l.knob, l.switch, l.meter];
            let lay = layout(WIDTH, HEIGHT);
            let t = tips(&rig);
            assert_eq!(t.len(), 3);
            for ((r, _), w) in t.iter().zip(want(&lay)) {
                let got = (r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32);
                assert_eq!(got, w);
            }
            assert_eq!(t[0].1, "Gain: 0.0 dB");
            assert_eq!(t[1].1, "Warm: Off");
            assert!(t[2].1.contains("Drop an audio file"), "{}", t[2].1);

            // The text is read when a tip shows, so a new value is current.
            rig.editor().param_changed(0, -12.0);
            rig.editor().param_changed(1, 1.0);
            let t = tips(&rig);
            assert_eq!((t[0].1.as_str(), t[1].1.as_str()), ("Gain: -12.0 dB", "Warm: On"));

            // A resize moves the rectangles with the controls.
            rig.view.as_ref().unwrap().resize(360, 240);
            run_loop(0.1);
            let lay2 = layout(360, 240);
            let t = tips(&rig);
            assert_eq!(t.len(), 3);
            for ((r, _), w) in t.iter().zip(want(&lay2)) {
                let got = (r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32);
                assert!((got.0 - w.0).abs() < 1e-3 && (got.1 - w.1).abs() < 1e-3 && (got.2 - w.2).abs() < 1e-3 && (got.3 - w.3).abs() < 1e-3, "{got:?} vs {w:?}");
            }
        }
    }

    /// A recording editor for the file hooks.
    #[derive(Default)]
    struct Log {
        accepted_at: Vec<(f32, f32)>,
        dropped: Vec<(Vec<PathBuf>, f32, f32)>,
        answers: Vec<(u32, Option<Vec<PathBuf>>)>,
        ask: Option<FileDialog>,
    }

    struct Files(Rc<RefCell<Log>>, Vec<u32>);

    impl Editor for Files {
        fn size(&self) -> (u32, u32) {
            (200, 100)
        }
        fn param_changed(&mut self, _: usize, _: f32) {}
        fn render(&mut self, _: f32) -> Frame<'_> {
            Frame { pixels: &[0], width: 1, height: 1 }
        }
        fn wants_drops(&self) -> bool {
            true
        }
        fn accepts_drop(&mut self, x: f32, y: f32) -> bool {
            self.0.borrow_mut().accepted_at.push((x, y));
            x < 100.0
        }
        fn files_dropped(&mut self, paths: &[PathBuf], x: f32, y: f32) -> bool {
            self.0.borrow_mut().dropped.push((paths.to_vec(), x, y));
            true
        }
        fn take_file_dialog(&mut self) -> Option<FileDialog> {
            self.0.borrow_mut().ask.take()
        }
        fn file_dialog_done(&mut self, tag: u32, paths: Option<Vec<PathBuf>>) {
            self.1.push(tag);
            self.0.borrow_mut().answers.push((tag, paths));
        }
    }

    fn files_rig() -> (Rig, Rc<RefCell<Log>>) {
        let log = Rc::new(RefCell::new(Log::default()));
        let l = log.clone();
        let rig = unsafe { Rig::new(move |_: EditorHost| Some(Box::new(Files(l, Vec::new())) as Box<dyn Editor>)) };
        (rig, log)
    }

    pub fn dropped_files_reach_the_editor() {
        unsafe {
            // The example editor takes drops anywhere and shows the name in the meter's tip.
            let rig = Rig::new(make_editor);
            let info = dragging_info(&[c"/Volumes/Samples/kick drum.wav"], CGPoint { x: 100.0, y: 60.0 });
            assert_eq!(drag_op(rig.raw(), c"draggingEntered:", info), 1);
            assert!(drag_bool(rig.raw(), c"performDragOperation:", info));
            let tip = rig.view.as_ref().unwrap().tooltip_rects()[2].1.clone();
            assert_eq!(tip, "Output level. Last file: kick drum.wav");

            // A recording editor: the hit position is in editor points, top left origin.
            let (rig, log) = files_rig();
            let h = rig.view_height();
            // Window point (30, 20 from the bottom) is editor y = height - 20.
            let near = dragging_info(&[c"/a/one.wav", c"/a/two.wav"], CGPoint { x: 30.0, y: 20.0 });
            assert_eq!(drag_op(rig.raw(), c"draggingEntered:", near), 1);
            assert_eq!(drag_op(rig.raw(), c"draggingUpdated:", near), 1);
            {
                let l = log.borrow();
                assert_eq!(l.accepted_at.len(), 2);
                let (x, y) = l.accepted_at[0];
                assert!((x - 30.0).abs() < 0.01 && (f64::from(y) - (h - 20.0)).abs() < 0.01, "{x} {y}");
            }
            // Over a place the editor refuses: no operation, and no drop.
            let far = dragging_info(&[c"/a/one.wav"], CGPoint { x: 150.0, y: 20.0 });
            assert_eq!(drag_op(rig.raw(), c"draggingEntered:", far), 0);
            assert!(!drag_bool(rig.raw(), c"prepareForDragOperation:", far));
            // The drop gives the paths and the position.
            // (the stand-in reads the latest info's point, so make the near one again)
            let near = dragging_info(&[c"/a/one.wav", c"/a/two.wav"], CGPoint { x: 30.0, y: 20.0 });
            assert!(drag_bool(rig.raw(), c"prepareForDragOperation:", near));
            assert!(drag_bool(rig.raw(), c"performDragOperation:", near));
            let l = log.borrow();
            assert_eq!(l.dropped.len(), 1);
            assert_eq!(l.dropped[0].0, [PathBuf::from("/a/one.wav"), PathBuf::from("/a/two.wav")]);
            assert!((l.dropped[0].1 - 30.0).abs() < 0.01);
        }
    }

    pub fn file_dialog_requests() {
        unsafe {
            let (rig, log) = files_rig();
            let seen: Rc<RefCell<Vec<FileDialog>>> = Rc::new(RefCell::new(Vec::new()));
            let answer = Rc::new(RefCell::new(Some(Some(vec![PathBuf::from("/a/chosen.wav")]))));
            {
                let (seen, answer) = (seen.clone(), answer.clone());
                rig.view.as_ref().unwrap().set_file_dialog_hook(Some(Box::new(move |req: &FileDialog| {
                    seen.borrow_mut().push(req.clone());
                    answer.borrow_mut().take()
                })));
            }
            let request = FileDialog { tag: 7, title: "Choose a sample".into(), extensions: vec!["wav".into(), ".AIF".into()], multiple: true };
            log.borrow_mut().ask = Some(request.clone());
            run_loop(0.15);
            // The request reached the view once; the answer came on a later tick.
            assert_eq!(seen.borrow().len(), 1);
            assert_eq!(seen.borrow()[0], request);
            assert_eq!(log.borrow().answers, [(7, Some(vec![PathBuf::from("/a/chosen.wav")]))]);

            // A cancel is None.
            *answer.borrow_mut() = Some(None);
            log.borrow_mut().ask = Some(FileDialog { tag: 8, ..request.clone() });
            run_loop(0.15);
            assert_eq!(log.borrow().answers[1], (8, None));

            // The real panel: configured from the request, never shown.
            let panel = openplugin::dri::cocoa::EditorView::open_panel_for(&request);
            assert!(bool_of(panel, c"allowsMultipleSelection"));
            assert_eq!(string_of(panel, c"message"), "Choose a sample");
            let types = send_types(panel);
            assert_eq!(types, ["wav", "AIF"]);
            let single = openplugin::dri::cocoa::EditorView::open_panel_for(&FileDialog { multiple: false, extensions: vec![], title: String::new(), tag: 1 });
            assert!(!bool_of(single, c"allowsMultipleSelection"));

            // The real completion block (what the system calls when the sheet ends) delivers a cancel.
            rig.view.as_ref().unwrap().set_file_dialog_hook(None);
            rig.view.as_ref().unwrap().simulate_panel_response(&FileDialog { tag: 9, ..request }, 0);
            run_loop(0.15);
            assert_eq!(log.borrow().answers[2], (9, None));
            assert_eq!(log.borrow().answers.len(), 3);
        }
    }

    unsafe fn send_types(panel: Id) -> Vec<String> { unsafe {
        let types = id_of(panel, c"allowedFileTypes");
        (0..count_of(types)).map(|i| text(array_item(types, i))).collect()
    }}
}
