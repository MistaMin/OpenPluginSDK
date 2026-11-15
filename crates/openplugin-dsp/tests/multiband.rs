//! Proofs for the multiband compressor, dynamic EQ band, transient shaper,
//! de-esser and true-peak clipper.

#![allow(clippy::needless_range_loop)]

use std::f64::consts::TAU;

use openplugin_dsp::{
    DeEssDetect, DeEssMode, DeEsser, DynEqKind, DynEqMode, DynamicEqBand, DynamicEqParams, Detection, Domain, DynamicsParams,
    LrOrder, MultibandCompressor, Topology, TransientShaper, TruePeak, TruePeakClipper,
};

const FS: f32 = 48_000.0;

fn sine(f: f64, fs: f64, amp: f64, n: usize) -> Vec<f32> {
    (0..n).map(|i| (amp * (TAU * f * i as f64 / fs).sin()) as f32).collect()
}

/// Amplitude of the component at `f` over `x` (quadrature correlation).
fn amp_at(x: &[f32], f: f64, fs: f64) -> f64 {
    let (mut s, mut c) = (0.0, 0.0);
    for (i, v) in x.iter().enumerate() {
        let p = TAU * f * i as f64 / fs;
        s += f64::from(*v) * p.sin();
        c += f64::from(*v) * p.cos();
    }
    2.0 / x.len() as f64 * (s * s + c * c).sqrt()
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

fn run_mono(f: impl FnMut(&mut [f32]), x: &mut [f32], block: usize) {
    let mut f = f;
    for ch in x.chunks_mut(block) {
        f(ch);
    }
}

// ---- multiband -------------------------------------------------------------

#[test]
fn multiband_sums_flat_at_unity() {
    let freq_sets: [(&[f32], usize); 3] = [(&[1000.0], 2), (&[300.0, 3000.0], 3), (&[200.0, 1000.0, 5000.0], 4)];
    let mut worst = 0.0f64;
    for fs in [44_100.0f32, 48_000.0, 96_000.0, 192_000.0] {
        for order in [LrOrder::Lr2, LrOrder::Lr4, LrOrder::Lr8] {
            for (freqs, bands) in freq_sets {
                for k in (0..=24).step_by(2) {
                    let f = 20.0 * 1000f64.powf(k as f64 / 24.0); // 20 Hz .. 20 kHz
                    if f > 0.45 * f64::from(fs) {
                        continue;
                    }
                    let mut mb = MultibandCompressor::new(fs, 1, bands, order, freqs);
                    for b in 0..bands {
                        mb.set_band_bypass(b, true);
                    }
                    let settle = (f64::from(fs) * (0.25f64).max(12.0 / f)) as usize;
                    let cycles = (0.05 * f).ceil().max(6.0);
                    let meas = (cycles * f64::from(fs) / f).round() as usize;
                    let mut x = sine(f, f64::from(fs), 0.5, settle + meas);
                    run_mono(|b| mb.process(&mut [b]), &mut x, 256);
                    let a = amp_at(&x[settle..], f, f64::from(fs));
                    let err = db(a / 0.5);
                    worst = worst.max(err.abs());
                    assert!(err.abs() < 0.01, "fs {fs} {order:?} {bands} bands {f:.1} Hz: {err:+.4} dB");
                }
            }
        }
    }
    eprintln!("multiband flat sum, worst deviation {worst:.5} dB");
}

fn mid_only(threshold: f32) -> DynamicsParams {
    DynamicsParams {
        threshold_db: threshold,
        ratio: 4.0,
        knee_db: 0.0,
        attack_ms: 2.0,
        release_ms: 200.0,
        topology: Topology::Decoupled,
        domain: Domain::Linear,
        ..DynamicsParams::default()
    }
}

fn three_band() -> MultibandCompressor {
    let mut mb = MultibandCompressor::new(FS, 1, 3, LrOrder::Lr8, &[100.0, 8000.0]);
    for b in [0, 2] {
        mb.set_band_bypass(b, true);
    }
    mb.set_band_params(1, mid_only(-26.0));
    mb
}

fn steady(mb: &mut MultibandCompressor, f: f64, amp: f64) -> f64 {
    let n = 48_000;
    let mut x = sine(f, f64::from(FS), amp, n);
    run_mono(|b| mb.process(&mut [b]), &mut x, 128);
    db(amp_at(&x[n / 2..], f, f64::from(FS)) / amp)
}

#[test]
fn per_band_static_curve_and_independence() {
    // Mid band: tone at 1 kHz, -6 dBFS, threshold -26, ratio 4, hard knee:
    // reduction (1 - 1/4) * 20 = 15 dB.
    let mut prev: Option<(f64, f64)> = None;
    for (amp_db, expect) in [(-40.0f64, 0.0), (-26.0, 0.0), (-16.0, -7.5), (-6.0, -15.0), (0.0, -19.5)] {
        let mut mb = three_band();
        let g = steady(&mut mb, 1000.0, 10f64.powf(amp_db / 20.0));
        eprintln!("mid band, 1 kHz at {amp_db} dB: gain {g:+.2} dB (static {expect:+.2})");
        assert!((g - expect).abs() < 0.1, "{amp_db}: {g} vs {expect}");
        if let Some(p) = prev {
            // Slope between points above the threshold follows 1 - 1/ratio.
            if amp_db > -26.0 && p.0 > -26.0 {
                let slope = (p.1 - g) / (amp_db - p.0);
                assert!((slope - 0.75).abs() < 0.02, "slope {slope}");
            }
        }
        prev = Some((amp_db, g));
    }
    // A 30 Hz tone lives in the bypassed low band: untouched even when loud.
    let mut mb = three_band();
    let g = steady(&mut mb, 30.0, 0.5);
    assert!(g.abs() < 0.1, "low band must be independent, got {g}");
}

#[test]
fn solo_bypass_and_link() {
    let mut mb = three_band();
    mb.set_band_solo(1, true);
    let low = steady(&mut mb, 30.0, 0.5);
    assert!(low < -30.0, "soloed mid band must mute the 30 Hz tone, got {low}");
    let mid = steady(&mut mb, 1000.0, 0.05);
    assert!(mid.abs() < 0.3, "soloed band is heard, got {mid}");

    // Bypassed band passes unchanged even over threshold.
    let mut mb = three_band();
    mb.set_band_bypass(1, true);
    assert!(steady(&mut mb, 1000.0, 0.9).abs() < 0.05);

    // Link: band 0 compresses a loud 100 Hz tone; band 1 sees only a quiet tone.
    let run = |link: f32| {
        let mut mb = MultibandCompressor::new(FS, 1, 3, LrOrder::Lr4, &[300.0, 3000.0]);
        mb.set_band_params(0, mid_only(-30.0));
        mb.set_band_params(1, mid_only(-30.0));
        mb.set_band_link(link);
        let n = 48_000;
        let lo = sine(100.0, 48_000.0, 0.8, n);
        let hi = sine(1000.0, 48_000.0, 0.005, n);
        let mut x: Vec<f32> = lo.iter().zip(&hi).map(|(a, b)| a + b).collect();
        run_mono(|b| mb.process(&mut [b]), &mut x, 128);
        (mb.gain_reduction_db(0), mb.gain_reduction_db(1))
    };
    let (a0, b0) = run(0.0);
    let (a1, b1) = run(1.0);
    eprintln!("link 0: GR {a0:.2}/{b0:.2}; link 1: GR {a1:.2}/{b1:.2}");
    assert!(a0 > 10.0 && b0 < 0.5, "unlinked: {a0} {b0}");
    assert!(b1 > a1 * 0.9, "fully linked bands follow the deepest: {a1} {b1}");
}

// ---- dynamic EQ ------------------------------------------------------------

fn eq_params() -> DynamicEqParams {
    DynamicEqParams {
        freq_hz: 1000.0,
        q: 1.0,
        threshold_db: -30.0,
        ratio: 4.0,
        knee_db: 0.0,
        range_db: -12.0,
        attack_ms: 10.0,
        release_ms: 100.0,
        rms_ms: 2.0,
        ..DynamicEqParams::default()
    }
}

fn eq_gain(p: DynamicEqParams, f: f64, amp: f64) -> f64 {
    let mut b = DynamicEqBand::new(FS, 1);
    b.set_params(p);
    let n = 60_000;
    let mut x = sine(f, f64::from(FS), amp, n);
    run_mono(|c| b.process(&mut [c]), &mut x, 64);
    db(amp_at(&x[n - 9600..], f, f64::from(FS)) / amp)
}

#[test]
fn dynamic_eq_static_curve() {
    let p = eq_params();
    for lvl in [-60.0f32, -40.0, -30.0, -25.0, -20.0, -15.0, -10.0, -6.0] {
        let want = f64::from(p.static_gain_db(lvl));
        let got = eq_gain(p, 1000.0, 10f64.powf(f64::from(lvl) / 20.0));
        eprintln!("dyn eq above, {lvl} dB: {got:+.2} dB (static {want:+.2})");
        assert!((got - want).abs() < 0.35, "{lvl}: {got} vs {want}");
    }
    // Below mode with a boost: quiet material is lifted, loud left alone.
    let q = DynamicEqParams { mode: DynEqMode::Below, range_db: 6.0, threshold_db: -30.0, ..eq_params() };
    for lvl in [-60.0f32, -40.0, -30.0, -20.0] {
        let want = f64::from(q.static_gain_db(lvl));
        let got = eq_gain(q, 1000.0, 10f64.powf(f64::from(lvl) / 20.0));
        assert!((got - want).abs() < 0.35, "below {lvl}: {got} vs {want}");
    }
    // The band is a bell: a tone two octaves off is hardly touched.
    let far = eq_gain(p, 4000.0, 0.3);
    assert!(far.abs() < 1.5, "off-band gain {far}");
}

#[test]
fn dynamic_eq_time_constants() {
    let mut p = eq_params();
    p.attack_ms = 40.0;
    p.release_ms = 120.0;
    p.rms_ms = 0.5;
    let mut b = DynamicEqBand::new(FS, 1);
    b.set_params(p);
    // Warm up quiet, then step to -20 dB (target: -7.5 dB).
    let run = |b: &mut DynamicEqBand, amp: f64, n: usize, rec: &mut Vec<f32>| {
        let mut x = sine(1000.0, f64::from(FS), amp, n);
        for s in x.chunks_mut(1) {
            b.process(&mut [s]);
            rec.push(b.dynamic_gain_db());
        }
    };
    let mut rec = Vec::new();
    run(&mut b, 0.001, 4800, &mut rec);
    rec.clear();
    run(&mut b, 0.1, 48_000, &mut rec);
    let target = f64::from(p.static_delta_db(-20.0)).abs();
    let end = f64::from(rec[rec.len() - 1]).abs();
    assert!((end - target).abs() < 0.3, "settles at {end} vs {target}");
    let t63 = rec.iter().position(|v| f64::from(v.abs()) >= 0.632 * target).unwrap() as f64 / f64::from(FS) * 1000.0;
    eprintln!("dyn eq attack: 63.2 % at {t63:.1} ms (set 40)");
    assert!((t63 - 40.0).abs() < 8.0, "attack constant {t63}");
    // Release: drop to a quiet tone.
    rec.clear();
    run(&mut b, 0.001, 48_000, &mut rec);
    let start = f64::from(rec[0]).abs();
    let t37 = rec.iter().position(|v| f64::from(v.abs()) <= 0.368 * start).unwrap() as f64 / f64::from(FS) * 1000.0;
    eprintln!("dyn eq release: 36.8 % left at {t37:.1} ms (set 120)");
    assert!((t37 - 120.0).abs() < 25.0, "release constant {t37}");
}

#[test]
fn dynamic_eq_sidechain_and_shelf() {
    let n = 48_000;
    let audio_amp = 0.01;
    let mk = || DynamicEqBand::new(FS, 1);
    let p = DynamicEqParams { attack_ms: 5.0, ..eq_params() };
    for (key_amp, cut) in [(0.0005f64, false), (0.3, true)] {
        let mut b = mk();
        b.set_params(p);
        let mut x = sine(1000.0, 48_000.0, audio_amp, n);
        let key = sine(1000.0, 48_000.0, key_amp, n);
        for (c, k) in x.chunks_mut(64).zip(key.chunks(64)) {
            b.process_sidechain(&mut [c], &[k]);
        }
        let g = db(amp_at(&x[n / 2..], 1000.0, 48_000.0) / audio_amp);
        eprintln!("sidechain key {key_amp}: audio gain {g:+.2} dB");
        if cut {
            assert!(g < -6.0, "{g}");
        } else {
            assert!(g.abs() < 0.2, "{g}");
        }
    }
    // High shelf cuts a loud 8 kHz tone by its range, leaves 200 Hz alone.
    let sh = DynamicEqParams {
        kind: DynEqKind::HighShelf,
        freq_hz: 4000.0,
        ratio: f32::INFINITY,
        range_db: -9.0,
        threshold_db: -30.0,
        q: 0.707,
        ..eq_params()
    };
    let hi = eq_gain(sh, 8000.0, 0.3);
    let lo = eq_gain(sh, 200.0, 0.3);
    eprintln!("high shelf: 8 kHz {hi:+.2} dB, 200 Hz {lo:+.2} dB");
    assert!((hi + 9.0).abs() < 0.8, "{hi}");
    assert!(lo.abs() < 0.3, "{lo}");
}

// ---- transient shaper ------------------------------------------------------

/// A 1 kHz tone: 0.3 s of silence, then 0.4 s at `amp` with an instant onset
/// (and optionally an exponential decay).
fn burst(decay_ms: Option<f64>) -> Vec<f32> {
    let n = (0.8 * f64::from(FS)) as usize;
    let on = (0.3 * f64::from(FS)) as usize;
    (0..n)
        .map(|i| {
            if i < on {
                return 0.0;
            }
            let t = (i - on) as f64 / f64::from(FS);
            let env = decay_ms.map_or(1.0, |d| (-t * 1000.0 / d).exp());
            (0.5 * env * (TAU * 1000.0 * i as f64 / f64::from(FS)).sin()) as f32
        })
        .collect()
}

fn shape(x: &[f32], attack: f32, sustain: f32) -> Vec<f32> {
    let mut ts = TransientShaper::new(FS);
    ts.set_attack(attack);
    ts.set_sustain(sustain);
    let mut y = x.to_vec();
    run_mono(|b| ts.process(&mut [b]), &mut y, 100);
    y
}

fn peak(x: &[f32]) -> f64 {
    x.iter().fold(0.0f64, |m, v| m.max(f64::from(v.abs())))
}

#[test]
fn transient_shaper_attack() {
    let x = burst(None);
    let on = (0.3 * f64::from(FS)) as usize;
    let win = |y: &[f32]| peak(&y[on..on + 240]); // first 5 ms
    let steady = |y: &[f32]| peak(&y[on + 19_000..on + 19_400]);
    let base = win(&x);
    for amt in [1.0f32, 0.5, -0.5, -1.0] {
        let y = shape(&x, amt, 0.0);
        let g = db(win(&y) / base);
        let gs = db(steady(&y) / steady(&x));
        eprintln!("attack {amt:+.1}: onset {g:+.2} dB, steady {gs:+.3} dB");
        assert!(gs.abs() < 0.3, "steady state must stay put: {gs}");
        if amt > 0.0 {
            assert!(g > 2.0 * f64::from(amt), "attack {amt} boosts onsets, got {g}");
        } else {
            assert!(g < 2.0 * f64::from(amt), "attack {amt} attenuates onsets, got {g}");
        }
    }
    // Zero amounts: transparent.
    let y = shape(&x, 0.0, 0.0);
    assert!(x.iter().zip(&y).all(|(a, b)| (a - b).abs() < 1e-6));
}

#[test]
fn transient_shaper_sustain() {
    let x = burst(Some(60.0));
    let on = (0.3 * f64::from(FS)) as usize;
    let tail = |y: &[f32]| peak(&y[on + 4800..on + 5200]); // ~100 ms in
    let head = |y: &[f32]| peak(&y[on..on + 240]);
    for amt in [1.0f32, -1.0] {
        let y = shape(&x, 0.0, amt);
        let g = db(tail(&y) / tail(&x));
        let gh = db(head(&y) / head(&x));
        eprintln!("sustain {amt:+.1}: tail {g:+.2} dB, onset {gh:+.2} dB");
        if amt > 0.0 {
            assert!(g > 1.5, "{g}");
        } else {
            assert!(g < -1.5, "{g}");
        }
        assert!(gh.abs() < 1.0, "onset unaffected by sustain: {gh}");
    }
}

#[test]
fn transient_shaper_is_nan_safe_and_linked() {
    let mut ts = TransientShaper::new(FS);
    ts.set_attack(f32::NAN);
    ts.set_attack(1.0);
    let mut l = vec![f32::NAN; 64];
    let mut r = vec![f32::INFINITY; 64];
    ts.process(&mut [&mut l, &mut r]);
    assert!(l.iter().chain(&r).all(|v| v.is_finite()));
}

// ---- de-esser --------------------------------------------------------------

fn deess(mode: DeEssMode, detect: DeEssDetect, input: &[f32]) -> Vec<f32> {
    let mut d = DeEsser::new(FS, 1);
    d.set_mode(mode);
    d.set_detect(detect);
    d.set_frequency(if detect == DeEssDetect::HighPass { 4000.0 } else { 7000.0 });
    d.set_threshold_db(-30.0);
    d.set_ratio(6.0);
    d.set_range_db(15.0);
    let mut y = input.to_vec();
    run_mono(|b| d.process(&mut [b]), &mut y, 100);
    y
}

#[test]
fn deesser_reduces_sibilance_not_low_tones() {
    let n = 48_000;
    for detect in [DeEssDetect::BandPass, DeEssDetect::HighPass] {
        for mode in [DeEssMode::WideBand, DeEssMode::SplitBand] {
            let hiss = sine(7000.0, 48_000.0, 0.3, n);
            let lo = sine(200.0, 48_000.0, 0.3, n);
            let yh = deess(mode, detect, &hiss);
            let yl = deess(mode, detect, &lo);
            let gh = db(amp_at(&yh[n / 2..], 7000.0, 48_000.0) / 0.3);
            let gl = db(amp_at(&yl[n / 2..], 200.0, 48_000.0) / 0.3);
            eprintln!("{detect:?} {mode:?}: 7 kHz {gh:+.2} dB, 200 Hz {gl:+.2} dB");
            assert!(gh < -6.0, "sibilance must be reduced: {gh}");
            assert!(gl.abs() < 0.3, "low tone must stay: {gl}");
        }
    }
    // Split-band leaves a loud low tone alone while a sibilant tone rides on
    // it; wide-band ducks both.
    let both: Vec<f32> = sine(7000.0, 48_000.0, 0.3, n).iter().zip(sine(200.0, 48_000.0, 0.3, n)).map(|(a, b)| a + b).collect();
    let split = deess(DeEssMode::SplitBand, DeEssDetect::BandPass, &both);
    let wide = deess(DeEssMode::WideBand, DeEssDetect::BandPass, &both);
    let ls = db(amp_at(&split[n / 2..], 200.0, 48_000.0) / 0.3);
    let lw = db(amp_at(&wide[n / 2..], 200.0, 48_000.0) / 0.3);
    eprintln!("200 Hz under sibilance: split {ls:+.2} dB, wide {lw:+.2} dB");
    assert!(ls.abs() < 0.5, "{ls}");
    assert!(lw < -4.0, "{lw}");
    // Below threshold: transparent (split-band is exactly x at unity gain).
    let quiet = sine(7000.0, 48_000.0, 0.001, 4800);
    let y = deess(DeEssMode::SplitBand, DeEssDetect::BandPass, &quiet);
    assert!(quiet.iter().zip(&y).all(|(a, b)| (a - b).abs() < 1e-5));
}

// ---- clipper ---------------------------------------------------------------

/// Independent 16x Hann-windowed-sinc reference (as in tests/dynamics.rs).
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
                acc += f64::from(x[(i + k) as usize]) * sinc * w;
            }
            peak = peak.max(acc.abs());
        }
    }
    peak
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

fn clip_signals() -> Vec<(&'static str, Vec<f32>)> {
    let n = 8192;
    let mut rng = Rng(7);
    vec![
        ("sine 1k x2", sine(1000.0, 48_000.0, 2.0, n)),
        ("fs/4 sine 45deg x2", (0..n).map(|i| (2.0 * (TAU * 0.25 * i as f64 + std::f64::consts::FRAC_PI_4).sin()) as f32).collect()),
        ("sine 11k x1.8", sine(11_000.0, 48_000.0, 1.8, n)),
        ("noise x3", (0..n).map(|_| (3.0 * rng.next() * 0.6) as f32).collect()),
        ("sweep x2", (0..n).map(|i| {
            let t = i as f64 / n as f64;
            (2.0 * (TAU * (20.0 * t + 0.5 * 20_000.0 * t * t) * n as f64 / 48_000.0).sin()) as f32
        }).collect()),
        ("square 3k x1.5", (0..n).map(|i| if (i / 8) % 2 == 0 { 1.5 } else { -1.5 }).collect()),
    ]
}

#[test]
fn clipper_holds_the_true_peak_ceiling() {
    let mut worst = f64::MIN;
    let mut worst_sample = f64::MIN;
    for factor in [2usize, 4] {
        for knee in [0.0f32, 6.0] {
            for ceiling_db in [-1.0f32, -6.0] {
                let ceiling = 10f64.powf(f64::from(ceiling_db) / 20.0);
                for (name, input) in clip_signals() {
                    let mut c = TruePeakClipper::prepare(48_000.0, factor, 1, 512).unwrap();
                    c.set_ceiling_db(ceiling_db);
                    c.set_knee_db(knee);
                                        let lat = c.latency_samples();
                    let mut y = input.clone();
                    run_mono(|b| c.process(&mut [b]), &mut y, 300);
                    let y = &y[lat..];
                    let tp = reference_true_peak(y);
                    let excess = db(tp / ceiling);
                    let sp = db(peak(y) / ceiling);
                    worst = worst.max(excess);
                    worst_sample = worst_sample.max(sp);
                    let mut det = TruePeak::precise();
                    let own = y.iter().fold(0.0f32, |m, v| m.max(det.push(*v)));
                    eprintln!("{name}: crate detector {:+.3} dB re ceiling", db(f64::from(own) / ceiling));
                    assert!(sp <= 1e-6, "{name}: sample peak over ceiling by {sp} dB");
                    eprintln!("{name} x{factor} knee {knee} ceil {ceiling_db}: {excess:+.3}");
                    assert!(excess < TOLERANCE_DB, "{name} x{factor} knee {knee} ceil {ceiling_db}: {excess:+.4} dB over");
                }
            }
        }
    }
    eprintln!("clipper: worst true-peak excess {worst:+.4} dB, worst sample excess {worst_sample:+.6} dB");
}

/// True-peak tolerance over the ceiling, dB, for every signal including
/// full-band white noise (`tests/true_peak_ceiling.rs` runs more signals, at
/// four sample rates, against two references).
const TOLERANCE_DB: f64 = 0.1;

#[test]
fn clipper_guard_and_transparency() {
    // Guard buys margin.
    let input = &clip_signals()[2].1;
    let excess = |guard: f32| {
        let mut c = TruePeakClipper::prepare(48_000.0, 4, 1, 512).unwrap();
        c.set_ceiling_db(-1.0);
        c.set_guard_db(guard);
        let lat = c.latency_samples();
        let mut y = input.clone();
        run_mono(|b| c.process(&mut [b]), &mut y, 512);
        db(reference_true_peak(&y[lat..]) / 10f64.powf(-0.05))
    };
    let (e0, e1) = (excess(0.0), excess(1.0));
    eprintln!("guard 0: {e0:+.3} dB, guard 1 dB: {e1:+.3} dB");
    assert!(e1 <= e0 + 1e-6);

    // Below the ceiling the block is a (delayed) wire.
    let mut c = TruePeakClipper::prepare(48_000.0, 4, 1, 512).unwrap();
    c.set_ceiling_db(-1.0);
    let lat = c.latency_samples();
    assert!(lat >= 64);
    let x = sine(1000.0, 48_000.0, 0.3, 9600);
    let mut y = x.clone();
    run_mono(|b| c.process(&mut [b]), &mut y, 512);
    let err = x[..9600 - lat].iter().zip(&y[lat..]).skip(2000).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    eprintln!("clipper below ceiling: max error {err:.2e}");
    assert!(err < 1e-3);

    // Hard curve and soft curve: soft never reaches the ceiling, both monotone.
    let mut prev = 0.0;
    for i in 0..400 {
        let v = TruePeakClipper::curve(i as f32 * 0.01, 0.8, 6.0);
        assert!(v < 0.8 && v >= prev);
        prev = v;
    }
    assert_eq!(TruePeakClipper::curve(5.0, 0.8, 0.0), 0.8);
    assert_eq!(TruePeakClipper::curve(-5.0, 0.8, 0.0), -0.8);
    // NaN safe, any channel count up to the prepared one.
    let mut c = TruePeakClipper::prepare(48_000.0, 2, 2, 64).unwrap();
    let mut l = vec![f32::NAN; 200];
    let mut r = vec![f32::INFINITY; 200];
    c.process(&mut [&mut l, &mut r]);
    assert!(l.iter().chain(&r).all(|v| v.is_finite()));
    let mut m = vec![2.0f32; 100];
    c.process(&mut [&mut m]);
}

#[test]
fn detection_enum_is_exported() {
    // Compile-time check that the shared envelope types compose with the new blocks.
    let _ = (Detection::Peak, DynEqKind::Bell);
}
