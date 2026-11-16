//! CPU cost of the linear-phase oversampler, the true-peak clipper, the block
//! convolver at small calls and the K-meter's worst block. Ignored by default;
//! run with
//! `cargo test --release -p openplugin-dsp --test dsp_speed_bench -- --ignored --nocapture`.
//!
//! "Load" is the share of one core: processing time over the audio's duration,
//! the fastest of five runs so that other work on the machine does not count.

use openplugin_dsp::{BlockConvolver, KMeter, KScale, LinearPhaseOversampler, TruePeakClipper};
use std::time::Instant;

fn noise(frames: usize, seed: u32) -> Vec<f32> {
    let mut s = seed.wrapping_mul(747_796_405).wrapping_add(1);
    (0..frames)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / (1u32 << 24) as f32 * 0.5 - 0.25
        })
        .collect()
}

fn fastest(mut run: impl FnMut() -> f64) -> f64 {
    (0..5).map(|_| run()).fold(f64::INFINITY, f64::min)
}

/// Seconds of wall time per second of stereo audio in 512-frame blocks.
fn time_stereo(rate: f32, seconds: f32, mut block: impl FnMut(&[&[f32]], &mut [&mut [f32]])) -> f64 {
    fastest(|| time_stereo_once(rate, seconds, &mut block))
}

fn time_stereo_once(rate: f32, seconds: f32, block: &mut impl FnMut(&[&[f32]], &mut [&mut [f32]])) -> f64 {
    let frames = 512;
    let left = noise(frames, 1);
    let right = noise(frames, 2);
    let mut out = [vec![0.0f32; frames], vec![0.0f32; frames]];
    let blocks = (rate * seconds / frames as f32) as usize;
    let start = Instant::now();
    for _ in 0..blocks {
        let [a, b] = &mut out;
        block(&[&left, &right], &mut [a, b]);
    }
    start.elapsed().as_secs_f64() / f64::from(seconds)
}

#[test]
#[ignore]
fn oversampler_and_clipper_load() {
    println!("| Case | 48 kHz | 192 kHz |");
    println!("| --- | ---: | ---: |");
    for factor in [2, 4] {
        let load: Vec<f64> = [48_000.0f32, 192_000.0]
            .iter()
            .map(|&rate| {
                let mut os = LinearPhaseOversampler::prepare(factor, 2, 512).unwrap();
                time_stereo(rate, 2.0, |i, o| os.process(i, o, |_| {}).unwrap())
            })
            .collect();
        println!("| LinearPhaseOversampler {factor}x | {:.2} % | {:.2} % |", load[0] * 100.0, load[1] * 100.0);
    }
    let load: Vec<f64> = [48_000.0f32, 192_000.0]
        .iter()
        .map(|&rate| {
            let mut clip = TruePeakClipper::prepare(rate, 4, 2, 512).unwrap();
            clip.set_ceiling_db(-20.0);
            time_stereo(rate, 2.0, |i, o| {
                o[0].copy_from_slice(i[0]);
                o[1].copy_from_slice(i[1]);
                clip.process(o);
            })
        })
        .collect();
    println!("| TruePeakClipper 4x | {:.2} % | {:.2} % |", load[0] * 100.0, load[1] * 100.0);
}

#[test]
#[ignore]
fn block_convolver_small_calls() {
    let fir = noise(1024, 3);
    let input = noise(48_000, 4);
    println!("| Call length | Load, mono 48 kHz |");
    println!("| ---: | ---: |");
    for call in [1usize, 16, 47, 48, 64, 1024] {
        let mut conv = BlockConvolver::new(1);
        conv.set_kernel(&fir);
        let mut buf = input.clone();
        let load = fastest(|| {
            let start = Instant::now();
            for chunk in buf.chunks_mut(call) {
                conv.process(0, chunk);
            }
            start.elapsed().as_secs_f64()
        });
        println!("| {call} | {:.3} % |", load * 100.0);
    }
}

#[test]
#[ignore]
fn kmeter_worst_block() {
    println!("| Case (32-sample blocks, 192 kHz) | mean block | worst block | block duration |");
    println!("| --- | ---: | ---: | ---: |");
    for channels in [2usize, 12] {
        let rate = 192_000.0;
        let block = noise(32, 5);
        let blocks = (rate as usize * 3) / 32;
        // Each block's fastest time over five identical runs: the re-sum lands
        // on the same blocks every run, a scheduler hiccup does not.
        let mut times = vec![f64::INFINITY; blocks];
        for _ in 0..5 {
            let mut meter = KMeter::new(rate, KScale::K20, channels);
            for time in times.iter_mut() {
                let t = Instant::now();
                for ch in 0..channels {
                    meter.process(ch, &block);
                }
                *time = time.min(t.elapsed().as_secs_f64());
            }
        }
        let worst = times.iter().copied().fold(0.0, f64::max);
        let mean = times.iter().sum::<f64>() / blocks as f64;
        println!(
            "| {channels} channels | {:.2} us | {:.1} us | {:.0} us |",
            mean * 1e6,
            worst * 1e6,
            32.0 / rate * 1e6
        );
    }
}
