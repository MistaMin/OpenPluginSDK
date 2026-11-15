//! Runs every before/after pair of `docs/PERFORMANCE.md` through the SDK's bench harness and
//! prints the table the guide quotes, then the stability numbers of `docs/STABILITY.md`.
//!
//! ```text
//! cargo run --release -p example-perf-guide              # everything
//! cargo run --release -p example-perf-guide -- perf      # timings only
//! cargo run --release -p example-perf-guide -- stability # stability numbers only
//! cargo run --release -p example-perf-guide -- perf --filter tanh
//! ```
//!
//! Each case is the median of five runs of the harness (2000 blocks each, 48 kHz, 4096 frames, two
//! channels). Use `--release`: the harness says so and the numbers mean nothing otherwise.

use std::process::ExitCode;

use perf_guide::approx::{fast_exp, fast_sin, fast_tanh, fast_tanh_cheap};
use perf_guide::kernels::*;
use perf_guide::stability;
use perf_guide::{Kernel, Rig};

use openplugin_testing::bench::{bench, BenchOptions};

const RATE: f32 = 48_000.0;
const BLOCK: usize = 4096;
const RUNS: usize = 5;

struct Row {
    group: &'static str,
    label: String,
    /// Microseconds per block, median of medians.
    us: f64,
    p99_us: f64,
}

fn options() -> BenchOptions {
    BenchOptions {
        sample_rates: vec![RATE],
        block_sizes: vec![BLOCK],
        channel_counts: Some(vec![2]),
        warmup_blocks: 300,
        iterations: 2000,
        ..BenchOptions::default()
    }
}

fn measure<K: Kernel>(make: impl Fn() -> K) -> (f64, f64) {
    let real_time_us = BLOCK as f64 / f64::from(RATE) * 1.0e6;
    let mut medians = Vec::new();
    let mut p99s = Vec::new();
    for _ in 0..RUNS {
        let report = bench(|| Rig(make()), &options());
        let case = &report.cases[0];
        medians.push(case.median * real_time_us);
        p99s.push(case.p99 * real_time_us);
    }
    medians.sort_by(|a, b| a.total_cmp(b));
    p99s.sort_by(|a, b| a.total_cmp(b));
    (medians[RUNS / 2], p99s[RUNS / 2])
}

fn perf(filter: Option<&str>) {
    let mut rows: Vec<Row> = Vec::new();
    let mut add = |group: &'static str, label: &str, make: &dyn Fn() -> (f64, f64)| {
        // `--filter a,b` keeps a case when its group or label contains any of the words.
        if let Some(f) = filter {
            if !f.split(',').any(|w| group.contains(w) || label.contains(w)) {
                return;
            }
        }
        let (us, p99_us) = make();
        rows.push(Row { group, label: label.to_string(), us, p99_us });
    };

    add("alloc", "Vec per block (before)", &|| measure(|| AllocPerBlock));
    add("alloc", "scratch from prepare (after)", &|| measure(Preallocated::default));

    add("denormal", "normal range, no flush (control)", &|| measure(|| Decay { start: 1.0e-3, flush: false }));
    add("denormal", "subnormal range, no flush (before)", &|| measure(|| Decay { start: 1.0e-36, flush: false }));
    add("denormal", "subnormal range, flushed (after)", &|| measure(|| Decay { start: 1.0e-36, flush: true }));

    add("coefficient", "exp every sample (before)", &|| measure(|| SweptLowpass::new(1)));
    add("coefficient", "exp every 16 samples (after)", &|| measure(|| SweptLowpass::new(16)));

    add("smoothing", "settled: one-pole every sample (before)", &|| measure(|| SmoothedGain::new(SmoothMode::PerSample, false)));
    add("smoothing", "settled: skip when settled (after)", &|| measure(|| SmoothedGain::new(SmoothMode::SettledSkip, false)));
    add("smoothing", "moving: one-pole every sample", &|| measure(|| SmoothedGain::new(SmoothMode::PerSample, true)));
    add("smoothing", "moving: linear ramp per block", &|| measure(|| SmoothedGain::new(SmoothMode::BlockRamp, true)));

    add("layout", "array of structures (before)", &|| measure(PartialsAos::new));
    add("layout", "structure of arrays (after)", &|| measure(PartialsSoa::new));

    add("vectorise", "indexed, bounds checks (before)", &|| measure(CurveIndexed::new));
    add("vectorise", "sliced + zip (after)", &|| measure(CurveZip::default));
    add("vectorise", "chunks_exact(8) (after)", &|| measure(CurveChunks::default));
    add("vectorise", "sliced + zip in f64", &|| measure(CurveZipF64::default));

    add("tanh", "f32::tanh (before)", &|| measure(|| Map { f: f32::tanh, pre: 4.0 }));
    add("tanh", "fast_tanh [7/6] (after)", &|| measure(|| Map { f: fast_tanh, pre: 4.0 }));
    add("tanh", "fast_tanh_cheap [3/2]", &|| measure(|| Map { f: fast_tanh_cheap, pre: 4.0 }));
    add("exp", "f32::exp (before)", &|| measure(|| Map { f: f32::exp, pre: 8.0 }));
    add("exp", "fast_exp (after)", &|| measure(|| Map { f: fast_exp, pre: 8.0 }));
    add("sin", "f32::sin (before)", &|| measure(|| Map { f: f32::sin, pre: 20.0 }));
    add("sin", "fast_sin (after)", &|| measure(|| Map { f: fast_sin, pre: 20.0 }));

    add("oversampling", "1x", &|| measure(|| Sat1x));
    add("oversampling", "2x IIR half-band (Oversampler2x)", &|| measure(SatIir2x::new));
    add("oversampling", "2x linear phase", &|| measure(|| SatLinearPhase::new(2)));
    add("oversampling", "4x linear phase", &|| measure(|| SatLinearPhase::new(4)));

    add("precision", "TDF2 low-pass, f32 state", &|| measure(Tdf2F32::new));
    add("precision", "TDF2 low-pass, f64 state", &|| measure(Tdf2F64::new));

    println!("bench harness: {RUNS} runs of 2000 blocks, {RATE} Hz, {BLOCK} frames, 2 channels; median of medians");
    println!("build: {}", if cfg!(debug_assertions) { "DEBUG (numbers are meaningless)" } else { "release" });
    println!("{:<13} {:<42} {:>10} {:>10} {:>10}", "group", "case", "us/block", "ns/frame", "p99 us");
    for r in &rows {
        println!(
            "{:<13} {:<42} {:>10.3} {:>10.3} {:>10.3}",
            r.group,
            r.label,
            r.us,
            r.us * 1000.0 / BLOCK as f64,
            r.p99_us
        );
    }
}

fn stability_numbers() {
    println!("== Cutoff swept 50 Hz..12 kHz exponentially by an LFO, coefficients every sample (48 kHz, noise +-0.5, 20 s) ==");
    for (q, hz) in [(0.707f32, 5.0f32), (10.0, 5.0), (10.0, 50.0), (40.0, 50.0)] {
        println!("-- Q {q}, LFO {hz} Hz");
        for mut f in stability::all_topologies() {
            let r = stability::sweep_run(f.as_mut(), q, hz, 20);
            println!("   {:<26} peak {:>12.4}   non-finite {}", f.name(), r.peak, r.non_finite);
        }
    }
    println!();
    println!("== Cutoff redrawn at random (worst case) (48 kHz, noise +-0.5, 20 s) ==");
    println!("cutoff redrawn log-uniform 50 Hz..12 kHz every N samples; peak |y| and non-finite count");
    for (q, every) in [(0.707f32, 16usize), (10.0, 16), (10.0, 1), (40.0, 1)] {
        println!("-- Q {q}, new cutoff every {every} sample(s)");
        for mut f in stability::all_topologies() {
            let r = stability::modulation_run(f.as_mut(), q, every, 20);
            println!("   {:<26} peak {:>12.4}   non-finite {}", f.name(), r.peak, r.non_finite);
        }
    }
    println!();
    println!("== Step response of a 20 Hz low-pass at 192 kHz, 4 s: final value (exact: 1) ==");
    for mut f in stability::all_topologies() {
        let y = stability::step_settles_to(f.as_mut(), 192_000.0, 20.0, 4);
        println!("   {:<26} {:.9}   error {:.3e}", f.name(), y, (y - 1.0).abs());
    }
    println!("== Same at 5 Hz, 8 s ==");
    for mut f in stability::all_topologies() {
        let y = stability::step_settles_to(f.as_mut(), 192_000.0, 5.0, 8);
        println!("   {:<26} {:.9}   error {:.3e}", f.name(), y, (y - 1.0).abs());
    }
    println!();
    let (raw, smooth, sine) = stability::zipper();
    println!("== Gain step 0 -> 1 on a 1 kHz sine (largest sample-to-sample change) ==");
    println!("   unsmoothed {raw:.4}   10 ms one-pole {smooth:.4}   the sine itself {sine:.4}");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let what = args.first().map(String::as_str).unwrap_or("all");
    let filter = args.iter().position(|a| a == "--filter").and_then(|i| args.get(i + 1)).map(String::as_str);
    match what {
        "all" => {
            perf(filter);
            println!();
            stability_numbers();
        }
        "perf" => perf(filter),
        "stability" => stability_numbers(),
        "approx" => {
            println!("{:<24} {:<12} {:>14} {:>12} {:>14}", "function", "domain", "max abs error", "at x", "max rel error");
            for e in perf_guide::approx::measure_errors() {
                println!("{:<24} {:<12} {:>14.3e} {:>12.4} {:>14.3e}", e.name, e.domain, e.max_abs, e.at_abs, e.max_rel);
            }
        }
        _ => {
            eprintln!("usage: perf_guide [all|perf|stability|approx] [--filter TEXT]");
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}
