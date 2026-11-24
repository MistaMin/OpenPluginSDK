//! Offline rendering of one plugin in a child process, for the null test.
//!
//! The parent writes a [`Job`] (JSON) and the input audio (raw planar `f32`);
//! the child, started as `openplugin-port __render <job.json>`, loads the
//! plugin with its format's host code, feeds it the input in blocks with the
//! parameter events at their sample offsets, and writes the output as raw
//! planar `f32`. The plugin's reported latency is read after activation and
//! that many extra frames of silence are rendered, so the parent can align the
//! output.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::probe::{FAULT_ENV, emit_result};
use crate::{au, clap, vst3};

/// Test hook: a number the child multiplies its output by (a deliberately changed port).
pub const GAIN_ENV: &str = "OPENPLUGIN_PORT_RENDER_GAIN";

/// One parameter change on the render timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Frame of the input timeline at which the value takes effect.
    pub frame: u64,
    /// The plugin's own parameter id.
    pub param: u32,
    /// The plain value, in the plugin's own units.
    pub value: f64,
}

/// The render child's input, passed as a JSON file. Built by [`crate::null::run_null`].
///
/// ```
/// use openplugin_port::render::{Event, Job};
///
/// let job = Job {
///     format: "clap".into(),
///     path: "Verb.clap".into(),
///     index: 0,
///     rate: 48_000.0,
///     block: 512,
///     frames: 48_000,
///     src_channels: 2,
///     in_file: "input.f32".into(),
///     out_file: "original.f32".into(),
///     events: vec![Event { frame: 9_600, param: 3, value: 0.5 }, Event { frame: 0, param: 3, value: 0.0 }],
/// };
/// assert_eq!(job.sorted_events()[0].frame, 0);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// `clap`, `vst3` or `au`.
    pub format: String,
    /// Absolute path of the plugin bundle or file.
    pub path: PathBuf,
    /// Which plugin of the bundle (the manifest's `source.index`).
    pub index: u32,
    /// Sample rate, in Hz.
    pub rate: f64,
    /// Block size, in frames.
    pub block: u32,
    /// Frames of input.
    pub frames: u32,
    /// Channels in the input file.
    pub src_channels: u32,
    /// Raw planar little-endian `f32` input, `src_channels × frames` samples.
    pub in_file: PathBuf,
    /// Where the child writes its output, raw planar little-endian `f32`.
    pub out_file: PathBuf,
    /// Parameter changes, in any order.
    pub events: Vec<Event>,
}

/// What the render child reports besides the audio.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RenderInfo {
    /// Latency the plugin reported after activation, in samples.
    pub latency: u32,
    /// Input channels the plugin was given.
    pub in_channels: u32,
    /// Output channels written.
    pub out_channels: u32,
    /// Frames written: the input frames plus the latency.
    pub out_frames: u32,
    /// Things worth knowing about the render.
    pub notes: Vec<String>,
}

/// The input as the plugin's host loop reads it.
pub struct Feed {
    src: Vec<Vec<f32>>,
    /// Frames of input; [`Feed::fill`] writes silence past them.
    pub frames: usize,
}

impl Feed {
    /// Fill `dst` with plugin input channel `ch` from frame `start`; silence beyond the input.
    /// Plugin channel `c` takes source channel `c % source channels`.
    pub fn fill(&self, ch: usize, start: usize, dst: &mut [f32]) {
        dst.fill(0.0);
        if self.src.is_empty() {
            return;
        }
        let src = &self.src[ch % self.src.len()];
        if start < src.len() {
            let n = dst.len().min(src.len() - start);
            dst[..n].copy_from_slice(&src[start..start + n]);
        }
    }
}

/// What a format's host loop returns.
pub struct Rendered {
    /// Latency reported after activation, in samples.
    pub latency: u32,
    /// Input channels the plugin was given.
    pub in_channels: u32,
    /// `out_frames` long per channel.
    pub out: Vec<Vec<f32>>,
}

impl Job {
    /// Events sorted by frame.
    pub fn sorted_events(&self) -> Vec<Event> {
        let mut e = self.events.clone();
        e.sort_by_key(|e| e.frame);
        e
    }

    /// Called once per block by every host loop: the test faults.
    pub fn checkpoint(&self, block_index: usize) {
        if block_index == 1 {
            match std::env::var(FAULT_ENV).as_deref() {
                Ok("crash-render") => std::process::abort(),
                Ok("hang-render") => loop {
                    std::thread::sleep(Duration::from_secs(3600));
                },
                _ => {}
            }
        }
    }
}

fn read_raw(path: &Path, channels: usize) -> Result<Vec<Vec<f32>>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let all: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    if channels == 0 {
        return Ok(Vec::new());
    }
    let n = all.len() / channels;
    Ok((0..channels).map(|c| all[c * n..(c + 1) * n].to_vec()).collect())
}

/// Write channels as raw planar little-endian `f32`, one channel after the other. `Err` with a
/// message when the file cannot be written.
///
/// ```
/// use openplugin_port::render::{read_raw_channels, write_raw};
///
/// let path = std::env::temp_dir().join(format!("op-port-raw-doc-{}.f32", std::process::id()));
/// write_raw(&path, &[vec![0.0, 0.5], vec![-0.5, 1.0]]).unwrap();
/// assert_eq!(read_raw_channels(&path, 2).unwrap(), [vec![0.0, 0.5], vec![-0.5, 1.0]]);
/// std::fs::remove_file(&path).unwrap();
/// ```
pub fn write_raw(path: &Path, channels: &[Vec<f32>]) -> Result<(), String> {
    let mut bytes = Vec::new();
    for ch in channels {
        for s in ch {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
    }
    std::fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Read a raw planar file written by [`write_raw`] as `channels` equal channels; trailing samples
/// that do not fill a frame are dropped, and `channels == 0` reads nothing. `Err` with a message
/// when the file cannot be read.
pub fn read_raw_channels(path: &Path, channels: usize) -> Result<Vec<Vec<f32>>, String> {
    read_raw(path, channels)
}

/// The child's entry point: `__render <job.json>`. Never returns.
pub fn child_main(args: &[String]) -> ! {
    let result = child_render(args);
    emit_result(&result)
}

fn child_render(args: &[String]) -> Result<RenderInfo, String> {
    let job_path = args.first().ok_or("usage: __render <job.json>")?;
    let text = std::fs::read_to_string(job_path).map_err(|e| format!("cannot read the job: {e}"))?;
    let job: Job = serde_json::from_str(&text).map_err(|e| format!("unreadable job: {e}"))?;
    match std::env::var(FAULT_ENV).as_deref() {
        Ok("crash") => std::process::abort(),
        Ok("hang") => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        _ => {}
    }
    let feed = Feed { src: read_raw(&job.in_file, job.src_channels as usize)?, frames: job.frames as usize };
    let rendered = match job.format.as_str() {
        "clap" => clap::render(&job, &feed)?,
        "vst3" => vst3::render(&job, &feed)?,
        "au" => au::render(&job, &feed)?,
        other => return Err(format!("cannot render format {other}: use clap, vst3 or au")),
    };
    let mut out = rendered.out;
    if let Some(g) = std::env::var(GAIN_ENV).ok().and_then(|v| v.parse::<f32>().ok()) {
        for ch in out.iter_mut() {
            for s in ch.iter_mut() {
                *s *= g;
            }
        }
    }
    write_raw(&job.out_file, &out)?;
    Ok(RenderInfo {
        latency: rendered.latency,
        in_channels: rendered.in_channels,
        out_channels: out.len() as u32,
        out_frames: out.first().map_or(0, Vec::len) as u32,
        notes: Vec::new(),
    })
}
