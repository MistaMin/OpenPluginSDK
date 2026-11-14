//! Helpers for a plugin's `build.rs`. Add the SDK as a build dependency
//! (`[build-dependencies] openplugin-core = "0.3.0-beta"`).
//!
//! ```ignore
//! // build.rs
//! fn main() {
//!     openplugin_core::build::embed_files("presets", "preset", "presets.rs").unwrap();
//! }
//!
//! // src/presets.rs
//! mod files {
//!     include!(concat!(env!("OUT_DIR"), "/presets.rs"));
//! }
//! static BANK: OnceLock<PresetBank> = OnceLock::new();
//! fn bank() -> &'static PresetBank {
//!     BANK.get_or_init(|| PresetBank::parse_all(&INFO, files::FILES).0)
//! }
//! ```

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn collect(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
            collect(&path, extension, out)?;
        } else if path.extension().is_some_and(|e| e == extension) {
            println!("cargo:rerun-if-changed={}", path.display());
            out.push(path);
        }
    }
    Ok(())
}

/// Embed every `*.<extension>` file under `dir` (relative to the crate) as
/// `pub static FILES: &[(&str, &str)]` in `$OUT_DIR/<out_name>`: the path
/// relative to `dir` with `/` separators, and the file's text, sorted by
/// path. No absolute path goes into the binary. A missing folder embeds
/// nothing. Call from `build.rs` only.
pub fn embed_files(dir: &str, extension: &str, out_name: &str) -> std::io::Result<()> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").map_err(std::io::Error::other)?;
    let out_dir = std::env::var("OUT_DIR").map_err(std::io::Error::other)?;
    let root = Path::new(&manifest).join(dir);
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    if root.is_dir() {
        collect(&root, extension, &mut files)?;
    }
    let mut entries = Vec::with_capacity(files.len());
    for file in files {
        let relative = file.strip_prefix(&root).unwrap_or(&file);
        let name = relative.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        entries.push((name, std::fs::read_to_string(&file)?));
    }
    entries.sort();
    let mut out = String::from("pub static FILES: &[(&str, &str)] = &[\n");
    for (name, text) in &entries {
        writeln!(out, "    ({name:?}, {text:?}),").map_err(std::io::Error::other)?;
    }
    out.push_str("];\n");
    std::fs::write(Path::new(&out_dir).join(out_name), out)
}
