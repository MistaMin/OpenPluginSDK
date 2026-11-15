//! Frame time of the editor in four situations, as the Cocoa view drives it: each frame is a tick,
//! then a question to the editor about what changed, then a render unless nothing did.
//!
//! `frames [scale] [frames]` prints one line per situation: the mean time per view frame, and how
//! many of the frames drew anything.
//!
//! * idle: nothing happens.
//! * knob: a drag moves the knob a little each frame, to values that do not repeat exactly.
//! * knob*: the knob jumps between eight values, shaded once each.
//! * meter: the level at the meter changes every frame.
use std::time::Instant;

use example_editor::{layout, EditorGain, GainEditor};
use openplugin::core::{Editor, EditorHost, Plugin};

/// One view frame. Returns whether it drew.
fn view_frame(editor: &mut GainEditor, scale: f32) -> bool {
    editor.tick(1.0 / 30.0);
    if matches!(editor.damage(), Some(rects) if rects.is_empty()) {
        return false;
    }
    std::hint::black_box(editor.render(scale).pixels.len());
    true
}

fn run(name: &str, scale: f32, frames: u32, mut step: impl FnMut(&mut GainEditor, &mut EditorGain, u32)) {
    let mut engine = EditorGain::default();
    let mut editor = GainEditor::new(EditorHost::detached());
    editor.bind(engine.editor_state());
    // The opening frame paints the plate; it is not part of the measurement.
    view_frame(&mut editor, scale);
    let start = Instant::now();
    let mut drawn = 0;
    for i in 0..frames {
        step(&mut editor, &mut engine, i);
        drawn += u32::from(view_frame(&mut editor, scale));
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(frames.max(1));
    println!("{name:<6} {ms:8.3} ms per view frame, {drawn} of {frames} frames drew");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let scale: f32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let frames: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(600);
    println!("scale {scale}, {frames} frames each");
    run("idle", scale, frames, |_, _, _| {});
    let lay = layout(240, 160);
    let (kx, ky) = (lay.knob.0 + lay.knob.2 * 0.5, lay.knob.1 + lay.knob.3 * 0.5);
    run("knob", scale, frames, |e, _, i| {
        if i == 0 {
            e.mouse_down(kx, ky, 1);
        }
        e.mouse_move(kx, ky - 40.0 * ((i as f32) * 0.2).sin());
    });
    // The host moves the knob between eight values. The lit knob is shaded once per value
    // (milliseconds, and cached), so after the first round this is the cost of painting alone.
    run("knob*", scale, frames, |e, _, i| e.param_changed(0, [-12.0, -6.0, 0.0, 6.0, 12.0, 6.0, 0.0, -6.0][(i % 8) as usize]));
    run("meter", scale, frames, |_, engine, i| {
        let level = 0.02 + 0.9 * ((i as f32) * 0.37).sin().abs();
        let mut io = vec![vec![level; 64]; 2];
        engine.process(&mut io, 64);
    });
}
