//! Bundle layout and `Info.plist` reading.
//!
//! A property list is read by `plutil -convert json`, the system tool, which
//! handles the XML and binary forms alike. It is run with an argument list (no
//! shell) on a file the scan was pointed at, and only reads.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

/// A property list as JSON. `None` when the file is missing or not a plist.
pub fn read_plist(path: &Path) -> Option<Value> {
    if !path.is_file() {
        return None;
    }
    let out = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

/// A JSON file, as a value. `None` when missing or not JSON.
pub fn read_plist_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// `<bundle>/Contents/Info.plist`.
pub fn info_plist(bundle: &Path) -> Option<Value> {
    read_plist(&bundle.join("Contents").join("Info.plist"))
}

/// The string value of `key` in a parsed plist; `None` when missing or not a string.
pub fn plist_str<'a>(plist: &'a Value, key: &str) -> Option<&'a str> {
    plist.get(key).and_then(Value::as_str)
}

/// The file to `dlopen` for a bundle: `Contents/MacOS/<CFBundleExecutable>`,
/// or the only file in `Contents/MacOS`, or the bundle's own stem. A path that
/// is a plain file is its own executable.
pub fn executable(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.is_dir() {
        return Err(format!("{} does not exist", path.display()));
    }
    let macos = path.join("Contents").join("MacOS");
    if let Some(plist) = info_plist(path) {
        if let Some(name) = plist_str(&plist, "CFBundleExecutable") {
            let p = macos.join(name);
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let p = macos.join(&stem);
    if p.is_file() {
        return Ok(p);
    }
    let mut files = std::fs::read_dir(&macos)
        .map_err(|_| format!("{} has no Contents/MacOS", path.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file());
    match (files.next(), files.next()) {
        (Some(only), None) => Ok(only),
        _ => Err(format!("cannot tell which file in {} is the executable", macos.display())),
    }
}

/// The format a path is, by extension. `None` when it is not one of the four.
pub fn format_of(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match ext.as_str() {
        "clap" => Some("clap"),
        "vst3" => Some("vst3"),
        "component" => Some("au"),
        "aaxplugin" => Some("aax"),
        _ => None,
    }
}
