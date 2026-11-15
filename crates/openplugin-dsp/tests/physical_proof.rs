//! Proof for the physical models: pitch, partials, decay times, stiffness,
//! pick-position notches, tube resonances and modal frequencies and decays,
//! each against theory.
//!
//! Measurements: a Hann-windowed DFT evaluated at one frequency (phasor
//! recurrence, `f64`), maximised by golden-section search for pitch, and
//! evaluated over consecutive blocks for decay time.

use core::f64::consts::PI;
use openplugin_dsp::{
    hz_from_midi, mode_ratios, stiff_partial_hz, BoreEnd, Exciter, ModalBank, ModalPreset,
    PluckedString, PresetSettings, Strike, Tuning, WaveguideBore, WaveguideString, MEMBRANE_ZEROS,
};

const RATES: [f32; 4] = [44_100.0, 48_000.0, 96_000.0, 192_000.0];

/// Magnitude of the Hann-windowed DFT of `sig[start..start+len]` at `f`.
fn dft_mag(sig: &[f32], start: usize, len: usize, f: f64, fs: f64) -> f64 {
    let (sw, cw) = (2.0 * PI * f / fs).sin_cos();
    let (mut re, mut im) = (1.0f64, 0.0f64);
    let (mut acc_r, mut acc_i) = (0.0, 0.0);
    for i in 0..len {
        let w = 0.5 - 0.5 * (2.0 * PI * (i as f64 + 0.5) / len as f64).cos();
        let x = f64::from(sig[start + i]) * w;
        acc_r += x * re;
        acc_i += x * im;
        let nr = re * cw - im * sw;
        im = re * sw + im * cw;
        re = nr;
        if i % 4096 == 4095 {
            let n = (re * re + im * im).sqrt();
            re /= n;
            im /= n;
        }
    }
    (acc_r * acc_r + acc_i * acc_i).sqrt()
}

fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

/// Frequency of the strongest component within `+-span_cents` of `guess`.
fn refine(sig: &[f32], start: usize, len: usize, guess: f64, span_cents: f64, fs: f64) -> f64 {
    let (mut lo, mut hi) = (guess * (2f64).powf(-span_cents / 1200.0), guess * (2f64).powf(span_cents / 1200.0));
    let gr = 0.618_033_988_749_894_8;
    for _ in 0..28 {
        let c = hi - gr * (hi - lo);
        let d = lo + gr * (hi - lo);
        if dft_mag(sig, start, len, c, fs) > dft_mag(sig, start, len, d, fs) {
            hi = d;
        } else {
            lo = c;
        }
    }
    0.5 * (lo + hi)
}

/// T60 from the decay of the component at `f`: least-squares slope of the dB
/// level over blocks, from the start until the level has fallen `range_db`.
fn t60_of(sig: &[f32], f: f64, fs: f64, skip: usize, block: usize, range_db: f64) -> f64 {
    let mut pts = Vec::new();
    let mut s = skip;
    while s + block <= sig.len() {
        let m = dft_mag(sig, s, block, f, fs);
        let t = (s + block / 2) as f64 / fs;
        pts.push((t, 20.0 * m.max(1e-30).log10()));
        s += block;
    }
    let top = pts[0].1;
    let used: Vec<(f64, f64)> = pts.iter().copied().take_while(|p| p.1 > top - range_db).collect();
    assert!(used.len() >= 4, "too few blocks ({}) to fit a decay", used.len());
    let n = used.len() as f64;
    let (st, sy) = used.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    let (mt, my) = (st / n, sy / n);
    let num: f64 = used.iter().map(|p| (p.0 - mt) * (p.1 - my)).sum();
    let den: f64 = used.iter().map(|p| (p.0 - mt) * (p.0 - mt)).sum();
    -60.0 / (num / den)
}

fn render_ks(s: &mut PluckedString, n: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; n];
    s.process(&mut v);
    v
}

#[test]
fn karplus_strong_pitch_across_midi_and_rates() {
    let (mut worst_allpass, mut worst_int_vs_model, mut worst_int_nominal) = (0.0f64, 0.0f64, 0.0f64);
    for &fs in &RATES {
        let mut s = PluckedString::new(fs, 20.0);
        s.set_stretch(0.05);
        s.set_decay(2.0);
        for note in (28..=100).step_by(4) {
            let f = f64::from(hz_from_midi(note as f32));
            let len = ((0.4 * f64::from(fs)) as usize).max((24.0 * f64::from(fs) / f) as usize);
            let skip = (0.01 * f64::from(fs)) as usize;

            s.set_frequency(f as f32);
            s.set_tuning(Tuning::Allpass);
            s.pluck(1.0);
            let sig = render_ks(&mut s, skip + len);
            let m = refine(&sig, skip, len, f, 10.0, f64::from(fs));
            let e = cents(m, f).abs();
            worst_allpass = worst_allpass.max(e);
            assert!(e < 0.15, "allpass: note {note} at {fs}: {e} cents");

            s.set_tuning(Tuning::Integer);
            s.pluck(1.0);
            let model = s.partial_frequency(1);
            let sig = render_ks(&mut s, skip + len);
            let m = refine(&sig, skip, len, model, 5.0, f64::from(fs));
            let e = cents(m, model).abs();
            worst_int_vs_model = worst_int_vs_model.max(e);
            assert!(e < 0.15, "integer: note {note} at {fs}: measured vs model {e} cents");
            worst_int_nominal = worst_int_nominal.max(cents(model, f).abs());
        }
    }
    println!(
        "KS pitch: allpass worst {worst_allpass:.4} cents; integer delay matches its model within {worst_int_vs_model:.4} cents, \
         worst quantisation error {worst_int_nominal:.1} cents"
    );
    assert!(worst_int_nominal > 5.0, "the untuned control must show quantisation");
}

#[test]
fn karplus_strong_partials_follow_the_loop_phase_condition() {
    let fs = 48_000.0f32;
    let mut s = PluckedString::new(fs, 20.0);
    s.set_stretch(0.05);
    s.set_decay(3.0);
    let mut worst_model = 0.0f64;
    let mut worst_harmonic = 0.0f64;
    for note in [36, 48, 60, 72] {
        let f = f64::from(hz_from_midi(note as f32));
        s.set_frequency(f as f32);
        s.pluck(1.0);
        let len = (0.4 * f64::from(fs)) as usize;
        let sig = render_ks(&mut s, 480 + len);
        for k in 2..=4usize {
            let model = s.partial_frequency(k);
            let m = refine(&sig, 480, len, model, 8.0, f64::from(fs));
            worst_model = worst_model.max(cents(m, model).abs());
            worst_harmonic = worst_harmonic.max(cents(model, k as f64 * f).abs());
        }
    }
    println!("KS partials 2-4: vs loop model {worst_model:.4} cents; vs exact harmonics {worst_harmonic:.2} cents");
    assert!(worst_model < 0.3);
    assert!(worst_harmonic < 12.0, "allpass inharmonicity of partials 2-4 is bounded");
}

#[test]
fn karplus_strong_decay_matches_t60() {
    let mut worst = 0.0f64;
    for &fs in &[48_000.0f32, 96_000.0] {
        for note in [36, 48, 60, 72] {
            for t60 in [0.6f32, 1.5, 4.0] {
                let f = f64::from(hz_from_midi(note as f32));
                let mut s = PluckedString::new(fs, 20.0);
                s.set_stretch(0.05);
                s.set_decay(t60);
                s.set_frequency(f as f32);
                assert!(s.loop_gain() < 0.9999, "gain clamp would bite");
                s.pluck(1.0);
                let n = (f64::from(t60) * 0.7 * f64::from(fs)) as usize + 4800;
                let sig = render_ks(&mut s, n);
                let m = refine(&sig, 480, 4096, f, 3.0, f64::from(fs));
                let block = ((6.0 * f64::from(fs) / f) as usize).max((0.04 * f64::from(fs)) as usize);
                let got = t60_of(&sig, m, f64::from(fs), 960, block, 30.0);
                let err = (got / f64::from(t60) - 1.0).abs();
                worst = worst.max(err);
                assert!(err < 0.05, "note {note} fs {fs} t60 {t60}: measured {got}");
            }
        }
    }
    println!("KS T60: worst error {:.2} %", worst * 100.0);
}

#[test]
fn stiffness_bends_partials_to_the_stiff_string_formula() {
    let fs = 96_000.0f32;
    let f1 = 196.0f64;
    let mut table = String::new();
    let mut fails = Vec::new();
    for &b in &[1e-4f32, 4e-4, 1e-3, 3e-3] {
        for &sections in &[2usize, 4, 8] {
            let mut s = PluckedString::new(fs, 20.0);
            s.set_stretch(0.05);
            s.set_decay(4.0);
            s.set_dispersion_sections(sections);
            s.set_stiffness(b);
            s.set_frequency(f1 as f32);
            s.pluck(1.0);
            let len = (0.5 * f64::from(fs)) as usize;
            let sig = render_ks(&mut s, 960 + len);
            let mut worst = 0.0f64;
            let mut worst_unbent = 0.0f64;
            for k in 1..=8usize {
                let want = stiff_partial_hz(f1, f64::from(b), k as f64);
                let m = refine(&sig, 960, len, want, 25.0, f64::from(fs));
                worst = worst.max(cents(m, want).abs());
                worst_unbent = worst_unbent.max(cents(want, k as f64 * f1).abs());
            }
            table.push_str(&format!(
                "B={b:e}, {sections} sections: worst partial error {worst:.2} cents (the formula moves partial 8 by {worst_unbent:.1})\n"
            ));
            if (sections == 8 && b <= 1e-3) || (sections == 4 && b <= 4e-4) {
                fails.push(worst);
            }
            let m1 = refine(&sig, 960, len, f1, 10.0, f64::from(fs));
            assert!(cents(m1, f1).abs() < 0.15);
        }
    }
    println!("{table}");
    assert!(fails.iter().all(|w| *w < 4.0), "{fails:?}");
}

#[test]
fn karplus_strong_pick_position_notches_partials() {
    // beta = 1/5 of the line: partials 5 and 10 vanish from the excitation.
    // S = 0 and L = 401: the line is exactly 400 samples, so beta 0.2 is 80 samples.
    let fs = 48_000.0f32;
    let f = f64::from(fs) / 401.0;
    let mut s = PluckedString::new(fs, 20.0);
    s.set_stretch(0.0);
    s.set_decay(10.0);
    s.set_frequency(f as f32);
    s.set_pick_position(0.2);
    let mut notch = f64::MAX;
    for _ in 0..3 {
        s.pluck(1.0);
        let len = 9600;
        let sig = render_ks(&mut s, len + 200);
        let mags: Vec<f64> =
            (1..=7).map(|k| dft_mag(&sig, 0, len, s.partial_frequency(k), f64::from(fs))).collect();
        // Relative to its neighbours, partial 5 is far down.
        let neighbours = 0.5 * (mags[3] + mags[5]);
        let drop = 20.0 * (mags[4] / neighbours).log10();
        notch = notch.min(-drop);
        assert!(drop < -20.0, "partial 5 is only {drop} dB below its neighbours");
    }
    println!("KS pick-position notch (beta 0.2, partial 5): at least {notch:.1} dB below neighbours");
}

fn waveguide_ratio_check(label: &str, fs: f32, expect_db_tol: f64) {
    // L = 401 samples: nr = nl = 200, so beta 0.2 is exactly 40 samples.
    let f1 = f64::from(fs) / 401.0;
    let mut w = WaveguideString::new(fs, 20.0);
    w.set_stretch(0.0);
    w.set_decay(600.0);
    w.set_frequency(f1 as f32);
    w.set_pluck_position(0.2);
    w.set_pickup_position(0.35);
    assert_eq!(w.delay_samples(), 400);
    w.pluck(1.0);
    let len = 401 * 24;
    let sig: Vec<f32> = (0..len).map(|_| w.next()).collect();
    let mags: Vec<f64> = (1..=12).map(|k| dft_mag(&sig, 0, len, f1 * k as f64, f64::from(fs))).collect();
    let theory = |k: usize| {
        let kf = k as f64;
        ((PI * kf * 0.2).sin() * (PI * kf * 0.35).sin() / (kf * kf)).abs()
    };
    let mut worst = 0.0f64;
    for k in [2usize, 3, 4, 6, 7, 8, 9] {
        let got = 20.0 * ((mags[k - 1] / mags[0]) / (theory(k) / theory(1))).log10();
        worst = worst.max(got.abs());
    }
    // Partials 5 and 10 are the pluck notches; 20/7 is not an integer, so the
    // pickup adds none below 20.
    let drop5 = 20.0 * (mags[4] / mags[0] / (1.0 / 25.0) ).log10();
    let drop10 = 20.0 * (mags[9] / mags[0] / (1.0 / 100.0)).log10();
    println!("{label}: spectral shape within {worst:.3} dB of sin(pi k b) sin(pi k q)/k^2; notches at 5 and 10: {drop5:.1}, {drop10:.1} dB");
    assert!(worst < expect_db_tol);
    assert!(drop5 < -50.0 && drop10 < -45.0);
}

#[test]
fn waveguide_string_pick_position_comb() {
    for fs in [48_000.0f32, 96_000.0] {
        waveguide_ratio_check(&format!("waveguide string at {fs}"), fs, 0.05);
    }
}

#[test]
fn waveguide_string_input_position_and_pickup_notches() {
    // An impulse injected at 1/4 of the string: partials 4 and 8 vanish.
    let fs = 48_000.0f32;
    let f1 = f64::from(fs) / 401.0;
    let mut w = WaveguideString::new(fs, 20.0);
    w.set_stretch(0.0);
    w.set_decay(600.0);
    w.set_frequency(f1 as f32);
    w.set_input_position(0.25);
    w.set_pickup_position(0.35);
    let len = 401 * 24;
    let sig: Vec<f32> = (0..len).map(|i| w.next_with(if i == 0 { 1.0 } else { 0.0 })).collect();
    let mags: Vec<f64> = (1..=9).map(|k| dft_mag(&sig, 0, len, f1 * k as f64, f64::from(fs))).collect();
    let down4 = 20.0 * (mags[3] / mags[0]).log10();
    let down8 = 20.0 * (mags[7] / mags[0]).log10();
    println!("waveguide input at 1/4: partials 4 and 8 are {down4:.1}, {down8:.1} dB below partial 1");
    assert!(down4 < -60.0 && down8 < -60.0);
    let r = mags[2] / mags[0];
    let theory = (PI * 3.0 * 0.25).sin() * (PI * 3.0 * 0.35).sin() / ((PI * 0.25).sin() * (PI * 0.35).sin());
    assert!((20.0 * (r / theory.abs()).log10()).abs() < 0.05);
}

#[test]
fn waveguide_string_pitch_across_midi_and_rates() {
    let mut worst = 0.0f64;
    for &fs in &RATES {
        let mut w = WaveguideString::new(fs, 20.0);
        w.set_stretch(0.05);
        w.set_decay(2.0);
        for note in (28..=100).step_by(6) {
            let f = f64::from(hz_from_midi(note as f32));
            w.set_frequency(f as f32);
            w.set_pluck_position(0.37);
            w.set_pickup_position(0.13);
            w.pluck(1.0);
            let len = ((0.3 * f64::from(fs)) as usize).max((14.0 * f64::from(fs) / f) as usize);
            let sig: Vec<f32> = (0..len + 480).map(|_| w.next()).collect();
            let m = refine(&sig, 480, len, f, 10.0, f64::from(fs));
            let e = cents(m, f).abs();
            worst = worst.max(e);
            assert!(e < 0.2, "note {note} at {fs}: {e} cents");
        }
    }
    println!("waveguide string pitch: worst {worst:.4} cents (notes 28-100, 44.1-192 kHz)");
}

#[test]
fn waveguide_string_decay_matches_t60() {
    let fs = 48_000.0f32;
    for (note, t60) in [(45, 1.0f32), (57, 2.5), (69, 1.2)] {
        let f = f64::from(hz_from_midi(note as f32));
        let mut w = WaveguideString::new(fs, 20.0);
        w.set_stretch(0.05);
        w.set_decay(t60);
        w.set_frequency(f as f32);
        w.set_pluck_position(0.3);
        w.set_pickup_position(0.2);
        w.pluck(1.0);
        let n = (f64::from(t60) * 0.7 * f64::from(fs)) as usize + 4800;
        let sig: Vec<f32> = (0..n).map(|_| w.next()).collect();
        let block = ((6.0 * f64::from(fs) / f) as usize).max(2000);
        let got = t60_of(&sig, f, f64::from(fs), 480, block, 30.0);
        println!("waveguide string note {note}: T60 {got:.3} s for {t60}");
        assert!((got / f64::from(t60) - 1.0).abs() < 0.06, "{got} vs {t60}");
    }
}

#[test]
fn waveguide_string_stiffness_follows_the_formula() {
    let fs = 96_000.0f32;
    let f1 = 196.0f64;
    let b = 4e-4f32;
    let mut w = WaveguideString::new(fs, 20.0);
    w.set_stretch(0.05);
    w.set_decay(6.0);
    w.set_stiffness(b);
    w.set_frequency(f1 as f32);
    w.set_pluck_position(0.3);
    w.set_pickup_position(0.11);
    w.pluck(1.0);
    let len = (0.5 * f64::from(fs)) as usize;
    let sig: Vec<f32> = (0..len + 960).map(|_| w.next()).collect();
    let mut worst = 0.0f64;
    for k in 1..=8usize {
        let want = stiff_partial_hz(f1, f64::from(b), k as f64);
        let m = refine(&sig, 960, len, want, 25.0, f64::from(fs));
        worst = worst.max(cents(m, want).abs());
    }
    println!("waveguide string, B = 4e-4: partials 1-8 within {worst:.2} cents of the stiff-string formula");
    assert!(worst < 2.0);
}

fn bore_peaks(left: BoreEnd, right: BoreEnd, fs: f32, f1: f32, ks: &[usize]) -> Vec<f64> {
    let mut b = WaveguideBore::new(fs, 20.0, left, right);
    b.set_stretch(0.1);
    b.set_decay(1.5);
    b.set_frequency(f1);
    // A short noise burst as the excitation.
    let mut ex = Exciter::new(fs, Strike::NoiseBurst { ms: 3.0 });
    ex.trigger(1.0);
    let len = (0.5 * f64::from(fs)) as usize;
    let sig: Vec<f32> = (0..len + 480).map(|_| b.next_with(ex.next())).collect();
    ks.iter()
        .map(|&k| {
            let model = b.resonance_frequency(k);
            refine(&sig, 480, len, model, 20.0, f64::from(fs))
        })
        .collect()
}

#[test]
fn bore_resonances_open_open_and_closed_closed_are_harmonic() {
    for &(l, r, name) in &[(BoreEnd::Open, BoreEnd::Open, "open-open"), (BoreEnd::Closed, BoreEnd::Closed, "closed-closed")] {
        for &fs in &[44_100.0f32, 96_000.0] {
            let f1 = 300.0f32;
            let peaks = bore_peaks(l, r, fs, f1, &[1, 2, 3, 4]);
            let mut worst = 0.0f64;
            for (i, p) in peaks.iter().enumerate() {
                let e = cents(*p, f64::from(f1) * (i + 1) as f64).abs();
                worst = worst.max(e);
                assert!(e < 8.0, "{name} {fs}: resonance {} off by {e} cents", i + 1);
            }
            assert!(cents(peaks[0], f64::from(f1)).abs() < 0.3, "{name} fundamental");
            println!("bore {name} at {fs}: fundamental {:.3} cents, harmonics 1-4 within {worst:.2} cents", cents(peaks[0], f64::from(f1)));
        }
    }
}

#[test]
fn bore_closed_open_has_only_odd_harmonics() {
    for &fs in &[44_100.0f32, 96_000.0] {
        let f1 = 261.6f32;
        let peaks = bore_peaks(BoreEnd::Closed, BoreEnd::Open, fs, f1, &[1, 2, 3, 4]);
        for (i, p) in peaks.iter().enumerate() {
            let want = f64::from(f1) * (2 * i + 1) as f64;
            assert!(cents(*p, want).abs() < 8.0, "resonance {} at {p} vs {want}", i + 1);
        }
        assert!(cents(peaks[0], f64::from(f1)).abs() < 0.3);
        // Even harmonics are absent: compare the spectrum at 2 f and 4 f to 3 f and 5 f.
        let mut b = WaveguideBore::new(fs, 20.0, BoreEnd::Closed, BoreEnd::Open);
        b.set_stretch(0.1);
        b.set_decay(1.5);
        b.set_frequency(f1);
        let len = (0.4 * f64::from(fs)) as usize;
        let sig: Vec<f32> = (0..len + 480).map(|i| b.next_with(if i == 0 { 1.0 } else { 0.0 })).collect();
        let m = |k: f64| dft_mag(&sig, 480, len, f64::from(f1) * k, f64::from(fs));
        let odd = m(3.0).min(m(5.0));
        let even = m(2.0).max(m(4.0));
        let down = 20.0 * (even / odd).log10();
        println!("bore closed-open at {fs}: even harmonics {down:.1} dB below the odd ones; fundamental {:.3} cents", cents(peaks[0], f64::from(f1)));
        assert!(down < -40.0);
        // Quarter-wave length.
        assert!((b.physical_length(343.0) - 343.0 / (4.0 * f64::from(f1))).abs() < 1e-9);
    }
}

fn bessel_j(m: i32, x: f64) -> f64 {
    // J_m(x) = (1 / 2 pi) integral over a period of cos(m t - x sin t) dt;
    // the trapezoid rule on a periodic integrand converges geometrically.
    let n = 2048;
    (0..n)
        .map(|i| {
            let t = 2.0 * PI * f64::from(i) / f64::from(n);
            (f64::from(m) * t - x * t.sin()).cos()
        })
        .sum::<f64>()
        / f64::from(n)
}

#[test]
fn membrane_zero_table_is_complete_and_correct() {
    for &(m, _n, z) in MEMBRANE_ZEROS.iter() {
        let v = bessel_j(i32::from(m), z);
        assert!(v.abs() < 1e-8, "J_{m}({z}) = {v}");
    }
    let top = MEMBRANE_ZEROS.last().unwrap().2;
    let mut total = 0;
    for m in 0..=14 {
        let mut count = 0;
        let mut x = f64::from(m).max(0.05);
        let mut prev = bessel_j(m, x);
        while x < top + 1e-3 {
            x += 0.002;
            let v = bessel_j(m, x);
            if v.signum() != prev.signum() {
                count += 1;
            }
            prev = v;
        }
        let in_table = MEMBRANE_ZEROS.iter().filter(|e| i32::from(e.0) == m).count();
        assert_eq!(count, in_table, "zeros of J_{m} below {top}");
        total += count;
    }
    assert_eq!(total, MEMBRANE_ZEROS.len());
    assert!(MEMBRANE_ZEROS.windows(2).all(|w| w[0].2 < w[1].2), "ascending");
    for (i, &(_, n, _)) in MEMBRANE_ZEROS.iter().enumerate() {
        assert!(n >= 1, "mode {i}");
    }
}

fn run_bank(fs: f32, bank: &mut ModalBank, kind: Strike, secs: f64) -> Vec<f32> {
    let mut ex = Exciter::new(fs, kind);
    ex.trigger(1.0);
    (0..(secs * f64::from(fs)) as usize).map(|_| bank.next(ex.next())).collect()
}

fn check_bank(name: &str, fs: f32, preset: ModalPreset, settings: &PresetSettings) {
    let mut bank = ModalBank::new(fs, 32);
    let n = bank.load_preset(preset, settings);
    assert!(n >= 4, "{name}");
    let secs = f64::from(settings.t60) * 0.9;
    let sig = run_bank(fs, &mut bank, Strike::Impulse, secs);
    let mut worst_f = 0.0f64;
    let mut worst_t = 0.0f64;
    for i in 0..n {
        let f = f64::from(bank.mode_frequency(i));
        if f > 0.45 * f64::from(fs) {
            continue;
        }
        // Frequency: windowed peak, first 0.25 s.
        let len = (0.25 * f64::from(fs)) as usize;
        let m = refine(&sig, 0, len, f, 15.0, f64::from(fs));
        // Neighbouring modes within the window's main lobe disturb the peak;
        // only judge modes that are alone.
        let gap = |width_hz: f64| {
            (0..n).filter(|&j| j != i).all(|j| (f64::from(bank.mode_frequency(j)) - f).abs() > width_hz)
        };
        if gap(40.0) {
            worst_f = worst_f.max(cents(m, f).abs());
        }
        // Decay: about 12 blocks over the first 25 dB, at least 4 periods long.
        let want_t = f64::from(settings.t60) * (f64::from(settings.f1) / f).powf(f64::from(settings.decay_tilt));
        let block = ((want_t * f64::from(fs) * 25.0 / 60.0 / 12.0) as usize)
            .clamp((4.0 * f64::from(fs) / f) as usize, 8000);
        if gap(4.0 * f64::from(fs) / block as f64) && want_t > 0.15 && want_t * 0.6 < secs {
            let got = t60_of(&sig, f, f64::from(fs), 0, block, 25.0);
            let err = (got / want_t - 1.0).abs();
            worst_t = worst_t.max(err);
            assert!(err < 0.04, "{name} mode {i} ({f:.1} Hz): T60 {got} vs {want_t}");
        }
    }
    println!("{name} at {fs}: {n} modes; isolated modes within {worst_f:.4} cents; T60 within {:.2} %", worst_t * 100.0);
    assert!(worst_f < 1.0, "{name}: {worst_f} cents");
}

#[test]
fn modal_bank_matches_specified_frequencies_and_decays() {
    for &fs in &[44_100.0f32, 96_000.0] {
        let s = PresetSettings { f1: 440.0, t60: 3.0, decay_tilt: 0.7, gain_tilt: 0.0, modes: 5 };
        check_bank("free-free bar", fs, ModalPreset::BarFreeFree, &s);
        check_bank("clamped-free bar", fs, ModalPreset::BarClampedFree, &PresetSettings { f1: 220.0, modes: 4, ..s });
        check_bank("plate 1:1.6", fs, ModalPreset::Plate { aspect: 1.6 }, &PresetSettings { f1: 150.0, modes: 8, ..s });
        check_bank("membrane", fs, ModalPreset::Membrane, &PresetSettings { f1: 120.0, modes: 12, ..s });
    }
}

#[test]
fn bar_partials_are_the_formula() {
    // 440 Hz free-free bar: partials at 440 * (x_n / x_1)^2 with x_n the roots of cos x cosh x = 1.
    let mut r = [0.0; 6];
    mode_ratios(ModalPreset::BarFreeFree, &mut r);
    for (i, &ratio) in r.iter().enumerate() {
        // Check the root equation directly.
        let x = 4.730_040_744_862_704 * ratio.sqrt();
        if i < 3 {
            assert!((x.cos() * x.cosh() - 1.0).abs() < 1e-6 * x.cosh(), "mode {i}");
        }
    }
    assert!((r[1] - 2.756_44).abs() < 1e-3 && (r[2] - 5.403_9).abs() < 1e-3);
}

#[test]
fn mallet_hardness_controls_the_bright_modes() {
    let fs = 48_000.0f32;
    let s = PresetSettings { f1: 300.0, t60: 2.0, decay_tilt: 0.0, gain_tilt: 0.0, modes: 6 };
    let level = |kind: Strike, mode: usize| {
        let mut bank = ModalBank::new(fs, 8);
        bank.load_preset(ModalPreset::BarFreeFree, &s);
        let sig = run_bank(fs, &mut bank, kind, 0.5);
        let f = f64::from(bank.mode_frequency(mode));
        dft_mag(&sig, 0, sig.len(), f, f64::from(fs))
    };
    // Mode 5 sits near 3.3 kHz: a 2 ms mallet (first null near 750 Hz) hardly excites it.
    let soft = level(Strike::Mallet { contact_ms: 2.0 }, 5) / level(Strike::Mallet { contact_ms: 2.0 }, 0);
    let hard = level(Strike::Mallet { contact_ms: 0.15 }, 5) / level(Strike::Mallet { contact_ms: 0.15 }, 0);
    let imp = level(Strike::Impulse, 5) / level(Strike::Impulse, 0);
    println!("mode 5 / mode 0 level: impulse {imp:.3}, hard mallet {hard:.3}, soft mallet {soft:.3}");
    assert!((imp - 1.0).abs() < 0.02, "an impulse excites all modes equally");
    assert!(soft < 0.5 * hard && hard < imp * 1.001);
}

#[test]
fn noise_burst_excites_every_mode() {
    let fs = 48_000.0f32;
    let s = PresetSettings { f1: 300.0, t60: 1.0, decay_tilt: 0.0, gain_tilt: 0.0, modes: 5 };
    let mut bank = ModalBank::new(fs, 8);
    bank.load_preset(ModalPreset::BarFreeFree, &s);
    let sig = run_bank(fs, &mut bank, Strike::NoiseBurst { ms: 10.0 }, 0.4);
    for i in 0..5 {
        let f = f64::from(bank.mode_frequency(i));
        assert!(dft_mag(&sig, 4800, 9600, f, f64::from(fs)) > 0.1, "mode {i}");
    }
}

#[test]
#[ignore = "timing; run with --release -- --ignored --nocapture"]
fn modal_bank_cost() {
    let mut bank = ModalBank::new(48_000.0, 128);
    let s = PresetSettings { f1: 5.0, t60: 2.0, decay_tilt: 0.0, gain_tilt: 0.0, modes: 64 };
    bank.load_preset(ModalPreset::BarSimplySupported, &s);
    let mut buf = vec![0.0f32; 48_000];
    buf[0] = 1.0;
    let t = std::time::Instant::now();
    for _ in 0..20 {
        bank.process(&mut buf);
    }
    let ns = t.elapsed().as_secs_f64() * 1e9 / (20.0 * 48_000.0 * bank.len() as f64);
    println!("modal bank: {ns:.2} ns per mode per sample ({} modes)", bank.len());
}
