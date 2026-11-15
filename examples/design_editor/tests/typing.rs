//! Tooltips and typing on the design-file editor, on the main thread (no
//! harness, as AppKit needs). Same rig as the gain editor's `typing` test: the
//! view sits in a window that is never shown. Every control from `design.csv`
//! has a tooltip rectangle at its registry area with its name and value, and
//! a double-click on a knob opens the text field, whose text commits through
//! `ParamInfo::parse` (a Hz value with a unit, a choice by name).

#[cfg(not(target_os = "macos"))]
fn main() {
    // Native view tests need AppKit; there is no Linux (X11/Wayland) editor view yet.
    println!("design_editor typing: skipped: macOS only (0 tests run)");
}

#[cfg(target_os = "macos")]
#[path = "../../editor/tests/a11y_support/mod.rs"]
mod a11y_support;

#[cfg(target_os = "macos")]
fn main() {
    mac::design_typing();
    println!("typing (design editor): 1 passed");
}

#[cfg(target_os = "macos")]
mod mac {
    use super::a11y_support::*;
    use example_design_editor::{make_editor, DESIGN, INFO};
    use openplugin::core::EditorHost;
    use openplugin::dri::DesignEditor;

    pub fn design_typing() {
        unsafe {
            let reference = DesignEditor::from_csv(DESIGN, &INFO, EditorHost::detached()).expect("design");
            let area = |i: usize| reference.controls().get(i).expect("control").area.bounds();
            let mut rig = Rig::new(make_editor);

            // One tooltip per control, at the control's area.
            let tips = rig.view.as_ref().unwrap().tooltip_rects();
            assert_eq!(tips.len(), 5);
            let want = ["Input: 0.0 dB", "Tone: 12.00 kHz", "Drive: 0%", "Warm: Off", "Mode: Clean"];
            for (i, ((r, text), want)) in tips.iter().zip(want).enumerate() {
                let a = area(i);
                assert_eq!((r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32), a, "control {i}");
                assert_eq!(text, want);
            }
            rig.editor().param_changed(4, 2.0);
            assert_eq!(rig.view.as_ref().unwrap().tooltip_rects()[4].1, "Mode: Hard");

            // Double-click the Tone knob and type a frequency with a unit.
            let a = area(1);
            let (x, y) = (a.0 + a.2 * 0.5, a.1 + a.3 * 0.5);
            rig.click(x, y, 2, 0);
            rig.gestures();
            assert!(rig.has_field());
            assert_eq!(rig.field_text(), "12.00 kHz");
            rig.set_field_text(c"1.5 kHz");
            assert!(rig.field_command(c"insertNewline:"));
            assert_eq!(rig.gestures(), ["begin 1", "perform 1 1500.000", "end 1"]);
            assert!(!rig.has_field());
            assert_eq!(rig.view.as_ref().unwrap().tooltip_rects()[1].1, "Tone: 1.50 kHz");

            // A key bank takes a choice by name on Return.
            let kids = children(rig.raw());
            set_focused(kids[4]);
            rig.key(36, c"\r", 0);
            assert!(rig.has_field());
            assert_eq!(rig.field_text(), "Hard");
            rig.set_field_text(c"soft");
            assert!(rig.field_command(c"insertNewline:"));
            assert_eq!(rig.gestures(), ["begin 4", "perform 4 1.000", "end 4"]);

            // Escape leaves everything as it was.
            rig.key(36, c"\r", 0);
            rig.set_field_text(c"clean");
            assert!(rig.field_command(c"cancelOperation:"));
            assert!(rig.gestures().is_empty() && !rig.has_field());

            // Return through the application's event path (a host that takes Return) commits.
            use openplugin::dri::cocoa::key_monitor_count;
            rig.click(x, y, 2, 0);
            rig.gestures();
            rig.set_field_text(c"2 kHz");
            assert_eq!(key_monitor_count(), 1);
            rig.app_key(36, c"\r", 0);
            assert_eq!(rig.gestures(), ["begin 1", "perform 1 2000.000", "end 1"]);
            assert!(!rig.has_field() && key_monitor_count() == 0);
            // Losing focus commits valid text.
            rig.click(x, y, 2, 0);
            rig.gestures();
            rig.set_field_text(c"3 kHz");
            rig.field_end_editing();
            assert_eq!(rig.gestures(), ["begin 1", "perform 1 3000.000", "end 1"]);
            assert!(!rig.has_field() && key_monitor_count() == 0);
        }
    }
}
