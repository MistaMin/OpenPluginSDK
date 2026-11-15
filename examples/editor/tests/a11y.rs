//! The editor as VoiceOver reads it, on the main thread (this test has no
//! harness, so `main` is the process's main thread, as AppKit needs).
//!
//! The editor sits in an `EditorView` in a borderless window. The test reads
//! the accessibility tree from the view (children, role, label, value, frame in
//! screen coordinates), performs increment, decrement and press on the
//! elements, and checks the gestures that reach a recording `EditorHost`; then
//! changes a value from the host's side and reads the new value, moves the
//! keyboard focus and resizes the editor and checks the tree follows.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod a11y_support;

#[cfg(target_os = "macos")]
fn main() {
    mac::editor_tree();
    println!("a11y (editor): 1 passed");
}

#[cfg(target_os = "macos")]
mod mac {
    use super::a11y_support::*;
    use example_editor::{layout, make_editor, HEIGHT, WIDTH};
    use openplugin::core::editor::{Key, Modifiers};

    pub fn editor_tree() {
        unsafe {
            let mut rig = Rig::new(make_editor);
            let lay = layout(WIDTH, HEIGHT);

            // The view is one group; its children are the two controls.
            assert_eq!(group_role(rig.raw()), ("AXGroup".to_string(), true));
            let kids = children(rig.raw());
            assert_eq!(kids.len(), 2);
            let (gain, warm) = (kids[0], kids[1]);

            // Name, value and role.
            let g = seen(gain);
            assert_eq!((g.role.as_str(), g.label.as_str(), g.value.as_str()), ("AXSlider", "Gain", "0.0 dB"));
            assert_eq!(g.description, "0.0 dB");
            assert!(g.enabled && !g.focused);
            let w = seen(warm);
            assert_eq!((w.role.as_str(), w.label.as_str(), w.value.as_str()), ("AXCheckBox", "Warm", "#0"));
            assert_eq!(w.description, "Off");

            // Frames are screen rectangles: the window's place plus the editor's.
            assert!(near(frame(gain), rig.expect_screen(lay.knob)), "{:?} vs {:?}", frame(gain), rig.expect_screen(lay.knob));
            assert!(near(frame(warm), rig.expect_screen(lay.switch)));
            // Pointing at a control finds its element, pointing at bare plate the group.
            let f = frame(warm);
            assert_eq!(hit_test(rig.raw(), f.origin.x + f.size.w * 0.5, f.origin.y + f.size.h * 0.5), warm);
            assert_eq!(hit_test(rig.raw(), WINDOW_AT.0 + 2.0, WINDOW_AT.1 + 2.0), rig.raw());

            // Increment is one 5% step of 48 dB, as the whole gesture a key sends.
            assert!(rig.increment(gain));
            assert_eq!(rig.gestures(), ["begin 0", "perform 0 2.400", "end 0"]);
            assert_eq!(seen(gain).value, "2.4 dB");
            assert!(rig.decrement(gain));
            assert_eq!(rig.gestures(), ["begin 0", "perform 0 0.000", "end 0"]);
            assert_eq!(seen(gain).value, "0.0 dB");
            // A slider does not press.
            assert!(!rig.press(gain));
            assert!(rig.gestures().is_empty());

            // Press flips the switch like a click: begin, perform, end.
            assert!(rig.press(warm));
            assert_eq!(rig.gestures(), ["begin 1", "perform 1 1.000", "end 1"]);
            let w = seen(warm);
            assert_eq!((w.value.as_str(), w.description.as_str()), ("#1", "On"));
            assert!(rig.press(warm));
            assert_eq!(rig.gestures(), ["begin 1", "perform 1 0.000", "end 1"]);

            // Automation: a value from the host reaches the element, with no gesture.
            rig.editor().param_changed(0, -12.0);
            assert_eq!(seen(gain).value, "-12.0 dB");
            rig.editor().param_changed(1, 1.0);
            assert_eq!(seen(warm).description, "On");
            run_loop(0.12); // ticks run; the notifications are posted
            assert_eq!(seen(gain).value, "-12.0 dB");
            assert!(rig.gestures().is_empty());

            // At the end of the range an increment does nothing.
            rig.editor().param_changed(0, 24.0);
            assert!(!rig.increment(gain));
            assert!(rig.gestures().is_empty());

            // VoiceOver's cursor takes the keyboard to the switch, and the tree says so.
            set_focused(warm);
            assert!(seen(warm).focused && !seen(gain).focused);
            assert_eq!(focused_element(rig.raw()), warm);
            // Keyboard focus moves the other way too: Shift-Tab goes back to the knob.
            let shift = Modifiers { shift: true, ..Modifiers::NONE };
            assert!(rig.editor().key_down(Key::Tab, shift));
            assert!(seen(gain).focused && !seen(warm).focused);
            assert_eq!(focused_element(rig.raw()), gain);

            // Resize: the elements move with the editor.
            let (w2, h2) = rig.view.as_ref().unwrap().resize(360, 240);
            assert_eq!((w2, h2), (360, 240));
            let lay2 = layout(360, 240);
            assert!(near(frame(gain), rig.expect_screen(lay2.knob)), "{:?} vs {:?}", frame(gain), rig.expect_screen(lay2.knob));
            assert!(near(frame(warm), rig.expect_screen(lay2.switch)));

            // Elements the host still holds go quiet when the view is gone.
            retain(gain);
            let view = rig.view.take();
            drop(view);
            assert_eq!(seen(gain).role, "<nil>");
            assert!(!rig.increment(gain));
            release(gain);
        }
    }
}
