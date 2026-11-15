//! Partitioned convolution against direct time-domain convolution (f64).
//!
//! Tolerance: IRs are random in [-1, 1] scaled by 1/sqrt(len) and the input is
//! random in [-1, 1], so outputs are O(1). f32 FFT convolution is held to an
//! absolute error of 5e-6 (measured worst: 3e-7).

use openplugin_core::worker::loader;
use openplugin_dsp::partitioned::{ConvolverConfig, Layout};
use std::time::{Duration, Instant};

const TOL: f32 = 5.0e-6;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 23) as f32) - 1.0
    }
    fn vec(&mut self, n: usize, scale: f32) -> Vec<f32> {
        (0..n).map(|_| self.next() * scale).collect()
    }
}

fn ir(rng: &mut Rng, len: usize) -> Vec<f32> {
    rng.vec(len, 1.0 / (len as f32).sqrt())
}

fn direct(h: &[f32], x: &[f32]) -> Vec<f64> {
    let mut y = vec![0.0f64; x.len()];
    for (n, out) in y.iter_mut().enumerate() {
        let taps = h.len().min(n + 1);
        *out = (0..taps).map(|k| h[k] as f64 * x[n - k] as f64).sum();
    }
    y
}

fn worst(got: &[f32], want: &[f64]) -> f32 {
    got.iter()
        .zip(want)
        .map(|(g, w)| (*g as f64 - w).abs() as f32)
        .fold(0.0, f32::max)
}

fn blocks<'a>(len: usize, block: usize) -> impl Iterator<Item = (usize, usize)> + 'a {
    (0..len)
        .step_by(block)
        .map(move |s| (s, (s + block).min(len)))
}

const IR_LENS: [usize; 8] = [1, 40, 64, 65, 600, 4096, 5000, 17000];
const BLOCKS: [usize; 5] = [1, 7, 64, 512, 4096];
const INPUT: usize = 9000;

fn configs(layout: Layout, max: usize) -> Vec<ConvolverConfig> {
    vec![
        ConvolverConfig::new(layout, max),
        ConvolverConfig::uniform(layout, 64, max).unwrap(),
        ConvolverConfig::new(layout, max)
            .with_stages(&[32, 256])
            .unwrap(),
    ]
}

#[test]
fn mono_matches_direct_convolution() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let x = rng.vec(INPUT, 1.0);
    let mut max_error = 0.0f32;
    for &len in &IR_LENS {
        let h = ir(&mut rng, len);
        let want = direct(&h, &x);
        for config in configs(Layout::Mono, len) {
            let ir = config.build_ir(&[&h]).unwrap();
            for &block in &BLOCKS {
                let mut conv = config.build().unwrap();
                let mut y = x.clone();
                for (a, b) in blocks(y.len(), block) {
                    conv.process_mono(&ir, &mut y[a..b]);
                }
                let e = worst(&y, &want);
                max_error = max_error.max(e);
                assert!(
                    e < TOL,
                    "len {len} block {block} stages {:?}: error {e}",
                    config.stages()
                );
            }
        }
    }
    eprintln!("mono worst absolute error over all cases: {max_error:e}");
}

#[test]
fn zero_latency_impulse_is_immediate() {
    let config = ConvolverConfig::new(Layout::Mono, 10_000);
    let mut h = vec![0.0f32; 10_000];
    h[0] = 1.0;
    h[9_999] = 0.5;
    let ir = config.build_ir(&[&h]).unwrap();
    let mut conv = config.build().unwrap();
    assert_eq!(conv.latency(), 0);
    let mut x = vec![0.0f32; 1];
    x[0] = 1.0;
    conv.process_mono(&ir, &mut x);
    assert!((x[0] - 1.0).abs() < 1e-6);
}

#[test]
fn stereo_channels_are_independent() {
    let mut rng = Rng(77);
    let (xl, xr) = (rng.vec(INPUT, 1.0), rng.vec(INPUT, 1.0));
    for &len in &[100usize, 5000, 12000] {
        let (hl, hr) = (ir(&mut rng, len), ir(&mut rng, len / 2 + 1));
        let (wl, wr) = (direct(&hl, &xl), direct(&hr, &xr));
        let config = ConvolverConfig::new(Layout::Stereo, len);
        let ir = config.build_ir(&[&hl, &hr]).unwrap();
        for &block in &BLOCKS {
            let mut conv = config.build().unwrap();
            let (mut l, mut r) = (xl.clone(), xr.clone());
            for (a, b) in blocks(l.len(), block) {
                conv.process_stereo(&ir, &mut l[a..b], &mut r[a..b]);
            }
            assert!(
                worst(&l, &wl) < TOL && worst(&r, &wr) < TOL,
                "len {len} block {block}"
            );
        }
    }
}

#[test]
fn true_stereo_matrix() {
    let mut rng = Rng(4242);
    let (xl, xr) = (rng.vec(INPUT, 1.0), rng.vec(INPUT, 1.0));
    for &len in &[30usize, 700, 5000, 9000] {
        // Distinct lengths so a swapped pair cannot hide.
        let ll = ir(&mut rng, len);
        let lr = ir(&mut rng, len * 3 / 4 + 1);
        let rl = ir(&mut rng, len / 2 + 1);
        let rr = ir(&mut rng, len);
        let sum = |a: Vec<f64>, b: Vec<f64>| -> Vec<f64> {
            a.iter().zip(&b).map(|(p, q)| p + q).collect()
        };
        // XY: input X to output Y.
        let want_l = sum(direct(&ll, &xl), direct(&rl, &xr));
        let want_r = sum(direct(&lr, &xl), direct(&rr, &xr));
        let config = ConvolverConfig::new(Layout::TrueStereo, len);
        let ir = config.build_ir(&[&ll, &lr, &rl, &rr]).unwrap();
        for &block in &BLOCKS {
            let mut conv = config.build().unwrap();
            let (mut l, mut r) = (xl.clone(), xr.clone());
            for (a, b) in blocks(l.len(), block) {
                conv.process_stereo(&ir, &mut l[a..b], &mut r[a..b]);
            }
            let (el, er) = (worst(&l, &want_l), worst(&r, &want_r));
            assert!(el < TOL && er < TOL, "len {len} block {block}: {el} {er}");
        }
    }
}

#[test]
fn irregular_block_sizes_match() {
    let mut rng = Rng(9);
    let x = rng.vec(20_000, 1.0);
    let h = ir(&mut rng, 9000);
    let want = direct(&h, &x);
    let config = ConvolverConfig::new(Layout::Mono, 9000);
    let ir = config.build_ir(&[&h]).unwrap();
    let mut conv = config.build().unwrap();
    let mut y = x.clone();
    let sizes = [1usize, 3, 63, 65, 127, 1000, 4097, 2, 11, 333, 5];
    let (mut at, mut i) = (0, 0);
    while at < y.len() {
        let end = (at + sizes[i % sizes.len()]).min(y.len());
        conv.process_mono(&ir, &mut y[at..end]);
        at = end;
        i += 1;
    }
    assert!(worst(&y, &want) < TOL);
}

#[test]
fn shorter_impulse_swap_and_reset() {
    let mut rng = Rng(5);
    let x = rng.vec(14_000, 1.0);
    let (long, short) = (ir(&mut rng, 5000), ir(&mut rng, 100));
    let config = ConvolverConfig::new(Layout::Mono, 5000);
    let (a, b) = (
        config.build_ir(&[&long]).unwrap(),
        config.build_ir(&[&short]).unwrap(),
    );
    // Swapping the impulse at a block boundary keeps the input history: the
    // result from then on is the new impulse applied to the whole past input.
    let mut conv = config.build().unwrap();
    let mut y = x.clone();
    conv.process_mono(&a, &mut y[..3000]);
    conv.process_mono(&b, &mut y[3000..]);
    let want = direct(&short, &x);
    // The stage results already computed with the old impulse for the current
    // partition block are still used, so compare after one largest block.
    let late = 3000 + 2 * 4096;
    assert!(worst(&y[late..], &want[late..]) < TOL);

    conv.reset();
    let mut z = x.clone();
    conv.process_mono(&a, &mut z);
    assert!(
        worst(&z, &direct(&long, &x)) < TOL,
        "reset restores a clean start"
    );
}

#[test]
fn wrong_use_is_refused_not_panicking() {
    let mono = ConvolverConfig::new(Layout::Mono, 100);
    let other = ConvolverConfig::new(Layout::Mono, 200);
    assert!(mono.build_ir(&[&[0.0; 101]]).is_err());
    assert!(mono.build_ir(&[&[0.0; 5], &[0.0; 5]]).is_err());
    assert!(mono.build_ir(&[&[f32::NAN]]).is_err());
    assert!(ConvolverConfig::new(Layout::Mono, 0).build().is_err());
    assert!(
        ConvolverConfig::new(Layout::Mono, 9)
            .with_stages(&[100])
            .is_err()
    );
    assert!(
        ConvolverConfig::new(Layout::Mono, 9)
            .with_stages(&[64, 64])
            .is_err()
    );
    let ir = other.build_ir(&[&[1.0]]).unwrap();
    let mut conv = mono.build().unwrap();
    let mut buf = [0.5f32; 8];
    conv.process_mono(&ir, &mut buf); // different configuration
    assert_eq!(buf, [0.5; 8]);
    let ok = mono.build_ir(&[&[1.0]]).unwrap();
    conv.process(&ok, &mut []); // wrong channel count
    conv.process_mono(&ok, &mut []); // empty
}

#[test]
fn nan_input_does_not_poison_the_tail() {
    let mut rng = Rng(3);
    let h = ir(&mut rng, 6000);
    let config = ConvolverConfig::new(Layout::Mono, 6000);
    let ir = config.build_ir(&[&h]).unwrap();
    let mut conv = config.build().unwrap();
    let mut bad = [f32::NAN, f32::INFINITY, 1.0e-40, -1.0e38];
    conv.process_mono(&ir, &mut bad);
    for _ in 0..40 {
        let mut z = vec![0.0f32; 512];
        conv.process_mono(&ir, &mut z);
        assert!(
            z.iter()
                .all(|v| v.is_finite() && (*v == 0.0 || v.abs() >= 1.0e-30))
        );
    }
}

/// The impulse is transformed on the worker thread and swapped in during
/// playback; the result equals direct convolution with the new impulse.
#[test]
fn impulse_prepared_off_thread_and_swapped_in() {
    let mut rng = Rng(11);
    let config = ConvolverConfig::new(Layout::Stereo, 48_000);
    let mut conv = config.build().unwrap();
    let unit = config.build_ir(&[&[1.0], &[1.0]]).unwrap();
    let (mut loader, mut slot) = loader(unit, 2);

    let (hl, hr) = (ir(&mut rng, 30_000), ir(&mut rng, 20_000));
    let (jl, jr, cfg) = (hl.clone(), hr.clone(), config.clone());
    loader
        .submit(move || cfg.build_ir(&[&jl, &jr]).unwrap())
        .unwrap();

    // Play silence until the impulse arrives, then an input whose reference
    // starts at the swap block.
    let start = Instant::now();
    let mut swapped = false;
    while !swapped {
        swapped = slot.poll();
        let (mut l, mut r) = ([0.0f32; 64], [0.0f32; 64]);
        conv.process_stereo(slot.get(), &mut l, &mut r);
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "impulse never arrived"
        );
        std::thread::sleep(Duration::from_micros(200));
    }
    conv.reset();
    let (xl, xr) = (rng.vec(8000, 1.0), rng.vec(8000, 1.0));
    let (wl, wr) = (direct(&hl, &xl), direct(&hr, &xr));
    let (mut l, mut r) = (xl.clone(), xr.clone());
    for (a, b) in blocks(l.len(), 64) {
        slot.poll();
        conv.process_stereo(slot.get(), &mut l[a..b], &mut r[a..b]);
    }
    assert!(worst(&l, &wl) < TOL && worst(&r, &wr) < TOL);
}
