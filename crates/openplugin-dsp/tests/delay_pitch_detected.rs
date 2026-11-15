//! MPM measures the input, its answer sets the delay-line shifter's period:
//! the shifter is given no knowledge of the true frequency.

use openplugin_dsp::{DelayPitchShifter, MpmDetector, PitchDetector};
use std::f64::consts::PI;

#[test]
fn delay_line_shifter_follows_a_detected_pitch() {
    let fs = 48_000.0f32;
    let f0 = 173.0f64;
    let n = (2.5 * fs) as usize;
    let partials = [(1.0, 0.4), (2.0, 0.25), (3.0, 0.15)];
    let x: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(fs);
            partials.iter().map(|&(m, a): &(f64, f64)| a * (2.0 * PI * f0 * m * t).sin()).sum::<f64>() as f32
        })
        .collect();
    let mut det = MpmDetector::new(fs, 50.0, 2200.0);
    let p = det.detect(&x[..det.window()]);
    assert!(p.confidence > 0.9);
    for &semis in &[-7.0f32, 7.0] {
        let mut ds = DelayPitchShifter::new(1, fs, 30.0, 2);
        ds.set_period_hz(Some(p.frequency));
        ds.set_semitones(semis);
        let mut y = x.clone();
        ds.process(0, &mut y);
        let r = 2f64.powf(f64::from(semis) / 12.0);
        let w = det.window();
        let q = det.detect(&y[fs as usize..fs as usize + w]);
        let e = 1200.0 * (f64::from(q.frequency) / (f0 * r)).log2();
        println!("detected {:.3} Hz, shifted {semis:+} st: {e:+.3} cents", p.frequency);
        assert!(e.abs() < 1.0, "{e}");
    }
}
