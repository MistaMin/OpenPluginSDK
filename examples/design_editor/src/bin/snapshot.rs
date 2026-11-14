//! Writes the example editor to a PPM: `snapshot out.ppm [scale]`.
use example_design_editor::make_editor;
use openplugin::core::EditorHost;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(out) = args.get(1) else {
        eprintln!("usage: snapshot <out.ppm> [scale]");
        std::process::exit(2);
    };
    let scale: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let Some(mut editor) = make_editor(EditorHost::detached()) else { std::process::exit(1) };
    for (i, v) in [(0, 6.0), (1, 2000.0), (2, 40.0), (3, 1.0), (4, 1.0)] {
        editor.param_changed(i, v);
    }
    let frame = editor.render(scale);
    let mut ppm = format!("P6\n{} {}\n255\n", frame.width, frame.height).into_bytes();
    for p in frame.pixels {
        ppm.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    std::fs::write(out, ppm).expect("write snapshot");
}
