//! Writes the LV2 description files into a bundle folder:
//! `lv2_ttl <folder> <library stem>` gives `manifest.ttl` and `<stem>.ttl`.
use example_editor::EditorGain;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(dir), Some(stem)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: lv2_ttl <folder> <library stem>");
        std::process::exit(2);
    };
    if let Err(e) = openplugin::targets::lv2::write_bundle(&EditorGain::default(), std::path::Path::new(dir), stem, true) {
        eprintln!("{dir}: {e}");
        std::process::exit(1);
    }
}
