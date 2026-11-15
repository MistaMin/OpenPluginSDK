//! State capture: save a plugin's state blobs, exactly as a host would store
//! them, for its default state and its factory presets, with every
//! parameter's plain value read back after each load. See `docs/PORTING.md`.
//!
//! Like the scanner, the plugin is loaded only in a child process
//! (`openplugin-port __capture <job json>`). The child writes each blob to
//! the output folder and prints the list of states; the parent adds the
//! plugin's identity from a scan and writes the manifest.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::manifest::{Ids, Source};
use crate::probe::{self, FAULT_ENV, emit_result};
use crate::{au, bundle, clap, scan, vst3, ScanOptions};

/// Version of the capture manifest layout.
pub const CAPTURE_SCHEMA_VERSION: u32 = 1;

/// The most bytes one blob may hold; a plugin that writes more is cut off and the state is skipped.
pub const MAX_BLOB: usize = 256 << 20;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    pub schema_version: u32,
    /// `openplugin-port <version>`.
    pub tool: String,
    pub source: Source,
    pub plugin: CapturedPlugin,
    pub states: Vec<State>,
    /// Things the capture could not do (a format without presets, a preset that would not load).
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CapturedPlugin {
    pub name: String,
    pub vendor: String,
    pub version: String,
    /// The same identity the scanner manifest holds.
    pub ids: Ids,
}

/// One saved state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// The preset's name, `default`, or `set` for the state after `--set`.
    pub name: String,
    /// `default`, `set`, `factory-preset` or `preset`.
    pub kind: String,
    /// Where the format says the preset came from (a file, a program list, a preset number).
    pub location: Option<String>,
    pub blobs: Vec<Blob>,
    /// Every parameter's plain value read back from the plugin right after this state was loaded.
    pub params: Vec<ParamValue>,
    pub notes: Vec<String>,
}

/// One blob file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Blob {
    /// What it is: `clap-state`, `vst3-component`, `vst3-controller`, `vst3-program-data`, `au-classinfo`.
    pub role: String,
    /// Path of the file, relative to the manifest's folder.
    pub file: String,
    pub bytes: u64,
    /// FNV-1a 64 of the file, 16 lower-case hex digits (a check against damage, not a signature).
    pub fnv1a64: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParamValue {
    /// The format's parameter id (the scanner manifest's `params[].id`).
    pub id: u32,
    pub name: String,
    pub plain: f64,
    /// VST3 only: the normalised value the controller reported.
    pub normalized: Option<f64>,
}

/// One `--set` assignment: a parameter id or name and a plain value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetSpec {
    pub param: String,
    pub value: f64,
}

/// The child's input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub format: String,
    pub path: PathBuf,
    pub index: u32,
    pub rate: f64,
    /// Folder that receives the blob folder.
    pub out_dir: PathBuf,
    /// Name of the blob folder inside `out_dir`.
    pub blob_dir: String,
    pub max_presets: usize,
    pub sets: Vec<SetSpec>,
}

/// The child's output.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChildResult {
    pub states: Vec<State>,
    pub notes: Vec<String>,
}

/// FNV-1a 64 as 16 hex digits.
pub fn fnv1a64(bytes: &[u8]) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn slug(text: &str) -> String {
    let mut s: String = text.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-');
    if s.is_empty() { "state".into() } else { s.chars().take(48).collect() }
}

/// Writes blobs as states are captured.
pub struct Sink {
    dir: PathBuf,
    rel: String,
    pub states: Vec<State>,
    pub notes: Vec<String>,
}

impl Sink {
    pub fn new(job: &Job) -> Result<Self, String> {
        let dir = job.out_dir.join(&job.blob_dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        Ok(Self { dir, rel: job.blob_dir.clone(), states: Vec::new(), notes: Vec::new() })
    }

    /// Record one state with its blobs (role, bytes) and parameter values.
    pub fn add(&mut self, name: &str, kind: &str, location: Option<String>, blobs: Vec<(&str, Vec<u8>)>, params: Vec<ParamValue>, notes: Vec<String>) -> Result<(), String> {
        let n = self.states.len();
        let mut out = Vec::new();
        for (role, bytes) in blobs {
            let file = format!("{n:03}-{}.{role}.bin", slug(name));
            std::fs::write(self.dir.join(&file), &bytes).map_err(|e| format!("cannot write {file}: {e}"))?;
            out.push(Blob { role: role.to_string(), file: format!("{}/{file}", self.rel), bytes: bytes.len() as u64, fnv1a64: fnv1a64(&bytes) });
        }
        self.states.push(State { name: name.to_string(), kind: kind.to_string(), location, blobs: out, params, notes });
        Ok(())
    }

    pub fn finish(self) -> ChildResult {
        ChildResult { states: self.states, notes: self.notes }
    }
}

/// Match `--set` entries to a plugin's parameters: by id first, then by name (any case).
pub fn resolve_sets(sets: &[SetSpec], table: &[(u32, String)]) -> Result<Vec<(u32, f64)>, String> {
    sets.iter()
        .map(|s| {
            let by_id = s.param.parse::<u32>().ok().and_then(|id| table.iter().find(|(i, _)| *i == id));
            let hit = by_id.or_else(|| table.iter().find(|(_, n)| n.eq_ignore_ascii_case(&s.param)));
            hit.map(|(id, _)| (*id, s.value)).ok_or_else(|| format!("--set: the plugin has no parameter {:?}", s.param))
        })
        .collect()
}

/// Parse `--set` text: `id=value` or `name=value` (the last `=` splits, so names may hold one).
pub fn parse_set(text: &str) -> Result<SetSpec, String> {
    let (param, value) = text.rsplit_once('=').ok_or_else(|| format!("--set needs param=value, got {text:?}"))?;
    let value: f64 = value.trim().parse().map_err(|_| format!("--set: {value:?} is not a number"))?;
    if !value.is_finite() || param.trim().is_empty() {
        return Err(format!("--set: bad assignment {text:?}"));
    }
    Ok(SetSpec { param: param.trim().to_string(), value })
}

pub struct CaptureOptions {
    /// The executable to re-run as `__capture`. The CLI passes itself.
    pub helper: PathBuf,
    pub timeout: Duration,
    /// `clap`, `vst3` or `au`; by default the path's extension decides.
    pub format: Option<String>,
    pub out_dir: PathBuf,
    pub max_presets: usize,
    pub sets: Vec<SetSpec>,
    pub env: Vec<(String, String)>,
}

impl CaptureOptions {
    pub fn new(helper: PathBuf, out_dir: PathBuf) -> Self {
        Self { helper, timeout: Duration::from_secs(120), format: None, out_dir, max_presets: 128, sets: Vec::new(), env: Vec::new() }
    }
}

/// Capture every plugin a bundle holds. Writes the blobs and one manifest per plugin; returns the manifests and their paths.
pub fn run(path: &Path, opts: &CaptureOptions) -> Result<Vec<(Capture, PathBuf)>, String> {
    let path = std::path::absolute(path).map_err(|e| e.to_string())?;
    let format = match &opts.format {
        Some(f) => f.clone(),
        None => bundle::format_of(&path).ok_or("cannot tell the format from the extension: use .clap, .vst3 or .component, or pass --format")?.to_string(),
    };
    if !["clap", "vst3", "au"].contains(&format.as_str()) {
        return Err(format!("cannot capture format {format}: use clap, vst3 or au"));
    }
    let mut scan_opts = ScanOptions::new(opts.helper.clone());
    scan_opts.format = Some(format.clone());
    scan_opts.timeout = opts.timeout;
    scan_opts.env = opts.env.clone();
    let manifests = scan(&path, &scan_opts)?;
    std::fs::create_dir_all(&opts.out_dir).map_err(|e| format!("cannot create {}: {e}", opts.out_dir.display()))?;
    let mut out = Vec::new();
    for m in manifests {
        let stem = Path::new(&m.source.path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let base = format!("{}.{}", slug(if m.source.count > 1 { &m.plugin.name } else { &stem }), format);
        let job = Job {
            format: format.clone(),
            path: path.clone(),
            index: m.source.index,
            rate: 48_000.0,
            out_dir: opts.out_dir.clone(),
            blob_dir: format!("{base}.blobs"),
            max_presets: opts.max_presets,
            sets: opts.sets.clone(),
        };
        let json = serde_json::to_string(&job).map_err(|e| e.to_string())?;
        let result: ChildResult = probe::run_helper(&opts.helper, &["__capture".into(), json.into()], opts.timeout, &opts.env)?;
        let capture = Capture {
            schema_version: CAPTURE_SCHEMA_VERSION,
            tool: crate::tool_id(),
            source: m.source.clone(),
            plugin: CapturedPlugin { name: m.plugin.name.clone(), vendor: m.plugin.vendor.clone(), version: m.plugin.version.clone(), ids: m.plugin.ids.clone() },
            states: result.states,
            notes: result.notes,
        };
        let file = opts.out_dir.join(format!("{base}.capture.json"));
        std::fs::write(&file, serde_json::to_string_pretty(&capture).map_err(|e| e.to_string())? + "\n").map_err(|e| format!("cannot write {}: {e}", file.display()))?;
        out.push((capture, file));
    }
    Ok(out)
}

/// The child's entry point: `__capture <job json>`. Never returns.
pub fn child_main(args: &[String]) -> ! {
    let result = child_capture(args);
    emit_result(&result)
}

fn child_capture(args: &[String]) -> Result<ChildResult, String> {
    let json = args.first().ok_or("usage: __capture <job json>")?;
    let job: Job = serde_json::from_str(json).map_err(|e| format!("unreadable job: {e}"))?;
    match std::env::var(FAULT_ENV).as_deref() {
        Ok("crash") => std::process::abort(),
        Ok("hang") => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        _ => {}
    }
    let mut sink = Sink::new(&job)?;
    match job.format.as_str() {
        "clap" => clap::capture(&job, &mut sink)?,
        "vst3" => vst3::capture(&job, &mut sink)?,
        "au" => au::capture(&job, &mut sink)?,
        other => return Err(format!("cannot capture format {other}")),
    }
    Ok(sink.finish())
}
