//! Minimum-phase half-band filters: rejection, passband, delay and signal path.

use openplugin_dsp::*;
use std::f64::consts::PI;

const FS: f64 = 44_100.0;

/// Amplitude of the component at `f` Hz (rate `rate`) over `x[skip..]`.
fn tone_amp(x: &[f32], f: f64, rate: f64, skip: usize) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    let n = x.len() - skip;
    for (i, &v) in x[skip..].iter().enumerate() {
        let a = 2.0 * PI * f / rate * i as f64;
        re += v as f64 * a.cos();
        im += v as f64 * a.sin();
    }
    2.0 * (re * re + im * im).sqrt() / n as f64
}

fn sine(f: f64, rate: f64, n: usize) -> Vec<f32> {
    (0..n).map(|i| (2.0 * PI * f / rate * i as f64).sin() as f32).collect()
}

#[test]
fn rejection_and_passband_against_spec() {
    for &(pass, atten) in &[(18_000.0, 60.0), (20_000.0, 80.0), (20_000.0, 100.0)] {
        let hb = MinPhaseHalfband::design(FS, pass, atten).unwrap();
        let h = hb.taps();
        let over = 2.0 * FS;
        let stop_edge = FS - pass;
        let stop = (0..=2000)
            .map(|i| stop_edge + (over / 2.0 - stop_edge) * i as f64 / 2000.0)
            .map(|f| fir_magnitude_db(h, f / over))
            .fold(f64::MIN, f64::max);
        let ripple = (0..=1000)
            .map(|i| pass * i as f64 / 1000.0)
            .map(|f| (fir_magnitude_db(h, f / over)).abs())
            .fold(0.0, f64::max);
        let gd_dc = hb.group_delay_base_samples(100.0, FS);
        let gd_edge = hb.group_delay_base_samples(pass, FS);
        let linear_gd =
            (kaiser_taps(atten, FS - 2.0 * pass, over) as f64 - 1.0) / 2.0 / 2.0;
        println!(
            "halfband {pass} Hz / {atten} dB: {} taps, stopband {stop:.1} dB, passband ripple {ripple:.6} dB, group delay {gd_dc:.2}..{gd_edge:.2} base samples (linear-phase equivalent {linear_gd:.1})",
            h.len()
        );
        assert!(stop < -(atten - 1.0), "stopband {stop}");
        assert!(ripple < 20.0 * (1.0 + 10f64.powf(-atten / 20.0 + 0.4)).log10() + 0.01, "ripple {ripple}");
        assert!(gd_edge < linear_gd);
    }
}

#[test]
fn signal_path_images_aliases_and_unity_gain() {
    let mut hb = MinPhaseHalfband::design(FS, 20_000.0, 80.0).unwrap();
    let n = 4096;
    // Upsample 10 kHz: the tone is kept at unity, its image at 2 fs - 10 kHz... (34.1 kHz) is rejected.
    let x = sine(10_000.0, FS, n);
    let mut up = vec![0.0f32; 2 * n];
    hb.upsample_2x(&x, &mut up);
    let skip = 600;
    let wanted = tone_amp(&up, 10_000.0, 2.0 * FS, skip);
    let image = tone_amp(&up, FS - 10_000.0, 2.0 * FS, skip);
    println!("upsample: tone {wanted:.5}, image {:.1} dB", 20.0 * (image / wanted).log10());
    assert!((wanted - 1.0).abs() < 0.001);
    assert!(20.0 * (image / wanted).log10() < -75.0);

    // Downsample a 2x-rate tone that folds onto 10 kHz.
    hb.reset();
    let alias_src = sine(FS - 10_000.0, 2.0 * FS, 2 * n);
    let mut down = vec![0.0f32; n];
    hb.downsample_2x(&alias_src, &mut down);
    let leak = tone_amp(&down, 10_000.0, FS, 300);
    println!("downsample alias leak {:.1} dB", 20.0 * leak.log10());
    assert!(20.0 * leak.log10() < -75.0);

    // Round trip of an in-band tone: unity gain.
    hb.reset();
    let x = sine(5_000.0, FS, n);
    let mut up = vec![0.0f32; 2 * n];
    let mut back = vec![0.0f32; n];
    hb.upsample_2x(&x, &mut up);
    hb.downsample_2x(&up, &mut back);
    let a = tone_amp(&back, 5_000.0, FS, 600);
    assert!((a - 1.0).abs() < 0.001, "{a}");
    // Block-size independence: the same stream in small pieces.
    hb.reset();
    let mut up2 = vec![0.0f32; 2 * n];
    for (xi, oi) in x.chunks(37).zip(up2.chunks_mut(74)) {
        hb.upsample_2x(xi, &mut oi[..2 * xi.len()]);
    }
    assert_eq!(up, up2);
}

#[test]
fn bad_specs_are_refused() {
    assert!(MinPhaseHalfband::design(FS, 23_000.0, 5.0).is_err());
    assert!(MinPhaseHalfband::design(FS, 30_000.0, 80.0).is_err());
}
