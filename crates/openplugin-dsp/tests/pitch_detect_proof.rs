//! YIN and MPM against known tones: 50 Hz to 2 kHz at 44.1, 48 and 96 kHz,
//! harmonic tones with a missing fundamental, noisy tones, and noise.

use openplugin_dsp::{MpmDetector, PitchDetector, WhiteNoise, YinDetector};

fn cents(f: f32, reference: f32) -> f32 {
    1200.0 * (f / reference).log2()
}

fn tone(fs: f32, f: f32, partials: &[(f32, f32)], n: usize, phase: f32) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(fs);
            partials
                .iter()
                .map(|&(mult, amp)| {
                    amp * (2.0 * std::f64::consts::PI * f64::from(f * mult) * t + f64::from(phase) * f64::from(mult)).sin() as f32
                })
                .sum::<f32>()
        })
        .collect()
}

fn noise(n: usize, seed: u64) -> Vec<f32> {
    let mut w = WhiteNoise::new(seed);
    (0..n).map(|_| w.next()).collect()
}

const RATES: [f32; 3] = [44_100.0, 48_000.0, 96_000.0];
const FREQS: [f32; 10] = [50.0, 65.4, 82.4, 110.0, 196.0, 330.0, 523.25, 880.0, 1396.9, 2000.0];

struct Stats {
    worst: f32,
    count: usize,
}

fn run(make: &dyn Fn(f32) -> Box<dyn PitchDetector>, partials: &[(f32, f32)], noise_amp: f32, label: &str) -> Stats {
    let mut worst = 0.0f32;
    let mut count = 0;
    let mut min_conf = 1.0f32;
    for &fs in &RATES {
        let mut det = make(fs);
        let n = det.window() + 512;
        for &f in &FREQS {
            for &phase in &[0.0f32, 1.3] {
                let mut x = tone(fs, f, partials, n, phase);
                if noise_amp > 0.0 {
                    for (s, w) in x.iter_mut().zip(noise(n, 7)) {
                        *s += noise_amp * w;
                    }
                }
                let p = det.detect(&x);
                let err = cents(p.frequency, f).abs();
                assert!(p.frequency > 0.0, "{label} fs {fs} f {f}: no estimate");
                if err > worst {
                    worst = err;
                }
                min_conf = min_conf.min(p.confidence);
                count += 1;
            }
        }
    }
    println!("{label}: worst error {worst:.3} cents over {count} cases, min confidence {min_conf:.3}");
    Stats { worst, count }
}

fn yin(fs: f32) -> Box<dyn PitchDetector> {
    Box::new(YinDetector::new(fs, 50.0, 2200.0))
}
fn mpm(fs: f32) -> Box<dyn PitchDetector> {
    Box::new(MpmDetector::new(fs, 50.0, 2200.0))
}

#[test]
fn pure_tones_within_stated_cents() {
    let s = run(&yin, &[(1.0, 0.5)], 0.0, "YIN sine");
    assert!(s.worst < 0.25, "{}", s.worst);
    let s = run(&mpm, &[(1.0, 0.5)], 0.0, "MPM sine");
    assert!(s.worst < 0.25, "{}", s.worst);
    assert_eq!(s.count, 60);
}

#[test]
fn harmonic_tones_find_the_fundamental() {
    let h: Vec<(f32, f32)> = (1..=6).map(|k| (k as f32, 0.3 / k as f32)).collect();
    assert!(run(&yin, &h, 0.0, "YIN harmonic").worst < 1.0);
    assert!(run(&mpm, &h, 0.0, "MPM harmonic").worst < 1.0);
}

#[test]
fn missing_fundamental_is_found() {
    // Partials 2..=6 only: the period is still 1 / f0.
    let h: Vec<(f32, f32)> = (2..=6).map(|k| (k as f32, 0.3)).collect();
    // f0 up to 1396.9 keeps all partials under Nyquist at 44.1 kHz; skip 2 kHz handled by run (partials fold at 44.1k).
    let mut worst_y = 0.0f32;
    let mut worst_m = 0.0f32;
    for &fs in &RATES {
        for &f in &[50.0f32, 82.4, 110.0, 196.0, 330.0, 523.25, 880.0, 1396.9] {
            if f * 6.0 > fs * 0.45 {
                continue;
            }
            for (d, w) in [(&mut YinDetector::new(fs, 50.0, 2200.0) as &mut dyn PitchDetector, &mut worst_y), (&mut MpmDetector::new(fs, 50.0, 2200.0), &mut worst_m)] {
                let x = tone(fs, f, &h, d.window() + 256, 0.4);
                let p = d.detect(&x);
                let e = cents(p.frequency, f).abs();
                assert!(e < 1.5, "fs {fs} f {f}: {} Hz", p.frequency);
                assert!(p.confidence > 0.8, "fs {fs} f {f}: conf {}", p.confidence);
                if e > *w {
                    *w = e;
                }
            }
        }
    }
    println!("missing fundamental: YIN worst {worst_y:.3} cents, MPM worst {worst_m:.3} cents");
}

#[test]
fn noisy_tones_stay_close_and_noise_has_low_confidence() {
    // SNR about 20 dB: tone amplitude 0.5 (rms 0.354), noise rms 0.035.
    let amp = 0.035 / 0.577; // WhiteNoise is uniform +-1 (rms 0.577)
    let s = run(&yin, &[(1.0, 0.5)], amp, "YIN noisy (20 dB)");
    assert!(s.worst < 8.0, "{}", s.worst);
    let s = run(&mpm, &[(1.0, 0.5)], amp, "MPM noisy (20 dB)");
    assert!(s.worst < 8.0, "{}", s.worst);
    for &fs in &RATES {
        for (name, mut d) in [("YIN", yin(fs)), ("MPM", mpm(fs))] {
            let mut worst = 0.0f32;
            for seed in 1..=8 {
                let x = noise(d.window() + 512, seed);
                worst = worst.max(d.detect(&x).confidence);
            }
            println!("{name} noise at {fs}: highest confidence {worst:.3}");
            assert!(worst < 0.3, "{name} {fs}: {worst}");
        }
    }
}

#[test]
fn silence_and_garbage_are_safe() {
    let mut d = YinDetector::new(48_000.0, 50.0, 2000.0);
    let mut m = MpmDetector::new(48_000.0, 50.0, 2000.0);
    let z = vec![0.0f32; 8000];
    assert_eq!(d.detect(&z).frequency, 0.0);
    assert_eq!(m.detect(&z).frequency, 0.0);
    let bad = vec![f32::NAN; 8000];
    let _ = d.detect(&bad);
    let _ = m.detect(&bad);
    let short = vec![0.1f32; 10];
    assert_eq!(d.detect(&short).confidence, 0.0);
    // Absurd constructor arguments.
    let _ = YinDetector::new(f32::NAN, f32::NAN, f32::INFINITY);
    let _ = MpmDetector::new(0.0, -5.0, 0.0);
}
