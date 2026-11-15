//! Measured proofs for the reverbs: decay time by Schroeder backward
//! integration, stability, echo density, stereo decorrelation.
//!
//! RT60 is measured per octave band: the impulse response is band-limited with
//! a zero-phase FFT mask (octave around the centre, raised-cosine skirts), the
//! energy decay curve is the backward integral of the squared band response
//! (M. R. Schroeder, "New method of measuring reverberation time", JASA 37,
//! 1965), and T30 is a least-squares line over -5 dB to -35 dB, extrapolated
//! to -60 dB (ISO 3382 style evaluation range).

use openplugin_dsp::{FdnMatrix, FdnReverb, PlateReverb};
use rustfft::{FftPlanner, num_complex::Complex};

fn fdn_ir(fs: f32, lines: usize, setup: impl Fn(&mut FdnReverb), seconds: f32) -> (Vec<f32>, Vec<f32>) {
    let mut r = FdnReverb::new(fs, lines);
    setup(&mut r);
    // Let any glide to new delay lengths finish before measuring.
    let mut warm = vec![0.0f32; 2 * fs as usize];
    let mut warm2 = warm.clone();
    r.process(&mut warm, &mut warm2);
    let n = (seconds * fs) as usize;
    let mut l = vec![0.0f32; n];
    let mut rr = vec![0.0f32; n];
    l[0] = 1.0;
    rr[0] = 1.0;
    r.process(&mut l, &mut rr);
    (l, rr)
}

fn band_limit(x: &[f32], fs: f32, centre: f32) -> Vec<f64> {
    let orig = x.len();
    let n = orig + fs as usize;
    let mut planner = FftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut buf: Vec<Complex<f64>> = x.iter().map(|v| Complex::new(f64::from(*v), 0.0)).collect();
    buf.resize(n, Complex::new(0.0, 0.0)); // zero pad so the zero-phase pre-ringing wraps into padding
    fwd.process(&mut buf);
    let lo = f64::from(centre) / 2f64.sqrt();
    let hi = f64::from(centre) * 2f64.sqrt();
    for (k, c) in buf.iter_mut().enumerate() {
        let f = if k <= n / 2 { k as f64 } else { (n - k) as f64 } * f64::from(fs) / n as f64;
        // Raised-cosine skirts a sixth of an octave wide on each edge.
        let edge = |f: f64, e: f64, up: bool| {
            let x = (f / e).log2() / (1.0 / 6.0);
            let t = if up { 0.5 + x } else { 0.5 - x };
            let t = t.clamp(0.0, 1.0);
            0.5 - 0.5 * (std::f64::consts::PI * t).cos()
        };
        *c *= edge(f, lo, true) * edge(f, hi, false);
    }
    inv.process(&mut buf);
    buf[..orig].iter().map(|c| c.re / n as f64).collect()
}

/// Schroeder backward integration; returns T60 from a -5..-35 dB fit, seconds.
fn rt60_of(band: &[f64], fs: f32) -> f64 {
    let mut edc = vec![0.0f64; band.len()];
    let mut acc = 0.0;
    for i in (0..band.len()).rev() {
        acc += band[i] * band[i];
        edc[i] = acc;
    }
    let total = edc[0];
    let db: Vec<f64> = edc.iter().map(|e| 10.0 * (e / total).max(1e-30).log10()).collect();
    let (mut n, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, d) in db.iter().enumerate() {
        if *d <= -5.0 && *d >= -35.0 {
            let t = i as f64 / f64::from(fs);
            n += 1.0;
            sx += t;
            sy += d;
            sxx += t * t;
            sxy += t * d;
        }
    }
    assert!(n > 10.0, "decay never reached the fit range");
    let slope = (n * sxy - sx * sy) / (n * sxx - sx * sx);
    -60.0 / slope
}

fn measure(l: &[f32], fs: f32, centre: f32) -> f64 {
    rt60_of(&band_limit(l, fs, centre), fs)
}

#[test]
fn fdn_rt60_matches_the_setting_at_low_mid_and_high() {
    let rts = [3.0f32, 2.0, 1.0];
    let mut worst = 0.0f64;
    for &fs in &[44_100.0f32, 48_000.0, 96_000.0] {
        for &(lines, mat, modulation) in &[
            (8, FdnMatrix::Householder, 0.0f32),
            (16, FdnMatrix::Hadamard, 0.0),
            (8, FdnMatrix::Householder, 0.25),
        ] {
            let (l, _) = fdn_ir(
                fs,
                lines,
                |r| {
                    r.set_matrix(mat);
                    r.set_modulation(modulation, 0.8);
                    r.set_rt60(rts[0], rts[1], rts[2]);
                },
                7.0,
            );
            for (centre, want) in [(125.0, rts[0]), (1000.0, rts[1]), (8000.0, rts[2])] {
                let got = measure(&l, fs, centre) as f32;
                let err = f64::from((got - want) / want);
                println!("fs {fs} lines {lines} {mat:?} mod {modulation}: {centre} Hz want {want} got {got:.3} ({:+.1} %)", err * 100.0);
                worst = worst.max(err.abs());
                assert!(err.abs() < 0.05, "RT60 off by {:.1} % at {centre} Hz, fs {fs}", err * 100.0);
            }
        }
    }
    println!("worst FDN RT60 error: {:.2} %", worst * 100.0);
}

#[test]
fn fdn_rt60_flat_and_inverted_settings() {
    let fs = 48_000.0;
    for rts in [[1.5f32, 1.5, 1.5], [0.8, 2.0, 3.5], [4.0, 1.0, 0.4]] {
        let (l, _) = fdn_ir(fs, 8, |r| { r.set_modulation(0.0, 1.0); r.set_rt60(rts[0], rts[1], rts[2]) }, 9.0);
        for (centre, want) in [(125.0, rts[0]), (1000.0, rts[1]), (8000.0, rts[2])] {
            let got = measure(&l, fs, centre);
            let err = (got - f64::from(want)) / f64::from(want);
            println!("{rts:?} {centre} Hz got {got:.3} ({:+.1} %)", err * 100.0);
            assert!(err.abs() < 0.08, "{rts:?} at {centre}: {got}");
        }
    }
}

#[test]
fn fdn_size_changes_decay_time_not() {
    // Jot's rule: RT60 is independent of the delay lengths.
    let fs = 48_000.0;
    for size in [0.5f32, 2.0] {
        let (l, _) = fdn_ir(fs, 8, |r| { r.set_modulation(0.0, 1.0); r.set_size(size); r.set_rt60(2.0, 2.0, 2.0) }, 6.0);
        let got = measure(&l, fs, 1000.0);
        println!("size {size}: {got:.3}");
        assert!((got - 2.0).abs() / 2.0 < 0.10, "size {size}: {got}");
    }
}

#[test]
fn plate_rt60_follows_the_setting() {
    let mut worst = 0.0f64;
    for &fs in &[44_100.0f32, 48_000.0, 96_000.0] {
        for want in [1.0f32, 2.5, 5.0] {
            let mut p = PlateReverb::new(fs);
            p.set_rt60(want);
            p.set_damping_hz(1.0e6);
            p.set_modulation(0.0, 1.0);
            let n = (want * 2.5 * fs) as usize + fs as usize;
            let mut l = vec![0.0f32; n];
            let mut r = vec![0.0f32; n];
            l[0] = 1.0;
            r[0] = 1.0;
            p.process(&mut l, &mut r);
            let got = measure(&l, fs, 1000.0);
            let err = (got - f64::from(want)) / f64::from(want);
            println!("plate fs {fs} want {want} got {got:.3} ({:+.1} %)", err * 100.0);
            worst = worst.max(err.abs());
            assert!(err.abs() < 0.05, "plate RT60 {got} vs {want} at {fs}");
        }
    }
    println!("worst plate RT60 error: {:.2} %", worst * 100.0);
}

#[test]
fn plate_damping_shortens_the_high_band() {
    let fs = 48_000.0;
    let mut p = PlateReverb::new(fs);
    p.set_rt60(3.0);
    p.set_damping_hz(3_000.0);
    p.set_modulation(0.0, 1.0);
    let n = (8.0 * fs) as usize;
    let mut l = vec![0.0f32; n];
    let mut r = vec![0.0f32; n];
    l[0] = 1.0;
    r[0] = 1.0;
    p.process(&mut l, &mut r);
    let mid = measure(&l, fs, 500.0);
    let hi = measure(&l, fs, 8000.0);
    println!("plate damped: 500 Hz {mid:.2}, 8 kHz {hi:.2}");
    assert!(hi < 0.6 * mid);
}

/// Cheap deterministic white noise.
struct Noise(u64);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

#[test]
fn fdn_is_bounded_at_extreme_settings_with_noise() {
    // Unit-amplitude white noise (a sustained full-scale input) into every
    // extreme corner, with freeze toggled during the run. A low rate keeps the
    // run short; delay lengths and gains are computed for the real rate.
    let fs = 22_050.0;
    let mut worst = 0.0f32;
    for (lines, mat) in [(8usize, FdnMatrix::Householder), (16, FdnMatrix::Hadamard)] {
        for size in [0.2f32, 4.0] {
            for rt in [0.05f32, 600.0] {
                for freeze in [false, true] {
                    for modulation in [0.0f32, 1.0] {
                        let mut r = FdnReverb::new(fs, lines);
                        r.set_matrix(mat);
                        r.set_size(size);
                        r.set_rt60(rt, rt, rt);
                        r.set_modulation(modulation, 5.0);
                        r.set_predelay_ms(500.0);
                        r.early_mut().set_level(4.0);
                        r.set_width(2.0);
                        let mut noise = Noise(11);
                        let mut peak = 0.0f32;
                        let hop = 2 * fs as usize;
                        for i in 0..5 * hop {
                            if freeze && i % hop == 0 {
                                r.set_freeze((i / hop) % 2 == 0);
                            }
                            let (a, b) = r.process_frame(noise.next(), noise.next());
                            assert!(a.is_finite() && b.is_finite());
                            peak = peak.max(a.abs()).max(b.abs());
                        }
                        worst = worst.max(peak);
                        assert!(peak < 400.0, "{lines} {mat:?} size {size} rt {rt} freeze {freeze} mod {modulation}: peak {peak}");
                    }
                }
            }
        }
    }
    println!("largest FDN peak over unit-amplitude noise, extreme settings: {worst}");
}

#[test]
fn fdn_freeze_holds_the_tail_and_stays_bounded() {
    let fs = 48_000.0;
    let mut r = FdnReverb::new(fs, 8);
    r.set_rt60(1.0, 1.0, 1.0);
    let mut noise = Noise(3);
    for _ in 0..(fs as usize / 2) {
        r.process_frame(noise.next() * 0.5, noise.next() * 0.5);
    }
    r.set_freeze(true);
    for _ in 0..fs as usize / 2 {
        r.process_frame(noise.next(), noise.next());
    }
    let rms = |r: &mut FdnReverb| {
        let mut e = 0.0f64;
        for _ in 0..fs as usize {
            let (a, b) = r.process_frame(0.0, 0.0);
            e += f64::from(a * a + b * b);
        }
        (e / (2.0 * f64::from(fs))).sqrt()
    };
    let first = rms(&mut r);
    for _ in 0..30 {
        rms(&mut r);
    }
    let later = rms(&mut r);
    println!("frozen rms: {first:.4} then {later:.4}");
    assert!(first > 1e-3 && later > 0.8 * first && later < 1.2 * first);
    r.set_freeze(false);
    for _ in 0..(8.0 * fs) as usize {
        r.process_frame(0.0, 0.0);
    }
    assert!(rms(&mut r) < 1e-3, "unfreezing lets the tail die");
}

#[test]
fn plate_is_bounded_at_extreme_settings_with_noise() {
    for fs in [22_050.0f32, 44_100.0] {
        for rt in [0.1f32, 120.0] {
            for freeze in [false, true] {
                for modulation in [0.0f32, 1.0] {
                    let mut p = PlateReverb::new(fs);
                    p.set_rt60(rt);
                    p.set_modulation(modulation, 5.0);
                    p.set_damping_hz(20.0);
                    p.set_bandwidth_hz(20.0);
                    p.set_width(2.0);
                    p.set_predelay_ms(500.0);
                    let mut noise = Noise(5);
                    let mut peak = 0.0f32;
                    let hop = 2 * fs as usize;
                    for i in 0..5 * hop {
                        if freeze && i % hop == 0 {
                            p.set_freeze((i / hop) % 2 == 0);
                        }
                        let (a, b) = p.process_frame(noise.next(), noise.next());
                        assert!(a.is_finite() && b.is_finite());
                        peak = peak.max(a.abs()).max(b.abs());
                    }
                    println!("plate fs {fs} rt {rt} freeze {freeze} mod {modulation}: peak {peak}");
                    assert!(peak < 200.0, "peak {peak}");
                }
            }
        }
    }
}

/// Normalised echo density (Abel and Huang, "A simple, robust measure of
/// reverberation echo density", AES 121, 2006): the fraction of samples in a
/// sliding window that lie outside one standard deviation of the window,
/// divided by erfc(1/sqrt 2) = 0.3173 (the value for Gaussian noise).
fn echo_density(x: &[f32], centre: usize, half: usize) -> f64 {
    let a = centre.saturating_sub(half);
    let b = (centre + half).min(x.len());
    let w = &x[a..b];
    let n = w.len() as f64;
    let mean = w.iter().map(|v| f64::from(*v)).sum::<f64>() / n;
    let sd = (w.iter().map(|v| (f64::from(*v) - mean).powi(2)).sum::<f64>() / n).sqrt();
    if sd == 0.0 {
        return 0.0;
    }
    let outside = w.iter().filter(|v| (f64::from(**v) - mean).abs() > sd).count() as f64;
    outside / n / 0.3173
}

#[test]
fn echo_density_grows_to_the_gaussian_value() {
    let fs = 48_000.0;
    let (l, _) = fdn_ir(fs, 16, |r| { r.set_modulation(0.0, 1.0); r.set_rt60(3.0, 3.0, 3.0) }, 1.5);
    let half = (0.01 * fs) as usize;
    let at = |ms: f32| echo_density(&l, (ms * 0.001 * fs) as usize, half);
    let (a, b, c, d) = (at(30.0), at(80.0), at(200.0), at(700.0));
    println!("FDN NED: 30 ms {a:.2}, 80 ms {b:.2}, 200 ms {c:.2}, 700 ms {d:.2}");
    assert!(a < b && b < c, "density must rise: {a} {b} {c}");
    assert!(c > 0.45 && d > 0.85 && d < 1.1, "{c} {d}");

    let fs = 48_000.0;
    let mut p = PlateReverb::new(fs);
    p.set_rt60(3.0);
    p.set_modulation(0.0, 1.0);
    let mut x = vec![0.0f32; (1.5 * fs) as usize];
    let mut y = x.clone();
    x[0] = 1.0;
    y[0] = 1.0;
    p.process(&mut x, &mut y);
    let at = |ms: f32| echo_density(&x, (ms * 0.001 * fs) as usize, half);
    let (a, b, c) = (at(30.0), at(120.0), at(700.0));
    println!("plate NED: 30 ms {a:.2}, 120 ms {b:.2}, 700 ms {c:.2}");
    assert!(a < b && b < c + 0.05 && c > 0.85);
}

fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        ab += f64::from(*x) * f64::from(*y);
        aa += f64::from(*x) * f64::from(*x);
        bb += f64::from(*y) * f64::from(*y);
    }
    ab / (aa * bb).sqrt()
}

#[test]
fn identical_inputs_give_decorrelated_outputs_and_width_collapses_them() {
    let fs = 48_000.0;
    let mut noise = Noise(9);
    let input: Vec<f32> = (0..(4.0 * fs) as usize).map(|_| noise.next()).collect();
    let skip = fs as usize;
    for lines in [8usize, 16] {
        let mut r = FdnReverb::new(fs, lines);
        let (mut l, mut rr) = (input.clone(), input.clone());
        r.process(&mut l, &mut rr);
        let c = correlation(&l[skip..], &rr[skip..]);
        println!("FDN {lines} lines mono-in correlation {c:.3}");
        assert!(c.abs() < 0.25, "{c}");
        r.reset();
        r.set_width(0.0);
        let (mut l, mut rr) = (input.clone(), input.clone());
        r.process(&mut l, &mut rr);
        assert!(correlation(&l[skip..], &rr[skip..]) > 0.999);
    }
    let mut p = PlateReverb::new(fs);
    let (mut l, mut rr) = (input.clone(), input.clone());
    p.process(&mut l, &mut rr);
    let c = correlation(&l[skip..], &rr[skip..]);
    println!("plate mono-in correlation {c:.3}");
    assert!(c.abs() < 0.25, "{c}");
}
