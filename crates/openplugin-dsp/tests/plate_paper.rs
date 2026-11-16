//! `PlateReverb` pinned to its source: Jon Dattorro, "Effect Design, Part 1:
//! Reverberator and Other Filters", J. Audio Eng. Soc., vol. 45, no. 9,
//! pp. 660-684, September 1997.
//!
//! The numbers in this file are typed from the paper (figure 1, table 1 and
//! table 2), not taken from the code under test.

use openplugin_dsp::PlateReverb;
use openplugin_dsp::plate::{PLATE_DECAY_CALIBRATION, PlateGeometry, PlateLine, PlateTap};

const PAPER_RATE: f32 = 29_761.0;

// Figure 1.
const INPUT_DIFFUSERS: [usize; 4] = [142, 107, 379, 277];
const LEFT: [usize; 4] = [672, 4453, 1800, 3720];
const RIGHT: [usize; 4] = [908, 4217, 2656, 3163];
// Table 1.
const EXCURSION: f64 = 16.0;
const INPUT_DIFFUSION_1: f32 = 0.750;
const INPUT_DIFFUSION_2: f32 = 0.625;
const DECAY_DIFFUSION_1: f32 = 0.70;
const DECAY: f32 = 0.50;
const BANDWIDTH: f32 = 0.9995;
const DAMPING: f32 = 0.0005;

use PlateLine::{LeftDelay1, LeftDelay2, LeftDiffuser2, RightDelay1, RightDelay2, RightDiffuser2};

// Table 2. node24_30 is the 4453 line, node31_33 the 1800 line, node33_39 the
// 3720 line, node48_54 the 4217 line, node55_59 the 2656 line and node59_63
// the 3163 line; `+=` is +1, `-=` is -1, every tap times 0.6.
const TAPS_LEFT: [(PlateLine, usize, f32); 7] = [
    (RightDelay1, 266, 1.0),     // accumulator  = 0.6 x node48_54[266]
    (RightDelay1, 2974, 1.0),    // accumulator += 0.6 x node48_54[2974]
    (RightDiffuser2, 1913, -1.0), // accumulator -= 0.6 x node55_59[1913]
    (RightDelay2, 1996, 1.0),    // accumulator += 0.6 x node59_63[1996]
    (LeftDelay1, 1990, -1.0),    // accumulator -= 0.6 x node24_30[1990]
    (LeftDiffuser2, 187, -1.0),  // accumulator -= 0.6 x node31_33[187]
    (LeftDelay2, 1066, -1.0),    // yL = accumulator - 0.6 x node33_39[1066]
];
const TAPS_RIGHT: [(PlateLine, usize, f32); 7] = [
    (LeftDelay1, 353, 1.0),      // accumulator  = 0.6 x node24_30[353]
    (LeftDelay1, 3627, 1.0),     // accumulator += 0.6 x node24_30[3627]
    (LeftDiffuser2, 1228, -1.0), // accumulator -= 0.6 x node31_33[1228]
    (LeftDelay2, 2673, 1.0),     // accumulator += 0.6 x node33_39[2673]
    (RightDelay1, 2111, -1.0),   // accumulator -= 0.6 x node48_54[2111]
    (RightDiffuser2, 335, -1.0), // accumulator -= 0.6 x node55_59[335]
    (RightDelay2, 121, -1.0),    // yR = accumulator - 0.6 x node59_63[121]
];

/// `n fs / 29761`, rounded to the nearest sample.
fn at_rate(n: usize, fs: f64) -> usize {
    (n as f64 * fs / 29_761.0).round() as usize
}

fn geometry_at(fs: f64) -> PlateGeometry {
    let taps = |t: &[(PlateLine, usize, f32); 7]| {
        t.map(|(line, offset, sign)| PlateTap { line, offset: at_rate(offset, fs), sign })
    };
    PlateGeometry {
        input_diffusers: INPUT_DIFFUSERS.map(|n| at_rate(n, fs)),
        left: LEFT.map(|n| at_rate(n, fs)),
        right: RIGHT.map(|n| at_rate(n, fs)),
        excursion: (EXCURSION * fs / 29_761.0) as f32,
        taps_left: taps(&TAPS_LEFT),
        taps_right: taps(&TAPS_RIGHT),
    }
}

fn paper_plate(fs: f32) -> PlateReverb {
    let mut p = PlateReverb::new(fs);
    p.set_paper_defaults();
    p
}

fn impulse_response(p: &mut PlateReverb, n: usize) -> (Vec<f32>, Vec<f32>) {
    let mut l = vec![0.0f32; n];
    let mut r = vec![0.0f32; n];
    l[0] = 1.0;
    r[0] = 1.0;
    p.process(&mut l, &mut r);
    (l, r)
}

#[test]
fn at_the_paper_rate_the_lengths_and_taps_are_the_papers() {
    let g = PlateReverb::new(PAPER_RATE).geometry();
    assert_eq!(g.input_diffusers, INPUT_DIFFUSERS);
    assert_eq!(g.left, LEFT);
    assert_eq!(g.right, RIGHT);
    assert_eq!(g.excursion, 16.0);
    for (got, want) in g.taps_left.iter().zip(&TAPS_LEFT) {
        assert_eq!((got.line, got.offset, got.sign), *want);
    }
    for (got, want) in g.taps_right.iter().zip(&TAPS_RIGHT) {
        assert_eq!((got.line, got.offset, got.sign), *want);
    }
    // Every tap lies inside its line.
    let len = |line| match line {
        LeftDelay1 => LEFT[1],
        LeftDiffuser2 => LEFT[2],
        LeftDelay2 => LEFT[3],
        RightDelay1 => RIGHT[1],
        RightDiffuser2 => RIGHT[2],
        RightDelay2 => RIGHT[3],
        _ => unimplemented!(),
    };
    for t in TAPS_LEFT.iter().chain(&TAPS_RIGHT) {
        assert!(t.1 < len(t.0));
    }
    // 21 589 samples around the figure-eight.
    let total: usize = LEFT.iter().chain(&RIGHT).sum();
    assert_eq!(total, 21_589);
    let p = PlateReverb::new(PAPER_RATE);
    assert!((f64::from(p.loop_time_s()) - 21_589.0 / 29_761.0).abs() < 1e-6);
}

#[test]
fn at_other_rates_the_lengths_and_taps_are_scaled_and_rounded() {
    for fs in [44_100.0f32, 48_000.0, 96_000.0, 192_000.0] {
        let p = PlateReverb::new(fs);
        let g = p.geometry();
        let want = geometry_at(f64::from(fs));
        assert_eq!(g, want, "{fs}");
        // The loop lasts as long in seconds as the paper's, within the rounding
        // of eight lengths.
        let loop_s = f64::from(p.loop_time_s());
        assert!((loop_s - 21_589.0 / 29_761.0).abs() < 4.0 / f64::from(fs), "{fs}: {loop_s}");
    }
    // Worked by hand: 4453 x 48000 / 29761 = 7181.99, 266 x 44100 / 29761 =
    // 394.16, 3163 x 96000 / 29761 = 10202.88, 672 x 192000 / 29761 = 4335.34,
    // 16 x 48000 / 29761 = 25.806.
    assert_eq!(PlateReverb::new(48_000.0).geometry().left[1], 7182);
    assert_eq!(PlateReverb::new(44_100.0).geometry().taps_left[0].offset, 394);
    assert_eq!(PlateReverb::new(96_000.0).geometry().right[3], 10_203);
    assert_eq!(PlateReverb::new(192_000.0).geometry().left[0], 4335);
    assert!((PlateReverb::new(48_000.0).geometry().excursion - 25.806).abs() < 1e-3);
}

/// Table 2 implies when each channel first sounds: the earliest tap of the left
/// output is 266 samples into the right half's first delay and the earliest of
/// the right output 353 samples into the left half's. The first sample to get
/// there takes the direct path of every lattice before it: `bandwidth` times
/// the four input diffusion coefficients, times `-decay diffusion 1` (the
/// reversed sign of figure 1), times 0.6.
#[test]
fn the_impulse_response_starts_where_table_2_says() {
    let direct = BANDWIDTH
        * INPUT_DIFFUSION_1
        * INPUT_DIFFUSION_1
        * INPUT_DIFFUSION_2
        * INPUT_DIFFUSION_2
        * -DECAY_DIFFUSION_1
        * 0.6;
    for fs in [PAPER_RATE, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        // The direct path does not pass a modulated tap, so this holds with
        // the modulation running.
        let mut p = paper_plate(fs);
        let first_l = at_rate(266, f64::from(fs));
        let first_r = at_rate(353, f64::from(fs));
        let (l, r) = impulse_response(&mut p, first_r + 2);
        assert!(l[..first_l].iter().all(|v| *v == 0.0), "{fs}: left sounds early");
        assert!(r[..first_r].iter().all(|v| *v == 0.0), "{fs}: right sounds early");
        assert!((l[first_l] - direct).abs() < 1e-6, "{fs}: left {} vs {direct}", l[first_l]);
        assert!((r[first_r] - direct).abs() < 1e-6, "{fs}: right {} vs {direct}", r[first_r]);
    }

    // At the paper's rate, with the input one-pole open (bandwidth 1) so that
    // each arrival is one sample, the next arrivals come through the input
    // diffusers' delays: 107, 142 and 107 + 142 samples after the first, each
    // the direct value with one or two coefficients `g` replaced by
    // `1 - g^2`, and 2 x 107 after it the second turn in the 107 diffuser
    // (weight `-g (1 - g^2)`). Nothing else arrives before the 277 diffuser's
    // delay has passed.
    let mut p = paper_plate(PAPER_RATE);
    p.set_bandwidth(1.0);
    p.set_modulation(0.0, 1.0);
    let (l, r) = impulse_response(&mut p, 700);
    let direct = direct / BANDWIDTH;
    let g = INPUT_DIFFUSION_1;
    let once = direct * (1.0 - g * g) / g;
    let twice = once * (1.0 - g * g) / g;
    for (name, out, first) in [("left", &l, 266usize), ("right", &r, 353)] {
        for (n, v) in out.iter().enumerate().take(first + 277) {
            let want = match n as isize - first as isize {
                0 => direct,
                107 | 142 => once,
                249 => twice,
                214 => -g * once,
                _ => 0.0,
            };
            assert!((v - want).abs() < 1e-6, "{name} at {n}: {v} vs {want}");
        }
    }
}

/// Schroeder backward integration of the whole response (M. R. Schroeder,
/// "New method of measuring reverberation time", JASA 37, 1965); T60 from a
/// least-squares line over -5 dB to -35 dB.
fn t60(x: &[f32], fs: f32) -> f64 {
    let mut edc = vec![0.0f64; x.len()];
    let mut acc = 0.0f64;
    for i in (0..x.len()).rev() {
        acc += f64::from(x[i]) * f64::from(x[i]);
        edc[i] = acc;
    }
    let total = edc[0];
    let (mut n, mut sx, mut sy, mut sxx, mut sxy) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
    for (i, e) in edc.iter().enumerate() {
        if *e <= 0.0 {
            break;
        }
        let db = 10.0 * (e / total).log10();
        if db < -35.0 {
            break;
        }
        if db <= -5.0 {
            let t = i as f64 / f64::from(fs);
            n += 1.0;
            sx += t;
            sy += db;
            sxx += t * t;
            sxy += t * db;
        }
    }
    let slope = (n * sxy - sx * sy) / (n * sxx - sx * sx);
    -60.0 / slope
}

/// What the decay coefficient implies: the figure-eight is `T` seconds of
/// delay with four multiplications by `decay`, so the level falls
/// `80 log10(decay)` dB per `T` and `T60 = -3 T / (4 log10 decay)`.
fn implied_t60(decay: f32, loop_s: f32) -> f64 {
    -3.0 * f64::from(loop_s) / (4.0 * f64::from(decay).log10())
}

/// Tolerance, stated: the measured T60 is 0 to 13 % longer than the decay
/// coefficient implies (never shorter by more than 1 %). With the modulation
/// off and `decay >= 0.5` it is 10 % longer within one point: the diffusers
/// are allpass only on the unit circle, and the resonances that sit where
/// their group delay is long outlast the nominal-length estimate. Modulation
/// moves those resonances about, and long tails then come back toward the
/// estimate.
#[test]
fn decay_time_rises_with_the_decay_coefficient_and_follows_it() {
    let mut ratios = Vec::new();
    for fs in [PAPER_RATE, 48_000.0] {
        for modulation in [0.0f32, 0.5] {
            let mut last = 0.0f64;
            for decay in [0.2f32, 0.35, DECAY, 0.65, 0.8, 0.9, 0.95] {
                let mut p = paper_plate(fs);
                assert_eq!(p.damping(), DAMPING);
                p.set_decay(decay);
                assert_eq!(p.decay(), decay);
                p.set_modulation(modulation, 1.0);
                let want = implied_t60(decay, p.loop_time_s());
                let n = ((want * 2.2 + 1.5) * f64::from(fs)) as usize;
                let (l, r) = impulse_response(&mut p, n);
                let (got_l, got_r) = (t60(&l, fs), t60(&r, fs));
                let got = 0.5 * (got_l + got_r);
                let ratio = got / want;
                println!(
                    "plate fs {fs} mod {modulation} decay {decay}: implied {want:.3} s, measured L {got_l:.3} R {got_r:.3} (ratio {ratio:.3})"
                );
                assert!(got > last, "T60 must rise with decay: {got} after {last}");
                assert!((0.99..1.13).contains(&ratio), "fs {fs} mod {modulation} decay {decay}: {ratio}");
                if modulation == 0.0 && decay >= 0.5 {
                    assert!((ratio - 1.10).abs() < 0.01, "fs {fs} decay {decay}: {ratio}");
                }
                last = got;
                ratios.push(ratio);
            }
        }
    }
    let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
    let (lo, hi) = ratios.iter().fold((f64::MAX, f64::MIN), |(a, b), r| (a.min(*r), b.max(*r)));
    println!("measured / implied T60: mean {mean:.3}, range {lo:.3} to {hi:.3}");
}

/// `set_rt60` maps seconds to the coefficient with the calibration `k`:
/// `decay = 10^(-3 k T / (4 RT60))`. Measured against the setting from 1 s up
/// (below that the tail is a few passes of the 0.73 s loop, a staircase the
/// line fit does not describe): within 6 % with the modulation off and within
/// 8 % at the default modulation.
#[test]
fn the_rt60_setting_is_what_is_measured() {
    assert_eq!(PLATE_DECAY_CALIBRATION, 1.07);
    let mut worst = [0.0f64; 2];
    for fs in [PAPER_RATE, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        for want in [1.0f32, 2.5, 5.0, 10.0] {
            if fs > 50_000.0 && want > 5.0 {
                continue;
            }
            for (i, modulation) in [0.0f32, 0.5].into_iter().enumerate() {
                let mut p = PlateReverb::new(fs);
                p.set_rt60(want);
                p.set_damping_hz(1.0e6);
                p.set_bandwidth_hz(1.0e6);
                p.set_modulation(modulation, 1.0);
                let k = f64::from(PLATE_DECAY_CALIBRATION);
                assert!((implied_t60(p.decay(), p.loop_time_s()) * k / f64::from(want) - 1.0).abs() < 1e-4);
                let n = ((want * 2.2 + 1.5) * fs) as usize;
                let (l, r) = impulse_response(&mut p, n);
                let got = 0.5 * (t60(&l, fs) + t60(&r, fs));
                let err = got / f64::from(want) - 1.0;
                println!(
                    "plate fs {fs} mod {modulation} rt60 {want}: decay {:.4}, measured {got:.3} s ({:+.1} %)",
                    p.decay(),
                    err * 100.0
                );
                worst[i] = worst[i].max(err.abs());
                assert!(err.abs() < [0.06, 0.08][i], "fs {fs} mod {modulation} rt60 {want}: {got}");
            }
        }
    }
    println!(
        "worst RT60 error: {:.1} % without modulation, {:.1} % at the default modulation",
        worst[0] * 100.0,
        worst[1] * 100.0
    );
}

/// Normalised echo density (Abel and Huang, "A simple, robust measure of
/// reverberation echo density", AES 121, 2006): the fraction of samples in a
/// window that lie outside one standard deviation of the window, divided by
/// erfc(1/sqrt 2) = 0.3173, its value for Gaussian noise.
fn echo_density(x: &[f32], centre: usize, half: usize) -> f64 {
    let w = &x[centre.saturating_sub(half)..(centre + half).min(x.len())];
    let n = w.len() as f64;
    let mean = w.iter().map(|v| f64::from(*v)).sum::<f64>() / n;
    let sd = (w.iter().map(|v| (f64::from(*v) - mean).powi(2)).sum::<f64>() / n).sqrt();
    if sd == 0.0 {
        return 0.0;
    }
    w.iter().filter(|v| (f64::from(**v) - mean).abs() > sd).count() as f64 / n / 0.3173
}

#[test]
fn echo_density_builds_up() {
    for fs in [PAPER_RATE, 48_000.0] {
        let mut p = paper_plate(fs);
        p.set_modulation(0.0, 1.0);
        let (l, r) = impulse_response(&mut p, (1.6 * fs) as usize);
        let half = (0.02 * fs) as usize;
        for (name, x) in [("left", &l), ("right", &r)] {
            let at = |ms: f32| echo_density(x, (ms * 0.001 * fs) as usize, half);
            let d = [at(40.0), at(120.0), at(300.0), at(700.0), at(1400.0)];
            println!(
                "plate fs {fs} {name} echo density at 40, 120, 300, 700, 1400 ms: {:.2} {:.2} {:.2} {:.2} {:.2}",
                d[0], d[1], d[2], d[3], d[4]
            );
            assert!(d[0] < d[1] && d[1] < d[2], "{name}: density must rise: {d:?}");
            assert!(d[0] < 0.6, "{name}: not yet dense at 40 ms: {d:?}");
            assert!(d[3] > 0.85 && d[4] > 0.85, "{name}: Gaussian late: {d:?}");
        }
    }
}

/// Cheap deterministic white noise in -1..1.
struct Noise(u64);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32 / (1u64 << 23) as f32) - 1.0
    }
}

fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        ab += f64::from(*x) * f64::from(*y);
        aa += f64::from(*x) * f64::from(*x);
        bb += f64::from(*y) * f64::from(*y);
    }
    ab / (aa * bb).sqrt()
}

/// The two outputs of table 2 are decorrelated although the tank's input is
/// mono: the interchannel correlation at lag zero stays under 0.05 for noise
/// and for the impulse response.
#[test]
fn the_stereo_outputs_are_decorrelated() {
    for fs in [PAPER_RATE, 48_000.0] {
        for modulation in [0.0f32, 0.5] {
            let mut p = paper_plate(fs);
            p.set_modulation(modulation, 1.0);
            let (l, r) = impulse_response(&mut p, (3.0 * fs) as usize);
            let ir = correlation(&l, &r);

            let mut p = paper_plate(fs);
            p.set_modulation(modulation, 1.0);
            let mut noise = Noise(9);
            let mut l: Vec<f32> = (0..(4.0 * fs) as usize).map(|_| noise.next()).collect();
            let mut r = l.clone();
            p.process(&mut l, &mut r);
            let skip = fs as usize;
            let steady = correlation(&l[skip..], &r[skip..]);
            println!("plate fs {fs} mod {modulation} interchannel correlation: impulse {ir:+.3}, noise {steady:+.3}");
            assert!(ir.abs() < 0.05, "{ir}");
            assert!(steady.abs() < 0.05, "{steady}");

            // Width 0 is the check that the measure can see correlation at all.
            p.reset();
            p.set_width(0.0);
            let mut noise = Noise(9);
            let mut l: Vec<f32> = (0..fs as usize).map(|_| noise.next()).collect();
            let mut r = l.clone();
            p.process(&mut l, &mut r);
            assert!(correlation(&l, &r) > 0.999);
        }
    }
}

/// After a loud burst the tail dies to exact zeros: no NaN, no infinity and no
/// subnormal number ever reaches the output.
#[test]
fn silence_after_a_burst_has_no_nan_and_no_denormals() {
    for fs in [PAPER_RATE, 48_000.0] {
        for (name, setup) in [
            ("paper defaults", (|p: &mut PlateReverb| p.set_paper_defaults()) as fn(&mut PlateReverb)),
            ("defaults", |_| {}),
            ("damped", |p| {
                p.set_rt60(0.5);
                p.set_damping_hz(200.0);
                p.set_bandwidth_hz(500.0);
            }),
            ("modulated", |p| {
                p.set_rt60(1.0);
                p.set_modulation(1.0, 5.0);
            }),
        ] {
            let mut p = PlateReverb::new(fs);
            setup(&mut p);
            let mut noise = Noise(3);
            let mut peak = 0.0f32;
            for _ in 0..(0.5 * fs) as usize {
                let (a, b) = p.process_frame(4.0 * noise.next(), 4.0 * noise.next());
                assert!(a.is_finite() && b.is_finite());
                peak = peak.max(a.abs()).max(b.abs());
            }
            assert!(peak > 0.1, "{name}: the burst is heard ({peak})");
            let mut zero_from = None;
            let n = (40.0 * fs) as usize;
            for i in 0..n {
                let (a, b) = p.process_frame(0.0, 0.0);
                for v in [a, b] {
                    assert!(v == 0.0 || v.is_normal(), "{name} at {fs}: {v:e} at sample {i}");
                }
                if a == 0.0 && b == 0.0 {
                    zero_from.get_or_insert(i);
                } else {
                    zero_from = None;
                }
            }
            let at = zero_from.unwrap_or_else(|| panic!("{name} at {fs}: the tail never reaches zero"));
            println!("plate fs {fs} {name}: exact silence {:.1} s after the burst", at as f32 / fs);
            assert!(at < n - fs as usize);
        }
    }
}
