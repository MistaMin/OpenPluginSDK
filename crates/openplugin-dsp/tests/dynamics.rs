//! Proofs for the dynamics building blocks: static curves, ballistics, gate
//! hysteresis, linked detection, the sidechain hook, and the limiter's ceiling
//! (checked with an independent 16x interpolator).

use openplugin_dsp::{
    Detection, Dynamics, DynamicsMode, DynamicsParams, GainComputer, Limiter, Link, Smoothing, Topology,
};

const FS: f32 = 48_000.0;

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-12).log10()
}

// ---- static curves ---------------------------------------------------------

#[test]
fn compressor_hard_knee_curve() {
    let g = GainComputer::new(DynamicsMode::Compressor, -20.0, 4.0, 0.0);
    assert_eq!(g.static_output_db(-40.0), -40.0);
    assert_eq!(g.static_output_db(-20.0), -20.0);
    assert!((g.static_output_db(-10.0) - (-20.0 + 10.0 / 4.0)).abs() < 1e-4);
    assert!((g.static_output_db(0.0) - (-20.0 + 20.0 / 4.0)).abs() < 1e-4);
}

#[test]
fn compressor_soft_knee_curve() {
    let w = 8.0;
    let g = GainComputer::new(DynamicsMode::Compressor, -20.0, 4.0, w);
    // Below the knee: identity. Above: the hard-knee line. Both edges continuous.
    assert_eq!(g.static_output_db(-24.5), -24.5);
    let hard = GainComputer::new(DynamicsMode::Compressor, -20.0, 4.0, 0.0);
    assert!((g.static_output_db(-15.5) - hard.static_output_db(-15.5)).abs() < 1e-4);
    assert!((g.static_output_db(-24.0) - -24.0).abs() < 1e-4);
    assert!((g.static_output_db(-16.0) - hard.static_output_db(-16.0)).abs() < 1e-4);
    // At the threshold the paper's value: x + (1/R - 1)(W/2)^2 / (2W).
    let want = -20.0 + (0.25 - 1.0) * (w / 2.0) * (w / 2.0) / (2.0 * w);
    assert!((g.static_output_db(-20.0) - want).abs() < 1e-4, "{}", g.static_output_db(-20.0));
    // Monotonic and with slope between 1/R and 1 across the knee.
    let mut prev = g.static_output_db(-30.0);
    for k in 1..400 {
        let x = -30.0 + k as f32 * 0.05;
        let y = g.static_output_db(x);
        let slope = (y - prev) / 0.05;
        assert!((0.249..=1.001).contains(&slope), "slope {slope} at {x}");
        prev = y;
    }
}

#[test]
fn infinite_ratio_limits_flat() {
    let g = GainComputer::new(DynamicsMode::Compressor, -6.0, f32::INFINITY, 0.0);
    for x in [-5.0f32, 0.0, 12.0, 100.0] {
        assert!((g.static_output_db(x) - -6.0).abs() < 1e-4);
    }
    let k = GainComputer::new(DynamicsMode::Compressor, -6.0, f32::INFINITY, 6.0);
    assert!(k.static_output_db(0.0).is_finite());
    assert!((k.static_output_db(0.0) - -6.0).abs() < 1e-4);
}

#[test]
fn expander_and_gate_curves_with_range() {
    let e = GainComputer::new(DynamicsMode::Expander, -40.0, 2.0, 0.0);
    assert_eq!(e.static_output_db(-30.0), -30.0);
    assert!((e.static_output_db(-50.0) - (-40.0 - 20.0)).abs() < 1e-4);
    let mut g = GainComputer::new(DynamicsMode::Gate, -40.0, 4.0, 0.0);
    g.range_db = 30.0;
    assert_eq!(g.static_gain_db(-39.0), 0.0);
    assert!((g.static_gain_db(-60.0) - -30.0).abs() < 1e-4);
    assert!((g.static_gain_db(-190.0) - -30.0).abs() < 1e-4);
    // Soft-knee expander is continuous at both edges.
    let k = GainComputer::new(DynamicsMode::Expander, -40.0, 3.0, 10.0);
    assert!(k.static_gain_db(-34.9).abs() < 1e-3);
    assert!((k.static_gain_db(-45.1) - 2.0 * -5.1).abs() < 0.02);
}

#[test]
fn gate_hysteresis_depends_on_history() {
    let mut g = GainComputer::new(DynamicsMode::Gate, -40.0, 4.0, 0.0);
    g.hysteresis_db = 6.0;
    g.range_db = 60.0;
    // Between -46 and -40: closed if it came from below, open if from above.
    assert!(g.gain_db(-43.0) < -1.0, "closed gate stays closed at -43");
    assert!(!g.is_open());
    assert_eq!(g.gain_db(-30.0), 0.0);
    assert!(g.is_open());
    assert_eq!(g.gain_db(-43.0), 0.0, "open gate stays open at -43");
    assert_eq!(g.gain_db(-45.9), 0.0);
    assert!(g.gain_db(-46.5) < -1.0, "closes below threshold - hysteresis");
    assert!(!g.is_open());
    assert!(g.gain_db(-43.0) < -1.0, "and stays closed in between");
    assert!(g.gain_db(-39.0) == 0.0);
}

// ---- Dynamics --------------------------------------------------------------

fn gain_params() -> DynamicsParams {
    DynamicsParams {
        threshold_db: -20.0,
        ratio: 4.0,
        knee_db: 0.0,
        attack_ms: 10.0,
        release_ms: 80.0,
        smoothing: Smoothing::Gain,
        ..DynamicsParams::default()
    }
}

#[test]
fn compressor_static_gain_at_steady_state() {
    for smoothing in [Smoothing::Level, Smoothing::Gain] {
        for topology in [Topology::Branching, Topology::Decoupled] {
            let mut d = Dynamics::new(FS);
            d.set_params(DynamicsParams { smoothing, topology, attack_ms: 1.0, release_ms: 10.0, ..gain_params() });
            let mut buf = vec![0.5f32; 48_000]; // -6.02 dB
            d.process(&mut [&mut buf]);
            // Expected: (1/4 - 1) * (-6.02 + 20) = -10.485 dB.
            let want = (0.25 - 1.0) * (db(0.5) + 20.0);
            let got = db(buf[47_999] / 0.5);
            assert!((got - want).abs() < 0.01, "{smoothing:?} {topology:?}: {got} vs {want}");
        }
    }
}

#[test]
fn gain_reduction_attack_and_release_time_constants() {
    let mut d = Dynamics::new(FS);
    d.set_params(gain_params());
    // Step to 0 dB: final gain reduction 15 dB. 63.2 % after the 10 ms attack.
    let n_att = {
        let mut n = 0;
        loop {
            n += 1;
            let mut s = [1.0f32];
            d.process(&mut [&mut s]);
            if d.gain_reduction_db() >= 15.0 * (1.0 - (-1.0f32).exp()) || n > 100_000 {
                break n;
            }
        }
    };
    let want = 0.010 * FS;
    assert!((n_att as f32 - want).abs() <= want * 0.03 + 1.0, "attack {n_att} vs {want}");
    for _ in 0..48_000 {
        let mut s = [1.0f32];
        d.process(&mut [&mut s]);
    }
    assert!((d.gain_reduction_db() - 15.0).abs() < 0.01);
    // Release to silence: GR falls to 1/e of 15 dB after 80 ms.
    let mut n = 0;
    loop {
        n += 1;
        let mut s = [0.0f32];
        d.process(&mut [&mut s]);
        if d.gain_reduction_db() <= 15.0 * (-1.0f32).exp() || n > 1_000_000 {
            break;
        }
    }
    let want = 0.080 * FS;
    assert!((n as f32 - want).abs() <= want * 0.03 + 1.0, "release {n} vs {want}");
}

#[test]
fn makeup_is_added() {
    let mut d = Dynamics::new(FS);
    d.set_params(DynamicsParams { makeup_db: 6.0, threshold_db: 0.0, ..gain_params() });
    let mut buf = vec![0.1f32; 4800];
    d.process(&mut [&mut buf]);
    assert!((db(buf[4799] / 0.1) - 6.0).abs() < 0.01);
}

#[test]
fn gate_hysteresis_in_the_processor_stops_chatter() {
    // A level hovering between the closing and opening thresholds must not
    // toggle the gate.
    let mut d = Dynamics::new(FS);
    d.set_params(DynamicsParams {
        mode: DynamicsMode::Gate,
        threshold_db: -30.0,
        hysteresis_db: 10.0,
        range_db: 60.0,
        knee_db: 0.0,
        attack_ms: 0.5,
        release_ms: 5.0,
        smoothing: Smoothing::Gain,
        ..DynamicsParams::default()
    });
    let level = |db: f32| 10f32.powf(db / 20.0);
    let run = |d: &mut Dynamics, a: f32, n: usize| {
        let mut b = vec![a; n];
        d.process(&mut [&mut b]);
        db(b[n - 1] / a)
    };
    // Rising through -35 dB: still closed.
    assert!(run(&mut d, level(-35.0), 4800) < -10.0);
    // Open with a loud burst, then back to -35 dB: stays open.
    assert!(run(&mut d, level(-20.0), 4800) > -0.01);
    assert!(run(&mut d, level(-35.0), 4800) > -0.01);
    // Below -40 dB it closes.
    assert!(run(&mut d, level(-45.0), 4800) < -10.0);
    // And at -35 dB it is closed again.
    assert!(run(&mut d, level(-35.0), 4800) < -10.0);
}

#[test]
fn hold_keeps_the_gate_open_between_bursts() {
    let mut d = Dynamics::new(FS);
    d.set_params(DynamicsParams {
        mode: DynamicsMode::Gate,
        threshold_db: -30.0,
        range_db: 60.0,
        knee_db: 0.0,
        attack_ms: 0.0,
        release_ms: 1.0,
        hold_ms: 50.0,
        smoothing: Smoothing::Gain,
        ..DynamicsParams::default()
    });
    let mut open = vec![0.5f32; 480];
    d.process(&mut [&mut open]);
    let quiet_amp = 1e-4f32;
    let mut quiet = vec![quiet_amp; 2000]; // 41 ms < 50 ms hold
    d.process(&mut [&mut quiet]);
    assert!(quiet[1999] > quiet_amp * 0.99, "still open during the hold");
    let mut more = vec![quiet_amp; 2000]; // hold has run out
    d.process(&mut [&mut more]);
    assert!(more[1999] < quiet_amp * 0.1, "closed after the hold and release");
}

#[test]
fn linked_detection_gives_every_channel_the_same_gain() {
    for link in [Link::Max, Link::Average] {
        for n_ch in [1usize, 2, 6, 8] {
            let mut d = Dynamics::new(FS);
            d.set_params(DynamicsParams { link, attack_ms: 2.0, release_ms: 20.0, ..gain_params() });
            let frames = 6000;
            let input: Vec<Vec<f32>> = (0..n_ch)
                .map(|c| {
                    (0..frames)
                        .map(|n| {
                            let amp = 0.05 + 0.9 * (c as f32 + 1.0) / n_ch as f32;
                            amp * (n as f32 * 0.03 * (c as f32 + 1.0)).sin()
                        })
                        .collect()
                })
                .collect();
            let mut out = input.clone();
            {
                let mut refs: Vec<&mut [f32]> = out.iter_mut().map(|v| v.as_mut_slice()).collect();
                d.process(&mut refs);
            }
            let mut reduced = false;
            for n in 0..frames {
                // Gain of each channel where its input is not tiny.
                let mut gains = Vec::new();
                for c in 0..n_ch {
                    if input[c][n].abs() > 0.02 {
                        gains.push(out[c][n] / input[c][n]);
                    }
                }
                for g in &gains {
                    assert!((g - gains[0]).abs() < 1e-5, "{link:?} {n_ch}ch frame {n}: {g} vs {}", gains[0]);
                    if *g < 0.9 {
                        reduced = true;
                    }
                }
            }
            assert!(reduced, "{link:?} {n_ch}ch: compression happened");
        }
    }
}

#[test]
fn max_link_follows_the_loudest_channel_average_follows_the_mean() {
    let run = |link| {
        let mut d = Dynamics::new(FS);
        d.set_params(DynamicsParams { link, attack_ms: 1.0, release_ms: 10.0, threshold_db: -30.0, ..gain_params() });
        let mut l = vec![0.5f32; 4800];
        let mut r = vec![0.0f32; 4800];
        d.process(&mut [&mut l, &mut r]);
        d.gain_reduction_db()
    };
    let max = run(Link::Max);
    let avg = run(Link::Average);
    assert!(max > avg + 1.0, "{max} vs {avg}");
}

#[test]
fn sidechain_input_and_filter_hook() {
    // External key: a loud key ducks a quiet program; a silent key does not.
    let mut d = Dynamics::new(FS);
    d.set_params(DynamicsParams { attack_ms: 0.5, release_ms: 10.0, ..gain_params() });
    let mut prog = vec![0.01f32; 4800];
    let loud = vec![0.8f32; 4800];
    d.process_sidechain(&mut [&mut prog], &[&loud]);
    assert!(prog[4799] < 0.01 * 0.4);
    d.reset();
    let mut prog = vec![0.01f32; 4800];
    let silent = vec![0.0f32; 4800];
    d.process_sidechain(&mut [&mut prog], &[&silent]);
    assert!((prog[4799] - 0.01).abs() < 1e-6);

    // Filter hook: a one-pole low-pass in the key path makes a high-frequency
    // tone nearly invisible to the detector, while a low tone still triggers.
    let tone = |f: f32| -> Vec<f32> { (0..9600).map(|n| 0.8 * (std::f32::consts::TAU * f * n as f32 / FS).sin()).collect() };
    let gr = |f: f32| {
        let mut d = Dynamics::new(FS);
        d.set_params(DynamicsParams { attack_ms: 0.5, release_ms: 10.0, ..gain_params() });
        let mut buf = tone(f);
        let mut lp = [0.0f32; 1];
        let a = 0.02f32;
        let mut worst = 0.0f32;
        let mut key_copy = buf.clone();
        d.process_filtered(&mut [&mut buf], Some(&[&key_copy.clone()]), |c, x| {
            lp[c] += a * (x - lp[c]);
            lp[c]
        });
        key_copy.clear();
        worst = worst.max(d.gain_reduction_db());
        worst
    };
    let low = gr(100.0);
    let high = gr(15_000.0);
    assert!(low > 5.0, "{low}");
    assert!(high < 1.0, "{high}");
}

#[test]
fn rms_detection_reads_below_peak_for_a_crest() {
    let tone: Vec<f32> = (0..9600).map(|n| 0.8 * (std::f32::consts::TAU * 200.0 * n as f32 / FS).sin()).collect();
    let gr = |det| {
        let mut d = Dynamics::new(FS);
        d.set_params(DynamicsParams { detection: det, rms_ms: 20.0, attack_ms: 1.0, release_ms: 30.0, ..gain_params() });
        let mut b = tone.clone();
        d.process(&mut [&mut b]);
        d.gain_reduction_db()
    };
    assert!(gr(Detection::Peak) > gr(Detection::Rms), "an RMS detector sees 3 dB less");
}

#[test]
fn hostile_input_is_finite() {
    let mut d = Dynamics::new(FS);
    d.set_params(gain_params());
    let mut a = vec![f32::NAN, f32::INFINITY, -f32::INFINITY, 1e30, -1e30, 1e-42, 0.0, 0.5];
    let mut b = a.clone();
    d.process(&mut [&mut a, &mut b]);
    // NaN passes through multiplied by a finite gain; the gain itself is finite.
    assert!(d.gain_db().is_finite());
    d.process(&mut []);
    let mut empty: [f32; 0] = [];
    d.process(&mut [&mut empty]);
    let mut e = vec![0.1f32; 3];
    d.process_sidechain(&mut [&mut e], &[]);
}

// ---- limiter ---------------------------------------------------------------

/// Independent reference: 16x windowed-sinc interpolation (Hann, radius 48
/// samples), evaluated directly in f64. Returns the largest |value| over the
/// interior of the signal.
fn reference_true_peak(x: &[f32]) -> f64 {
    const R: usize = 16;
    const RADIUS: i64 = 48;
    let n = x.len() as i64;
    let mut peak = 0.0f64;
    for i in RADIUS..(n - RADIUS - 1) {
        for s in 0..R {
            let t = s as f64 / R as f64;
            let mut acc = 0.0f64;
            for k in -RADIUS + 1..=RADIUS {
                let d = t - k as f64;
                let sinc = if d == 0.0 { 1.0 } else { (std::f64::consts::PI * d).sin() / (std::f64::consts::PI * d) };
                let w = 0.5 * (1.0 + (std::f64::consts::PI * d / (RADIUS as f64 + 1.0)).cos());
                acc += x[(i + k) as usize] as f64 * sinc * w;
            }
            peak = peak.max(acc.abs());
        }
    }
    peak
}

fn run_limiter(input: &[f32], ceiling_db: f32, lookahead_ms: f32, release_ms: f32) -> (Vec<f32>, usize) {
    let mut lim = Limiter::prepare(FS, 1, lookahead_ms, release_ms);
    lim.set_ceiling_db(ceiling_db);
    let latency = lim.latency_samples();
    let mut buf = input.to_vec();
    // Process in odd-sized blocks to exercise block boundaries.
    let mut pos = 0;
    let mut size = 1;
    while pos < buf.len() {
        let end = (pos + size).min(buf.len());
        lim.process(&mut [&mut buf[pos..end]]);
        pos = end;
        size = size * 3 % 97 + 1;
    }
    (buf, latency)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
    }
}

fn test_signals() -> Vec<(&'static str, Vec<f32>)> {
    let len: usize = 24_000;
    let mut sigs: Vec<(&'static str, Vec<f32>)> = Vec::new();
    // fs/4 sine at 45 degrees: sample peak 0.707 * A, true peak A.
    sigs.push((
        "fs/4 at 45 deg",
        (0..len).map(|n| 1.6 * (std::f32::consts::FRAC_PI_2 * n as f32 + std::f32::consts::FRAC_PI_4).sin()).collect(),
    ));
    sigs.push((
        "fs/4 burst",
        (0..len)
            .map(|n| {
                let on = (n / 2400) % 2 == 0;
                if on { 2.0 * (std::f32::consts::FRAC_PI_2 * n as f32 + std::f32::consts::FRAC_PI_4).sin() } else { 0.05 }
            })
            .collect(),
    ));
    let hard_square: Vec<f32> = (0..len)
        .map(|n| {
            let ph = (n / 37) % 2;
            let burst = (n / 3000) % 2 == 0;
            if burst { if ph == 0 { 1.4 } else { -1.4 } } else { 0.1 * ph as f32 }
        })
        .collect();
    // Square-ish: edges rounded by a 3-tap smoother, as any real source's would be.
    let smoothed: Vec<f32> = (0..len)
        .map(|n| 0.25 * hard_square[n.saturating_sub(1)] + 0.5 * hard_square[n] + 0.25 * hard_square[(n + 1).min(len - 1)])
        .collect();
    sigs.push(("square-ish bursts", smoothed));
    let mut rng = Rng(7);
    // Noise band-limited to 0.4 fs (101-tap Hann-windowed sinc), normalised to a 1.8 sample peak.
    let white: Vec<f32> = (0..len + 100).map(|_| rng.next()).collect();
    let kernel: Vec<f64> = (-50i32..=50)
        .map(|k| {
            let x = 0.8 * k as f64;
            let sinc = if k == 0 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
            0.8 * sinc * (0.5 + 0.5 * (std::f64::consts::PI * k as f64 / 51.0).cos())
        })
        .collect();
    let filtered: Vec<f32> = (0..len)
        .map(|n| kernel.iter().enumerate().map(|(j, h)| h * white[n + j] as f64).sum::<f64>() as f32)
        .collect();
    let top = filtered.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    sigs.push(("noise to 0.4 fs", filtered.iter().map(|v| 1.8 * v / top).collect()));
    sigs.push((
        "high sines",
        (0..len)
            .map(|n| {
                let t = n as f32;
                1.2 * (std::f32::consts::TAU * 0.43 * t).sin() + 0.9 * (std::f32::consts::TAU * 0.21 * t + 0.5).sin()
            })
            .collect(),
    ));
    sigs.push((
        "sweep to Nyquist",
        (0..len)
            .map(|n| {
                let t = n as f64 / len as f64;
                (1.7 * (std::f64::consts::TAU * (0.5 * 0.5 * t * t) * len as f64 / 2.0 * 0.05).sin()) as f32
                    + 1.5 * (std::f64::consts::TAU * 0.4999 * n as f64 + 0.3).sin() as f32
            })
            .collect(),
    ));
    sigs
}

#[test]
fn limiter_true_peak_never_exceeds_the_ceiling() {
    let ceiling_db = -1.0f32;
    let ceiling = 10f64.powf(ceiling_db as f64 / 20.0);
    let mut worst_db = f64::NEG_INFINITY;
    for (name, input) in test_signals() {
        let (out, latency) = run_limiter(&input, ceiling_db, 1.5, 50.0);
        // Skip the start-up where the delay line is still silent.
        let tp = reference_true_peak(&out[latency..]);
        let excess_db = 20.0 * (tp / ceiling).log10();
        eprintln!("{name:>18}: reference true peak {:.3} dBTP, excess over ceiling {excess_db:+.4} dB", 20.0 * tp.log10());
        worst_db = worst_db.max(excess_db);
        assert!(out.iter().all(|v| v.is_finite()));
        // The input really was over the ceiling, so the limiter did work.
        assert!(reference_true_peak(&input) > ceiling * 1.05, "{name} test signal too quiet");
        assert!(excess_db < 0.1, "{name}: {excess_db} dB over the ceiling");
    }
    eprintln!("worst excess {worst_db:+.4} dB");
}

#[test]
fn intersample_peak_is_caught_where_a_sample_peak_limiter_fails() {
    let input: Vec<f32> = (0..24_000)
        .map(|n| 1.0 * (std::f32::consts::FRAC_PI_2 * n as f32 + std::f32::consts::FRAC_PI_4).sin())
        .collect(); // sample peak 0.707, true peak 1.0
    let ceiling_db = -3.0;
    let ceiling = 10f64.powf(ceiling_db / 20.0); // 0.708: sample peak just at ceiling
    let mut sample_lim = Limiter::prepare(FS, 1, 1.0, 50.0);
    sample_lim.set_ceiling_db(ceiling_db as f32);
    sample_lim.set_true_peak(false);
    let mut a = input.clone();
    sample_lim.process(&mut [&mut a]);
    let lat = sample_lim.latency_samples();
    let sample_mode_tp = reference_true_peak(&a[lat..]);
    assert!(sample_mode_tp > ceiling * 1.3, "sample-peak mode lets the intersample peak through ({sample_mode_tp})");
    let (b, lat) = run_limiter(&input, ceiling_db as f32, 1.0, 50.0);
    let tp = reference_true_peak(&b[lat..]);
    assert!(tp <= ceiling * 1.006, "true-peak mode holds the ceiling ({tp} vs {ceiling})");
}

/// Sample-peak mode measures the sample that is about to leave the delay line,
/// so even the fastest release cannot let a lone peak through.
#[test]
fn sample_peak_mode_holds_the_sample_ceiling_with_the_fastest_release() {
    let ceiling = 10f32.powf(-1.0 / 20.0);
    for fs in [44_100.0f32, 48_000.0, 192_000.0] {
        let mut lim = Limiter::prepare(fs, 1, 1.0, 1.0);
        lim.set_ceiling_db(-1.0);
        lim.set_true_peak(false);
        let mut x: Vec<f32> = (0..20_000)
            .map(|n| match n % 701 {
                0 => 2.0,
                350 => -3.0,
                351 => 2.5,
                _ => 0.1 * (n as f32 * 0.3).sin(),
            })
            .collect();
        lim.process(&mut [&mut x]);
        let top = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(top <= ceiling * 1.000_01, "{fs}: sample peak {top} over {ceiling}");
        assert!(top > ceiling * 0.99, "{fs}: the peaks reach the ceiling ({top})");
    }
}

#[test]
fn limiter_is_transparent_below_the_ceiling_and_reports_its_latency() {
    let mut lim = Limiter::prepare(FS, 2, 2.0, 100.0);
    lim.set_ceiling_db(-0.1);
    let latency = lim.latency_samples();
    // The lookahead plus the detector's 48 frames.
    assert_eq!(latency, lim.lookahead_samples() + 48);
    assert_eq!(lim.lookahead_samples(), 96);
    let input: Vec<f32> = (0..5000).map(|n| 0.5 * (n as f32 * 0.05).sin()).collect();
    let mut l = input.clone();
    let mut r = input.iter().map(|v| -v).collect::<Vec<_>>();
    lim.process(&mut [&mut l, &mut r]);
    for n in latency..5000 {
        assert_eq!(l[n], input[n - latency]);
        assert_eq!(r[n], -input[n - latency]);
    }
    assert_eq!(lim.gain_reduction_db(), 0.0);
    // The first `latency` frames are silence.
    assert!(l[..latency].iter().all(|&v| v == 0.0));
}

#[test]
fn limiter_gain_moves_smoothly() {
    // A loud burst in quiet material: the gain falls and recovers without steps.
    let lookahead_ms = 2.0;
    let l = (FS * 0.002) as usize;
    let input: Vec<f32> =
        (0..20_000).map(|n| if (5000..5400).contains(&n) { 3.0 * (n as f32 * 0.2).sin() } else { 0.2 * (n as f32 * 0.05).sin() }).collect();
    let (out, latency) = run_limiter(&input, -1.0, lookahead_ms, 80.0);
    let mut prev: Option<f32> = None;
    let mut max_step = 0.0f32;
    let mut min_gain = 1.0f32;
    for n in latency..20_000 {
        let x = input[n - latency];
        if x.abs() > 0.1 {
            let g = out[n] / x;
            if let Some(p) = prev {
                max_step = max_step.max((g - p).abs());
            }
            min_gain = min_gain.min(g);
            prev = Some(g);
        } else {
            prev = None;
        }
    }
    assert!(min_gain < 0.45, "limiter engaged ({min_gain})");
    // A moving average of L frames of a signal bounded by [0, 1] moves by at most 1/L per frame.
    assert!(max_step <= 1.05 / l as f32 + 0.002, "gain step {max_step}");
}

#[test]
fn limiter_links_channels() {
    let mut lim = Limiter::prepare(FS, 2, 1.0, 50.0);
    lim.set_ceiling_db(-6.0);
    let mut l: Vec<f32> = (0..9600).map(|n| 2.0 * (n as f32 * 0.1).sin()).collect();
    let mut r: Vec<f32> = (0..9600).map(|n| 0.1 * (n as f32 * 0.1).sin()).collect();
    let (il, ir) = (l.clone(), r.clone());
    lim.process(&mut [&mut l, &mut r]);
    let lat = lim.latency_samples();
    for n in lat + 5000..9600 {
        if il[n - lat].abs() > 0.5 {
            let gl = l[n] / il[n - lat];
            let gr = r[n] / ir[n - lat];
            assert!((gl - gr).abs() < 1e-5);
        }
    }
}

#[test]
fn limiter_survives_hostile_input_and_odd_shapes() {
    let mut lim = Limiter::prepare(FS, 2, 1.0, 50.0);
    let mut a = vec![f32::NAN, f32::INFINITY, -f32::INFINITY, 1e30, -1e30, 1e-42, 0.0, 0.5];
    let mut b = a.clone();
    lim.process(&mut [&mut a, &mut b]);
    let mut more = vec![0.0f32; 400];
    lim.process(&mut [&mut more, &mut vec![0.0; 400]]);
    assert!(a.iter().chain(b.iter()).all(|v| v.is_finite()));
    lim.process(&mut []);
    let mut e: [f32; 0] = [];
    lim.process(&mut [&mut e]);
    // More channels than prepared: extras untouched. Fewer: fine.
    let mut c0 = vec![0.1f32; 8];
    let mut c1 = vec![0.1f32; 8];
    let mut c2 = vec![0.7f32; 8];
    lim.process(&mut [&mut c0, &mut c1, &mut c2]);
    assert!(c2.iter().all(|&v| v == 0.7));
    lim.process(&mut [&mut c0]);
    // Extreme parameters.
    for (fs, ms, rel) in [(1000.0f32, 50.0f32, 5000.0f32), (768_000.0, 0.1, 1.0), (f32::NAN, f32::NAN, f32::NAN), (0.0, -3.0, -1.0)] {
        let mut l = Limiter::prepare(fs, 0, ms, rel);
        l.set_ceiling_db(f32::NAN);
        let mut x = vec![1.0f32; 2000];
        l.process(&mut [&mut x]);
        assert!(x.iter().all(|v| v.is_finite()));
    }
}

#[test]
fn limiter_works_at_other_sample_rates() {
    for fs in [8000.0f32, 44_100.0, 96_000.0, 192_000.0] {
        let mut lim = Limiter::prepare(fs, 1, 1.0, 50.0);
        lim.set_ceiling_db(-2.0);
        let lat = lim.latency_samples();
        let want = (fs * 0.001).round() as usize + 48;
        assert_eq!(lat, want, "{fs}");
        let mut x: Vec<f32> = (0..(fs as usize / 2)).map(|n| 1.5 * (std::f32::consts::FRAC_PI_2 * n as f32 + 0.8).sin()).collect();
        lim.process(&mut [&mut x]);
        let tp = reference_true_peak(&x[lat..]);
        assert!(tp < 10f64.powf(-2.0 / 20.0) * 1.006, "{fs}: {tp}");
    }
}

/// A hard square wave is not band-limited: an ideal brick-wall reconstruction
/// rings by about 9 %. The detector's long filter sees that ringing, so the
/// ceiling holds against the 16x reference here too.
#[test]
fn hard_square_wave_excess_is_small_against_an_ideal_reconstruction() {
    let input: Vec<f32> = (0..24_000).map(|n| if (n / 37) % 2 == 0 { 1.4 } else { -1.4 }).collect();
    let (out, latency) = run_limiter(&input, -1.0, 1.5, 50.0);
    let tp = reference_true_peak(&out[latency..]);
    let excess_db = 20.0 * (tp / 10f64.powf(-1.0 / 20.0)).log10();
    eprintln!("hard square: excess {excess_db:+.3} dB over the ceiling by the 16x reference");
    assert!(excess_db < 0.1, "{excess_db}");
}

/// Full-band white noise has energy right up to Nyquist, where a short
/// detector filter reads low (the old 16-tap detector let this case through by
/// 1.5 dB here and up to 1.9 dB in `tests/true_peak_ceiling.rs`). The limiter's
/// detector now includes a filter as long as the reference's, and the ceiling
/// holds. `tests/true_peak_ceiling.rs` has the full matrix of signals and rates.
#[test]
fn full_band_noise_excess_is_bounded() {
    let mut rng = Rng(11);
    let input: Vec<f32> = (0..24_000).map(|_| 1.8 * rng.next()).collect();
    let (out, latency) = run_limiter(&input, -1.0, 1.5, 50.0);
    let tp = reference_true_peak(&out[latency..]);
    let excess_db = 20.0 * (tp / 10f64.powf(-1.0 / 20.0)).log10();
    eprintln!("full-band noise: excess {excess_db:+.3} dB over the ceiling by the 16x reference");
    assert!(excess_db < 0.1, "{excess_db}");
}
