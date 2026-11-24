//! `openplugin-port`: the porting toolkit's scanner. It loads an existing CLAP,
//! VST3 or Audio Unit (and reads an AAX bundle's identity), and writes a
//! manifest and a Rust snippet that start a port to OpenPluginSDK, compares a
//! port with the original (`null`), and captures the original's saved states
//! (`capture`), and starts the port's crate from a manifest (`new`, in
//! [`scaffold`]). See `docs/PORTING.md`.
//!
//! Every plugin is loaded in a child process of the scanner itself
//! ([`probe`]), so a plugin that crashes or hangs costs one scan result, not
//! the scanner.
#![deny(missing_docs)]

pub mod aax;
#[cfg(target_os = "macos")]
pub mod au;
#[cfg(not(target_os = "macos"))]
#[path = "au_unsupported.rs"]
pub mod au;
pub mod bundle;
pub mod capture;
pub mod clap;
pub mod manifest;
pub mod null;
pub mod probe;
pub mod render;
pub mod scaffold;
pub mod signal;
pub mod snippet;
#[cfg(test)]
mod snippet_tests;
pub mod sys;
pub mod vst3;
pub mod wav;

use std::path::{Path, PathBuf};
use std::time::Duration;

use manifest::{Manifest, Source, SCHEMA_VERSION};

/// The tool name and version written into every manifest.
///
/// ```
/// assert!(openplugin_port::tool_id().starts_with("openplugin-port "));
/// ```
pub fn tool_id() -> String {
    format!("openplugin-port {}", env!("CARGO_PKG_VERSION"))
}

/// What [`scan`] does. Start from [`ScanOptions::new`] and set fields.
///
/// ```
/// use std::time::Duration;
/// use openplugin_port::ScanOptions;
///
/// let mut options = ScanOptions::new("openplugin-port".into());
/// options.format = Some("vst3".into());
/// options.timeout = Duration::from_secs(60);
/// ```
pub struct ScanOptions {
    /// The executable to re-run as `__probe`. The CLI passes itself.
    pub helper: PathBuf,
    /// How long the child may run before it is killed and the scan fails. Default 30 s.
    pub timeout: Duration,
    /// `clap`, `vst3`, `au` or `aax`; by default the path's extension decides.
    pub format: Option<String>,
    /// Extra environment for the child (used by the tests to inject faults).
    pub env: Vec<(String, String)>,
}

impl ScanOptions {
    /// Options with the defaults: 30 s timeout, format from the extension, no extra environment.
    pub fn new(helper: PathBuf) -> Self {
        Self { helper, timeout: Duration::from_secs(30), format: None, env: Vec::new() }
    }
}

/// Scan one plugin file or bundle. One manifest per plugin it holds.
///
/// CLAP, VST3 and Audio Unit code runs only in a child process ([`probe::run_child`]); AAX is
/// read from files. Blocks for up to `opts.timeout`. `Err` with a message for a path that does
/// not exist, a format that cannot be told or is unknown, or a child that crashed, hung or
/// found nothing.
pub fn scan(path: &Path, opts: &ScanOptions) -> Result<Vec<Manifest>, String> {
    let path = std::path::absolute(path).map_err(|e| e.to_string())?;
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    let format = match &opts.format {
        Some(f) => f.clone(),
        None => bundle::format_of(&path).ok_or("cannot tell the format from the extension: use .clap, .vst3, .component or .aaxplugin, or pass --format")?.to_string(),
    };
    if !["clap", "vst3", "au", "aax"].contains(&format.as_str()) {
        return Err(format!("unknown format {format}: use clap, vst3, au or aax"));
    }
    // AAX is read from files only; the other formats load code, so they run in a child.
    let result = if format == "aax" { aax::probe(&path)? } else { probe::run_child(&opts.helper, &format, &path, opts.timeout, &opts.env)? };

    let bundle_plist = bundle::info_plist(&path);
    let bundle_id = bundle_plist.as_ref().and_then(|p| bundle::plist_str(p, "CFBundleIdentifier").map(str::to_string));
    let module_info = if format == "vst3" { bundle::read_plist_json(&path.join("Contents").join("Resources").join("moduleinfo.json")) } else { None };

    let count = result.plugins.len() as u32;
    let mut out = Vec::new();
    for (index, mut plugin) in result.plugins.into_iter().enumerate() {
        for n in &result.notes {
            plugin.warnings.push(n.clone());
        }
        if plugin.version.is_empty() {
            if let Some(v) = bundle_plist.as_ref().and_then(|p| bundle::plist_str(p, "CFBundleShortVersionString")) {
                plugin.version = v.to_string();
            }
        }
        if let (Some(v), Some(info)) = (plugin.ids.vst3.as_mut(), module_info.as_ref()) {
            v.module_info = true;
            let class = info.get("Classes").and_then(|c| c.as_array()).and_then(|list| {
                list.iter().find(|c| c.get("CID").and_then(|x| x.as_str()).is_some_and(|cid| cid.eq_ignore_ascii_case(&v.class_id)))
            });
            match class {
                Some(c) => v.module_info_class = Some(c.clone()),
                None => plugin.warnings.push("moduleinfo.json has no class with this class id".into()),
            }
        }
        out.push(Manifest {
            schema_version: SCHEMA_VERSION,
            tool: tool_id(),
            source: Source { format: format.clone(), path: path.display().to_string(), index: index as u32, count, bundle_id: bundle_id.clone() },
            plugin,
        });
    }
    if out.is_empty() {
        return Err("the bundle holds no plugin".into());
    }
    Ok(out)
}

fn file_slug(text: &str) -> String {
    let mut s: String = text.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s.trim_matches('-').to_string()
}

/// Write `<name>.manifest.json` and `<name>.params.rs` for each manifest. Returns the paths written.
pub fn write_outputs(manifests: &[Manifest], out_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(out_dir)?;
    let mut written = Vec::new();
    for m in manifests {
        let stem = Path::new(&m.source.path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut name = format!("{}.{}", file_slug(if m.source.count > 1 { &m.plugin.name } else { &stem }), m.source.format);
        if name.starts_with('.') {
            name = format!("plugin{name}");
        }
        let json = serde_json::to_string_pretty(m).map_err(std::io::Error::other)?;
        let jp = out_dir.join(format!("{name}.manifest.json"));
        std::fs::write(&jp, json + "\n")?;
        let rp = out_dir.join(format!("{name}.params.rs"));
        std::fs::write(&rp, snippet::render(m))?;
        written.push(jp);
        written.push(rp);
    }
    Ok(written)
}
