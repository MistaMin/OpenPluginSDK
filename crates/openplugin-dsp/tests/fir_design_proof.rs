//! Proofs for FIR design and phase: designed responses against targets.
//! Run with `--nocapture` to see the measured figures quoted in the cookbook.

use openplugin_dsp::*;

const FS: f64 = 48_000.0;

fn grid(lo: f64, hi: f64, n: usize) -> impl Iterator<Item = f64> {
    (0..=n).map(move |i| lo + (hi - lo) * i as f64 / n as f64)
}

/// Max |dB| deviation from 0 over `[lo, hi]` Hz.
fn ripple_db(h: &[f32], lo: f64, hi: f64, fs: f64) -> f64 {
    grid(lo, hi, 600)
        .map(|f| fir_magnitude_db(h, f / fs).abs())
        .fold(0.0, f64::max)
}

/// Highest level in dB over `[lo, hi]` Hz.
fn peak_db(h: &[f32], lo: f64, hi: f64, fs: f64) -> f64 {
    grid(lo, hi, 1200)
        .map(|f| fir_magnitude_db(h, f / fs))
        .fold(f64::MIN, f64::max)
}

#[test]
fn kaiser_low_pass_meets_spec_and_estimate() {
    for &(cut, tw, atten) in &[(6000.0, 1000.0, 60.0), (6000.0, 500.0, 80.0), (10_000.0, 2000.0, 100.0)] {
        let kind = FirKind::LowPass { cutoff_hz: cut };
        let est = kaiser_taps(atten, tw, FS);
        let h = design_kaiser(kind, FS, atten, tw).unwrap();
        assert_eq!(h.len(), est);
        let stop = peak_db(&h, cut + tw / 2.0, FS / 2.0, FS);
        let ripple = ripple_db(&h, 0.0, cut - tw / 2.0, FS);
        // Achieved transition: from -delta (passband edge) to delta (stop edge).
        let delta = 10f64.powf(-atten / 20.0);
        let lo_db = 20.0 * (1.0 - delta).log10();
        let mut f_pass = 0.0;
        for f in grid(0.0, FS / 2.0, 24_000) {
            if fir_magnitude_db(&h, f / FS) < lo_db {
                break;
            }
            f_pass = f;
        }
        let mut f_stop = FS / 2.0;
        for f in grid(FS / 2.0, 0.0, 24_000) {
            if fir_magnitude_db(&h, f / FS) > 20.0 * delta.log10() {
                break;
            }
            f_stop = f;
        }
        println!(
            "kaiser LP A={atten} tw={tw}: taps {} ; stopband peak {stop:.1} dB ; passband ripple {ripple:.5} dB ; achieved transition {:.0} Hz",
            h.len(),
            f_stop - f_pass
        );
        assert!(stop < -(atten - 2.0), "stopband {stop}");
        let want = 20.0 * (1.0 + delta).log10() * 1.5;
        assert!(ripple < want, "ripple {ripple} vs {want}");
        assert!((f_stop - f_pass) < tw * 1.2, "transition {}", f_stop - f_pass);
    }
}

#[test]
fn kaiser_high_band_pass_and_stop() {
    let tw = 800.0;
    let a = 70.0;
    let hp = design_kaiser(FirKind::HighPass { cutoff_hz: 8000.0 }, FS, a, tw).unwrap();
    assert!(peak_db(&hp, 0.0, 8000.0 - tw / 2.0, FS) < -(a - 2.0));
    assert!(ripple_db(&hp, 8000.0 + tw / 2.0, 23_000.0, FS) < 0.01);
    let bp = design_kaiser(FirKind::BandPass { low_hz: 4000.0, high_hz: 9000.0 }, FS, a, tw).unwrap();
    assert!(peak_db(&bp, 0.0, 4000.0 - tw / 2.0, FS) < -(a - 2.0));
    assert!(peak_db(&bp, 9000.0 + tw / 2.0, FS / 2.0, FS) < -(a - 2.0));
    assert!(ripple_db(&bp, 4000.0 + tw / 2.0, 9000.0 - tw / 2.0, FS) < 0.01);
    let bs = design_kaiser(FirKind::BandStop { low_hz: 4000.0, high_hz: 9000.0 }, FS, a, tw).unwrap();
    assert!(peak_db(&bs, 4000.0 + tw / 2.0, 9000.0 - tw / 2.0, FS) < -(a - 2.0));
    assert!(ripple_db(&bs, 0.0, 4000.0 - tw / 2.0, FS) < 0.01);
    assert!(ripple_db(&bs, 9000.0 + tw / 2.0, 23_900.0, FS) < 0.01);
    println!("kaiser HP/BP/BS taps {} {} {}", hp.len(), bp.len(), bs.len());
}

#[test]
fn every_window_designs_a_low_pass() {
    for w in [
        WindowKind::Rectangular,
        WindowKind::Hann,
        WindowKind::Hamming,
        WindowKind::Blackman,
        WindowKind::BlackmanHarris,
        WindowKind::Kaiser { beta: 8.0 },
    ] {
        let h = design_windowed(FirKind::LowPass { cutoff_hz: 6000.0 }, FS, 129, w).unwrap();
        let dc: f32 = h.iter().sum();
        assert!((dc - 1.0).abs() < 0.1, "{w:?} dc {dc}");
        assert!(peak_db(&h, 12_000.0, 24_000.0, FS) < -20.0, "{w:?}");
    }
}

#[test]
fn least_squares_fit_beats_windowed_in_rms() {
    let n = 101;
    let bands = [
        LsBand::flat(0.0, 5000.0, 1.0, 1.0),
        LsBand::flat(7000.0, 24_000.0, 0.0, 1.0),
    ];
    let fit = design_least_squares(n, FS, &ls_grid(&bands, FS, n)).unwrap();
    let kaiser = design_kaiser(FirKind::LowPass { cutoff_hz: 6000.0 }, FS, 50.0, 2000.0).unwrap();
    // Evaluate both on one grid with the same measure (amplitude error in the two bands).
    let err = |h: &[f32]| {
        let m = (h.len() as f64 - 1.0) / 2.0;
        let mut s = 0.0;
        let mut c = 0;
        for (lo, hi, g) in [(0.0, 5000.0, 1.0), (7000.0, 24_000.0, 0.0)] {
            for f in grid(lo, hi, 800) {
                let a: f64 = h
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| v as f64 * (2.0 * std::f64::consts::PI * f / FS * (i as f64 - m)).cos())
                    .sum();
                s += (a - g).powi(2);
                c += 1;
            }
        }
        (s / c as f64).sqrt()
    };
    let (e_ls, e_k) = (err(&fit.taps), err(&kaiser[..]));
    println!(
        "LS N=101: rms {:.3e} (solver {:.3e}), cond~{:.2e}; same-measure Kaiser {} taps rms {:.3e}",
        e_ls, fit.rms_error, fit.condition_estimate, kaiser.len(), e_k
    );
    assert!((e_ls - fit.rms_error).abs() < 0.2 * fit.rms_error + 1e-6);
    assert!(e_ls < 0.01);
    // Symmetric, linear phase.
    for i in 0..n {
        assert_eq!(fit.taps[i], fit.taps[n - 1 - i]);
    }
    // Weighting the stopband x100 trades passband accuracy for rejection.
    let heavy = [
        LsBand::flat(0.0, 5000.0, 1.0, 1.0),
        LsBand::flat(7000.0, 24_000.0, 0.0, 100.0),
    ];
    let fit2 = design_least_squares(n, FS, &ls_grid(&heavy, FS, n)).unwrap();
    assert!(peak_db(&fit2.taps, 7000.0, 24_000.0, FS) < peak_db(&fit.taps, 7000.0, 24_000.0, FS));
}

#[test]
fn least_squares_even_length_and_ill_conditioning() {
    let n = 64;
    let bands = [
        LsBand::flat(0.0, 4000.0, 1.0, 1.0),
        LsBand::flat(8000.0, 24_000.0, 0.0, 1.0),
    ];
    let fit = design_least_squares(n, FS, &ls_grid(&bands, FS, n)).unwrap();
    assert!(fit.rms_error < 0.02);
    assert!(fir_group_delay(&fit.taps, 0.05) - 31.5 < 1e-6);
    // A long filter with a very narrow gap: the system degrades, and says so.
    let tight = [
        LsBand::flat(0.0, 5000.0, 1.0, 1.0),
        LsBand::flat(15_000.0, 24_000.0, 0.0, 1.0),
    ];
    match design_least_squares(801, FS, &ls_grid(&tight, FS, 801)) {
        Ok(f) => {
            println!("wide gap N=801: cond estimate {:.2e}", f.condition_estimate);
            assert!(f.condition_estimate > 1e4);
        }
        Err(e) => {
            println!("wide gap N=801: {e}");
            assert_eq!(e, FirError::Singular);
        }
    }
}

const CURVE: [EqPoint; 6] = [
    (30.0, 0.0),
    (200.0, 0.0),
    (1000.0, 12.0),
    (3000.0, 0.0),
    (8000.0, -9.0),
    (16_000.0, -9.0),
];

fn target_db(f: f64) -> f64 {
    let m = sample_curve(&CURVE, FS, 65_536).unwrap();
    let k = (f / FS * 65_536.0).round() as usize;
    20.0 * m[k].log10()
}

#[test]
fn linear_phase_eq_is_symmetric_and_delays_by_half() {
    let n = 2049;
    let h = linear_phase_eq(&CURVE, FS, n, WindowKind::Kaiser { beta: 8.0 }).unwrap();
    assert_eq!(h.len(), n);
    for i in 0..n {
        assert_eq!(h[i], h[n - 1 - i], "tap {i}");
    }
    assert_eq!(linear_phase_latency(n), 1024.0);
    let mut worst_gd = 0.0f64;
    for f in grid(50.0, 20_000.0, 80) {
        worst_gd = worst_gd.max((fir_group_delay(&h, f / FS) - 1024.0).abs());
    }
    let mut worst_mag = 0.0f64;
    for f in [100.0, 500.0, 1000.0, 2000.0, 6000.0, 12_000.0, 20_000.0] {
        worst_mag = worst_mag.max((fir_magnitude_db(&h, f / FS) - target_db(f)).abs());
    }
    println!("linear-phase EQ N={n}: group-delay error {worst_gd:.2e} samples, magnitude error {worst_mag:.3} dB");
    assert!(worst_gd < 1e-3);
    assert!(worst_mag < 0.3);
    // Even length: half-sample delay.
    let he = linear_phase_eq(&CURVE, FS, 1024, WindowKind::Blackman).unwrap();
    assert!((fir_group_delay(&he, 0.02) - 511.5).abs() < 1e-3);
}

#[test]
fn minimum_phase_matches_magnitude_with_less_delay_and_front_loaded_energy() {
    let n = 2049;
    let lin = linear_phase_eq(&CURVE, FS, n, WindowKind::Kaiser { beta: 8.0 }).unwrap();
    let min = minimum_phase_eq(&CURVE, FS, n).unwrap();
    let mut worst = 0.0f64;
    for f in [60.0, 100.0, 300.0, 700.0, 1000.0, 1500.0, 2500.0, 5000.0, 8000.0, 12_000.0, 20_000.0] {
        let m = fir_magnitude_db(&min, f / FS);
        worst = worst.max((m - target_db(f)).abs());
    }
    let gd_min = fir_group_delay(&min, 1000.0 / FS);
    let gd_lin = fir_group_delay(&lin, 1000.0 / FS);
    let e_min = partial_energy(&min, 256);
    let e_lin = partial_energy(&lin, 256);
    println!(
        "min-phase N={n}: magnitude error {worst:.4} dB ; group delay @1k {gd_min:.1} vs linear {gd_lin:.1} samples ; energy in first 256 taps {:.6} vs linear {:.2e}",
        e_min, e_lin
    );
    assert!(worst < 0.05, "{worst}");
    assert!(gd_min < 0.05 * gd_lin);
    assert!(e_min > 0.999);
    assert!(e_lin < 1e-3);
    // Cepstral aliasing is controlled by oversizing: compare with a 4x larger FFT.
    let big = sample_curve(&CURVE, FS, 1 << 18).unwrap();
    let min_big = minimum_phase_from_magnitude(&big, n).unwrap();
    let diff = min.iter().zip(&min_big).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
    println!("min-phase taps, FFT 65536 vs 262144: max difference {diff:.2e}");
    assert!(diff < 1e-4);
}

#[test]
fn mixed_phase_sits_between() {
    let n = 1025;
    let mag = sample_curve(&CURVE, FS, 32_768).unwrap();
    let mut prev = -1.0;
    for amount in [1.0, 0.5, 0.0] {
        let h = mixed_phase_from_magnitude(&mag, n, amount).unwrap();
        let gd = fir_group_delay(&h, 400.0 / FS);
        println!("mixed phase amount {amount}: group delay @400 Hz {gd:.1} samples");
        assert!(gd > prev);
        prev = gd;
        let worst = [300.0, 1000.0, 5000.0, 12_000.0]
            .iter()
            .map(|&f| (fir_magnitude_db(&h, f / FS) - target_db(f)).abs())
            .fold(0.0, f64::max);
        assert!(worst < 0.1, "amount {amount}: {worst}");
    }
    assert!((prev - 512.0).abs() < 0.01);
}

#[test]
fn minimum_phase_from_linear_halves_the_order() {
    let proto = design_kaiser(FirKind::LowPass { cutoff_hz: 8000.0 }, FS, 120.0, 2000.0).unwrap();
    let min = minimum_phase_from_linear(&proto).unwrap();
    assert_eq!(min.len(), proto.len().div_ceil(2));
    // Squared magnitude equals the zero-phase amplitude (plus lift).
    println!("min-phase factor of {} taps -> {}", proto.len(), min.len());
    let pass = ripple_db(&min, 0.0, 7000.0, FS);
    let stop = peak_db(&min, 9000.0, 24_000.0, FS);
    println!("factor: passband ripple {pass:.4} dB, stopband {stop:.1} dB");
    assert!(pass < 0.01);
    assert!(stop < -50.0);
}

#[test]
fn bad_input_is_an_error_not_a_panic() {
    assert_eq!(linear_phase_eq(&[], FS, 101, WindowKind::Hann), Err(FirError::BadCurve));
    assert_eq!(
        linear_phase_eq(&[(1000.0, 0.0), (500.0, 0.0)], FS, 101, WindowKind::Hann),
        Err(FirError::BadCurve)
    );
    assert_eq!(
        minimum_phase_eq(&[(1000.0, f64::NAN)], FS, 101),
        Err(FirError::BadCurve)
    );
    assert_eq!(linear_phase_eq(&CURVE, FS, 2, WindowKind::Hann), Err(FirError::TooShort));
}
