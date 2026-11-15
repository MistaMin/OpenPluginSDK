//! Quality of the phase vocoder: phasiness with and without identity phase
//! locking, and transient smearing on a click train with and without phase
//! reset at onsets.

use openplugin_dsp::{PhaseLocking, PitchShifter, TimeStretch, VocoderConfig, WindowKind};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use std::f64::consts::PI;

const TAU: f64 = 2.0 * PI;

/// Harmonic tone with 1 % vibrato at 5 Hz, evaluated at time `t` seconds.
/// Phase is the exact integral of the instantaneous frequency.
fn vibrato_tone(t: f64, f0: f64, rate: f64) -> f64 {
    let depth = 0.01;
    let phase = TAU * f0 * (t + depth / (TAU * rate) * (1.0 - (TAU * rate * t).cos()));
    (1..=6).map(|k| (1.0 / k as f64) * (k as f64 * phase).sin()).sum::<f64>() * 0.3
}

/// Magnitude spectrogram (Hann, frame 2048, hop 512) of `x` between `from` and `to`.
fn spectrogram(x: &[f32], from: usize, to: usize) -> Vec<Vec<f64>> {
    let n = 2048;
    let w = WindowKind::Hann.to_vec(n, true);
    let fft = FftPlanner::<f64>::new().plan_fft_forward(n);
    let mut out = Vec::new();
    let mut s = from;
    while s + n <= to.min(x.len()) {
        let mut b: Vec<Complex<f64>> = (0..n).map(|i| Complex::new(f64::from(x[s + i]) * f64::from(w[i]), 0.0)).collect();
        fft.process(&mut b);
        out.push(b[..n / 2].iter().map(|c| c.norm()).collect());
        s += 512;
    }
    out
}

/// Phasiness metric: relative L2 distance between the magnitude spectrogram of
/// a stretched signal and that of the ideal stretched signal, which is known
/// in closed form because the test tone is analytic: the same partials at the
/// same pitch, with the vibrato slowed to `5 Hz / S`.
/// Zero means the stretched signal has the same time-frequency magnitude
/// pattern as the ideal; phasiness (partials smeared across bins, beating
/// between neighbouring bins, amplitude flutter) raises it.
fn phasiness(fs: f32, ratio: f32, locking: PhaseLocking) -> f64 {
    let f0 = 180.0;
    let n = (3.0 * fs) as usize;
    let x: Vec<f32> = (0..n).map(|i| vibrato_tone(i as f64 / f64::from(fs), f0, 5.0) as f32).collect();
    let mut cfg = VocoderConfig::for_rate(fs);
    cfg.locking = locking;
    let mut t = TimeStretch::new(1, fs, cfg);
    t.set_ratio(ratio);
    let y = t.stretch_offline(0, &x);
    let ideal: Vec<f32> = (0..y.len())
        .map(|i| vibrato_tone(i as f64 / f64::from(fs), f0, 5.0 / f64::from(ratio)) as f32)
        .collect();
    let lo = (0.4 * fs) as usize;
    let hi = y.len() - lo;
    let (a, b) = (spectrogram(&y, lo, hi), spectrogram(&ideal, lo, hi));
    let (mut num, mut den) = (0.0, 0.0);
    for (fa, fb) in a.iter().zip(&b) {
        for (u, v) in fa.iter().zip(fb) {
            num += (u - v).powi(2);
            den += v * v;
        }
    }
    (num / den).sqrt()
}

#[test]
fn identity_phase_locking_reduces_phasiness() {
    for &fs in &[44_100.0f32, 48_000.0] {
        for &ratio in &[0.8f32, 1.25, 1.5, 2.0] {
            let plain = phasiness(fs, ratio, PhaseLocking::None);
            let locked = phasiness(fs, ratio, PhaseLocking::Identity);
            println!("phasiness fs {fs} ratio {ratio}: plain {plain:.4}, locked {locked:.4} ({:.2}x lower)", plain / locked);
            assert!(locked < plain / 3.0, "fs {fs} ratio {ratio}: {locked} vs {plain}");
        }
    }
}

// ---- transients -------------------------------------------------------------

/// A click: a 0.5 ms Hann-shaped burst of white-ish content (broadband).
fn click_train(fs: f32, period_s: f32, count: usize) -> (Vec<f32>, Vec<usize>) {
    let n = ((count as f32 + 1.0) * period_s * fs) as usize;
    let mut x = vec![0.0f32; n];
    let mut pos = Vec::new();
    let len = (0.0005 * fs) as usize;
    let mut s = 1u32;
    for c in 0..count {
        let p = ((c as f32 + 0.5) * period_s * fs) as usize + 1234 * (c % 2);
        pos.push(p);
        for i in 0..len {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let r = (s >> 8) as f32 / 8_388_608.0 - 1.0;
            let w = (PI * i as f64 / len as f64).sin() as f32;
            x[p + i] = 0.8 * r * w;
        }
    }
    (x, pos)
}

/// Onset spread in ms: around `centre` (output samples), within +-`half`, the
/// time between 5 % and 95 % of the cumulative energy. A perfect click gives
/// about its own duration; a smeared one gives the length of the smear.
fn spread_ms(y: &[f32], centre: usize, half: usize, fs: f32) -> (f64, f64) {
    let a = centre.saturating_sub(half);
    let b = (centre + half).min(y.len());
    let total: f64 = y[a..b].iter().map(|v| f64::from(*v).powi(2)).sum();
    if total <= 0.0 {
        return (f64::INFINITY, 0.0);
    }
    let (mut c, mut lo, mut hi) = (0.0, a, a);
    let mut got_lo = false;
    for (i, v) in y[a..b].iter().enumerate() {
        c += f64::from(*v).powi(2);
        if !got_lo && c >= 0.05 * total {
            lo = a + i;
            got_lo = true;
        }
        if c >= 0.95 * total {
            hi = a + i;
            break;
        }
    }
    // Position of the energy peak (1 ms smoothed) for the timing error.
    let w = (0.001 * fs) as usize;
    let mut best = (0.0, a);
    let mut run: f64 = y[a..(a + w).min(b)].iter().map(|v| f64::from(*v).powi(2)).sum();
    for i in a..b.saturating_sub(w) {
        if run > best.0 {
            best = (run, i + w / 2);
        }
        run += f64::from(y[i + w]).powi(2) - f64::from(y[i]).powi(2);
    }
    ((hi - lo) as f64 * 1000.0 / f64::from(fs), best.1 as f64)
}

struct Smear {
    mean_ms: f64,
    worst_ms: f64,
    mean_err_ms: f64,
}

fn click_smear(fs: f32, ratio: f32, transients: bool) -> Smear {
    let (x, pos) = click_train(fs, 0.35, 8);
    let mut cfg = VocoderConfig::for_rate(fs);
    cfg.transients = transients;
    let mut t = TimeStretch::new(1, fs, cfg);
    t.set_ratio(ratio);
    let y = t.stretch_offline(0, &x);
    let half = (0.15 * fs) as usize;
    let (mut sum, mut worst, mut err, mut cnt) = (0.0, 0.0f64, 0.0, 0.0);
    for &p in &pos[1..pos.len() - 1] {
        let ideal = (p as f64 * f64::from(ratio)) as usize;
        let (s, at) = spread_ms(&y, ideal, half.min(ideal / 2), fs);
        sum += s;
        worst = worst.max(s);
        err += (at - ideal as f64).abs() * 1000.0 / f64::from(fs);
        cnt += 1.0;
    }
    Smear { mean_ms: sum / cnt, worst_ms: worst, mean_err_ms: err / cnt }
}

#[test]
fn transient_option_reduces_smearing() {
    for &fs in &[44_100.0f32, 48_000.0] {
        for &ratio in &[0.5f32, 0.8, 1.25, 2.0] {
            let off = click_smear(fs, ratio, false);
            let on = click_smear(fs, ratio, true);
            println!(
                "clicks fs {fs} ratio {ratio}: spread off {:.2} ms (worst {:.2}), on {:.2} ms (worst {:.2}); timing error off {:.2} ms, on {:.2} ms",
                off.mean_ms, off.worst_ms, on.mean_ms, on.worst_ms, off.mean_err_ms, on.mean_err_ms
            );
            assert!(on.mean_ms < 0.7 * off.mean_ms, "{fs} {ratio}");
        }
    }
}

#[test]
fn transient_mode_through_the_pitch_shifter() {
    let fs = 48_000.0;
    let (x, _) = click_train(fs, 0.35, 6);
    for transients in [false, true] {
        let mut cfg = VocoderConfig::for_rate(fs);
        cfg.transients = transients;
        let mut p = PitchShifter::new(1, fs, cfg);
        p.set_semitones(7.0);
        let mut y = x.clone();
        p.process(0, &mut y);
        assert!(y.iter().all(|v| v.is_finite()));
        println!("pitch shifter +7 st, transients {transients}: onsets {}", p.onsets(0));
    }
}
