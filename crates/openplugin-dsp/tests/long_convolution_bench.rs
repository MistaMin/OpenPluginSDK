//! CPU cost of long convolution. Ignored by default; run with
//! `cargo test --release -p openplugin-dsp --test long_convolution_bench -- --ignored --nocapture`.
//!
//! Prints, for a 3 s impulse at 48 kHz (and 10 s at 192 kHz), per host block
//! size: mean and worst microseconds per block, and both as a percentage of
//! the real-time budget of that block.

use openplugin_dsp::partitioned::{ConvolverConfig, Layout};
use std::time::Instant;

fn noise(n: usize) -> Vec<f32> {
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 40) as f32 / (1u64 << 23) as f32 - 1.0) / (n as f32).sqrt()
        })
        .collect()
}

fn run(label: &str, config: &ConvolverConfig, rate: f32, len: usize, block: usize) {
    let filters: Vec<Vec<f32>> = (0..config.layout().filters()).map(|_| noise(len)).collect();
    let refs: Vec<&[f32]> = filters.iter().map(|v| v.as_slice()).collect();
    let ir = config.build_ir(&refs).unwrap();
    let mut conv = config.build().unwrap();
    let mut l = vec![0.1f32; block];
    let mut r = vec![0.1f32; block];
    let blocks = (rate * 2.0) as usize / block;
    let (mut mean, mut worst) = (f64::MAX, f64::MAX);
    // Best of five passes: the worst block of a pass is often an OS preemption
    // (other processes on the machine), so the smallest worst-case is reported.
    for _ in 0..5 {
        let mut times = Vec::with_capacity(blocks);
        // Warm up through one full large partition cycle.
        for _ in 0..(8192 / block + 4) {
            conv.process_stereo(&ir, &mut l, &mut r);
        }
        for _ in 0..blocks {
            let t = Instant::now();
            match config.layout() {
                Layout::Mono => conv.process_mono(&ir, &mut l),
                _ => conv.process_stereo(&ir, &mut l, &mut r),
            }
            times.push(t.elapsed().as_secs_f64() * 1.0e6);
        }
        mean = mean.min(times.iter().sum::<f64>() / times.len() as f64);
        worst = worst.min(times.iter().cloned().fold(0.0, f64::max));
    }
    let budget = block as f64 / rate as f64 * 1.0e6;
    println!(
        "{label:<28} block {block:>4}: mean {mean:8.1} us ({:5.2}%)  worst {worst:8.1} us ({:6.1}%)  budget {budget:.0} us",
        100.0 * mean / budget,
        100.0 * worst / budget
    );
}

#[test]
#[ignore]
fn cost_per_block() {
    for (rate, secs) in [(48_000.0f32, 3usize), (192_000.0, 10)] {
        let len = rate as usize * secs;
        println!(
            "--- {secs} s impulse at {} kHz ({len} taps) ---",
            rate / 1000.0
        );
        for layout in [Layout::Mono, Layout::TrueStereo] {
            let nonuniform = ConvolverConfig::new(layout, len);
            let lower = ConvolverConfig::new(layout, len)
                .with_stages(&[64, 512, 2048])
                .unwrap();
            let uniform = ConvolverConfig::uniform(layout, 64, len).unwrap();
            for block in [64usize, 512] {
                run(
                    &format!("{layout:?} 64/512/4096"),
                    &nonuniform,
                    rate,
                    len,
                    block,
                );
                run(&format!("{layout:?} 64/512/2048"), &lower, rate, len, block);
                if rate < 100_000.0 {
                    run(
                        &format!("{layout:?} uniform 64"),
                        &uniform,
                        rate,
                        len,
                        block,
                    );
                }
            }
        }
    }
}
