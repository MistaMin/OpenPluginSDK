//! Proofs for the filter library: TPT SVF, analog-matched biquads, Butterworth
//! cascades, Linkwitz-Riley crossovers and the simple filters.
//!
//! Magnitudes are measured from the impulse response through the public
//! `process_sample` (so the real f32 I/O path is what is tested) with a
//! single-frequency DFT, and compared with closed-form analog prototypes.
//! Every tolerance is stated at its assertion.

use openplugin_dsp::*;
use std::f64::consts::PI;

/// 44.1k, 48k, 96k, 192k and a low rate.
const RATES: [f32; 5] = [44_100.0, 48_000.0, 96_000.0, 192_000.0, 8_000.0];

/// Impulse response of `n` samples.
fn impulse(n: usize, mut f: impl FnMut(f32) -> f32) -> Vec<f64> {
    (0..n).map(|i| f64::from(f(if i == 0 { 1.0 } else { 0.0 }))).collect()
}

/// `|sum h[n] e^{-j w n}|` at `freq`, by complex rotation.
fn mag(h: &[f64], fs: f32, freq: f64) -> f64 {
    let w = 2.0 * PI * freq / f64::from(fs);
    let (rs, rc) = w.sin_cos();
    let (mut c, mut s) = (1.0f64, 0.0f64);
    let (mut re, mut im) = (0.0, 0.0);
    for v in h {
        re += v * c;
        im -= v * s;
        let nc = c * rc - s * rs;
        s = s * rc + c * rs;
        c = nc;
    }
    re.hypot(im)
}

fn db(m: f64) -> f64 {
    20.0 * m.max(1e-30).log10()
}

/// `|N(j w)| / |D(j w)|` for `[s^2, s, 1]` coefficient triples.
fn ratio(n: [f64; 3], d: [f64; 3], w: f64) -> f64 {
    let w2 = w * w;
    let (nr, ni) = (n[2] - n[0] * w2, n[1] * w);
    let (dr, di) = (d[2] - d[0] * w2, d[1] * w);
    nr.hypot(ni) / dr.hypot(di)
}

/// Analog prototype of the SVF/RBJ shapes: (numerator, denominator), s normalised to f0.
fn prototype(mode: SvfMode, q: f64, gain_db: f64) -> ([f64; 3], [f64; 3]) {
    let a = 10f64.powf(gain_db / 40.0);
    let sa = a.sqrt();
    let iq = 1.0 / q;
    match mode {
        SvfMode::LowPass => ([0.0, 0.0, 1.0], [1.0, iq, 1.0]),
        SvfMode::HighPass => ([1.0, 0.0, 0.0], [1.0, iq, 1.0]),
        SvfMode::BandPass => ([0.0, iq, 0.0], [1.0, iq, 1.0]),
        SvfMode::Notch => ([1.0, 0.0, 1.0], [1.0, iq, 1.0]),
        SvfMode::Peak => ([1.0, 0.0, -1.0], [1.0, iq, 1.0]),
        SvfMode::AllPass => ([1.0, -iq, 1.0], [1.0, iq, 1.0]),
        SvfMode::Bell => ([1.0, a * iq, 1.0], [1.0, iq / a, 1.0]),
        SvfMode::LowShelf => ([a, a * sa * iq, a * a], [a, sa * iq, 1.0]),
        SvfMode::HighShelf => ([a * a, a * sa * iq, a], [1.0, sa * iq, a]),
        _ => unimplemented!(),
    }
}

const SVF_MODES: [SvfMode; 9] = [
    SvfMode::LowPass,
    SvfMode::HighPass,
    SvfMode::BandPass,
    SvfMode::Notch,
    SvfMode::Peak,
    SvfMode::AllPass,
    SvfMode::Bell,
    SvfMode::LowShelf,
    SvfMode::HighShelf,
];

fn n_for(fs: f32) -> usize {
    ((0.6 * fs) as usize).max(8000)
}

struct Rng(u64);
impl Rng {
    /// Uniform noise in [-1, 1).
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32) / (1u64 << 23) as f32 - 1.0
    }
}

// ---------------------------------------------------------------- SVF

/// The SVF is the pre-warped bilinear transform: its response at digital
/// frequency f is the analog prototype at Omega = tan(pi f/fs) / tan(pi f0/fs).
/// Tolerance: 5e-4 relative to max(|H|, 1e-3), i.e. 0.004 dB near unity.
#[test]
fn svf_equals_prewarped_analog_prototype() {
    for fs in RATES {
        let fsd = f64::from(fs);
        for mode in SVF_MODES {
            for (q, g) in [(0.707, 9.0), (2.0, -9.0)] {
                let f0 = fsd / 48.0;
                let mut svf = Svf::new();
                svf.configure(mode, fs, f0 as f32, q as f32, g as f32);
                let h = impulse(n_for(fs), |x| svf.process_sample(x));
                let (n, d) = prototype(mode, q, g);
                let g0 = (PI * f0 / fsd).tan();
                for frac in [0.003, 0.01, 0.03, 0.1, 0.2, 0.3, 0.4, 0.47] {
                    let f = fsd * frac;
                    let expect = ratio(n, d, (PI * f / fsd).tan() / g0);
                    let got = mag(&h, fs, f);
                    let err = (got - expect).abs() / expect.max(1e-3);
                    assert!(
                        err < 5e-4,
                        "{mode:?} fs {fs} q {q} g {g} f {f:.1}: got {got} expect {expect} err {err}"
                    );
                }
            }
        }
    }
}

/// Against the raw (un-warped) analog prototype the SVF only cramps: with
/// f0 = fs/48 it stays within 0.5 dB up to fs/16 wherever the prototype is
/// above -40 dB.
#[test]
fn svf_tracks_analog_prototype_below_fs_over_16() {
    for fs in RATES {
        let fsd = f64::from(fs);
        for mode in SVF_MODES {
            let (q, g) = (1.0, 6.0);
            let f0 = fsd / 48.0;
            let mut svf = Svf::new();
            svf.configure(mode, fs, f0 as f32, q as f32, g as f32);
            let h = impulse(n_for(fs), |x| svf.process_sample(x));
            let (n, d) = prototype(mode, q, g);
            for frac in [0.3, 0.7, 1.4, 2.0, 3.0] {
                let f = f0 * frac * 0.999;
                if f > fsd / 16.0 {
                    continue;
                }
                let expect = ratio(n, d, f / f0);
                if expect < 0.01 {
                    continue;
                }
                let err = (db(mag(&h, fs, f)) - db(expect)).abs();
                assert!(err < 0.5, "{mode:?} fs {fs} f {f}: {err} dB");
            }
        }
    }
}

#[test]
fn svf_split_outputs_sum_to_input() {
    let mut svf = Svf::new();
    svf.configure(SvfMode::LowPass, 48_000.0, 700.0, 3.0, 0.0);
    let mut rng = Rng(1);
    for _ in 0..5000 {
        let x = rng.next();
        let (l, b, h) = svf.process_split(x);
        assert!((l + b + h - x).abs() < 1e-5);
    }
}

// ------------------------------------------------------------ matched

fn matched_prototype(kind: MatchedKind, q: f64, gain_db: f64) -> ([f64; 3], [f64; 3]) {
    let mode = match kind {
        MatchedKind::LowPass => SvfMode::LowPass,
        MatchedKind::HighPass => SvfMode::HighPass,
        MatchedKind::BandPass => SvfMode::BandPass,
        MatchedKind::Notch => SvfMode::Notch,
        MatchedKind::Peaking => SvfMode::Bell,
        MatchedKind::LowShelf => SvfMode::LowShelf,
        MatchedKind::HighShelf => SvfMode::HighShelf,
        _ => unimplemented!(),
    };
    prototype(mode, q, gain_db)
}

fn rbj(kind: MatchedKind) -> FilterType {
    match kind {
        MatchedKind::LowPass => FilterType::LowPass,
        MatchedKind::HighPass => FilterType::HighPass,
        MatchedKind::BandPass => FilterType::BandPass,
        MatchedKind::Notch => FilterType::Notch,
        MatchedKind::Peaking => FilterType::Peaking,
        MatchedKind::LowShelf => FilterType::LowShelf,
        MatchedKind::HighShelf => FilterType::HighShelf,
        _ => unimplemented!(),
    }
}

/// Worst dB error against the analog prototype over 0.5 % .. 49 % of fs,
/// counting only points where the prototype is above -12 dB. Returns
/// (matched, rbj).
fn matched_vs_rbj(kind: MatchedKind, fs: f32, f0r: f64, q: f64, g: f64) -> (f64, f64) {
    let fsd = f64::from(fs);
    let f0 = fsd * f0r;
    let mut m = MatchedBiquad::new();
    m.configure(kind, fs, f0 as f32, q as f32, g as f32);
    let mut b = Biquad::new();
    b.configure(rbj(kind), fs, f0 as f32, q as f32, g as f32);
    let hb = impulse(n_for(fs), |x| b.process_sample(x));
    let (n, d) = matched_prototype(kind, q, g);
    let (mut em, mut eb) = (0.0f64, 0.0f64);
    for i in 1..=98 {
        let f = fsd * 0.005 * f64::from(i);
        let expect = ratio(n, d, f / f0);
        if expect < 0.25 {
            continue;
        }
        em = em.max((db(m.magnitude_at(fs, f as f32)) - db(expect)).abs());
        eb = eb.max((db(mag(&hb, fs, f)) - db(expect)).abs());
    }
    (em, eb)
}

/// `magnitude_at` (from the coefficients) is what the processing does.
#[test]
fn matched_magnitude_at_agrees_with_processing() {
    for fs in RATES {
        for kind in [MatchedKind::LowPass, MatchedKind::Peaking, MatchedKind::HighShelf] {
            let fsd = f64::from(fs);
            let mut m = MatchedBiquad::new();
            m.configure(kind, fs, (fsd * 0.2) as f32, 1.5, 8.0);
            let h = impulse(n_for(fs), |x| m.process_sample(x));
            for frac in [0.01, 0.1, 0.2, 0.3, 0.45] {
                let f = fsd * frac;
                let a = m.magnitude_at(fs, f as f32);
                let b = mag(&h, fs, f);
                assert!((a - b).abs() / a.max(1e-3) < 5e-4, "{kind:?} {fs} {f}: {a} vs {b}");
            }
        }
    }
}

/// Peaking EQ: matched error against the analog bell is below 5 dB and below
/// 0.75x the RBJ error for every tried centre up to 0.4 fs (the RBJ error there
/// reaches 11 dB). Results depend only on f0/fs, so the rate loop checks that.
#[test]
fn matched_peaking_beats_rbj_near_nyquist() {
    for fs in [44_100.0f32, 96_000.0, 8_000.0] {
        for f0r in [0.02, 0.1, 0.25, 0.4] {
            for (q, g) in [(0.5, 12.0), (0.707, -12.0), (2.0, 12.0), (8.0, -6.0)] {
                let (m, r) = matched_vs_rbj(MatchedKind::Peaking, fs, f0r, q, g);
                assert!(m < 5.0, "fs {fs} f0 {f0r} q {q} g {g}: matched {m} dB");
                assert!(m <= 0.75 * r || m < 0.2, "fs {fs} f0 {f0r} q {q} g {g}: matched {m} rbj {r}");
            }
        }
    }
}

/// Low-pass: within 1.3 dB of the analog response (where it is above -12 dB)
/// for f0 up to 0.4 fs and Q 0.5..8. The RBJ low-pass has a zero at Nyquist
/// where the analog filter at f0 = fs/4 is still at -12 dB.
#[test]
fn matched_lowpass_has_no_cramping() {
    for fs in RATES {
        for f0r in [0.02, 0.1, 0.25, 0.4] {
            for q in [0.5, 0.707, 2.0, 8.0] {
                let (m, _) = matched_vs_rbj(MatchedKind::LowPass, fs, f0r, q, 0.0);
                assert!(m < 1.3, "fs {fs} f0 {f0r} q {q}: {m} dB");
            }
        }
    }
    let fsd = 48_000.0f64;
    let mut m = MatchedBiquad::new();
    m.configure(MatchedKind::LowPass, 48_000.0, (fsd * 0.25) as f32, 0.707, 0.0);
    let analog = 1.0 / (1.0 + 2.0f64.powi(4)).sqrt(); // Butterworth 2 at Nyquist = 2 f0
    let ny = m.magnitude_at(48_000.0, 23_900.0);
    assert!((db(ny) - db(analog)).abs() < 0.6, "matched at Nyquist: {} dB vs {} dB", db(ny), db(analog));
    let mut b = Biquad::new();
    b.configure(FilterType::LowPass, 48_000.0, (fsd * 0.25) as f32, 0.707, 0.0);
    let hb = impulse(8000, |x| b.process_sample(x));
    assert!(db(mag(&hb, 48_000.0, 23_990.0)) < -40.0, "RBJ drops to its Nyquist zero");
}

/// High-pass with Q <= 0.707: within 2.2 dB wherever the analog response is
/// above -12 dB, f0 up to 0.4 fs. (For Q >= 2 the stopband skirt is shallower
/// than the analog filter's and the error approaches RBJ's; not asserted.)
#[test]
fn matched_highpass_moderate_q() {
    for fs in RATES {
        for f0r in [0.02, 0.1, 0.25, 0.4] {
            for q in [0.5, 0.707] {
                let (m, r) = matched_vs_rbj(MatchedKind::HighPass, fs, f0r, q, 0.0);
                assert!(m < 2.2, "fs {fs} f0 {f0r} q {q}: {m} dB");
                if f0r >= 0.25 {
                    assert!(m < 0.5 * r, "fs {fs} f0 {f0r} q {q}: matched {m} rbj {r}");
                }
            }
        }
    }
}

/// Shelves with Q <= 0.707 and f0 <= 0.25 fs: within 1.7 dB of the analog
/// shelf and never worse than RBJ.
#[test]
fn matched_shelves() {
    for kind in [MatchedKind::LowShelf, MatchedKind::HighShelf] {
        for fs in [44_100.0f32, 192_000.0, 8_000.0] {
            for f0r in [0.02, 0.1, 0.25] {
                for q in [0.5, 0.707] {
                    for g in [12.0, -12.0] {
                        let (m, r) = matched_vs_rbj(kind, fs, f0r, q, g);
                        assert!(m < 1.7, "{kind:?} fs {fs} f0 {f0r} q {q} g {g}: {m} dB");
                        assert!(m <= r + 0.05, "{kind:?} fs {fs} f0 {f0r} q {q} g {g}: matched {m} rbj {r}");
                    }
                }
            }
        }
    }
}

/// Band-pass: exactly 0 dB at f0 (0.01 dB), zero at DC, and within 2 dB of the
/// analog band-pass (above -12 dB) for Q >= 2, f0 <= 0.1 fs. Like every
/// `[b0, 0, -b0]` band-pass it is zero at Nyquist, so it cannot follow a
/// low-Q analog band-pass that is still at -6 dB there. The notch has its null
/// at f0 and is within 1.2 dB of analog for f0 <= 0.1 fs.
#[test]
fn matched_bandpass_and_notch() {
    for fs in RATES {
        let fsd = f64::from(fs);
        for f0r in [0.02, 0.1, 0.25, 0.4] {
            for q in [0.5, 2.0, 8.0] {
                let f0 = fsd * f0r;
                let mut m = MatchedBiquad::new();
                m.configure(MatchedKind::BandPass, fs, f0 as f32, q as f32, 0.0);
                assert!((db(m.magnitude_at(fs, f0 as f32))).abs() < 0.01, "bp peak {fs} {f0r} {q}");
                assert!(m.magnitude_at(fs, 0.01) < 1e-3);
                let mut n = MatchedBiquad::new();
                n.configure(MatchedKind::Notch, fs, f0 as f32, q as f32, 0.0);
                assert!(db(n.magnitude_at(fs, f0 as f32)) < -80.0, "notch null {fs} {f0r} {q}");
            }
        }
        for f0r in [0.02, 0.1] {
            for q in [2.0, 8.0] {
                let (m, _) = matched_vs_rbj(MatchedKind::BandPass, fs, f0r, q, 0.0);
                assert!(m < 2.0, "bp fs {fs} f0 {f0r} q {q}: {m} dB");
            }
            for q in [0.5, 2.0, 8.0] {
                let (m, _) = matched_vs_rbj(MatchedKind::Notch, fs, f0r, q, 0.0);
                assert!(m < 1.2, "notch fs {fs} f0 {f0r} q {q}: {m} dB");
            }
        }
    }
}

// ---------------------------------------------------------- simple filters

#[test]
fn one_pole_and_dc_blocker_match_prewarped_first_order() {
    for fs in RATES {
        let fsd = f64::from(fs);
        let f0 = fsd / 40.0;
        let g0 = (PI * f0 / fsd).tan();
        let mut lp = OnePoleFilter::new();
        lp.set(OnePoleMode::LowPass, fs, f0 as f32);
        let mut hp = OnePoleFilter::new();
        hp.set(OnePoleMode::HighPass, fs, f0 as f32);
        let mut dc = DcBlocker::new(fs, f0 as f32);
        let hl = impulse(n_for(fs), |x| lp.process_sample(x));
        let hh = impulse(n_for(fs), |x| hp.process_sample(x));
        let hd = impulse(n_for(fs), |x| dc.process_sample(x));
        for frac in [0.003, 0.02, 0.05, 0.2, 0.4, 0.48] {
            let f = fsd * frac;
            let w = (PI * f / fsd).tan() / g0;
            let el = 1.0 / (1.0 + w * w).sqrt();
            let eh = w / (1.0 + w * w).sqrt();
            for (got, exp) in [(mag(&hl, fs, f), el), (mag(&hh, fs, f), eh), (mag(&hd, fs, f), eh)] {
                assert!((got - exp).abs() / exp.max(1e-3) < 5e-4, "fs {fs} f {f}: {got} vs {exp}");
            }
        }
        assert!((db(mag(&hl, fs, f0)) + 3.0103).abs() < 0.01, "-3 dB corner");
    }
}

#[test]
fn dc_blocker_removes_dc() {
    let mut dc = DcBlocker::new(48_000.0, 20.0);
    let mut y = 0.0;
    for _ in 0..200_000 {
        y = dc.process_sample(0.7);
    }
    assert!(y.abs() < 1e-6, "dc left {y}");
}

#[test]
fn allpass_filters_have_unit_gain() {
    for fs in RATES {
        let fsd = f64::from(fs);
        let mut a1 = Allpass1::new();
        a1.set(fs, (fsd / 30.0) as f32);
        let mut a2 = Allpass2::new();
        a2.set(fs, (fsd / 30.0) as f32, 3.0);
        let h1 = impulse(n_for(fs), |x| a1.process_sample(x));
        let h2 = impulse(n_for(fs), |x| a2.process_sample(x));
        for frac in [0.002, 0.01, 0.03, 0.1, 0.3, 0.45] {
            for h in [&h1, &h2] {
                let m = mag(h, fs, fsd * frac);
                assert!((m - 1.0).abs() < 1e-3, "fs {fs} f {frac}: {m}");
            }
        }
    }
}

/// Butterworth order 1..=8, LP and HP: equals |H| = 1/sqrt(1 + W^(2N)) with
/// W = tan(pi f/fs)/tan(pi fc/fs) within 2e-3 relative to max(|H|, 1e-3), and
/// exactly -3 dB (0.02 dB) at the cutoff.
#[test]
fn butterworth_orders_match_prototype() {
    for fs in RATES {
        let fsd = f64::from(fs);
        let fc = fsd / 20.0;
        let g0 = (PI * fc / fsd).tan();
        for order in 1..=8usize {
            for kind in [ButterworthKind::LowPass, ButterworthKind::HighPass] {
                let mut b = Butterworth::new();
                b.set(kind, order, fs, fc as f32);
                assert_eq!(b.order(), order);
                let h = impulse(n_for(fs), |x| b.process_sample(x));
                for frac in [0.004, 0.02, 0.04, 0.05, 0.06, 0.1, 0.25, 0.45] {
                    let f = fsd * frac;
                    let w = (PI * f / fsd).tan() / g0;
                    let lp = 1.0 / (1.0 + w.powi(2 * order as i32)).sqrt();
                    let exp = if kind == ButterworthKind::LowPass { lp } else { (1.0 - lp * lp).sqrt() };
                    let got = mag(&h, fs, f);
                    assert!(
                        (got - exp).abs() / exp.max(1e-3) < 2e-3,
                        "{kind:?} order {order} fs {fs} f {f}: {got} vs {exp}"
                    );
                }
                assert!((db(mag(&h, fs, fc)) + 3.0103).abs() < 0.02, "{kind:?} {order} -3 dB");
            }
        }
    }
    let mut b = Butterworth::new();
    b.set(ButterworthKind::LowPass, 99, 48_000.0, 1000.0);
    assert_eq!(b.order(), 8);
    b.set(ButterworthKind::LowPass, 0, 48_000.0, 1000.0);
    assert_eq!(b.order(), 1);
}

// ---------------------------------------------------------- crossovers

fn log_freqs(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    (0..n).map(|i| lo * (hi / lo).powf(i as f64 / (n - 1) as f64)).collect()
}

/// Band sum is flat within 0.01 dB from 20 Hz to 20 kHz for LR2/LR4/LR8, 2/3/4
/// bands, at 44.1k, 48k, 96k and 192k. For two bands each band is -6 dB at the
/// crossover.
#[test]
fn crossover_band_sum_is_flat() {
    let freq_sets: [[f32; 3]; 2] = [[120.0, 1000.0, 8000.0], [60.0, 500.0, 6000.0]];
    for fs in [44_100.0f32, 48_000.0, 96_000.0, 192_000.0] {
        let n = (fs * 1.2) as usize;
        let freqs = log_freqs(20.0, 20_000.0, 48);
        for order in [LrOrder::Lr2, LrOrder::Lr4, LrOrder::Lr8] {
            for bands in 2..=4usize {
                for set in &freq_sets {
                    let mut xo = Crossover::new(order, bands, fs, &set[..bands - 1]);
                    assert_eq!(xo.bands(), bands);
                    let mut per_band = vec![vec![0.0f64; n]; bands];
                    let mut sum = vec![0.0f64; n];
                    for i in 0..n {
                        let out = xo.process_sample(if i == 0 { 1.0 } else { 0.0 });
                        for b in 0..bands {
                            per_band[b][i] = f64::from(out[b]);
                            sum[i] += f64::from(out[b]);
                        }
                        for v in &out[bands..MAX_BANDS] {
                            assert_eq!(*v, 0.0);
                        }
                    }
                    for &f in &freqs {
                        let m = db(mag(&sum, fs, f));
                        assert!(m.abs() < 0.01, "{order:?} {bands} bands fs {fs} f {f:.1}: {m} dB");
                    }
                    if bands == 2 {
                        let lo = db(mag(&per_band[0], fs, f64::from(set[0])));
                        assert!((lo + 6.0206).abs() < 0.05, "{order:?} low band at crossover: {lo} dB");
                    }
                }
            }
        }
    }
}

/// The bands separate the spectrum (so the flat sum is not a trivial pass-through).
#[test]
fn crossover_bands_partition_the_spectrum() {
    let fs = 48_000.0f32;
    let mut xo = Crossover::new(LrOrder::Lr4, 3, fs, &[200.0, 4000.0]);
    let n = 48_000;
    let mut bands = vec![vec![0.0f64; n]; 3];
    for i in 0..n {
        let o = xo.process_sample(if i == 0 { 1.0 } else { 0.0 });
        for (band, v) in bands.iter_mut().zip(o) {
            band[i] = f64::from(v);
        }
    }
    assert!(db(mag(&bands[0], fs, 40.0)) > -0.5);
    assert!(db(mag(&bands[0], fs, 2000.0)) < -40.0);
    assert!(db(mag(&bands[1], fs, 1000.0)) > -0.5);
    assert!(db(mag(&bands[1], fs, 20.0)) < -30.0);
    assert!(db(mag(&bands[1], fs, 15000.0)) < -40.0);
    assert!(db(mag(&bands[2], fs, 15000.0)) > -0.5);
    assert!(db(mag(&bands[2], fs, 400.0)) < -30.0);
    assert!((db(mag(&bands[1], fs, 200.0)) + 6.02).abs() < 0.5);
}

#[test]
fn crossover_frequencies_are_forced_ascending_and_safe() {
    let mut xo = Crossover::new(LrOrder::Lr4, 4, 48_000.0, &[5000.0, 100.0, f32::NAN]);
    let mut rng = Rng(7);
    for _ in 0..20_000 {
        let o = xo.process_sample(rng.next());
        assert!(o.iter().all(|v| v.is_finite()));
    }
    assert_eq!(Crossover::new(LrOrder::Lr8, 99, 48_000.0, &[]).bands(), 4);
    assert_eq!(Crossover::new(LrOrder::Lr2, 0, 48_000.0, &[]).bands(), 2);
}

// -------------------------------------------------- robustness

/// Modulation stress: for 6 seconds at 48 kHz, white noise in, cutoff, Q and
/// gain changed on every sample, both by fast LFOs and by random jumps over the
/// whole range (20 Hz..20 kHz, Q 0.5..20, +-18 dB). Output must stay finite.
/// Bounds: SVF and crossover/Butterworth cascades (unit-amplitude noise,
/// resonance up to 20) stay below 100; the matched biquad only has to stay
/// finite and within its documented +-1e6 runaway guard.
#[test]
fn modulation_stress_is_bounded() {
    let fs = 48_000.0f32;
    let n = 6 * 48_000;
    let kinds = [
        MatchedKind::LowPass,
        MatchedKind::HighPass,
        MatchedKind::BandPass,
        MatchedKind::Notch,
        MatchedKind::Peaking,
        MatchedKind::LowShelf,
        MatchedKind::HighShelf,
    ];
    for random_jumps in [false, true] {
        let mut rng = Rng(0x1234_5678_9abc_def1);
        let mut svf: Vec<Svf> = SVF_MODES.iter().map(|_| Svf::new()).collect();
        let mut matched: Vec<MatchedBiquad> = kinds.iter().map(|_| MatchedBiquad::new()).collect();
        let mut butter = Butterworth::new();
        let mut xo = Crossover::new(LrOrder::Lr8, 4, fs, &[100.0, 1000.0, 8000.0]);
        let (mut peak_svf, mut peak_matched, mut peak_other) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..n {
            let t = i as f32 / fs;
            let (f, q, g) = if random_jumps {
                (
                    20.0 * 1000.0f32.powf(0.5 * (rng.next() + 1.0)),
                    0.5 + 19.5 * 0.5 * (rng.next() + 1.0),
                    18.0 * rng.next(),
                )
            } else {
                (
                    20.0 * 1000.0f32.powf(0.5 + 0.5 * (std::f32::consts::TAU * 37.0 * t).sin()),
                    0.5 + 19.5 * (0.5 + 0.5 * (std::f32::consts::TAU * 11.0 * t).cos()),
                    18.0 * (std::f32::consts::TAU * 5.0 * t).sin(),
                )
            };
            let x = rng.next();
            for (s, mode) in svf.iter_mut().zip(SVF_MODES) {
                s.configure(mode, fs, f, q, g);
                let y = s.process_sample(x);
                assert!(y.is_finite(), "svf {mode:?} not finite at {i}");
                peak_svf = peak_svf.max(y.abs());
            }
            for (m, kind) in matched.iter_mut().zip(kinds) {
                m.configure(kind, fs, f, q, g);
                let y = m.process_sample(x);
                assert!(y.is_finite(), "matched {kind:?} not finite at {i}");
                peak_matched = peak_matched.max(y.abs());
            }
            butter.set(ButterworthKind::LowPass, 1 + i % 8, fs, f);
            let y = butter.process_sample(x);
            assert!(y.is_finite());
            peak_other = peak_other.max(y.abs());
            xo.set_frequencies(fs, &[f * 0.05, f, (f * 4.0).min(20_000.0)]);
            for v in xo.process_sample(x) {
                assert!(v.is_finite());
                peak_other = peak_other.max(v.abs());
            }
        }
        println!("jumps {random_jumps}: svf {peak_svf} matched {peak_matched} other {peak_other}");
        assert!(peak_svf < 100.0, "svf peak {peak_svf}");
        assert!(peak_other < 100.0, "cascade peak {peak_other}");
        assert!(peak_matched <= 1.0e6, "matched peak {peak_matched}");
    }
}

/// Non-finite samples are replaced by 0.0, so the filter must behave exactly
/// like a twin fed zeros there, and never output a non-finite value.
fn run_with_poison(mut f: impl FnMut(f32) -> f32, mut reference: impl FnMut(f32) -> f32) {
    let mut rng = Rng(99);
    for i in 0..30_000 {
        let clean = rng.next();
        let bad = match i % 4001 {
            10 => f32::NAN,
            11 => f32::INFINITY,
            12 => f32::NEG_INFINITY,
            _ => clean,
        };
        let ref_in = if bad.is_finite() { bad } else { 0.0 };
        let y = f(bad);
        let r = reference(ref_in);
        assert!(y.is_finite(), "non-finite output at {i}");
        assert_eq!(y, r, "state poisoned at {i}");
    }
}

/// NaN/inf policy: a non-finite input sample is treated as 0.0 and never
/// reaches the state; the filter then continues exactly as if it had been fed 0.
#[test]
fn non_finite_input_does_not_poison_state() {
    let mk_svf = || {
        let mut s = Svf::new();
        s.configure(SvfMode::Bell, 48_000.0, 900.0, 4.0, 9.0);
        s
    };
    let (mut a, mut b) = (mk_svf(), mk_svf());
    run_with_poison(|x| a.process_sample(x), |x| b.process_sample(x));

    let mk_m = || {
        let mut s = MatchedBiquad::new();
        s.configure(MatchedKind::LowShelf, 48_000.0, 900.0, 0.7, 9.0);
        s
    };
    let (mut a, mut b) = (mk_m(), mk_m());
    run_with_poison(|x| a.process_sample(x), |x| b.process_sample(x));

    let mk_b = || {
        let mut s = Butterworth::new();
        s.set(ButterworthKind::HighPass, 7, 48_000.0, 300.0);
        s
    };
    let (mut a, mut b) = (mk_b(), mk_b());
    run_with_poison(|x| a.process_sample(x), |x| b.process_sample(x));

    let mut a = DcBlocker::new(48_000.0, 20.0);
    let mut b = DcBlocker::new(48_000.0, 20.0);
    run_with_poison(|x| a.process_sample(x), |x| b.process_sample(x));

    let mk_x = || Crossover::new(LrOrder::Lr4, 3, 48_000.0, &[300.0, 3000.0]);
    let (mut a, mut b) = (mk_x(), mk_x());
    run_with_poison(|x| a.process_sample(x).iter().sum(), |x| b.process_sample(x).iter().sum());
}

/// Enormous finite input (f32::MAX alternating sign) can overflow the f32
/// output range of a resonant filter: the filter resets itself instead of
/// returning infinity, and works normally afterwards.
#[test]
fn huge_input_resets_instead_of_overflowing() {
    let mut s = Svf::new();
    s.configure(SvfMode::LowPass, 48_000.0, 1000.0, 1000.0, 0.0);
    for i in 0..5000 {
        let y = s.process_sample(if i % 2 == 0 { f32::MAX } else { -f32::MAX });
        assert!(y.is_finite());
    }
    s.configure(SvfMode::LowPass, 48_000.0, 1000.0, 0.707, 0.0);
    let mut last = 0.0;
    for _ in 0..20_000 {
        last = s.process_sample(1.0);
    }
    assert!((last - 1.0).abs() < 1e-3, "recovered to {last}");
}

/// Bad parameters never panic or poison: NaN/inf are ignored (previous
/// coefficients kept), out-of-range values are clamped, odd sample rates go
/// through `effective_rate`.
#[test]
fn bad_parameters_are_harmless() {
    let weird = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -5.0, 1.0e-9, 1.0e9, f32::MAX, f32::MIN];
    for &fs in &weird {
        for &f in &weird {
            for &q in &weird {
                let mut s = Svf::new();
                let mut m = MatchedBiquad::new();
                let mut b = Butterworth::new();
                let mut xo = Crossover::new(LrOrder::Lr4, 3, fs, &[f, f]);
                let mut a1 = Allpass1::new();
                let mut dc = DcBlocker::new(fs, f);
                for mode in SVF_MODES {
                    s.configure(mode, fs, f, q, q);
                    m.configure(MatchedKind::Peaking, fs, f, q, q);
                }
                b.set(ButterworthKind::LowPass, 5, fs, f);
                a1.set(fs, f);
                xo.set_frequencies(fs, &[f, q]);
                dc.set(fs, f);
                for i in 0..50 {
                    let x = (i as f32 * 0.3).sin();
                    let outs = [
                        s.process_sample(x),
                        m.process_sample(x),
                        b.process_sample(x),
                        xo.process_sample(x).iter().sum(),
                        a1.process_sample(x),
                        dc.process_sample(x),
                    ];
                    assert!(outs.iter().all(|v| v.is_finite()), "fs {fs} f {f} q {q}: {outs:?}");
                }
            }
        }
    }
}

/// Denormal safety: after an impulse the tails decay to exactly zero and no
/// output is ever subnormal.
#[test]
fn tails_flush_to_zero_without_denormals() {
    fn check(name: &str, mut f: impl FnMut(f32) -> f32, n: usize) {
        let mut last_nonzero = 0;
        for i in 0..n {
            let y = f(if i == 0 { 1.0 } else { 0.0 });
            assert!(y == 0.0 || y.is_normal(), "{name}: subnormal {y:e} at {i}");
            if y != 0.0 {
                last_nonzero = i;
            }
        }
        assert!(last_nonzero + 1000 < n, "{name}: tail still ringing at {last_nonzero}");
    }
    let n = 1_000_000;
    let mut s = Svf::new();
    s.configure(SvfMode::LowPass, 48_000.0, 1000.0, 0.707, 0.0);
    check("svf", |x| s.process_sample(x), n);
    let mut m = MatchedBiquad::new();
    m.configure(MatchedKind::LowPass, 48_000.0, 1000.0, 0.707, 0.0);
    check("matched", |x| m.process_sample(x), n);
    let mut b = Butterworth::new();
    b.set(ButterworthKind::LowPass, 8, 48_000.0, 1000.0);
    check("butterworth", |x| b.process_sample(x), n);
    let mut dc = DcBlocker::new(48_000.0, 20.0);
    check("dc", |x| dc.process_sample(x), n);
    let mut xo = Crossover::new(LrOrder::Lr8, 4, 48_000.0, &[100.0, 1000.0, 8000.0]);
    check("crossover", |x| xo.process_sample(x).iter().sum(), n);
}

/// Every rate the bridges accept (down to 1 kHz) gives finite, sensible output.
#[test]
fn works_at_low_and_high_rates() {
    for fs in [1_000.0f32, 1_234.5, 4_000.0, 768_000.0, f32::NAN] {
        let mut s = Svf::new();
        s.configure(SvfMode::LowPass, fs, 100.0, 1.0, 0.0);
        let mut b = Butterworth::new();
        b.set(ButterworthKind::HighPass, 4, fs, 100.0);
        let mut xo = Crossover::new(LrOrder::Lr4, 2, fs, &[100.0]);
        let mut last = (0.0, 0.0);
        for _ in 0..100_000 {
            last = (s.process_sample(1.0), xo.process_sample(1.0).iter().sum::<f32>());
            assert!(b.process_sample(1.0).is_finite());
        }
        assert!((last.0 - 1.0).abs() < 1e-3, "dc through low-pass at {fs}");
        assert!((last.1 - 1.0).abs() < 1e-2, "dc through crossover at {fs}: {}", last.1);
    }
}
