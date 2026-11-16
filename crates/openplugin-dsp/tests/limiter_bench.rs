//! CPU cost of the true-peak limiter and clipper. Ignored by default; run with
//! `cargo test --release -p openplugin-dsp --test limiter_bench -- --ignored --nocapture`.
//!
//! Each block is wrapped in a minimal `Plugin` and measured with
//! `openplugin_testing::bench` (noise at -12 dBFS into a -20 dB ceiling, so the
//! gain is working). Prints the median load and nanoseconds per sample per
//! channel.

use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo};
use openplugin_dsp::{Limiter, TruePeakClipper};
use openplugin_testing::bench::{bench, BenchOptions};

static PARAMS: [ParamInfo; 0] = [];

const fn info(name: &'static str) -> PluginInfo {
    PluginInfo::new("com.opengrid.bench.limiter", name, "OpenGrid", "0", Category::Utility, &PARAMS)
}

static LIMITER_INFO: PluginInfo = info("Limiter (true peak)");
static SAMPLE_INFO: PluginInfo = info("Limiter (sample peak)");
static CLIPPER_INFO: PluginInfo = info("TruePeakClipper 4x");

struct LimiterBench {
    limiter: Limiter,
    true_peak: bool,
}

impl LimiterBench {
    fn new(true_peak: bool) -> Self {
        Self { limiter: Limiter::prepare(48_000.0, 2, 1.5, 80.0), true_peak }
    }
}

impl Plugin for LimiterBench {
    fn info(&self) -> &'static PluginInfo {
        if self.true_peak {
            &LIMITER_INFO
        } else {
            &SAMPLE_INFO
        }
    }
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.limiter = Limiter::prepare(sample_rate, channels, 1.5, 80.0);
        self.limiter.set_ceiling_db(-20.0);
        self.limiter.set_true_peak(self.true_peak);
    }
    fn reset(&mut self) {
        self.limiter.reset();
    }
    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        false
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let mut parts: [&mut [f32]; 8] = Default::default();
        let n = io.len().min(8);
        for (slot, ch) in parts.iter_mut().zip(io.iter_mut()) {
            *slot = &mut ch[..frames];
        }
        self.limiter.process(&mut parts[..n]);
    }
}

struct ClipperBench {
    clipper: TruePeakClipper,
}

impl Plugin for ClipperBench {
    fn info(&self) -> &'static PluginInfo {
        &CLIPPER_INFO
    }
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.clipper = TruePeakClipper::prepare(sample_rate, 4, channels, max_block).unwrap();
        self.clipper.set_ceiling_db(-20.0);
    }
    fn reset(&mut self) {
        self.clipper.reset();
    }
    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        false
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let mut parts: [&mut [f32]; 8] = Default::default();
        let n = io.len().min(8);
        for (slot, ch) in parts.iter_mut().zip(io.iter_mut()) {
            *slot = &mut ch[..frames];
        }
        self.clipper.process(&mut parts[..n]);
    }
}

fn report<P: Plugin>(make: impl Fn() -> P) {
    let options = BenchOptions {
        sample_rates: vec![48_000.0, 192_000.0],
        block_sizes: vec![64, 512],
        channel_counts: Some(vec![1, 2]),
        warmup_blocks: 200,
        iterations: 3000,
        ..BenchOptions::default()
    };
    let r = bench(make, &options);
    for c in &r.cases {
        let ns = c.median / f64::from(c.sample_rate) * 1.0e9 / c.channels as f64;
        let p99 = c.p99 / f64::from(c.sample_rate) * 1.0e9 / c.channels as f64;
        println!(
            "{:<22} {:>6} Hz block {:>4} {} ch: load {:6.3} %  {:6.1} ns/sample/channel (p99 {:6.1})",
            r.plugin,
            c.sample_rate,
            c.block,
            c.channels,
            100.0 * c.median,
            ns,
            p99
        );
    }
}

#[test]
#[ignore]
fn cost_per_sample() {
    report(|| LimiterBench::new(true));
    report(|| LimiterBench::new(false));
    report(|| ClipperBench { clipper: TruePeakClipper::prepare(48_000.0, 4, 2, 512).unwrap() });
}
