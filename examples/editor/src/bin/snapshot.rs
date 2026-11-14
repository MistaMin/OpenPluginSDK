//! Renders the editor without a window: `snapshot <out.ppm> [scale] [gain dB] [warm 0|1]`.
//! With `OP_SNAPSHOT_FRAMES=n` it also renders n more frames and prints the time per frame.
use example_editor::make_editor;
use openplugin::core::EditorHost;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(out) = args.get(1) else {
        eprintln!("usage: snapshot <out.ppm> [scale] [gain dB] [warm 0|1]");
        std::process::exit(2);
    };
    let scale: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let gain: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let warm: f32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let Some(mut editor) = make_editor(EditorHost::detached()) else { std::process::exit(1) };
    editor.param_changed(0, gain);
    editor.param_changed(1, warm);
    if let Some(n) = std::env::var("OP_SNAPSHOT_FRAMES").ok().and_then(|v| v.parse::<u32>().ok()) {
        editor.render(scale);
        let start = std::time::Instant::now();
        for i in 0..n {
            // Move the knob a little each frame, as a drag does.
            editor.param_changed(0, gain + (i % 7) as f32 * 0.1);
            editor.render(scale);
        }
        println!("{:.2} ms per frame at scale {scale}", start.elapsed().as_secs_f64() * 1000.0 / n.max(1) as f64);
        editor.param_changed(0, gain);
    }
    let frame = editor.render(scale);
    let mut ppm = format!("P6\n{} {}\n255\n", frame.width, frame.height).into_bytes();
    for p in frame.pixels {
        ppm.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    if let Err(e) = std::fs::write(out, ppm) {
        eprintln!("{out}: {e}");
        std::process::exit(1);
    }
}
