//! Identity resynthesis: no processing reproduces the input delayed by `latency()`.

use openplugin_dsp::{Complex, Identity, Stft, StftConfig, WindowKind};

fn noise(n: usize) -> Vec<f32> {
    let mut s = 0x1234_5678u32;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32) * 1.8 - 0.9
        })
        .collect()
}

fn run(cfg: StftConfig, block: usize, input: &[f32]) -> (Stft, Vec<f32>) {
    let mut stft = Stft::new(1, cfg).unwrap();
    let mut out = vec![0.0; input.len()];
    let mut i = 0;
    let mut k = 0usize;
    while i < input.len() {
        // Vary the block around its nominal size so call boundaries shift.
        let len = if block > 1 { (block - (k % 3)).max(1) } else { 1 }.min(input.len() - i);
        let (a, b) = (&input[i..i + len], &mut out[i..i + len]);
        stft.process(0, a, b, &mut Identity);
        i += len;
        k += 1;
    }
    (stft, out)
}

#[test]
fn identity_is_a_pure_delay_for_several_pairs_and_blocks() {
    use WindowKind::*;
    let input = noise(20_000);
    // (frame, hop, analysis, synthesis)
    let pairs = [
        (256, 64, Hann, Rectangular),
        (256, 128, Hann, Rectangular),
        (1024, 256, Hann, Rectangular),
        (512, 256, Hamming, Rectangular),
        (1024, 256, BlackmanHarris, Rectangular),
        (512, 128, SqrtHann, SqrtHann),
        (1024, 512, SqrtHann, SqrtHann),
        (2048, 512, Hann, Hann), // WOLA: hann^2 at 75 % overlap
        (512, 1, Hann, Rectangular),
    ];
    let mut worst = 0.0f32;
    for (frame, hop, analysis, synthesis) in pairs {
        let cfg = StftConfig { frame, hop, analysis, synthesis };
        for block in [1, 7, 64, 512] {
            let (stft, out) = run(cfg, block, &input);
            assert!(stft.ola_error().is_cola(1e-5), "{cfg:?}: {:?}", stft.ola_error());
            let lat = stft.latency();
            assert_eq!(lat, frame);
            // Silence in, so only FFT rounding (about 1e-9) may appear.
            for v in out.iter().take(lat) {
                assert!(v.abs() < 1e-6, "{cfg:?} block {block}: output before latency");
            }
            for t in lat..input.len() {
                let e = (out[t] - input[t - lat]).abs();
                worst = worst.max(e);
                assert!(e < 2e-5, "{cfg:?} block {block} t {t}: err {e}");
            }
        }
    }
    println!("worst identity error {worst:e}");
}

#[test]
fn a_bad_pair_is_not_an_identity_and_says_so() {
    let cfg = StftConfig {
        frame: 256,
        hop: 192,
        analysis: WindowKind::Hann,
        synthesis: WindowKind::Rectangular,
    };
    let input = vec![1.0f32; 4000];
    let (stft, out) = run(cfg, 64, &input);
    assert!(stft.ola_error().ripple > 0.5);
    let tail = &out[1000..];
    let (lo, hi) = tail.iter().fold((f32::MAX, f32::MIN), |(l, h), &v| (l.min(v), h.max(v)));
    assert!(hi - lo > 0.3, "expected modulation, got {lo}..{hi}");
}

#[test]
fn processing_in_the_spectrum_filters() {
    // Zero every bin above a quarter of Nyquist: a tone above is removed, a tone below passes.
    let (frame, sr) = (1024usize, 48_000.0f32);
    let mut stft = Stft::new(1, StftConfig::hann(frame)).unwrap();
    let cutoff = frame / 8;
    let mut lowpass = |_: usize, bins: &mut [Complex<f32>]| {
        for b in &mut bins[cutoff..] {
            *b = Complex::new(0.0, 0.0);
        }
    };
    let tone = |f: f32, n: usize| -> Vec<f32> {
        (0..n).map(|i| (std::f64::consts::TAU * f as f64 * i as f64 / sr as f64).sin() as f32).collect()
    };
    let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
    let mut low = tone(1500.0, 16_384);
    stft.process_in_place(0, &mut low, &mut lowpass);
    stft.reset();
    let mut high = tone(9000.0, 16_384);
    stft.process_in_place(0, &mut high, &mut lowpass);
    assert!((rms(&low[4096..]) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "{}", rms(&low[4096..]));
    assert!(rms(&high[4096..]) < 1e-3, "{}", rms(&high[4096..]));
}

#[test]
fn channels_are_independent() {
    let input = noise(5000);
    let mut stft = Stft::new(2, StftConfig::hann(256)).unwrap();
    let mut a = input.clone();
    let mut b = vec![0.0; 5000];
    stft.process_in_place(0, &mut a, &mut Identity);
    stft.process_in_place(1, &mut b, &mut Identity);
    assert!(b.iter().all(|&v| v == 0.0));
    assert!((a[300] - input[300 - 256]).abs() < 2e-5);
}
