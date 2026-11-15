//! Latency of both shifters against what they report, and the quality of the
//! delay-line shifter (pitch accuracy and spurious components).

use openplugin_dsp::{DelayPitchShifter, MpmDetector, PitchDetector, PitchShifter, VocoderConfig};
use std::f64::consts::PI;

fn burst(fs: f32, n: usize, centre: usize, hz: f64, width_s: f64) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = (i as f64 - centre as f64) / f64::from(fs);
            ((-0.5 * (t / width_s).powi(2)).exp() * (2.0 * PI * hz * t).sin() * 0.6) as f32
        })
        .collect()
}

/// Energy centroid of the signal (samples).
fn centroid(y: &[f32]) -> f64 {
    let (mut n, mut d) = (0.0, 0.0);
    for (i, v) in y.iter().enumerate() {
        let e = f64::from(*v).powi(2);
        n += i as f64 * e;
        d += e;
    }
    n / d
}

#[test]
fn vocoder_latency_is_as_reported_at_every_ratio() {
    for &fs in &[44_100.0f32, 48_000.0, 96_000.0] {
        let cfg = VocoderConfig::for_rate(fs);
        for &semis in &[-12.0f32, -7.0, 0.0, 3.0, 7.0, 12.0] {
            let mut p = PitchShifter::new(1, fs, cfg);
            p.set_semitones(semis);
            let lat = p.latency();
            let centre = (0.5 * fs) as usize;
            let x = burst(fs, (1.5 * fs) as usize + lat, centre, 700.0, 0.02);
            let mut y = x.clone();
            p.process(0, &mut y);
            assert_eq!(p.underruns(0), 0);
            let got = centroid(&y) - centroid(&x);
            let err_ms = (got - lat as f64) * 1000.0 / f64::from(fs);
            println!("vocoder fs {fs} {semis:+} st: reported {lat} samples, measured {got:.1} (error {err_ms:+.2} ms)");
            assert!(err_ms.abs() < 2.0, "{err_ms}");
        }
    }
}

#[test]
fn delay_line_shifter_latency_and_pitch() {
    let fs = 48_000.0f32;
    for &taps in &[2usize, 3, 4] {
        let ds = DelayPitchShifter::new(1, fs, 30.0, taps);
        println!("delay-line shifter: {taps} taps, latency {} samples = {:.2} ms", ds.latency(), ds.latency() as f64 * 1000.0 / f64::from(fs));
        assert_eq!(ds.latency(), 722); // 2 + 1440 / 2
    }
    // Perceived pitch (median of MPM over a second) and level ripple (95th
    // over 5th percentile of 10 ms RMS) for each setting.
    let mut worst = 0.0f64;
    for &(taps, fade, sync) in &[(2usize, 0.3f32, false), (2, 0.3, true), (2, 1.0, false), (2, 1.0, true), (4, 1.0, false)] {
        for &semis in &[-12.0f32, -7.0, 3.0, 7.0, 12.0] {
            for &f0 in &[220.0f64, 330.0, 1000.0] {
                let r = 2f64.powf(f64::from(semis) / 12.0);
                let mut ds = DelayPitchShifter::new(1, fs, 30.0, taps);
                ds.set_crossfade(fade);
                if sync {
                    ds.set_period_hz(Some(f0 as f32));
                }
                ds.set_semitones(semis);
                let n = (2.5 * fs) as usize;
                let mut y: Vec<f32> = (0..n).map(|i| (0.5 * (2.0 * PI * f0 * i as f64 / f64::from(fs)).sin()) as f32).collect();
                ds.process(0, &mut y);
                let mut det = MpmDetector::new(fs, 50.0, 2200.0);
                let w = det.window();
                let mut cents: Vec<f64> = Vec::new();
                let mut s = (0.5 * fs) as usize;
                while s + w <= y.len() {
                    let p = det.detect(&y[s..s + w]);
                    if p.frequency > 0.0 {
                        cents.push(1200.0 * (f64::from(p.frequency) / (f0 * r)).log2());
                    }
                    s += 480;
                }
                cents.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let med = if cents.is_empty() { 9999.0 } else { cents[cents.len() / 2] };
                // Level ripple.
                let blk = (0.01 * fs) as usize;
                let mut rms: Vec<f64> = y[(0.5 * fs) as usize..]
                    .chunks_exact(blk)
                    .map(|c| (c.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / blk as f64).sqrt().max(1e-9))
                    .collect();
                rms.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let ripple = 20.0 * (rms[rms.len() * 95 / 100] / rms[rms.len() * 5 / 100]).log10();
                println!("  {taps} taps fade {fade} sync {sync} {semis:+} st {f0} Hz: {med:+.2} cents (MPM median), level ripple {ripple:.1} dB");
                if taps == 2 && fade < 0.5 && !sync {
                    worst = worst.max(med.abs());
                    assert!(med.abs() < 60.0, "{med}");
                }
                if taps == 2 && sync {
                    assert!(med.abs() < 0.2, "{med}");
                    assert!(ripple < 1.5, "{ripple}");
                }
            }
        }
    }
    println!("delay-line shifter (2 taps, crossfade 0.3): worst median pitch error {worst:.3} cents");
}

#[test]
fn delay_line_shifter_delay_is_as_reported() {
    let fs = 48_000.0f32;
    let mut ds = DelayPitchShifter::new(1, fs, 30.0, 2);
    ds.set_ratio(1.0);
    let x = burst(fs, 48_000, 20_000, 700.0, 0.01);
    let mut y = x.clone();
    ds.process(0, &mut y);
    let d = centroid(&y) - centroid(&x);
    println!("delay-line shifter at ratio 1: delay {d:.2} samples (reported {})", ds.latency());
    assert!((d - ds.latency() as f64).abs() < 1.0);
}
