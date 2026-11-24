//! Benchmark harness: CPU time per block as a fraction of real time.
//!
//! `load = time to process a block / (frames / sample_rate)`. A load of 0.01
//! means the block took one percent of the time it lasts. Each case is warmed
//! up, then timed block by block; the report keeps the median and the 99th
//! percentile. The report prints as text or JSON in a fixed order so two
//! commits can be compared with [`compare`].

use std::fmt::Write as _;
use std::hint::black_box;
use std::time::Instant;

use openplugin_core::{Channels, ChannelsMut, IoConfig, Plugin};

use crate::json::{self, Value};
use crate::rng::Rng;

/// The case matrix [`bench()`] runs: every rate × block size × channel count.
///
/// ```
/// use openplugin_testing::bench::BenchOptions;
///
/// let options = BenchOptions { block_sizes: vec![128], iterations: 50, ..BenchOptions::quick() };
/// assert_eq!(options.sample_rates, [48_000.0]);
/// ```
#[derive(Clone, Debug)]
pub struct BenchOptions {
    /// Sample rates to run, in Hz. Default 44.1, 48 and 96 kHz.
    pub sample_rates: Vec<f32>,
    /// Block sizes to run, in frames. Default 32, 64, 256, 512 and 1024.
    pub block_sizes: Vec<usize>,
    /// Channel counts to run with `prepare`. `None` runs each layout the
    /// plugin lists in `supported_layouts`.
    pub channel_counts: Option<Vec<usize>>,
    /// Blocks processed before timing starts.
    pub warmup_blocks: usize,
    /// Blocks timed per case.
    pub iterations: usize,
    /// Only cases whose key contains this text.
    pub filter: Option<String>,
    /// Seed of the noise fed to the plugin. Default `0x0BE05EED`.
    pub seed: u64,
}

impl Default for BenchOptions {
    fn default() -> Self {
        Self {
            sample_rates: vec![44_100.0, 48_000.0, 96_000.0],
            block_sizes: vec![32, 64, 256, 512, 1024],
            channel_counts: None,
            warmup_blocks: 200,
            iterations: 2000,
            filter: None,
            seed: 0x0BE0_5EED,
        }
    }
}

impl BenchOptions {
    /// Fewer cases and iterations, for a smoke run.
    pub fn quick() -> Self {
        Self {
            sample_rates: vec![48_000.0],
            block_sizes: vec![64, 512],
            warmup_blocks: 50,
            iterations: 300,
            ..Self::default()
        }
    }
}

/// One measured case of a [`BenchReport`]. The loads are fractions of real time (`0.01` is
/// one percent: the block took 1 % of the time it lasts), with the timer's own overhead
/// subtracted.
#[derive(Clone, Debug, PartialEq)]
pub struct BenchCase {
    /// Sample rate, in Hz.
    pub sample_rate: f32,
    /// Block size, in frames.
    pub block: usize,
    /// Main-bus channel count given to `prepare`.
    pub channels: usize,
    /// Layout name, or `channels` when the channel count was set by hand.
    pub layout: String,
    /// Median load over the timed blocks.
    pub median: f64,
    /// 99th-percentile load.
    pub p99: f64,
    /// Mean load.
    pub mean: f64,
    /// Fastest block's load.
    pub min: f64,
    /// Slowest block's load.
    pub max: f64,
}

impl BenchCase {
    /// The identity used to match cases between two reports, such as `"48000Hz/64fr/2ch"`.
    /// The layout name is not part of it.
    pub fn key(&self) -> String {
        format!("{}Hz/{}fr/{}ch", self.sample_rate, self.block, self.channels)
    }
}

/// What [`bench()`] measured. Prints as text ([`BenchReport::to_text`]) or JSON
/// ([`BenchReport::to_json`]); a saved JSON report reads back with [`BenchReport::from_json`]
/// as the baseline of [`compare`].
///
/// ```
/// # use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
/// # static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];
/// # static INFO: PluginInfo = PluginInfo::new("com.example.gain", "Gain", "Example", "1.0.0", Category::Utility, &PARAMS);
/// # #[derive(Default)]
/// # struct MyPlugin { db: f32 }
/// # impl Plugin for MyPlugin {
/// #     fn info(&self) -> &'static PluginInfo { &INFO }
/// #     fn prepare(&mut self, _: f32, _: usize, _: usize) {}
/// #     fn reset(&mut self) {}
/// #     fn set_param(&mut self, id: &str, v: f32) -> bool { if id != "gain" { return false; } self.db = PARAMS[0].clamp(v); true }
/// #     fn param(&self, id: &str) -> Option<f32> { (id == "gain").then_some(self.db) }
/// #     fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
/// #         let g = 10f32.powf(self.db / 20.0);
/// #         for c in io { for s in &mut c[..frames] { *s *= g; } }
/// #     }
/// # }
/// use openplugin_testing::bench::{bench, compare, BenchOptions, BenchReport};
///
/// let options = BenchOptions {
///     block_sizes: vec![64],
///     channel_counts: Some(vec![2]),
///     warmup_blocks: 4,
///     iterations: 16,
///     ..BenchOptions::quick()
/// };
/// let report = bench(MyPlugin::default, &options);
/// assert_eq!(report.cases.len(), 1);
/// assert_eq!(report.cases[0].key(), "48000Hz/64fr/2ch");
///
/// let baseline = BenchReport::from_json(&report.to_json()).unwrap();
/// assert_eq!(baseline.cases[0].key(), report.cases[0].key());
/// assert!(!compare(&baseline, &report, 0.25).has_regression());
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct BenchReport {
    /// The plugin's name ([`PluginInfo::name`](openplugin_core::PluginInfo::name)).
    pub plugin: String,
    /// Whether the harness itself was built with debug assertions. Compare only like with like.
    pub debug_build: bool,
    /// Blocks timed per case ([`BenchOptions::iterations`]).
    pub iterations: usize,
    /// One entry per case, layouts outermost, then rates, then block sizes.
    pub cases: Vec<BenchCase>,
}

/// Measure `P` over the matrix in `options`. Build with `--release` for numbers that mean anything.
///
/// `make` is called once to read the layouts and once per case. Runs on the calling thread and
/// takes roughly `(warmup_blocks + iterations) × cases` blocks of the plugin's time; allocates
/// the buffers and the report. See [`BenchReport`] for an example.
pub fn bench<P: Plugin>(make: impl Fn() -> P, options: &BenchOptions) -> BenchReport {
    let probe = make();
    let name = probe.info().name.to_string();
    let layouts: Vec<(String, IoConfig, usize)> = match &options.channel_counts {
        Some(counts) => counts.iter().map(|&c| (format!("{c}ch"), IoConfig::STEREO, c.max(1))).collect(),
        None => probe
            .supported_layouts()
            .iter()
            .map(|l| (l.name(), *l, l.main_channels().max(1)))
            .collect(),
    };
    let by_count = options.channel_counts.is_some();
    drop(probe);
    let overhead = timer_overhead();
    let mut cases = Vec::new();
    for (layout_name, layout, channels) in &layouts {
        for &rate in &options.sample_rates {
            for &block in &options.block_sizes {
                let case = BenchCase {
                    sample_rate: rate,
                    block,
                    channels: *channels,
                    layout: layout_name.clone(),
                    median: 0.0,
                    p99: 0.0,
                    mean: 0.0,
                    min: 0.0,
                    max: 0.0,
                };
                if let Some(f) = &options.filter {
                    if !case.key().contains(f.as_str()) {
                        continue;
                    }
                }
                let mut plugin = make();
                if !plugin.accepts_sample_rate(rate) {
                    continue;
                }
                if by_count {
                    plugin.prepare(rate, block, *channels);
                } else {
                    if !plugin.set_layout(*layout) {
                        continue;
                    }
                    plugin.prepare_layout(rate, block, *layout);
                }
                cases.push(measure_case(&mut plugin, case, overhead, options, layout));
            }
        }
    }
    BenchReport { plugin: name, debug_build: cfg!(debug_assertions), iterations: options.iterations, cases }
}

fn timer_overhead() -> f64 {
    let mut samples = Vec::with_capacity(2001);
    for _ in 0..2001 {
        let t = Instant::now();
        black_box(());
        samples.push(t.elapsed().as_secs_f64());
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    samples[samples.len() / 2]
}

fn measure_case<P: Plugin>(plugin: &mut P, mut case: BenchCase, overhead: f64, options: &BenchOptions, layout: &IoConfig) -> BenchCase {
    let block = case.block;
    let mut rng = Rng::new(options.seed ^ (block as u64) ^ ((case.channels as u64) << 32));
    // A ring of noise blocks, copied into the buffer before each timed call.
    let ring_len = 8;
    let ring: Vec<Vec<Vec<f32>>> = (0..ring_len)
        .map(|_| (0..case.channels).map(|_| (0..block).map(|_| 0.25 * rng.bipolar()).collect()).collect())
        .collect();
    let mut io: Vec<Vec<f32>> = vec![vec![0.0; block]; case.channels];
    let side: Vec<Vec<f32>> = vec![vec![0.1; block]; layout.sidechain_channels()];
    let has_side = layout.sidechain.is_some() && options.channel_counts.is_none();
    let instrument = plugin.note_ports().input;
    // A new note every 100 ms of audio, so a decaying voice is measured playing, not idle.
    let retrigger = (case.sample_rate as usize / 10 / block).max(1);
    let real_time = block as f64 / f64::from(case.sample_rate);
    let mut loads = Vec::with_capacity(options.iterations);
    for i in 0..options.warmup_blocks + options.iterations {
        if instrument && i % retrigger == 0 {
            plugin.note_off(0, 60);
            plugin.note_on(0, 60, 100);
        }
        for (dst, src) in io.iter_mut().zip(&ring[i % ring_len]) {
            dst.copy_from_slice(src);
        }
        // The slices are made outside the timed region.
        let mut channels = ChannelsMut::new(&mut io);
        let side_channels = Channels::new(&side);
        let start = Instant::now();
        if has_side {
            plugin.process_buses(black_box(&mut channels), Some(&side_channels), block);
        } else {
            plugin.process(black_box(&mut channels), block);
        }
        let elapsed = start.elapsed().as_secs_f64();
        black_box(&io);
        if i >= options.warmup_blocks {
            loads.push(((elapsed - overhead).max(0.0)) / real_time);
        }
    }
    loads.sort_by(|a, b| a.total_cmp(b));
    let n = loads.len().max(1);
    case.median = loads.get(n / 2).copied().unwrap_or(0.0);
    case.p99 = loads.get((n * 99 / 100).min(n - 1)).copied().unwrap_or(0.0);
    case.mean = loads.iter().sum::<f64>() / n as f64;
    case.min = loads.first().copied().unwrap_or(0.0);
    case.max = loads.last().copied().unwrap_or(0.0);
    case
}

impl BenchReport {
    /// Fixed-width text: one line per case, load as a percentage of real time.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "bench: {} ({} blocks per case, {} build)",
            self.plugin,
            self.iterations,
            if self.debug_build { "DEBUG" } else { "release" }
        );
        let _ = writeln!(out, "{:<10} {:>6} {:>3} {:<14} {:>10} {:>10} {:>10}", "rate", "block", "ch", "layout", "median %", "p99 %", "max %");
        for c in &self.cases {
            let _ = writeln!(
                out,
                "{:<10} {:>6} {:>3} {:<14} {:>10.4} {:>10.4} {:>10.4}",
                c.sample_rate,
                c.block,
                c.channels,
                c.layout,
                c.median * 100.0,
                c.p99 * 100.0,
                c.max * 100.0
            );
        }
        out
    }

    /// JSON with a fixed key order and fixed precision, so reports diff cleanly.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{{");
        let _ = writeln!(out, "  \"plugin\": {},", json::quote(&self.plugin));
        let _ = writeln!(out, "  \"debug_build\": {},", self.debug_build);
        let _ = writeln!(out, "  \"iterations\": {},", self.iterations);
        let _ = writeln!(out, "  \"cases\": [");
        for (i, c) in self.cases.iter().enumerate() {
            let _ = write!(
                out,
                "    {{\"sample_rate\": {}, \"block\": {}, \"channels\": {}, \"layout\": {}, \"median\": {:.9}, \"p99\": {:.9}, \"mean\": {:.9}, \"min\": {:.9}, \"max\": {:.9}}}",
                c.sample_rate,
                c.block,
                c.channels,
                json::quote(&c.layout),
                c.median,
                c.p99,
                c.mean,
                c.min,
                c.max
            );
            let _ = writeln!(out, "{}", if i + 1 < self.cases.len() { "," } else { "" });
        }
        let _ = writeln!(out, "  ]");
        let _ = writeln!(out, "}}");
        out
    }

    /// Read a report written by [`BenchReport::to_json`]. `Err` carries an English message for
    /// text that is not JSON or lacks `plugin`, `cases` or a case's numbers; a missing
    /// `debug_build` reads as false and a missing `iterations` as 0.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let v = json::parse(text)?;
        let plugin = v.get("plugin").and_then(Value::as_str).ok_or("missing plugin")?.to_string();
        let debug_build = v.get("debug_build").and_then(Value::as_bool).unwrap_or(false);
        let iterations = v.get("iterations").and_then(Value::as_f64).unwrap_or(0.0) as usize;
        let list = v.get("cases").and_then(Value::as_array).ok_or("missing cases")?;
        let mut cases = Vec::new();
        for c in list {
            let num = |k: &str| c.get(k).and_then(Value::as_f64).ok_or(format!("case without {k}"));
            cases.push(BenchCase {
                sample_rate: num("sample_rate")? as f32,
                block: num("block")? as usize,
                channels: num("channels")? as usize,
                layout: c.get("layout").and_then(Value::as_str).unwrap_or("").to_string(),
                median: num("median")?,
                p99: num("p99")?,
                mean: num("mean")?,
                min: num("min")?,
                max: num("max")?,
            });
        }
        Ok(Self { plugin, debug_build, iterations, cases })
    }
}

/// How one case moved between two reports ([`compare`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Verdict {
    /// Within the tolerance, or the change is below the noise floor.
    Same,
    /// The median shrank by more than the tolerance.
    Faster,
    /// The median grew by more than the tolerance: a regression.
    Slower,
}

/// One case present in both reports of a [`Comparison`].
#[derive(Clone, Debug)]
pub struct Row {
    /// The case's [`BenchCase::key`].
    pub key: String,
    /// Median load in the baseline.
    pub base_median: f64,
    /// Median load in the new report.
    pub new_median: f64,
    /// `new_median / base_median`; infinity when the baseline median is zero.
    pub ratio: f64,
    /// Same, faster or slower.
    pub verdict: Verdict,
}

/// The result of [`compare`]: matched cases, unmatched keys, and the tolerance used.
#[derive(Clone, Debug)]
pub struct Comparison {
    /// One row per case in both reports, in the new report's order.
    pub rows: Vec<Row>,
    /// Keys of cases only the baseline has.
    pub only_in_base: Vec<String>,
    /// Keys of cases only the new report has.
    pub only_in_new: Vec<String>,
    /// The tolerance passed to [`compare`] (0.25 is 25 %).
    pub tolerance: f64,
    /// Set when one report is a debug build and the other is not.
    pub build_mismatch: bool,
}

impl Comparison {
    /// The rows whose verdict is [`Verdict::Slower`]. Allocates.
    pub fn regressions(&self) -> Vec<&Row> {
        self.rows.iter().filter(|r| r.verdict == Verdict::Slower).collect()
    }

    /// True when any row is [`Verdict::Slower`].
    pub fn has_regression(&self) -> bool {
        self.rows.iter().any(|r| r.verdict == Verdict::Slower)
    }

    /// Fixed-width text: one line per row, then the unmatched keys and a summary line.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "compare (tolerance {:.0}%{}):", self.tolerance * 100.0, if self.build_mismatch { ", WARNING: debug vs release" } else { "" });
        let _ = writeln!(out, "{:<24} {:>12} {:>12} {:>8}  verdict", "case", "base median %", "new median %", "ratio");
        for r in &self.rows {
            let v = match r.verdict {
                Verdict::Same => "same",
                Verdict::Faster => "faster",
                Verdict::Slower => "SLOWER",
            };
            let _ = writeln!(out, "{:<24} {:>12.4} {:>12.4} {:>8.2}  {v}", r.key, r.base_median * 100.0, r.new_median * 100.0, r.ratio);
        }
        for k in &self.only_in_base {
            let _ = writeln!(out, "{k:<24} only in the baseline");
        }
        for k in &self.only_in_new {
            let _ = writeln!(out, "{k:<24} only in the new report");
        }
        let n = self.regressions().len();
        let _ = writeln!(out, "{}", if n == 0 { "no regression".to_string() } else { format!("{n} case(s) slower") });
        out
    }
}

/// Median load below which a change is noise whatever the ratio (0.0005 % of real time).
const NOISE_FLOOR: f64 = 5e-6;

/// Compare two reports case by case on the median. A case is slower when its
/// median grew by more than `tolerance` (0.25 is 25 %) and by more than a
/// small absolute floor.
pub fn compare(base: &BenchReport, new: &BenchReport, tolerance: f64) -> Comparison {
    let mut rows = Vec::new();
    let mut only_in_new = Vec::new();
    for n in &new.cases {
        match base.cases.iter().find(|b| b.key() == n.key()) {
            Some(b) => {
                let ratio = if b.median > 0.0 { n.median / b.median } else { f64::INFINITY };
                let verdict = if ratio > 1.0 + tolerance && n.median - b.median > NOISE_FLOOR {
                    Verdict::Slower
                } else if ratio < 1.0 / (1.0 + tolerance) && b.median - n.median > NOISE_FLOOR {
                    Verdict::Faster
                } else {
                    Verdict::Same
                };
                rows.push(Row { key: n.key(), base_median: b.median, new_median: n.median, ratio, verdict });
            }
            None => only_in_new.push(n.key()),
        }
    }
    let only_in_base = base
        .cases
        .iter()
        .filter(|b| !new.cases.iter().any(|n| n.key() == b.key()))
        .map(BenchCase::key)
        .collect();
    Comparison { rows, only_in_base, only_in_new, tolerance, build_mismatch: base.debug_build != new.debug_build }
}

/// A command-line front end for a plugin's bench runner.
///
/// Flags: `--json`, `--out FILE`, `--baseline FILE`, `--tolerance 0.25`,
/// `--quick`, `--filter TEXT`, `--iterations N`. Returns a failing exit code
/// when `--baseline` is given and a case is slower.
///
/// Exit codes: 0 success, 1 a regression against the baseline, 2 an unknown argument or a file
/// that cannot be written or read. A `--tolerance` or `--iterations` value that does not parse
/// keeps the default. `--out` writes JSON to the file and prints text. A bench target's `main`
/// passes `std::env::args().skip(1)`; here the arguments are given by hand:
///
/// ```
/// # use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
/// # static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];
/// # static INFO: PluginInfo = PluginInfo::new("com.example.gain", "Gain", "Example", "1.0.0", Category::Utility, &PARAMS);
/// # #[derive(Default)]
/// # struct MyPlugin { db: f32 }
/// # impl Plugin for MyPlugin {
/// #     fn info(&self) -> &'static PluginInfo { &INFO }
/// #     fn prepare(&mut self, _: f32, _: usize, _: usize) {}
/// #     fn reset(&mut self) {}
/// #     fn set_param(&mut self, id: &str, v: f32) -> bool { if id != "gain" { return false; } self.db = PARAMS[0].clamp(v); true }
/// #     fn param(&self, id: &str) -> Option<f32> { (id == "gain").then_some(self.db) }
/// #     fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
/// #         let g = 10f32.powf(self.db / 20.0);
/// #         for c in io { for s in &mut c[..frames] { *s *= g; } }
/// #     }
/// # }
/// use std::process::ExitCode;
///
/// let args = ["--quick", "--iterations", "8", "--filter", "48000Hz/64fr"].map(String::from);
/// let code = openplugin_testing::bench::cli(MyPlugin::default, args);
/// assert_eq!(code, ExitCode::SUCCESS);
/// ```
pub fn cli<P: Plugin>(make: impl Fn() -> P, args: impl IntoIterator<Item = String>) -> std::process::ExitCode {
    let mut opts = BenchOptions::default();
    let (mut as_json, mut out, mut baseline, mut tolerance) = (false, None, None, 0.25f64);
    let args: Vec<String> = args.into_iter().collect();
    if args.iter().any(|a| a == "--quick") {
        opts = BenchOptions::quick();
    }
    let mut i = 0;
    while i < args.len() {
        let next = |i: &mut usize| -> Option<String> {
            *i += 1;
            args.get(*i).cloned()
        };
        match args[i].as_str() {
            "--json" => as_json = true,
            "--quick" => {}
            "--out" => out = next(&mut i),
            "--baseline" => baseline = next(&mut i),
            "--filter" => opts.filter = next(&mut i),
            "--tolerance" => tolerance = next(&mut i).and_then(|s| s.parse().ok()).unwrap_or(tolerance),
            "--iterations" => opts.iterations = next(&mut i).and_then(|s| s.parse().ok()).unwrap_or(opts.iterations),
            other => {
                eprintln!("unknown argument {other}");
                return std::process::ExitCode::from(2);
            }
        }
        i += 1;
    }
    let report = bench(make, &opts);
    let rendered = if as_json { report.to_json() } else { report.to_text() };
    match &out {
        Some(path) => {
            if let Err(e) = std::fs::write(path, report.to_json()) {
                eprintln!("cannot write {path}: {e}");
                return std::process::ExitCode::from(2);
            }
            print!("{}", report.to_text());
        }
        None => print!("{rendered}"),
    }
    if let Some(path) = baseline {
        let base = match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| BenchReport::from_json(&t)) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("cannot read baseline {path}: {e}");
                return std::process::ExitCode::from(2);
            }
        };
        let cmp = compare(&base, &report, tolerance);
        print!("{}", cmp.to_text());
        if cmp.has_regression() {
            return std::process::ExitCode::from(1);
        }
    }
    std::process::ExitCode::SUCCESS
}
