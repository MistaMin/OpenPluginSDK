//! Proofs for the modulation and delay blocks: interpolator delay against
//! theory, click-free sweeps, LFO lock to the host transport, phaser notches
//! against theory, and flanger/chorus behaviour.

use openplugin_core::Transport;
use openplugin_dsp::{
    group_delay, phase_delay, Chorus, Feel, Flanger, Interpolation, Lfo, LfoRate, LfoShape,
    ModDelay, NoteDivision, Phaser,
};
use std::f64::consts::PI;

/// DFT of `h` at angular frequency `w`: `(re, im)`.
fn dft(h: &[f32], w: f64) -> (f64, f64) {
    let (mut re, mut im) = (0.0, 0.0);
    for (n, v) in h.iter().enumerate() {
        let ph = -w * n as f64;
        re += f64::from(*v) * ph.cos();
        im += f64::from(*v) * ph.sin();
    }
    (re, im)
}

/// Phase delay and group delay (samples) measured from an impulse response.
fn measured_delays(h: &[f32], w: f64) -> (f64, f64) {
    let (re, im) = dft(h, w);
    let ideal_guess = 0.0; // unwrap handled by the caller's small w
    let _ = ideal_guess;
    let pd = -im.atan2(re) / w;
    let step = 1e-4;
    let p = |x: f64| {
        let (r, i) = dft(h, x);
        i.atan2(r)
    };
    let mut d = p(w + step) - p(w - step);
    d -= 2.0 * PI * (d / (2.0 * PI)).round();
    (pd, -d / (2.0 * step))
}

fn impulse_response(interp: Interpolation, delay: f32, len: usize) -> Vec<f32> {
    let mut line = ModDelay::new(48_000.0, 20.0, interp);
    (0..len).map(|n| line.process(if n == 0 { 1.0 } else { 0.0 }, delay)).collect()
}

const KINDS: [Interpolation; 7] = [
    Interpolation::Linear,
    Interpolation::Hermite,
    Interpolation::Lagrange(3),
    Interpolation::Lagrange(5),
    Interpolation::Thiran(1),
    Interpolation::Thiran(2),
    Interpolation::Thiran(3),
];

#[test]
fn delay_matches_theory_at_several_fractions() {
    // Low frequency: w = 0.03 pi (720 Hz at 48 kHz).
    let w = 0.03 * PI;
    for interp in KINDS {
        for base in [8.0f32, 20.0] {
            for frac in [0.0f32, 0.1, 0.25, 0.4, 0.5, 0.6, 0.75, 0.9] {
                let d = base + frac;
                let h = impulse_response(interp, d, 6000);
                let (pd, gd) = measured_delays(&h, w);
                // 1. The running delay line agrees with the coefficient theory.
                let pd_theory = phase_delay(interp, d, w as f32);
                let gd_theory = group_delay(interp, d, w as f32);
                assert!((pd - pd_theory).abs() < 2e-4, "{interp:?} d={d} pd {pd} vs {pd_theory}");
                assert!((gd - gd_theory).abs() < 2e-3, "{interp:?} d={d} gd {gd} vs {gd_theory}");
                // 2. Theory against the ideal delay. Maximally flat designs are
                // exact at DC, so at 0.03 pi the error is tiny; linear is the
                // poorest.
                let bound = match interp {
                    Interpolation::Linear => 0.02,
                    Interpolation::Hermite => 0.01,
                    Interpolation::Lagrange(3) | Interpolation::Thiran(_) => 0.002,
                    _ => 2e-4,
                };
                let gd_err = (gd_theory - f64::from(d)).abs();
                let pd_err = (pd_theory - f64::from(d)).abs();
                assert!(gd_err < bound * 2.0 && pd_err < bound, "{interp:?} d={d}: pd err {pd_err}, gd err {gd_err}");
            }
        }
    }
}

#[test]
fn delay_at_dc_is_exact_for_maximally_flat_designs() {
    // Group delay at DC equals D for Lagrange and Thiran (theory), tiny omega.
    for interp in [
        Interpolation::Linear,
        Interpolation::Lagrange(3),
        Interpolation::Lagrange(4),
        Interpolation::Thiran(1),
        Interpolation::Thiran(2),
        Interpolation::Thiran(3),
        Interpolation::Thiran(4),
    ] {
        for frac in [0.2f32, 0.5, 0.8] {
            let d = 12.0 + frac;
            let gd = group_delay(interp, d, 0.002);
            assert!((gd - f64::from(d)).abs() < 1e-3, "{interp:?} d={d} gd={gd}");
        }
    }
}

#[test]
fn print_error_table() {
    // Documentation aid: delay error (samples) at fractions 0.25 and 0.5.
    if std::env::var("OPENPLUGIN_TABLE").is_err() {
        return;
    }
    for interp in KINDS {
        for w_pi in [0.05, 0.1, 0.25, 0.5] {
            for frac in [0.25f32, 0.5] {
                let d = 12.0 + frac;
                let w = (w_pi * PI) as f32;
                println!(
                    "{interp:?} w={w_pi}pi frac={frac}: pd err {:+.5} gd err {:+.5} |H| {:.4}",
                    phase_delay(interp, d, w) - f64::from(d),
                    group_delay(interp, d, w) - f64::from(d),
                    {
                        let (r, i) = interp.frequency_response(d, w);
                        (r * r + i * i).sqrt()
                    }
                );
            }
        }
    }
}

#[test]
fn thiran_is_allpass_and_the_others_are_not_louder_than_unity_much() {
    for n in 1..=4u8 {
        for frac in [0.0f32, 0.3, 0.7] {
            let interp = Interpolation::Thiran(n);
            let d = 10.0 + frac;
            for w in [0.1f32, 0.8, 1.7, 2.9] {
                let (r, i) = interp.frequency_response(d, w);
                assert!(((r * r + i * i).sqrt() - 1.0).abs() < 1e-9, "N={n} w={w}");
            }
        }
    }
    for interp in [Interpolation::Linear, Interpolation::Lagrange(3), Interpolation::Hermite] {
        for k in 1..60 {
            let w = k as f32 * 0.05;
            let (r, i) = interp.frequency_response(10.5, w);
            assert!((r * r + i * i).sqrt() < 1.05, "{interp:?} w={w}");
        }
    }
}

/// Largest sample-to-sample step of a sine passing through a delay swept by a sine.
fn max_jump(interp: Interpolation, tone_hz: f64, sweep_hz: f64, centre_ms: f64, depth_ms: f64) -> (f64, f64) {
    let fs = 48_000.0f64;
    let mut line = ModDelay::new(fs as f32, 40.0, interp);
    let centre = centre_ms * fs / 1000.0;
    let depth = depth_ms * fs / 1000.0;
    let mut prev = 0.0f64;
    let (mut max, mut max_rate): (f64, f64) = (0.0, 0.0);
    let n = (fs * 2.0) as usize;
    for i in 0..n {
        let t = i as f64 / fs;
        let x = (2.0 * PI * tone_hz * t).sin();
        let d = centre + depth * (2.0 * PI * sweep_hz * t).sin();
        let y = f64::from(line.process(x as f32, d as f32));
        if i > 2000 {
            max = max.max((y - prev).abs());
        }
        prev = y;
        max_rate = max_rate.max(depth * 2.0 * PI * sweep_hz / fs);
    }
    (max, max_rate)
}

#[test]
fn swept_delay_has_no_clicks() {
    // y(n) = x(n - d(n)): the step is bounded by the tone's slope times
    // (1 + max |dd/dn|). A click would exceed that by a large factor.
    let tone = 1000.0;
    let w = 2.0 * PI * tone / 48_000.0;
    for interp in [
        Interpolation::Linear,
        Interpolation::Hermite,
        Interpolation::Lagrange(3),
        Interpolation::Lagrange(5),
    ] {
        // Depth 5 ms at 2 Hz: delay slope about 0.97 samples per sample / 100.
        let (jump, rate) = max_jump(interp, tone, 2.0, 12.0, 5.0);
        let bound = w * (1.0 + rate) * 1.03;
        println!("{interp:?}: max jump {jump:.5} bound {bound:.5} (slope {rate:.5})");
        assert!(jump <= bound, "{interp:?} jump {jump} > {bound}");
    }
    // A fast sweep (30 Hz, 1 ms deep) shifts pitch heavily but still must not click.
    for interp in [Interpolation::Hermite, Interpolation::Lagrange(3)] {
        let (jump, rate) = max_jump(interp, tone, 30.0, 6.0, 1.0);
        let bound = w * (1.0 + rate) * 1.05;
        assert!(jump <= bound, "{interp:?} fast jump {jump} > {bound}");
    }
    // Thiran: its state sees every coefficient change. Inside one integer
    // interval of the allpass delay it is clean; across integer crossings it
    // is not, which is why chorus and flanger refuse it (documented).
    let fs = 48_000.0f64;
    let sweep = |centre: f64, depth: f64| {
        let mut line = ModDelay::new(fs as f32, 40.0, Interpolation::Thiran(2));
        let (mut prev, mut max) = (0.0f64, 0.0f64);
        let mut slope: f64 = 0.0;
        for i in 0..(fs as usize * 2) {
            let t = i as f64 / fs;
            let x = (2.0 * PI * tone * t).sin();
            let d = centre + depth * (2.0 * PI * 0.5 * t).sin();
            slope = slope.max(depth * 2.0 * PI * 0.5 / fs);
            let y = f64::from(line.process(x as f32, d as f32));
            if i > 2000 {
                max = max.max((y - prev).abs());
            }
            prev = y;
        }
        (max, slope)
    };
    let (jump, slope) = sweep(11.0, 0.4);
    let bound = w * (1.0 + slope) * 1.1;
    println!("Thiran(2) inside one interval: max jump {jump:.5} bound {bound:.5}");
    assert!(jump <= bound, "Thiran jump {jump} > {bound}");
    let (crossing, _) = sweep(12.0, 5.0);
    println!("Thiran(2) across integer crossings: max jump {crossing:.5} (not click-free, by design)");
}

#[test]
fn an_abrupt_delay_change_clicks_without_glide_and_not_with_it() {
    use openplugin_dsp::Glide;
    let fs = 48_000.0f32;
    let run = |glide: bool| {
        let mut line = ModDelay::new(fs, 40.0, Interpolation::Hermite);
        let mut g = Glide::new(fs, 20.0);
        let mut prev = 0.0f32;
        let mut max = 0.0f32;
        for i in 0..20_000 {
            let x = (i as f32 * 2.0 * std::f32::consts::PI * 500.0 / fs).sin();
            let target = if i < 5000 { 100.0 } else { 600.0 };
            let d = if glide { g.next(target) } else { target };
            let y = line.process(x, d);
            if i > 3000 {
                max = max.max((y - prev).abs());
            }
            prev = y;
        }
        max
    };
    let hard = run(false);
    let soft = run(true);
    println!("jump without glide {hard:.4}, with glide {soft:.4}");
    assert!(hard > 0.2, "the control must click");
    assert!(soft < 0.5 * 2.0 * std::f32::consts::PI * 500.0 / fs * 20.0, "glided jump {soft}");
}

fn transport(playing: bool, tempo: f64, quarter_notes: f64) -> Transport {
    Transport { playing, tempo, quarter_notes, numerator: 4, denominator: 4, ..Transport::default() }
}

/// Expected LFO value of the shape at host position `qn` for a cycle of `q` quarters.
fn expected_sine(qn: f64, q: f64) -> f32 {
    (2.0 * PI * (qn / q)).sin() as f32
}

#[test]
fn lfo_locks_to_the_transport_across_jumps_and_tempo_changes() {
    let fs = 44_100.0f32;
    let div = NoteDivision::new(4, Feel::Straight); // one cycle per quarter note
    assert_eq!(div.quarters(), 1.0);
    let mut lfo = Lfo::new(fs);
    lfo.set_rate(LfoRate::Sync(div));
    lfo.set_shape(LfoShape::Sine);
    let block = 512usize;

    // (tempo, start quarter-note) per block: play on, a loop jump back, a
    // locate far ahead, then tempo changes.
    let mut qn = 3.2f64;
    let mut tempo = 120.0f64;
    let mut worst = 0.0f32;
    for b in 0..400 {
        if b == 50 {
            qn = 0.0; // loop back to the start
        }
        if b == 100 {
            qn = 1234.37; // locate
        }
        if b == 150 {
            tempo = 97.5; // tempo change
        }
        if b == 250 {
            tempo = 211.0;
        }
        lfo.set_transport(&transport(true, tempo, qn));
        for i in 0..block {
            let host_qn = qn + i as f64 * tempo / 60.0 / f64::from(fs);
            let got = lfo.next();
            let want = expected_sine(host_qn, 1.0);
            worst = worst.max((got - want).abs());
        }
        qn += block as f64 * tempo / 60.0 / f64::from(fs);
    }
    println!("worst LFO error against the host position: {worst:e}");
    assert!(worst < 2e-4, "{worst}");
}

#[test]
fn lfo_division_lengths_and_phase_offset() {
    // Dotted eighth = 0.75 quarter, triplet quarter = 2/3, whole = 4.
    assert_eq!(NoteDivision::new(8, Feel::Dotted).quarters(), 0.75);
    assert!((NoteDivision::new(4, Feel::Triplet).quarters() - 2.0 / 3.0).abs() < 1e-12);
    assert_eq!(NoteDivision::new(1, Feel::Straight).quarters(), 4.0);

    let mut lfo = Lfo::new(48_000.0);
    lfo.set_rate(LfoRate::Sync(NoteDivision::new(1, Feel::Straight)));
    lfo.set_shape(LfoShape::Saw);
    lfo.set_phase_offset(0.25);
    // At quarter note 1.0 of a 4-quarter cycle the phase is 0.25, plus 0.25.
    lfo.set_transport(&transport(true, 100.0, 1.0));
    assert!((lfo.value() - 0.0).abs() < 1e-6, "{}", lfo.value()); // saw at 0.5 is 0
    // Bars: 8 quarters per cycle.
    lfo.set_rate(LfoRate::Quarters(8.0));
    lfo.set_phase_offset(0.0);
    lfo.set_transport(&transport(true, 100.0, 6.0));
    assert!((lfo.value() - 0.5).abs() < 1e-6); // phase 0.75
}

#[test]
fn stopped_host_free_runs_and_bad_transport_is_ignored() {
    let fs = 48_000.0;
    let mut lfo = Lfo::new(fs);
    lfo.set_rate(LfoRate::Sync(NoteDivision::new(4, Feel::Straight)));
    lfo.set_shape(LfoShape::Saw);
    lfo.set_transport(&transport(true, 120.0, 0.0));
    lfo.advance(1000);
    let p = lfo.position();
    // Stopped: position stays on its own track and still advances.
    lfo.set_transport(&transport(false, 120.0, 77.0));
    assert_eq!(lfo.position(), p);
    lfo.advance(1000);
    assert!(lfo.position() > p);
    // Non-finite quarter_notes and a zero tempo change nothing.
    let q = lfo.position();
    let inc = lfo.increment();
    lfo.set_transport(&transport(true, 0.0, f64::NAN));
    lfo.set_transport(&transport(true, f64::NAN, f64::INFINITY));
    assert_eq!(lfo.position(), q);
    assert_eq!(lfo.increment(), inc);
    assert!(lfo.next().is_finite());
    // Default transport (tempo 0) does not break a free-running LFO either.
    let mut free = Lfo::new(fs);
    free.set_rate(LfoRate::Hz(5.0));
    free.set_transport(&Transport::default());
    assert!(free.next().is_finite());
}

#[test]
fn random_lfo_returns_to_the_same_value_after_a_jump() {
    for shape in [LfoShape::SampleHold, LfoShape::SmoothRandom] {
        let mut a = Lfo::new(48_000.0);
        a.set_rate(LfoRate::Sync(NoteDivision::new(4, Feel::Straight)));
        a.set_shape(shape);
        let mut b = a.clone();
        a.set_transport(&transport(true, 120.0, 40.3));
        // b arrives at the same position by a very different route.
        b.set_transport(&transport(true, 120.0, 5.0));
        b.advance(777);
        b.set_transport(&transport(true, 120.0, 40.3));
        assert_eq!(a.value(), b.value());
    }
}

#[test]
fn free_run_lfo_shapes_have_zero_mean_and_full_range() {
    for shape in [LfoShape::Sine, LfoShape::Triangle, LfoShape::Saw, LfoShape::Square] {
        let mut l = Lfo::new(48_000.0);
        l.set_rate(LfoRate::Hz(3.0));
        l.set_shape(shape);
        let n = 48_000 * 10;
        let (mut sum, mut lo, mut hi) = (0.0f64, f32::MAX, f32::MIN);
        for _ in 0..n {
            let v = l.next();
            sum += f64::from(v);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        assert!((sum / n as f64).abs() < 1e-3, "{shape:?}");
        assert!(lo < -0.999 && hi > 0.999, "{shape:?} {lo} {hi}");
    }
}

/// Magnitude of the response of `process` (impulse in, many samples) at `w`.
fn magnitude(ir: &[f32], w: f64) -> f64 {
    let (r, i) = dft(ir, w);
    (r * r + i * i).sqrt()
}

#[test]
fn phaser_notches_sit_where_the_theory_puts_them() {
    for fs in [44_100.0f32, 48_000.0, 96_000.0] {
        for stages in [4usize, 6, 8, 10, 12] {
            for break_hz in [300.0f32, 1_000.0, 2_500.0] {
                let mut p = Phaser::new(1, fs);
                p.set_stages(stages);
                p.set_range(break_hz, break_hz);
                p.set_feedback(0.0);
                p.set_mix(0.5);
                let ir: Vec<f32> =
                    (0..16_384).map(|i| p.process_sample(0, if i == 0 { 1.0 } else { 0.0 })).collect();
                let mut want = [0.0f32; 8];
                let count = Phaser::notch_frequencies(fs, break_hz, stages, &mut want);
                assert_eq!(count, stages / 2);
                for &f in &want[..count] {
                    // Find the minimum of |H| within +-3% of the predicted notch.
                    let (mut best_f, mut best) = (0.0f64, f64::MAX);
                    let steps = 600;
                    for s in 0..=steps {
                        let ff = f64::from(f) * (0.97 + 0.06 * s as f64 / steps as f64);
                        let m = magnitude(&ir, 2.0 * PI * ff / f64::from(fs));
                        if m < best {
                            best = m;
                            best_f = ff;
                        }
                    }
                    let cents = 1200.0 * (best_f / f64::from(f)).log2();
                    assert!(cents.abs() < 2.0, "fs {fs} N {stages} fb {break_hz}: notch {f} found {best_f} ({cents} cents)");
                    assert!(best < 1e-3, "notch depth {best}");
                }
            }
        }
    }
}

#[test]
fn phaser_sweeps_between_its_limits_and_feedback_peaks() {
    let fs = 48_000.0;
    let mut p = Phaser::new(1, fs);
    p.set_stages(4);
    p.set_range(200.0, 4_000.0);
    p.set_rate(LfoRate::Hz(0.5));
    p.set_feedback(0.9);
    // Bounded and finite over a long sweep with feedback.
    let mut peak = 0.0f32;
    for i in 0..(fs as usize * 4) {
        let x = ((i as f32) * 0.07).sin() * 0.5;
        let y = p.process_sample(0, x);
        assert!(y.is_finite());
        peak = peak.max(y.abs());
    }
    assert!(peak < 20.0, "peak {peak}");
    // Feedback creates a peak above unity at the resonance of a fixed phaser.
    let mut q = Phaser::new(1, fs);
    q.set_stages(4);
    q.set_range(1_000.0, 1_000.0);
    q.set_feedback(0.9);
    q.set_mix(1.0);
    let ir: Vec<f32> = (0..32_768).map(|i| q.process_sample(0, if i == 0 { 1.0 } else { 0.0 })).collect();
    let max = (1..2000).map(|k| magnitude(&ir, PI * k as f64 / 2000.0)).fold(0.0, f64::max);
    assert!(max > 3.0, "resonant peak {max}");
}

#[test]
fn flanger_comb_notches_match_theory() {
    // dry + delayed (mix 0.5, no feedback): |1 + e^{-jwd}| = 0 at w d = pi (2k+1).
    let fs = 48_000.0f32;
    let mut f = Flanger::new(1, fs);
    f.set_delay_ms(1.0); // d = 48 samples, notches at 500 Hz + k 1000 Hz
    f.set_depth_ms(0.0);
    f.set_feedback(0.0);
    f.set_mix(0.5);
    let ir: Vec<f32> = (0..4096).map(|i| f.process_sample(0, if i == 0 { 1.0 } else { 0.0 })).collect();
    for k in 0..6 {
        let hz = 500.0 + 1000.0 * k as f64;
        assert!(magnitude(&ir, 2.0 * PI * hz / f64::from(fs)) < 1e-4, "{hz}");
        let peak = 1000.0 * (k + 1) as f64;
        assert!((magnitude(&ir, 2.0 * PI * peak / f64::from(fs)) - 1.0).abs() < 1e-4);
    }
    // Negative mix moves a null to DC.
    f.reset();
    f.set_mix(-0.5);
    let ir: Vec<f32> = (0..4096).map(|i| f.process_sample(0, if i == 0 { 1.0 } else { 0.0 })).collect();
    assert!(magnitude(&ir, 1e-6) < 1e-4);
}

#[test]
fn through_zero_flanger_cancels_when_the_paths_cross() {
    let fs = 48_000.0f32;
    let mut f = Flanger::new(1, fs);
    f.set_through_zero(true);
    f.set_delay_ms(2.0);
    f.set_depth_ms(1.9);
    f.set_feedback(0.0);
    f.set_mix(-0.5);
    f.set_shape(LfoShape::Sine);
    f.set_rate(LfoRate::Hz(2.0));
    // Pink-ish test signal: a swept tone. Measure output energy against input.
    let n = fs as usize * 2;
    let (mut min_ratio, mut max_ratio) = (f64::MAX, 0.0f64);
    let block = 480;
    let mut phase = 0.0f64;
    let mut i = 0;
    while i + block <= n {
        let (mut ein, mut eout) = (0.0f64, 0.0f64);
        for _ in 0..block {
            phase += 2.0 * PI * 90.0 / f64::from(fs); // low tone: wavelength >> delay at the crossing
            let x = phase.sin() as f32;
            let y = f.process_sample(0, x);
            ein += f64::from(x) * f64::from(x);
            eout += f64::from(y) * f64::from(y);
        }
        if i > 4800 {
            let r = eout / ein;
            min_ratio = min_ratio.min(r);
            max_ratio = max_ratio.max(r);
        }
        i += block;
    }
    println!("through-zero energy ratio range {min_ratio:.6} .. {max_ratio:.6}");
    assert!(min_ratio < 0.01, "paths never cancel: {min_ratio}");
    assert!(max_ratio > 0.05);
}

#[test]
fn chorus_detunes_by_the_delay_slope() {
    // A swept delay shifts pitch by -dd/dt. Depth 1 ms at 1 Hz: peak slope
    // 2 pi * 48 samples / 48000 = 0.00628 -> about 0.63 percent, 10.9 cents.
    let fs = 48_000.0f32;
    let mut c = Chorus::new(1, fs);
    c.set_voices(1);
    c.set_mix(1.0);
    c.set_delay_ms(10.0);
    c.set_depth_ms(1.0);
    c.set_rate(LfoRate::Hz(1.0));
    c.set_stereo_spread(0.0);
    let tone = 2_000.0f64;
    let mut zero_cross: Vec<f64> = Vec::new();
    let mut prev = 0.0f32;
    let n = fs as usize * 2;
    for i in 0..n {
        let x = (2.0 * PI * tone * i as f64 / f64::from(fs)).sin() as f32;
        let y = c.process_sample(0, x);
        if i > 4800 && prev < 0.0 && y >= 0.0 {
            zero_cross.push(i as f64 - f64::from(y) / f64::from(y - prev));
        }
        prev = y;
    }
    // Instantaneous frequency from consecutive crossings; spread should be ~+-0.63 %.
    let freqs: Vec<f64> = zero_cross.windows(2).map(|w| f64::from(fs) / (w[1] - w[0])).collect();
    let max = freqs.iter().cloned().fold(0.0, f64::max);
    let min = freqs.iter().cloned().fold(f64::MAX, f64::min);
    println!("chorus pitch range {:.2} .. {:.2} Hz", min, max);
    let expect = tone * 2.0 * PI * 48.0 / f64::from(fs);
    assert!((max - tone - expect).abs() < 0.15 * expect, "{}", max - tone);
    assert!((tone - min - expect).abs() < 0.15 * expect, "{}", tone - min);
}

#[test]
fn chorus_flanger_phaser_survive_every_rate_and_hostile_values() {
    for fs in [1_000.0f32, 8_000.0, 22_050.0, 48_000.0, 192_000.0, 768_000.0, f32::NAN] {
        let mut c = Chorus::new(2, fs);
        let mut f = Flanger::new(2, fs);
        let mut p = Phaser::new(2, fs);
        c.set_voices(8);
        c.set_depth_ms(f32::NAN);
        c.set_delay_ms(1e9);
        f.set_feedback(f32::INFINITY);
        p.set_range(f32::NAN, -5.0);
        for i in 0..3000 {
            let x = match i % 7 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                2 => f32::NEG_INFINITY,
                3 => 1e38,
                _ => ((i as f32) * 0.3).sin(),
            };
            let ch = i % 3; // includes an out-of-range channel
            assert!(c.process_sample(ch, x).is_finite());
            assert!(f.process_sample(ch, x).is_finite());
            assert!(p.process_sample(ch, x).is_finite());
        }
    }
}

#[test]
fn tails_decay_to_exact_zero() {
    let fs = 48_000.0f32;
    let mut f = Flanger::new(1, fs);
    f.set_feedback(-0.9);
    let mut p = Phaser::new(1, fs);
    let mut c = Chorus::new(1, fs);
    for i in 0..100 {
        let x = if i == 0 { 1.0 } else { 0.0 };
        f.process_sample(0, x);
        p.process_sample(0, x);
        c.process_sample(0, x);
    }
    let (mut a, mut b, mut d) = (1.0f32, 1.0f32, 1.0f32);
    for _ in 0..3_000_000 {
        a = f.process_sample(0, 0.0);
        b = p.process_sample(0, 0.0);
        d = c.process_sample(0, 0.0);
        assert!(a == 0.0 || a.is_normal(), "denormal {a:e}");
        assert!(b == 0.0 || b.is_normal(), "denormal {b:e}");
        assert!(d == 0.0 || d.is_normal(), "denormal {d:e}");
    }
    assert_eq!((a, b, d), (0.0, 0.0, 0.0));
}
