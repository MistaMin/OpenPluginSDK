//! `openplugin-port null`: render an original plugin and its port through the
//! same input and the same parameter automation, align them by their reported
//! latencies and compare. See `docs/PORTING.md`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::manifest::{Manifest, Param};
use crate::render::{Event, Job, RenderInfo, read_raw_channels, write_raw};
use crate::wav::{self, Audio};
use crate::{ScanOptions, probe, scan, signal};

pub struct NullOptions {
    /// The executable to re-run as `__render`. The CLI passes itself.
    pub helper: PathBuf,
    pub original: PathBuf,
    pub port: PathBuf,
    pub original_format: Option<String>,
    pub port_format: Option<String>,
    /// A built-in signal (`sweep`, `noise:3`, ...) or a WAV path.
    pub input: String,
    /// `frame:param=value` entries; see [`parse_automation`].
    pub automation: Option<String>,
    /// `None`: a WAV input's own rate, else 48000.
    pub rate: Option<u32>,
    pub block: u32,
    pub tolerance: f64,
    /// JSON object mapping original parameter ids or names to port ids or names.
    pub param_map: Option<PathBuf>,
    pub timeout: Duration,
    /// Where the raw render files go (default: a folder in the system temp dir, removed afterwards).
    pub work_dir: Option<PathBuf>,
    /// When set, write `input.wav`, `original.wav`, `port.wav` and `difference.wav` here.
    pub save_dir: Option<PathBuf>,
    /// Extra environment for each plugin's child (used by the tests to inject faults).
    pub env_original: Vec<(String, String)>,
    pub env_port: Vec<(String, String)>,
}

impl NullOptions {
    pub fn new(helper: PathBuf, original: PathBuf, port: PathBuf) -> Self {
        Self {
            helper,
            original,
            port,
            original_format: None,
            port_format: None,
            input: "sweep".into(),
            automation: None,
            rate: None,
            block: 512,
            tolerance: 1e-6,
            param_map: None,
            timeout: Duration::from_secs(120),
            work_dir: None,
            save_dir: None,
            env_original: Vec::new(),
            env_port: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Side {
    pub path: String,
    pub format: String,
    pub name: String,
    pub latency_samples: u32,
    pub input_channels: u32,
    pub output_channels: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SamplePair {
    pub index: usize,
    pub original: f32,
    pub port: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelReport {
    pub channel: usize,
    pub max_abs_diff: f64,
    pub max_at: usize,
    /// 20 log10 of the RMS of the difference; `None` for exactly zero (minus infinity).
    pub rms_diff_db: Option<f64>,
    /// First sample that is not bit-identical.
    pub first_differing: Option<SamplePair>,
    /// First sample whose difference exceeds the tolerance.
    pub first_over_tolerance: Option<SamplePair>,
    pub samples_over_tolerance: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct NullReport {
    pub original: Side,
    pub port: Side,
    pub rate: u32,
    pub block: u32,
    pub frames: usize,
    pub tolerance: f64,
    pub latency_differs: bool,
    pub channels: Vec<ChannelReport>,
    /// Parameter pairs used: original id and name, port id and name.
    pub parameter_map: Vec<String>,
    pub notes: Vec<String>,
    pub pass: bool,
}

impl NullReport {
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let side = |label: &str, x: &Side| {
            format!(
                "{label:<9} {} [{}] {:?}: latency {} samples, {} in, {} out\n",
                x.path, x.format, x.name, x.latency_samples, x.input_channels, x.output_channels
            )
        };
        s += &side("original", &self.original);
        s += &side("port", &self.port);
        s += &format!("{} Hz, blocks of {}, {} frames compared, tolerance {:e}\n", self.rate, self.block, self.frames, self.tolerance);
        if self.latency_differs {
            s += &format!(
                "LATENCY DIFFERS: original {} samples, port {}; outputs are aligned by their own latency\n",
                self.original.latency_samples, self.port.latency_samples
            );
        }
        for m in &self.parameter_map {
            s += &format!("parameter {m}\n");
        }
        for n in &self.notes {
            s += &format!("note: {n}\n");
        }
        for c in &self.channels {
            s += &format!(
                "channel {}: max |diff| {:e} at sample {}, RMS diff {}, {} samples over tolerance\n",
                c.channel,
                c.max_abs_diff,
                c.max_at,
                c.rms_diff_db.map_or("-inf dB".into(), |v| format!("{v:.2} dB")),
                c.samples_over_tolerance
            );
            if let Some(p) = &c.first_over_tolerance {
                s += &format!("  first sample over tolerance: {} original {:e} port {:e}\n", p.index, p.original, p.port);
            } else if let Some(p) = &c.first_differing {
                s += &format!("  first differing sample (within tolerance): {} original {:e} port {:e}\n", p.index, p.original, p.port);
            }
        }
        s += if self.pass { "PASS\n" } else { "FAIL: the port does not null against the original\n" };
        s
    }
}

// ---------- automation ----------

/// One automation entry: when, which parameter (id or name, as the original calls it), plain value.
#[derive(Debug, Clone, PartialEq)]
pub struct Automation {
    pub frame: u64,
    pub param: String,
    pub value: f64,
}

fn parse_time(text: &str, rate: u32) -> Result<u64, String> {
    let t = text.trim();
    let r = f64::from(rate);
    let frames = if let Some(ms) = t.strip_suffix("ms") {
        ms.trim().parse::<f64>().map(|v| v * r / 1000.0)
    } else if let Some(s) = t.strip_suffix('s') {
        s.trim().parse::<f64>().map(|v| v * r)
    } else {
        t.parse::<f64>()
    }
    .map_err(|_| format!("bad time {text:?} in the automation"))?;
    if frames < 0.0 {
        return Err(format!("negative time {text:?} in the automation"));
    }
    Ok(frames.round() as u64)
}

/// `frame:param=value`, separated by `,` `;` or newlines. A time is a frame number or has an `s` or
/// `ms` suffix; `param` is an id or a name; `@file` reads the entries from a file.
pub fn parse_automation(spec: &str, rate: u32) -> Result<Vec<Automation>, String> {
    let text = match spec.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?,
        None => spec.to_string(),
    };
    let mut out = Vec::new();
    for entry in text.split([',', ';', '\n']) {
        let entry = entry.trim();
        if entry.is_empty() || entry.starts_with('#') {
            continue;
        }
        let (time, rest) = entry.split_once(':').ok_or_else(|| format!("automation entry {entry:?}: expected frame:param=value"))?;
        let (param, value) = rest.rsplit_once('=').ok_or_else(|| format!("automation entry {entry:?}: expected frame:param=value"))?;
        let value = value.trim().parse::<f64>().map_err(|_| format!("automation entry {entry:?}: the value is not a number"))?;
        out.push(Automation { frame: parse_time(time, rate)?, param: param.trim().to_string(), value });
    }
    Ok(out)
}

fn find_param<'a>(params: &'a [Param], key: &str) -> Option<&'a Param> {
    if let Ok(id) = key.trim().parse::<u32>() {
        if let Some(p) = params.iter().find(|p| p.id == id) {
            return Some(p);
        }
    }
    let k = key.trim().to_lowercase();
    params
        .iter()
        .find(|p| p.name.to_lowercase() == k)
        .or_else(|| params.iter().find(|p| p.short_name.as_ref().is_some_and(|s| s.to_lowercase() == k)))
}

fn json_key(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Read a map file: an object of original id or name to port id or name.
pub fn read_param_map(path: &Path) -> Result<Vec<(String, String)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let obj = v.as_object().ok_or_else(|| format!("{}: expected a JSON object of original id or name to port id or name", path.display()))?;
    obj.iter()
        .map(|(k, v)| json_key(v).map(|v| (k.clone(), v)).ok_or_else(|| format!("{}: the value for {k:?} must be a string or number", path.display())))
        .collect()
}

/// The port parameter for an original one: the map file first (by id, then by name), then the same
/// name, then the same id.
pub fn match_param<'a>(orig: &Param, port: &'a [Param], map: &[(String, String)]) -> Option<&'a Param> {
    let id = orig.id.to_string();
    let by_map = map.iter().find(|(k, _)| *k == id).or_else(|| map.iter().find(|(k, _)| k.to_lowercase() == orig.name.to_lowercase()));
    if let Some((_, to)) = by_map {
        return find_param(port, to);
    }
    let name = orig.name.to_lowercase();
    port.iter().find(|p| p.name.to_lowercase() == name).or_else(|| port.iter().find(|p| p.id == orig.id))
}

/// The map from original to port parameter ids, written by `openplugin-port param-map`.
pub fn generate_param_map(original: &Manifest, port: &Manifest, map: &[(String, String)]) -> (serde_json::Value, Vec<String>) {
    let mut obj = serde_json::Map::new();
    let mut unmatched = Vec::new();
    for p in &original.plugin.params {
        match match_param(p, &port.plugin.params, map) {
            Some(q) => {
                obj.insert(p.id.to_string(), serde_json::Value::from(q.id));
            }
            None => unmatched.push(format!("{} ({})", p.id, p.name)),
        }
    }
    (serde_json::Value::Object(obj), unmatched)
}

// ---------- running ----------

fn scan_one(opts: &NullOptions, path: &Path, format: &Option<String>, env: &[(String, String)]) -> Result<Manifest, String> {
    let mut o = ScanOptions::new(opts.helper.clone());
    o.timeout = opts.timeout;
    o.format = format.clone();
    o.env = env.to_vec();
    let mut manifests = scan(path, &o).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(manifests.remove(0))
}

#[allow(clippy::too_many_arguments)]
fn render_side(opts: &NullOptions, work: &Path, tag: &str, manifest: &Manifest, events: Vec<Event>, in_file: &Path, src_channels: usize, frames: usize, rate: u32, env: &[(String, String)]) -> Result<(RenderInfo, Vec<Vec<f32>>), String> {
    let format = manifest.source.format.clone();
    if !["clap", "vst3", "au"].contains(&format.as_str()) {
        return Err(format!("{}: the null test renders CLAP, VST3 and Audio Unit plugins, not {format}", manifest.source.path));
    }
    let out_file = work.join(format!("{tag}.out.f32"));
    let job = Job {
        format,
        path: PathBuf::from(&manifest.source.path),
        index: manifest.source.index,
        rate: f64::from(rate),
        block: opts.block,
        frames: frames as u32,
        src_channels: src_channels as u32,
        in_file: in_file.to_path_buf(),
        out_file: out_file.clone(),
        events,
    };
    let job_file = work.join(format!("{tag}.job.json"));
    std::fs::write(&job_file, serde_json::to_string(&job).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let result: Result<RenderInfo, String> = probe::run_helper(&opts.helper, &["__render".into(), job_file.as_os_str().to_owned()], opts.timeout, env);
    let _ = std::fs::remove_file(&job_file);
    let info = result.map_err(|e| format!("{tag} ({}): {e}", manifest.source.path))?;
    let audio = read_raw_channels(&out_file, info.out_channels as usize);
    let _ = std::fs::remove_file(&out_file);
    Ok((info, audio?))
}

/// Compare two aligned renders.
pub fn compare(a: &[Vec<f32>], b: &[Vec<f32>], tolerance: f64) -> Vec<ChannelReport> {
    a.iter()
        .zip(b)
        .enumerate()
        .map(|(channel, (x, y))| {
            let mut max = 0f64;
            let mut max_at = 0;
            let mut sum_sq = 0f64;
            let mut first_differing = None;
            let mut first_over = None;
            let mut over = 0;
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                let d = (f64::from(*p) - f64::from(*q)).abs();
                let differs = p.to_bits() != q.to_bits() && !(p.is_nan() && q.is_nan());
                // A NaN or an infinity on one side only is a difference of any size.
                let d = if d.is_nan() { f64::INFINITY } else { d };
                if d > max {
                    max = d;
                    max_at = i;
                }
                sum_sq += if d.is_finite() { d * d } else { f64::INFINITY };
                if differs && first_differing.is_none() {
                    first_differing = Some(SamplePair { index: i, original: *p, port: *q });
                }
                if d > tolerance {
                    over += 1;
                    if first_over.is_none() {
                        first_over = Some(SamplePair { index: i, original: *p, port: *q });
                    }
                }
            }
            let rms = (sum_sq / x.len().max(1) as f64).sqrt();
            ChannelReport {
                channel,
                max_abs_diff: max,
                max_at,
                rms_diff_db: (rms > 0.0).then(|| 20.0 * rms.log10()),
                first_differing,
                first_over_tolerance: first_over,
                samples_over_tolerance: over,
            }
        })
        .collect()
}

fn latency_aligned(out: &[Vec<f32>], latency: usize, frames: usize) -> Vec<Vec<f32>> {
    out.iter().map(|c| c.iter().skip(latency).take(frames).copied().collect()).collect()
}

/// Run the whole null test.
pub fn run_null(opts: &NullOptions) -> Result<NullReport, String> {
    if opts.block == 0 {
        return Err("--block must be at least 1".into());
    }
    let mut notes = Vec::new();
    let orig_m = scan_one(opts, &opts.original, &opts.original_format, &opts.env_original)?;
    let port_m = scan_one(opts, &opts.port, &opts.port_format, &opts.env_port)?;
    if orig_m.source.count > 1 {
        notes.push(format!("the original bundle holds {} plugins; the first one, {:?}, is rendered", orig_m.source.count, orig_m.plugin.name));
    }
    if port_m.source.count > 1 {
        notes.push(format!("the port bundle holds {} plugins; the first one, {:?}, is rendered", port_m.source.count, port_m.plugin.name));
    }

    let input = signal::from_spec(&opts.input, opts.rate)?;
    let rate = input.rate;
    let frames = input.frames();
    if frames == 0 {
        return Err("the input has no samples".into());
    }

    // Automation, resolved to each plugin's own parameter ids.
    let map = match &opts.param_map {
        Some(p) => read_param_map(p)?,
        None => Vec::new(),
    };
    let entries = match &opts.automation {
        Some(spec) => parse_automation(spec, rate)?,
        None => Vec::new(),
    };
    let mut orig_events = Vec::new();
    let mut port_events = Vec::new();
    let mut parameter_map = Vec::new();
    for e in &entries {
        let o = find_param(&orig_m.plugin.params, &e.param).ok_or_else(|| format!("the original has no parameter {:?}", e.param))?;
        let p = match_param(o, &port_m.plugin.params, &map)
            .ok_or_else(|| format!("the port has no parameter for the original's {} ({}): add it to --param-map", o.id, o.name))?;
        let line = format!("{} ({}) -> {} ({})", o.id, o.name, p.id, p.name);
        if !parameter_map.contains(&line) {
            parameter_map.push(line);
        }
        orig_events.push(Event { frame: e.frame, param: o.id, value: e.value });
        port_events.push(Event { frame: e.frame, param: p.id, value: e.value });
    }

    let (work, own_work) = match &opts.work_dir {
        Some(w) => (w.clone(), false),
        None => {
            static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            (std::env::temp_dir().join(format!("openplugin-port-null-{}-{n}", std::process::id())), true)
        }
    };
    std::fs::create_dir_all(&work).map_err(|e| format!("cannot create {}: {e}", work.display()))?;
    let in_file = work.join("input.f32");
    write_raw(&in_file, &input.channels)?;

    let r1 = render_side(opts, &work, "original", &orig_m, orig_events, &in_file, input.channels.len(), frames, rate, &opts.env_original);
    let r2 = render_side(opts, &work, "port", &port_m, port_events, &in_file, input.channels.len(), frames, rate, &opts.env_port);
    let _ = std::fs::remove_file(&in_file);
    if own_work {
        let _ = std::fs::remove_dir(&work);
    }
    let (info_o, out_o) = r1?;
    let (info_p, out_p) = r2?;

    if info_o.out_channels != info_p.out_channels {
        return Err(format!("the plugins have different output channel counts: original {}, port {}", info_o.out_channels, info_p.out_channels));
    }
    if info_o.in_channels != info_p.in_channels {
        notes.push(format!("input channel counts differ: original {}, port {}", info_o.in_channels, info_p.in_channels));
    }
    let a = latency_aligned(&out_o, info_o.latency as usize, frames);
    let b = latency_aligned(&out_p, info_p.latency as usize, frames);
    let channels = compare(&a, &b, opts.tolerance);
    let pass = channels.iter().all(|c| c.samples_over_tolerance == 0);

    if let Some(dir) = &opts.save_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let diff: Vec<Vec<f32>> = a.iter().zip(&b).map(|(x, y)| x.iter().zip(y).map(|(p, q)| p - q).collect()).collect();
        wav::write(&dir.join("input.wav"), &input)?;
        wav::write(&dir.join("original.wav"), &Audio { rate, channels: a })?;
        wav::write(&dir.join("port.wav"), &Audio { rate, channels: b })?;
        wav::write(&dir.join("difference.wav"), &Audio { rate, channels: diff })?;
    }

    let side = |m: &Manifest, i: &RenderInfo| Side {
        path: m.source.path.clone(),
        format: m.source.format.clone(),
        name: m.plugin.name.clone(),
        latency_samples: i.latency,
        input_channels: i.in_channels,
        output_channels: i.out_channels,
    };
    Ok(NullReport {
        original: side(&orig_m, &info_o),
        port: side(&port_m, &info_p),
        rate,
        block: opts.block,
        frames,
        tolerance: opts.tolerance,
        latency_differs: info_o.latency != info_p.latency,
        channels,
        parameter_map,
        notes,
        pass,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_parses() {
        let a = parse_automation("0:Gain=-3, 0.5s:Gain=6;100ms:Warm=1\n# c\n700:3 x=2", 48000).unwrap();
        assert_eq!(a.len(), 4);
        assert_eq!((a[0].frame, a[0].param.as_str(), a[0].value), (0, "Gain", -3.0));
        assert_eq!(a[1].frame, 24000);
        assert_eq!(a[2].frame, 4800);
        assert_eq!(a[3].param, "3 x");
        assert!(parse_automation("Gain=1", 48000).is_err());
        assert!(parse_automation("0:Gain", 48000).is_err());
        assert!(parse_automation("0:Gain=x", 48000).is_err());
    }

    #[test]
    fn compare_reports_the_first_difference() {
        let a = vec![vec![0.0, 0.5, 1.0, 1.0]];
        let b = vec![vec![0.0, 0.5, 1.001, 1.0]];
        let r = &compare(&a, &b, 1e-6)[0];
        let first = r.first_over_tolerance.as_ref().unwrap();
        assert_eq!((first.index, first.original, first.port), (2, 1.0, 1.001));
        assert_eq!(r.samples_over_tolerance, 1);
        assert_eq!(r.max_at, 2);
        assert!(r.rms_diff_db.is_some());
        let same = &compare(&a, &a, 1e-6)[0];
        assert_eq!((same.max_abs_diff, same.rms_diff_db.is_none(), same.first_differing.is_none()), (0.0, true, true));
    }
}
