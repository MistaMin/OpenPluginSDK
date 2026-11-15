//! Proofs for the virtual analog blocks: small-signal responses against the
//! linear analog prototypes, self-oscillation, stability under modulation,
//! the WDF diode clipper against an independent implicit solve, tape
//! hysteresis loop shape and harmonics, and aliasing with and without the
//! crate's oversampling.

use openplugin_dsp::*;
use rustfft::{num_complex::Complex, FftPlanner};
use std::f64::consts::PI;

type C = (f64, f64);
fn cmul(a: C, b: C) -> C {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}
fn cadd(a: C, b: C) -> C {
    (a.0 + b.0, a.1 + b.1)
}
fn cdiv(a: C, b: C) -> C {
    let d = b.0 * b.0 + b.1 * b.1;
    ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
}
fn abs(a: C) -> f64 {
    a.0.hypot(a.1)
}
fn db(x: f64) -> f64 {
    20.0 * x.max(1e-300).log10()
}

/// Complex gain of `f` at `freq` (Hann-windowed correlation after settling).
fn gain<F: FnMut(f32) -> f32>(f: &mut F, fs: f64, freq: f64, amp: f64) -> C {
    let settle = 20_000usize;
    let cycles = 40.0;
    let n = ((cycles * fs / freq) as usize).max(20_000);
    let w = 2.0 * PI * freq / fs;
    for i in 0..settle {
        f((amp * (w * i as f64).sin()) as f32);
    }
    let (mut re, mut im, mut ws) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let k = settle + i;
        let y = f64::from(f((amp * (w * k as f64).sin()) as f32));
        let win = 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos();
        re += win * y * (w * k as f64).cos();
        im -= win * y * (w * k as f64).sin();
        ws += win;
    }
    // y ~ amp |H| sin(w k + phi): sum y e^{-jwk} = amp |H| e^{j phi} / (2j) * ws
    let c = (re / ws * 2.0 / amp, im / ws * 2.0 / amp);
    cmul(c, (0.0, 1.0))
}

fn ladder_proto(om: f64, k: f64) -> C {
    let one = (1.0, om);
    let p2 = cmul(one, one);
    let p4 = cmul(p2, p2);
    cdiv((1.0, 0.0), cadd(p4, (k, 0.0)))
}

fn diode_proto(om: f64, k: f64) -> C {
    let s = (0.0, om);
    let s2 = cmul(s, s);
    let s3 = cmul(s2, s);
    let s4 = cmul(s2, s2);
    let mut p = cadd(s4, (7.0 * s3.0, 7.0 * s3.1));
    p = cadd(p, (15.0 * s2.0, 15.0 * s2.1));
    p = cadd(p, (10.0 * s.0, 10.0 * s.1));
    p = cadd(p, (1.0 + k, 0.0));
    cdiv((1.0, 0.0), p)
}

fn korg_proto(om: f64, k: f64, hp: bool) -> C {
    let s = (0.0, om);
    let s2 = cmul(s, s);
    let den = cadd(cadd(s2, ((2.0 - k) * s.0, (2.0 - k) * s.1)), (1.0, 0.0));
    cdiv(if hp { s2 } else { (1.0, 0.0) }, den)
}

fn warped(fs: f64, f: f64, fc: f64) -> f64 {
    (PI * f / fs).tan() / (PI * fc / fs).tan()
}

#[test]
fn transistor_ladder_matches_analog_prototype() {
    let mut worst_db: f64 = 0.0;
    let mut worst_ph: f64 = 0.0;
    for fs in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        for fc in [1000.0, 4000.0] {
            for r in [0.0f32, 0.5, 0.9] {
                let k = 4.0 * f64::from(r);
                let mut lad = TransistorLadder::new(fs as f32);
                lad.set_cutoff(fc as f32);
                lad.set_resonance(r);
                for ratio in [0.3, 0.7, 1.0, 1.5, 3.0] {
                    let f = fc * ratio;
                    if f > 0.4 * fs {
                        continue;
                    }
                    lad.reset();
                    let h = gain(&mut |x| lad.process_sample(x), fs, f, 1e-3);
                    let p = ladder_proto(warped(fs, f, fc), k);
                    let e = (db(abs(h)) - db(abs(p))).abs();
                    let ph = (h.1.atan2(h.0) - p.1.atan2(p.0)).sin().atan2((h.1.atan2(h.0) - p.1.atan2(p.0)).cos()).abs();
                    worst_db = worst_db.max(e);
                    worst_ph = worst_ph.max(ph);
                    assert!(e < 0.05, "fs {fs} fc {fc} r {r} f {f}: {e} dB");
                    assert!(ph < 0.02, "fs {fs} fc {fc} r {r} f {f}: {ph} rad");
                }
                // At the cutoff the gain is exactly 1/(4-k) and the phase -180 degrees.
                lad.reset();
                let h = gain(&mut |x| lad.process_sample(x), fs, fc, 1e-3);
                let want = 1.0 / (4.0 - k);
                assert!((abs(h) / want - 1.0).abs() < 0.005, "fs {fs} fc {fc} r {r}: {} vs {want}", abs(h));
            }
        }
    }
    println!("transistor ladder: worst magnitude error {worst_db:.4} dB, phase {worst_ph:.4} rad");
}

#[test]
fn transistor_ladder_resonance_peak() {
    // Peak against the unwarped analog prototype at a high rate.
    let (fs, fc) = (192_000.0, 1000.0);
    let r = 0.9f32;
    let k = 3.6;
    let mut lad = TransistorLadder::new(fs as f32);
    lad.set_cutoff(fc as f32);
    lad.set_resonance(r);
    let (mut best_f, mut best_g) = (0.0, 0.0);
    let (mut abest_f, mut abest_g) = (0.0, 0.0);
    for i in 0..=60 {
        let ratio = 0.6 + 0.8 * f64::from(i) / 60.0;
        let f = fc * ratio;
        lad.reset();
        let g = abs(gain(&mut |x| lad.process_sample(x), fs, f, 1e-3));
        if g > best_g {
            best_g = g;
            best_f = f;
        }
        let a = abs(ladder_proto(ratio, k));
        if a > abest_g {
            abest_g = a;
            abest_f = f;
        }
    }
    println!("ladder peak: measured {best_f:.1} Hz {:.3} dB, analog {abest_f:.1} Hz {:.3} dB", db(best_g), db(abest_g));
    assert!((best_f / abest_f - 1.0).abs() < 0.02);
    assert!((db(best_g) - db(abest_g)).abs() < 0.1);
}

#[test]
fn diode_ladder_matches_analog_prototype() {
    let mut worst: f64 = 0.0;
    for fs in [48_000.0, 192_000.0] {
        let fc = 1500.0;
        for r in [0.0f32, 0.4, 0.8] {
            let k = DIODE_K_CRIT * f64::from(r);
            let mut lad = DiodeLadder::new(fs as f32);
            lad.set_cutoff(fc as f32);
            lad.set_resonance(r);
            for ratio in [0.3, 0.7, 1.2, 2.0] {
                let f = fc * ratio;
                lad.reset();
                let h = gain(&mut |x| lad.process_sample(x), fs, f, 1e-3);
                let p = diode_proto(warped(fs, f, fc), k);
                let e = (db(abs(h)) - db(abs(p))).abs();
                worst = worst.max(e);
                assert!(e < 0.1, "fs {fs} r {r} f {f}: {e} dB ({} vs {})", abs(h), abs(p));
            }
        }
    }
    println!("diode ladder: worst magnitude error {worst:.4} dB");
}

#[test]
fn diode_critical_gain_is_where_the_prototype_oscillates() {
    // p(jw) + k = 0 at w^2 = 10/7, k = 901/49.
    let w = (10.0f64 / 7.0).sqrt();
    let p = diode_proto(w, DIODE_K_CRIT);
    assert!(abs(p) > 1e9, "{}", abs(p));
}

#[test]
fn korg35_matches_analog_prototype() {
    let mut worst: f64 = 0.0;
    for fs in [44_100.0, 96_000.0, 192_000.0] {
        for hp in [false, true] {
            for r in [0.0f32, 0.5, 0.9] {
                let fc = 2000.0;
                let mut f = Korg35::new(fs as f32);
                f.set_mode(if hp { Korg35Mode::HighPass } else { Korg35Mode::LowPass });
                f.set_cutoff(fc as f32);
                f.set_resonance(r);
                for ratio in [0.3, 0.7, 1.0, 1.5, 3.0] {
                    let fr = fc * ratio;
                    f.reset();
                    let h = gain(&mut |x| f.process_sample(x), fs, fr, 1e-3);
                    let p = korg_proto(warped(fs, fr, fc), 2.0 * f64::from(r), hp);
                    let e = (db(abs(h)) - db(abs(p))).abs();
                    worst = worst.max(e);
                    assert!(e < 0.05, "fs {fs} hp {hp} r {r} f {fr}: {e} dB");
                }
            }
        }
    }
    println!("korg35: worst magnitude error {worst:.4} dB");
}

/// Runs a filter from a tiny kick with no input; returns (peak over the last
/// 4000 samples, estimated frequency Hz from zero crossings).
fn ring<F: FnMut(f32) -> f32>(mut f: F, fs: f64, seconds: f64, kick: f32) -> (f64, f64) {
    let n = (seconds * fs) as usize;
    let mut buf = vec![0.0f32; n];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = f(if i == 0 { kick } else { 0.0 });
    }
    let tail = &buf[n - 4000..];
    let peak = tail.iter().fold(0.0f64, |m, v| m.max(f64::from(v.abs())));
    let mut crossings = 0;
    for w in tail.windows(2) {
        if w[0] <= 0.0 && w[1] > 0.0 {
            crossings += 1;
        }
    }
    (peak, f64::from(crossings) * fs / 4000.0)
}

#[test]
fn self_oscillation_onset() {
    let fs = 48_000.0;
    let fc = 1000.0;
    // Transistor ladder: k = 4 r.
    for (r, grows) in [(0.95f32, false), (1.05, true)] {
        let mut l = TransistorLadder::new(fs as f32);
        l.set_cutoff(fc as f32);
        l.set_resonance(r);
        let (peak, hz) = ring(|x| l.process_sample(x), fs, 3.0, 1e-4);
        println!("transistor r {r}: peak {peak:.3e}, {hz:.0} Hz");
        if grows {
            assert!(peak > 0.05 && peak < 5.0 && (hz / fc - 1.0).abs() < 0.1, "{peak} {hz}");
        } else {
            assert!(peak < 1e-8, "{peak}");
        }
    }
    // Diode ladder: oscillates at 1.195 fc.
    for (r, grows) in [(0.95f32, false), (1.1, true)] {
        let mut l = DiodeLadder::new(fs as f32);
        l.set_cutoff(fc as f32);
        l.set_resonance(r);
        let (peak, hz) = ring(|x| l.process_sample(x), fs, 3.0, 1e-4);
        println!("diode r {r}: peak {peak:.3e}, {hz:.0} Hz");
        if grows {
            assert!(peak > 0.02 && peak < 5.0 && (hz / (fc * 1.195) - 1.0).abs() < 0.15, "{peak} {hz}");
        } else {
            assert!(peak < 1e-8, "{peak}");
        }
    }
    // Korg-35: K = 2 r, oscillates at fc.
    for (r, grows) in [(0.95f32, false), (1.05, true)] {
        let mut l = Korg35::new(fs as f32);
        l.set_cutoff(fc as f32);
        l.set_resonance(r);
        let (peak, hz) = ring(|x| l.process_sample(x), fs, 3.0, 1e-4);
        println!("korg35 r {r}: peak {peak:.3e}, {hz:.0} Hz");
        if grows {
            assert!(peak > 0.02 && peak < 5.0 && (hz / fc - 1.0).abs() < 0.1, "{peak} {hz}");
        } else {
            assert!(peak < 1e-8, "{peak}");
        }
    }
}

#[test]
fn stable_under_per_sample_modulation_and_noise() {
    for fs in [44_100.0f32, 192_000.0] {
        let mut rng = WhiteNoise::new(7);
        let mut t = TransistorLadder::new(fs);
        let mut d = DiodeLadder::new(fs);
        let mut klp = Korg35::new(fs);
        let mut khp = Korg35::new(fs);
        khp.set_mode(Korg35Mode::HighPass);
        let (mut peak_t, mut peak_d, mut peak_k, mut it_t, mut it_d) = (0.0f32, 0.0f32, 0.0f32, 0, 0);
        for n in 0..60_000 {
            let u1 = 0.5 + 0.5 * rng.next();
            let u2 = 0.5 + 0.5 * rng.next();
            let u3 = 0.5 + 0.5 * rng.next();
            // log-uniform cutoff from 20 Hz to 0.45 fs, resonance up to the maximum.
            let fc = 20.0 * (0.45 * fs / 20.0).powf(u1);
            let r = 1.25 * u2;
            let drive = if n % 2000 < 1000 { 1.0 } else { 8.0 * u3 };
            let x = if n % 5000 < 4000 { rng.next() } else { 0.0 };
            t.set_cutoff(fc);
            t.set_resonance(r);
            t.set_drive(drive);
            d.set_cutoff(fc);
            d.set_resonance(r);
            d.set_drive(drive);
            klp.set_cutoff(fc);
            klp.set_resonance(r);
            khp.set_cutoff(fc);
            khp.set_resonance(r);
            let yt = t.process_sample(x);
            let yd = d.process_sample(x);
            let yk = klp.process_sample(x);
            let yh = khp.process_sample(x);
            for y in [yt, yd, yk, yh] {
                assert!(y.is_finite());
            }
            peak_t = peak_t.max(yt.abs());
            peak_d = peak_d.max(yd.abs());
            peak_k = peak_k.max(yk.abs()).max(yh.abs());
            it_t = it_t.max(t.last_iterations());
            it_d = it_d.max(d.last_iterations());
        }
        println!("fs {fs}: peaks ladder {peak_t:.2} diode {peak_d:.2} korg {peak_k:.2}; max iterations {it_t}/{it_d}");
        assert!(peak_t < 10.0 && peak_d < 10.0 && peak_k < 20.0);
        assert!(it_t <= ladder::MAX_ITERATIONS && it_d <= ladder::MAX_ITERATIONS);
    }
}

#[test]
fn diode_ladder_residual_is_small_when_converged() {
    let mut d = DiodeLadder::new(48_000.0);
    d.set_resonance(0.9);
    let mut rng = WhiteNoise::new(3);
    let mut worst = 0.0f64;
    for n in 0..20_000 {
        d.set_cutoff(100.0 * (200.0f32).powf(0.5 + 0.5 * (n as f32 * 0.001).sin()));
        d.process_sample(rng.next() * 2.0);
        worst = worst.max(d.last_residual());
    }
    println!("diode ladder worst residual {worst:.2e}");
    assert!(worst < 1e-6);
}

// ---------------------------------------------------------------------------
// WDF diode clipper against an independent implicit solve
// ---------------------------------------------------------------------------

const R_CLIP: f64 = 2200.0;
const C_CLIP: f64 = 10e-9;
const IS: f64 = 2.52e-9;
const NVT: f64 = 1.752 * 0.02585;

/// Trapezoidal ODE reference `C dv/dt = (vs - v)/R - 2 Is sinh(v/nVt)` at
/// `rate`, input `vs(t)`; returns v at every `step_every`-th step.
fn clipper_reference(vs: &dyn Fn(f64) -> f64, rate: f64, n_out: usize, step_every: usize) -> Vec<f64> {
    let h = 1.0 / rate;
    let f = |v: f64, e: f64| ((e - v) / R_CLIP - 2.0 * IS * (v / NVT).sinh()) / C_CLIP;
    let df = |v: f64| (-1.0 / R_CLIP - 2.0 * IS / NVT * (v / NVT).cosh()) / C_CLIP;
    let mut v = 0.0;
    let mut out = Vec::with_capacity(n_out);
    for n in 0..n_out * step_every {
        if n % step_every == 0 {
            out.push(v);
        }
        let (e0, e1) = (vs(n as f64 * h), vs((n + 1) as f64 * h));
        let f0 = f(v, e0);
        let mut x = v;
        for _ in 0..200 {
            let g = x - v - 0.5 * h * (f0 + f(x, e1));
            let dg = 1.0 - 0.5 * h * df(x);
            let step = (g / dg).clamp(-0.05, 0.05);
            x -= step;
            if step.abs() < 1e-14 {
                break;
            }
        }
        v = x;
    }
    out
}

#[test]
fn diode_clipper_wdf_matches_implicit_reference() {
    let mut table = Vec::new();
    for &(freq, amp) in &[(200.0, 0.3), (1000.0, 2.0), (1000.0, 8.0), (3000.0, 4.0)] {
        let mut prev = f64::MAX;
        for &fs in &[48_000.0, 96_000.0, 192_000.0] {
            let n = (fs * 0.02) as usize;
            let over = (3_072_000.0 / fs) as usize;
            let vs = |t: f64| amp * (2.0 * PI * freq * t).sin();
            let reference = clipper_reference(&vs, fs * over as f64, n, over);
            let mut clip = DiodeClipper::new(fs as f32);
            let mut max_err: f64 = 0.0;
            let mut peak: f64 = 0.0;
            for (i, r) in reference.iter().enumerate() {
                let y = f64::from(clip.process_sample(vs(i as f64 / fs) as f32));
                max_err = max_err.max((y - r).abs());
                peak = peak.max(r.abs());
            }
            let rel = max_err / peak;
            println!("clipper {freq} Hz {amp} V @ {fs}: peak {peak:.4} V, max error {max_err:.2e} V ({:.3} % of peak)", rel * 100.0);
            table.push((freq, amp, fs, rel));
            // The scheme is second order, so the error falls with the rate.
            assert!(rel <= prev * 1.05, "error did not fall with the rate");
            prev = rel;
        }
    }
    // Stated tolerances against the continuous circuit (max error over peak):
    // 2.5 % at 192 kHz, 5.5 % at 96 kHz, 11 % at 48 kHz, worst case (hard-driven) included.
    for (freq, amp, fs, rel) in table {
        let tol = if fs >= 192_000.0 { 0.025 } else if fs >= 96_000.0 { 0.055 } else { 0.11 };
        assert!(rel < tol, "{freq} Hz {amp} V @ {fs}: {rel}");
    }
}

#[test]
fn diode_clipper_wdf_equals_trapezoidal_solve_at_its_own_rate() {
    // The bilinear WDF capacitor with an exact nonlinear root is the trapezoidal
    // rule on the circuit ODE; a reference at the same rate must agree to the solver tolerance.
    for &(freq, amp, fs) in &[(1000.0, 8.0, 48_000.0), (3000.0, 4.0, 96_000.0), (200.0, 0.5, 44_100.0)] {
        let vs = |t: f64| amp * (2.0 * PI * freq * t).sin();
        let n = 2000;
        let reference = clipper_reference(&vs, fs, n, 1);
        let mut clip = DiodeClipper::new(fs as f32);
        let mut worst: f64 = 0.0;
        for (i, r) in reference.iter().enumerate() {
            let x = vs(i as f64 / fs) as f32;
            let y = f64::from(clip.process_sample(x));
            // The reference saw the f64 source; allow the f32 input rounding.
            worst = worst.max((y - r).abs());
        }
        println!("same-rate agreement {freq} Hz {amp} V @ {fs}: {worst:.2e} V");
        assert!(worst < 2e-6, "{worst}");
    }
}

#[test]
fn diode_clipper_clips_near_a_diode_drop() {
    let mut c = DiodeClipper::new(96_000.0);
    let mut peak = 0.0f32;
    for n in 0..9600 {
        let y = c.process_sample(10.0 * (2.0 * std::f32::consts::PI * 500.0 * n as f32 / 96_000.0).sin());
        peak = peak.max(y.abs());
    }
    println!("clipper peak with a 10 V drive: {peak:.3} V");
    assert!(peak > 0.4 && peak < 1.1);
}

// ---------------------------------------------------------------------------
// Tape
// ---------------------------------------------------------------------------

fn tape_cycle(drive: f32, width: f32, sat: f32, fs: f32, amp: f32) -> (Vec<f32>, Vec<f32>) {
    let f = 1000.0f32;
    let period = (fs / f) as usize;
    let mut t = TapeHysteresis::new(fs);
    t.set_drive(drive);
    t.set_width(width);
    t.set_saturation(sat);
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for n in 0..=period * 8 {
        let x = amp * (2.0 * std::f32::consts::PI * n as f32 / period as f32).sin();
        let y = t.process_sample(x);
        if n >= period * 7 {
            xs.push(x);
            ys.push(y);
        }
    }
    (xs, ys)
}

/// Signed area enclosed by the (x, y) trace: sum of y dx.
fn loop_area(xs: &[f32], ys: &[f32]) -> f64 {
    let mut a = 0.0;
    for i in 0..xs.len() {
        let j = (i + 1) % xs.len();
        a += 0.5 * f64::from(ys[i] + ys[j]) * f64::from(xs[j] - xs[i]);
    }
    a
}

#[test]
fn tape_traces_a_closed_hysteresis_loop() {
    let (xs, ys) = tape_cycle(1.0, 0.5, 0.5, 192_000.0, 1.0);
    // One extra sample: the last equals the first one period later.
    let closure = (ys[0] - ys[ys.len() - 1]).abs();
    let (xs, ys) = (&xs[..xs.len() - 1], &ys[..ys.len() - 1]);
    let n = xs.len();
    // Closed: the trace returns to where it started.
    // Two-valued at the input's zero crossings: ascending below descending.
    let mut asc = None;
    let mut desc = None;
    for i in 0..n {
        let prev = (i + n - 1) % n;
        if xs[prev] < 0.0 && xs[i] >= 0.0 {
            asc = Some(ys[i]);
        }
        if xs[prev] > 0.0 && xs[i] <= 0.0 {
            desc = Some(ys[i]);
        }
    }
    let (asc, desc) = (asc.unwrap(), desc.unwrap());
    let area = loop_area(xs, ys);
    println!("tape loop: closure {closure:.2e}, remanent branches {asc:.3} / {desc:.3}, area {area:.4}");
    assert!(closure < 1e-3);
    assert!(desc - asc > 0.02, "not a loop: {asc} {desc}");
    assert!(area.abs() > 1e-3);
    // Wider pinning dissipates more.
    let a_narrow = loop_area(&tape_cycle(1.0, 0.0, 0.5, 192_000.0, 1.0).0, &tape_cycle(1.0, 0.0, 0.5, 192_000.0, 1.0).1).abs();
    let a_wide = loop_area(&tape_cycle(1.0, 1.0, 0.5, 192_000.0, 1.0).0, &tape_cycle(1.0, 1.0, 0.5, 192_000.0, 1.0).1).abs();
    println!("loop area narrow {a_narrow:.4}, wide {a_wide:.4}");
    assert!(a_wide > 1.5 * a_narrow);
}

fn harmonics(drive: f32, fs: f32) -> Vec<f64> {
    let f = 1000.0f32;
    let period = (fs / f) as usize;
    let mut t = TapeHysteresis::new(fs);
    t.set_drive(drive);
    let total = period * 60;
    let mut ys = Vec::with_capacity(total);
    for n in 0..total {
        ys.push(t.process_sample(0.5 * (2.0 * std::f32::consts::PI * n as f32 / period as f32).sin()));
    }
    let tail = &ys[period * 40..];
    (1..=9)
        .map(|h| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &y) in tail.iter().enumerate() {
                let ph = 2.0 * PI * (h * i) as f64 / period as f64;
                re += f64::from(y) * ph.cos();
                im += f64::from(y) * ph.sin();
            }
            2.0 * re.hypot(im) / tail.len() as f64
        })
        .collect()
}

#[test]
fn tape_harmonics_grow_with_drive() {
    let mut last_thd = 0.0;
    let mut last_rms = 0.0;
    for drive in [0.25f32, 0.5, 1.0, 2.0, 4.0] {
        let h = harmonics(drive, 96_000.0);
        let thd = (h[1..].iter().map(|v| v * v).sum::<f64>()).sqrt() / h[0];
        let rms = (h.iter().map(|v| v * v).sum::<f64>() / 2.0).sqrt();
        println!("tape drive {drive}: fundamental {:.4}, H3/H1 {:.4}, THD {:.2} %, rms {rms:.4}", h[0], h[2] / h[0], thd * 100.0);
        assert!(thd > last_thd, "THD fell at drive {drive}");
        assert!(rms > last_rms, "energy fell at drive {drive}");
        last_thd = thd;
        last_rms = rms;
    }
}

// ---------------------------------------------------------------------------
// Aliasing with and without the crate's oversampling
// ---------------------------------------------------------------------------

/// Power in the spectrum outside the harmonics of `f0` below Nyquist,
/// relative to the fundamental, in dB.
fn alias_db(y: &[f32], fs: f64, f0: f64) -> f64 {
    let n = y.len();
    let mut buf: Vec<Complex<f64>> = y
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos();
            Complex::new(f64::from(v) * w, 0.0)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    let bin = |f: f64| (f * n as f64 / fs).round() as isize;
    let fund = (bin(f0) - 3..=bin(f0) + 3).map(|b| buf[b as usize].norm_sqr()).sum::<f64>();
    let mut harmonic = vec![false; n / 2 + 1];
    let mut h = 1.0;
    while h * f0 < fs / 2.0 {
        let b = bin(h * f0);
        for k in b - 3..=b + 3 {
            if k >= 0 && (k as usize) < harmonic.len() {
                harmonic[k as usize] = true;
            }
        }
        h += 1.0;
    }
    let mut alias = 0.0;
    for (k, hit) in harmonic.iter().enumerate().skip(2) {
        if !*hit {
            alias += buf[k].norm_sqr();
        }
    }
    10.0 * (alias / fund).log10()
}

fn run_with_oversampling<F: FnMut(f32) -> f32>(input: &[f32], factor: usize, mut f: F) -> Vec<f32> {
    let mut os = LinearPhaseOversampler::prepare(factor, 1, input.len()).unwrap();
    let mut out = vec![0.0f32; input.len()];
    os.process(&[input], &mut [&mut out[..]], |s| s[0] = f(s[0])).unwrap();
    out
}

#[test]
fn aliasing_with_and_without_oversampling() {
    let fs = 48_000.0f64;
    let f0 = 7001.0;
    let n = 48_000usize;
    let input: Vec<f32> = (0..n).map(|i| (0.9 * (2.0 * PI * f0 * i as f64 / fs).sin()) as f32).collect();
    let skip = 4000;

    // Tape.
    let mk = |rate: f32| {
        let mut t = TapeHysteresis::new(rate);
        t.set_drive(3.0);
        t.set_saturation(0.8);
        t
    };
    let mut t1 = mk(fs as f32);
    let plain: Vec<f32> = input.iter().map(|&x| t1.process_sample(x)).collect();
    let mut t4 = mk(4.0 * fs as f32);
    let over = run_with_oversampling(&input, 4, |x| t4.process_sample(x));
    let a1 = alias_db(&plain[skip..skip + 32768], fs, f0);
    // The oversampler is linear phase with a latency; the window skips it.
    let a4 = alias_db(&over[skip..skip + 32768], fs, f0);
    println!("tape aliasing: 1x {a1:.1} dB, 4x {a4:.1} dB (re fundamental)");
    assert!(a4 < a1 - 6.0);

    // Transistor ladder at high drive and resonance.
    let mk = |rate: f32| {
        let mut l = TransistorLadder::new(rate);
        l.set_cutoff(12_000.0);
        l.set_resonance(0.3);
        l.set_drive(10.0);
        l
    };
    let mut l1 = mk(fs as f32);
    let plain: Vec<f32> = input.iter().map(|&x| l1.process_sample(x)).collect();
    let mut l4 = mk(4.0 * fs as f32);
    let over = run_with_oversampling(&input, 4, |x| l4.process_sample(x));
    let b1 = alias_db(&plain[skip..skip + 32768], fs, f0);
    let b4 = alias_db(&over[skip..skip + 32768], fs, f0);
    println!("ladder aliasing: 1x {b1:.1} dB, 4x {b4:.1} dB (re fundamental)");
    assert!(b4 < b1 - 6.0);
}
