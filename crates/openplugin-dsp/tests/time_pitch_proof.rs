//! Phase vocoder and delay-line shifter against measurement: pitch shift in
//! cents, time stretch pitch and duration, phase locking against the plain
//! vocoder, transient smearing on a click train, latency.

use openplugin_dsp::{
    PitchShifter, TimeStretch, VocoderConfig, WindowKind,
};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use std::f64::consts::PI;

const TAU: f64 = 2.0 * PI;

fn cents(f: f64, r: f64) -> f64 {
    1200.0 * (f / r).log2()
}

fn synth(fs: f32, n: usize, partials: &[(f64, f64)], f0: f64) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(fs);
            partials.iter().map(|&(m, a)| a * (TAU * f0 * m * t).sin()).sum::<f64>() as f32
        })
        .collect()
}

/// Spectrum magnitude of a Hann-windowed segment, zero padded 8x.
fn spectrum(x: &[f32]) -> Vec<f64> {
    let n = x.len();
    let m = (n * 8).next_power_of_two();
    let w = WindowKind::Hann.to_vec(n, true);
    let mut buf: Vec<Complex<f64>> = (0..m)
        .map(|i| if i < n { Complex::new(f64::from(x[i]) * f64::from(w[i]), 0.0) } else { Complex::new(0.0, 0.0) })
        .collect();
    FftPlanner::<f64>::new().plan_fft_forward(m).process(&mut buf);
    buf[..m / 2].iter().map(|c| c.norm()).collect()
}

/// Frequency of the strongest spectral line within `lo..hi` Hz, parabolic on dB.
fn peak_hz(x: &[f32], fs: f32, lo: f64, hi: f64) -> f64 {
    let s = spectrum(x);
    let m = s.len() * 2;
    let hz = |k: f64| k * f64::from(fs) / m as f64;
    let (mut best, mut bk) = (0.0, 0usize);
    for (k, &v) in s.iter().enumerate() {
        let f = hz(k as f64);
        if f >= lo && f <= hi && v > best {
            best = v;
            bk = k;
        }
    }
    if bk == 0 || bk + 1 >= s.len() {
        return hz(bk as f64);
    }
    let (a, b, c) = (s[bk - 1].ln(), s[bk].ln(), s[bk + 1].ln());
    let off = 0.5 * (a - c) / (a - 2.0 * b + c);
    hz(bk as f64 + off)
}

fn steady(y: &[f32], fs: f32, start_s: f32, len_s: f32) -> &[f32] {
    let a = (start_s * fs) as usize;
    let b = (a + (len_s * fs) as usize).min(y.len());
    &y[a..b]
}

fn shift_vocoder(fs: f32, x: &[f32], semis: f32, cfg: VocoderConfig) -> (Vec<f32>, usize, u64) {
    let mut p = PitchShifter::new(1, fs, cfg);
    p.set_semitones(semis);
    let mut y = x.to_vec();
    // Awkward host block sizes.
    let mut i = 0;
    let sizes = [64usize, 333, 1, 1024, 480];
    let mut k = 0;
    while i < y.len() {
        let n = sizes[k % sizes.len()].min(y.len() - i);
        p.process(0, &mut y[i..i + n]);
        i += n;
        k += 1;
    }
    (y, p.latency(), p.underruns(0))
}

#[test]
fn vocoder_pitch_shift_within_cents() {
    let semis = [-12.0f32, -7.0, 3.0, 7.0, 12.0];
    let mut worst = 0.0f64;
    let mut worst_h = 0.0f64;
    for &fs in &[44_100.0f32, 48_000.0, 96_000.0] {
        let cfg = VocoderConfig::for_rate(fs);
        for &f0 in &[220.0f64, 440.0, 1000.0] {
            for &s in &semis {
                let r = 2f64.powf(f64::from(s) / 12.0);
                // Sine.
                let x = synth(fs, (2.5 * fs) as usize, &[(1.0, 0.5)], f0);
                let (y, lat, under) = shift_vocoder(fs, &x, s, cfg);
                assert_eq!(under, 0, "underruns");
                assert_eq!(lat, cfg.frame / 2 + cfg.frame + 64);
                let seg = steady(&y, fs, 1.0, 1.0);
                let f = peak_hz(seg, fs, f0 * r * 0.9, f0 * r * 1.1);
                let e = cents(f, f0 * r).abs();
                worst = worst.max(e);
                assert!(e < 0.5, "sine fs {fs} f0 {f0} {s} st: {e} cents");
                // Harmonic tone: fundamental and third partial both land.
                let h = [(1.0, 0.4), (2.0, 0.25), (3.0, 0.2), (4.0, 0.1)];
                let x = synth(fs, (2.5 * fs) as usize, &h, f0);
                let (y, _, _) = shift_vocoder(fs, &x, s, cfg);
                let seg = steady(&y, fs, 1.0, 1.0);
                for &(m, _) in &h {
                    let target = f0 * r * m;
                    if target > f64::from(fs) * 0.4 {
                        continue;
                    }
                    let f = peak_hz(seg, fs, target * 0.97, target * 1.03);
                    let e = cents(f, target).abs();
                    worst_h = worst_h.max(e);
                    assert!(e < 0.5, "harm fs {fs} f0 {f0} {s} st partial {m}: {e} cents");
                }
            }
        }
    }
    println!("vocoder pitch shift: worst sine error {worst:.3} cents, worst harmonic partial {worst_h:.3} cents");
}

#[test]
fn time_stretch_preserves_pitch_and_duration() {
    let mut worst = 0.0f64;
    for &fs in &[44_100.0f32, 48_000.0, 96_000.0] {
        for &ratio in &[0.5f32, 0.8, 1.25, 2.0] {
            for &f0 in &[110.0f64, 440.0, 1500.0] {
                let n = (1.5 * fs) as usize + 7;
                let h = [(1.0, 0.4), (2.0, 0.2), (3.0, 0.1)];
                let x = synth(fs, n, &h, f0);
                let mut t = TimeStretch::new(1, fs, VocoderConfig::for_rate(fs));
                t.set_ratio(ratio);
                let y = t.stretch_offline(0, &x);
                let want = (n as f64 * f64::from(ratio)).round() as usize;
                assert_eq!(y.len(), want, "duration");
                let a = (0.3 * fs * ratio) as usize;
                let b = (y.len() - (0.3 * fs * ratio) as usize).min(a + (0.5 * fs) as usize);
                let f = peak_hz(&y[a..b], fs, f0 * 0.95, f0 * 1.05);
                let e = cents(f, f0).abs();
                worst = worst.max(e);
                assert!(e < 0.5, "fs {fs} ratio {ratio} f0 {f0}: {e} cents");
                // Level: steady-state RMS within 1 dB of the input.
                let rms = |v: &[f32]| (v.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
                let db = 20.0 * (rms(&y[a..b]) / rms(&x[(n / 4)..(3 * n / 4)])).log10();
                assert!(db.abs() < 1.0, "level {db} dB");
            }
        }
    }
    println!("time stretch: worst pitch error {worst:.3} cents, duration exact");
}
