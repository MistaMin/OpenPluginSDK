//! The design-file editor as VoiceOver reads it, on the main thread (no test
//! harness, so `main` is the process's main thread, as AppKit needs). Same
//! reading and acting as the gain editor's test (`examples/editor/tests/a11y.rs`),
//! on a `DesignEditor` built from `design.csv`: two knobs, a fader, a switch
//! and a key bank, every control found with name, value, role and place, and
//! every action producing the gestures the mouse would.

#[cfg(not(target_os = "macos"))]
fn main() {
    // Native view tests need AppKit; there is no Linux (X11/Wayland) editor view yet.
    println!("design_editor a11y: skipped: macOS only (0 tests run)");
}

#[cfg(target_os = "macos")]
#[path = "../../editor/tests/a11y_support/mod.rs"]
mod a11y_support;

#[cfg(target_os = "macos")]
fn main() {
    mac::design_tree();
    println!("a11y (design editor): 1 passed");
}

#[cfg(target_os = "macos")]
mod mac {
    use super::a11y_support::*;
    use example_design_editor::{make_editor, DESIGN, INFO};
    use openplugin::core::EditorHost;
    use openplugin::dri::DesignEditor;

    pub fn design_tree() {
        unsafe {
            // The places the registry gives the controls, from a second editor of the same design.
            let reference = DesignEditor::from_csv(DESIGN, &INFO, EditorHost::detached()).expect("design");
            let area = |i: usize| reference.controls().get(i).expect("control").area.bounds();

            let mut rig = Rig::new(make_editor);
            assert_eq!(group_role(rig.raw()), ("AXGroup".to_string(), true));
            let kids = children(rig.raw());
            assert_eq!(kids.len(), 5);
            let tree: Vec<(String, String, String)> = kids.iter().map(|&k| {
                let s = seen(k);
                (s.role, s.label, s.value)
            }).collect();
            let want = [
                ("AXSlider", "Input", "0.0 dB"),
                ("AXSlider", "Tone", "12.00 kHz"),
                ("AXSlider", "Drive", "0%"),
                ("AXCheckBox", "Warm", "#0"),
                ("AXSlider", "Mode", "Clean"),
            ];
            for (got, want) in tree.iter().zip(want) {
                assert_eq!((got.0.as_str(), got.1.as_str(), got.2.as_str()), want);
            }

            // Frames: the registry's areas, placed on the screen. The fader's body
            // sits in its CSV row (192, 14, 36 x 150), above the value strip.
            for (i, &k) in kids.iter().enumerate() {
                assert!(near(frame(k), rig.expect_screen(area(i))), "control {i}: {:?} vs {:?}", frame(k), rig.expect_screen(area(i)));
            }
            let body = area(2);
            assert_eq!((body.0, body.1, body.2), (192.0, 14.0, 36.0));
            assert!(body.3 < 150.0 && body.3 > 100.0, "{body:?}");

            let (input, tone, drive, warm, mode) = (kids[0], kids[1], kids[2], kids[3], kids[4]);

            // A knob, a log-taper knob and a fader step 5% of their travel.
            assert!(rig.increment(input));
            assert_eq!(rig.gestures(), ["begin 0", "perform 0 2.400", "end 0"]);
            assert_eq!(seen(input).value, "2.4 dB");
            assert!(!rig.decrement(drive), "Drive is at its minimum");
            assert!(rig.gestures().is_empty());
            assert!(rig.increment(drive));
            assert_eq!(rig.gestures(), ["begin 2", "perform 2 5.000", "end 2"]);
            assert_eq!(seen(drive).value, "5%");
            // The Tone knob is logarithmic: 5% of the travel down from 12 kHz.
            assert!(rig.decrement(tone));
            let g = rig.gestures();
            assert_eq!((g[0].as_str(), g[2].as_str()), ("begin 1", "end 1"));
            let hz: f32 = g[1].trim_start_matches("perform 1 ").parse().unwrap();
            let expect = 200.0 * (12000.0f32 / 200.0).powf(0.95);
            assert!((hz - expect).abs() < 1.0, "{hz} vs {expect}");

            // The key bank moves one key; the switch presses; both as whole gestures.
            assert!(rig.increment(mode));
            assert_eq!(rig.gestures(), ["begin 4", "perform 4 1.000", "end 4"]);
            assert_eq!(seen(mode).value, "Soft");
            assert!(rig.decrement(mode));
            assert_eq!(seen(mode).value, "Clean");
            assert!(!rig.decrement(mode), "no key below the first");
            rig.gestures();
            assert!(rig.press(warm));
            assert_eq!(rig.gestures(), ["begin 3", "perform 3 1.000", "end 3"]);
            let w = seen(warm);
            assert_eq!((w.value.as_str(), w.description.as_str()), ("#1", "On"));
            assert!(!rig.press(mode), "a key bank is not pressed");

            // A value from the host's automation shows at once.
            rig.editor().param_changed(4, 2.0);
            assert_eq!(seen(mode).value, "Hard");
            rig.editor().param_changed(0, -3.0);
            assert_eq!(seen(input).value, "-3.0 dB");
            run_loop(0.12);
            assert_eq!(seen(input).value, "-3.0 dB");
            assert!(rig.gestures().is_empty());

            // The cursor moves focus; the tree reports the focused element.
            set_focused(drive);
            assert!(seen(drive).focused);
            assert_eq!(kids.iter().filter(|&&k| seen(k).focused).count(), 1);
            assert_eq!(focused_element(rig.raw()), drive);
        }
    }
}
