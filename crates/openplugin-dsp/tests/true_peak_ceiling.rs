//! The true-peak ceiling of `Limiter` and `TruePeakClipper`, measured.
//!
//! Every test signal is driven well over the ceiling at 44.1, 48, 96 and
//! 192 kHz with ceilings of -1 and -6 dBFS, and the output is read by two
//! references that share no code with the limiter's detector:
//!
//! - a Hann-windowed sinc of 48 samples each side evaluated directly at 16
//!   points per sample in f64 (`reference_true_peak`), and
//! - `TruePeakMeter` (ITU-R BS.1770-4 Annex 2: 4x below 96 kHz, 2x below
//!   192 kHz, the sample peak from 192 kHz).
//!
//! Both must read at or below the ceiling + 0.1 dB. The tests print the
//! measured excess per signal; `docs/DSP_COOKBOOK.md` quotes the worst cases.

use openplugin_dsp::{Limiter, TruePeak, TruePeakClipper, TruePeakMeter};
use std::f64::consts::{PI, TAU};
use std::sync::OnceLock;

const TOLERANCE_DB: f64 = 0.1;
const RATES: [f32; 4] = [44_100.0, 48_000.0, 96_000.0, 192_000.0];
const CEILINGS_DB: [f32; 2] = [-1.0, -6.0];
const LEN: usize = 12_000;

/// Largest |value| of the windowed-sinc interpolation of `x` (Hann, `radius`
/// samples each side, `over` points per sample), over the interior.
fn sinc_true_peak(x: &[f32], radius: usize, over: usize, table: &[f64]) -> f64 {
    let taps = 2 * radius;
    let mut peak = 0.0f64;
    for i in radius..x.len() - radius - 1 {
        let w = &x[i + 1 - radius..=i + radius];
        for row in table.chunks_exact(taps).take(over) {
            let mut acc = 0.0f64;
            for (v, h) in w.iter().zip(row) {
                acc += f64::from(*v) * h;
            }
            peak = peak.max(acc.abs());
        }
    }
    peak
}

fn sinc_table(radius: usize, over: usize) -> Vec<f64> {
    let mut table = Vec::with_capacity(over * 2 * radius);
    for s in 0..over {
        let t = s as f64 / over as f64;
        for j in 0..2 * radius {
            let d = t - (j as f64 + 1.0 - radius as f64);
            let sinc = if d == 0.0 { 1.0 } else { (PI * d).sin() / (PI * d) };
            table.push(sinc * 0.5 * (1.0 + (PI * d / (radius as f64 + 1.0)).cos()));
        }
    }
    table
}

/// The reference of `tests/dynamics.rs` and `tests/multiband.rs`: 16 points
/// per sample, 48 samples each side.
fn reference_true_peak(x: &[f32]) -> f64 {
    static TABLE: OnceLock<Vec<f64>> = OnceLock::new();
    sinc_true_peak(x, 48, 16, TABLE.get_or_init(|| sinc_table(48, 16)))
}

fn meter_true_peak(x: &[f32], fs: f32) -> f64 {
    let mut m = TruePeakMeter::new(fs, 1);
    m.process(0, x);
    10f64.powf(f64::from(m.max_dbtp(0)) / 20.0)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

fn normalised(v: Vec<f64>, peak: f64) -> Vec<f32> {
    let top = v.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    v.iter().map(|x| (x * peak / top) as f32).collect()
}

/// The test signals, all with a sample peak of 1.5 to 2 (3.5 to 6 dB over
/// full scale), built for the rate `fs`.
fn signals(fs: f64) -> Vec<(&'static str, Vec<f32>)> {
    let n = LEN;
    let mut rng = Rng(0x5eed);
    let mut out: Vec<(&'static str, Vec<f32>)> = Vec::new();

    out.push(("white noise", (0..n).map(|_| (2.0 * rng.next()) as f32).collect()));

    // Pink: seven one-pole low-passes an octave apart from fs/4 down, summed
    // with equal weight (about -3 dB per octave over that span).
    let mut state = [0.0f64; 7];
    let pink: Vec<f64> = (0..n)
        .map(|_| {
            let w = rng.next();
            let mut sum = 0.0;
            for (k, s) in state.iter_mut().enumerate() {
                let a = (-TAU * 0.25 / f64::from(1u32 << k)).exp();
                *s = a * *s + (1.0 - a) * w;
                sum += *s / (1.0 - a).sqrt();
            }
            sum
        })
        .collect();
    out.push(("pink noise", normalised(pink, 2.0)));

    // Hard squares (not band-limited): 1 kHz, and a period of 7 samples.
    let period = (fs / 1000.0).round() as usize;
    out.push(("square 1 kHz", (0..n).map(|i| if (i % period) < period / 2 { 1.5 } else { -1.5 }).collect()));
    out.push(("square fs/7", (0..n).map(|i| if (i % 7) < 3 { 1.5 } else { -1.5 }).collect()));

    // Logarithmic sweep, 20 Hz to 0.499 fs.
    let (f0, f1) = (20.0 / fs, 0.499);
    let k = (f1 / f0).ln();
    out.push((
        "sweep to Nyquist",
        (0..n)
            .map(|i| {
                let t = i as f64 / n as f64;
                (1.8 * (TAU * f0 * n as f64 * ((k * t).exp() - 1.0) / k).sin()) as f32
            })
            .collect(),
    ));

    // Impulses: single samples of either sign, an opposite pair, an
    // alternating run of five, over a quiet tone.
    let mut imp: Vec<f32> = (0..n).map(|i| (0.05 * (TAU * 0.01 * i as f64).sin()) as f32).collect();
    let mut at = 300;
    let mut kind = 0;
    while at + 8 < n {
        match kind % 4 {
            0 => imp[at] = 2.0,
            1 => imp[at] = -2.0,
            2 => {
                imp[at] = 2.0;
                imp[at + 1] = -2.0;
            }
            _ => {
                for j in 0..5 {
                    imp[at + j] = if j % 2 == 0 { 1.9 } else { -1.9 };
                }
            }
        }
        kind += 1;
        at += 331;
    }
    out.push(("impulses", imp));

    // Drum-like hits, hard clipped: a 60 Hz body and a noise burst, each
    // decaying, six times over, clipped flat at 1.5.
    let hit = n / 6;
    out.push((
        "clipped drum hits",
        (0..n)
            .map(|i| {
                let t = (i % hit) as f64 / fs;
                let body = 4.0 * (-t / 0.030).exp() * (TAU * 60.0 * t).sin();
                let snap = 6.0 * (-t / 0.004).exp() * rng.next();
                (body + snap).clamp(-1.5, 1.5) as f32
            })
            .collect(),
    ));

    // The classic inter-sample case: fs/4 sampled at 45 degrees.
    out.push(("fs/4 at 45 degrees", (0..n).map(|i| (1.6 * (TAU * 0.25 * i as f64 + PI / 4.0).sin()) as f32).collect()));
    out
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

/// One block's output for one signal: (excess by the 16x reference, excess by
/// the meter), both in dB over the ceiling.
struct Reading {
    name: &'static str,
    reference: f64,
    meter: f64,
}

fn run_matrix(label: &str, make: impl Fn(f32, f32, &mut [f32]) + Sync) -> Vec<Reading> {
    let mut all = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = RATES
            .iter()
            .map(|&fs| {
                let make = &make;
                scope.spawn(move || {
                    let mut rows = Vec::new();
                    for ceiling_db in CEILINGS_DB {
                        let ceiling = 10f64.powf(f64::from(ceiling_db) / 20.0);
                        for (name, input) in signals(f64::from(fs)) {
                            assert!(reference_true_peak(&input) > ceiling * 1.3, "{name}: the test signal is too quiet");
                            let mut y = input.clone();
                            make(fs, ceiling_db, &mut y);
                            assert!(y.iter().all(|v| v.is_finite()));
                            // The whole output is read, from its silent start
                            // (cutting into it would hand the meter a step the
                            // block never produced).
                            let reference = db(reference_true_peak(&y) / ceiling);
                            let meter = db(meter_true_peak(&y, fs) / ceiling);
                            rows.push((fs, ceiling_db, Reading { name, reference, meter }));
                        }
                    }
                    rows
                })
            })
            .collect();
        for h in handles {
            for (fs, ceiling_db, r) in h.join().unwrap() {
                eprintln!(
                    "{label} {fs:>6} Hz ceiling {ceiling_db:+.0} dB {:>18}: 16x reference {:+.3} dB, meter {:+.3} dB",
                    r.name, r.reference, r.meter
                );
                all.push(r);
            }
        }
    });
    // Worst case per signal over the rates and ceilings.
    let mut names: Vec<&'static str> = Vec::new();
    for r in &all {
        if !names.contains(&r.name) {
            names.push(r.name);
        }
    }
    for name in names {
        let worst = |f: fn(&Reading) -> f64| all.iter().filter(|r| r.name == name).map(f).fold(f64::MIN, f64::max);
        eprintln!("{label} worst {name:>18}: 16x reference {:+.3} dB, meter {:+.3} dB", worst(|r| r.reference), worst(|r| r.meter));
    }
    all
}

fn assert_held(label: &str, rows: &[Reading]) {
    for r in rows {
        assert!(
            r.reference <= TOLERANCE_DB && r.meter <= TOLERANCE_DB,
            "{label}, {}: {:+.3} dB over the ceiling by the 16x reference, {:+.3} dB by the meter",
            r.name,
            r.reference,
            r.meter
        );
    }
}

/// Process in uneven blocks, to cross block boundaries everywhere.
fn in_blocks(buf: &mut [f32], mut f: impl FnMut(&mut [f32])) {
    let (mut pos, mut size) = (0, 1);
    while pos < buf.len() {
        let end = (pos + size).min(buf.len());
        f(&mut buf[pos..end]);
        pos = end;
        size = size * 3 % 97 + 1;
    }
}

#[test]
fn limiter_holds_the_ceiling_on_every_signal_at_every_rate() {
    let rows = run_matrix("limiter 1.5 ms", |fs, ceiling_db, buf| {
        let mut lim = Limiter::prepare(fs, 1, 1.5, 50.0);
        lim.set_ceiling_db(ceiling_db);
        in_blocks(buf, |b| lim.process(&mut [b]));
    });
    assert_held("limiter 1.5 ms", &rows);
}

/// A short lookahead and a fast release: the gain moves faster, which is the
/// hard case for a gain applied sample by sample (see the limiter's "Margin").
#[test]
fn limiter_holds_the_ceiling_with_a_short_lookahead() {
    let rows = run_matrix("limiter 1 ms", |fs, ceiling_db, buf| {
        let mut lim = Limiter::prepare(fs, 1, 1.0, 20.0);
        lim.set_ceiling_db(ceiling_db);
        in_blocks(buf, |b| lim.process(&mut [b]));
    });
    assert_held("limiter 1 ms", &rows);
}

#[test]
fn clipper_holds_the_ceiling_on_every_signal_at_every_rate() {
    for (factor, knee) in [(4usize, 0.0f32), (2, 6.0)] {
        let label = format!("clipper {factor}x knee {knee}");
        let rows = run_matrix(&label, |fs, ceiling_db, buf| {
            let mut clip = TruePeakClipper::prepare(fs, factor, 1, 256).unwrap();
            clip.set_ceiling_db(ceiling_db);
            clip.set_knee_db(knee);
            in_blocks(buf, |b| clip.process(&mut [b]));
        });
        assert_held(&label, &rows);
        // The sample peak is under the ceiling by construction; checked on one case.
        let mut clip = TruePeakClipper::prepare(48_000.0, factor, 1, 256).unwrap();
        clip.set_ceiling_db(-1.0);
        clip.set_knee_db(knee);
        let mut y = signals(48_000.0).remove(0).1;
        clip.process(&mut [&mut y]);
        let top = y.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(f64::from(top) <= 10f64.powf(-1.0 / 20.0) * (1.0 + 1e-6), "{top}");
    }
}

/// What other reconstruction filters make of the limited white noise. Full-band
/// noise has energy at the edge of the band, where the reading depends on the
/// filter's length; the limiter bounds the three filters it evaluates (the 16x
/// reference's kernel, the meter's and `TruePeak::new`'s), and a filter that is
/// none of them can read a little higher. Stated, with bounds, not hidden.
#[test]
fn other_reconstructions_of_limited_white_noise() {
    let fs = 48_000.0f32;
    let ceiling = 10f64.powf(-1.0 / 20.0);
    let mut y = signals(f64::from(fs)).remove(0).1;
    let mut lim = Limiter::prepare(fs, 1, 1.5, 50.0);
    lim.set_ceiling_db(-1.0);
    lim.process(&mut [&mut y]);
    let y = &y[..];

    let run = |mut det: TruePeak| db(f64::from(y.iter().fold(0.0f32, |m, v| m.max(det.push(*v)))) / ceiling);
    let standard = run(TruePeak::new());
    let precise = run(TruePeak::precise());
    let mid = db(sinc_true_peak(y, 24, 16, &sinc_table(24, 16)) / ceiling);
    let long = db(sinc_true_peak(y, 200, 16, &sinc_table(200, 16)) / ceiling);
    eprintln!("limited white noise, excess over the ceiling by other filters:");
    eprintln!("  TruePeak::new (12 taps per phase)      {standard:+.3} dB");
    eprintln!("  TruePeak::precise (16 taps per phase)  {precise:+.3} dB");
    eprintln!("  Hann sinc, 24 samples each side, 16x   {mid:+.3} dB");
    eprintln!("  Hann sinc, 200 samples each side, 16x  {long:+.3} dB");
    assert!(standard <= TOLERANCE_DB, "{standard}");
    assert!(precise < 0.5, "{precise}");
    assert!(mid < 0.5, "{mid}");
    assert!(long < 1.2, "{long}");
}
