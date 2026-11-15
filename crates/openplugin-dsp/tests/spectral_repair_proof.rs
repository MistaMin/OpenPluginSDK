//! Proof for `SpectralRepair`: SNR improvement on tone, harmonic and noise
//! mixtures, signal distortion, a musical-noise measure, identity, profile
//! convergence, state bytes and hostile input.

use openplugin_dsp::{
    Complex, GainRule, Identity, PinkNoise, ProfileError, RepairConfig, RepairMode, RepairParams,
    SpectralRepair, Stft, StftConfig, WhiteNoise,
};

const SR: f32 = 48_000.0;
const FRAME: usize = 2048;
const HOP: usize = 512;

fn db(x: f64) -> f64 {
    10.0 * x.max(1e-30).log10()
}

fn power(x: &[f32]) -> f64 {
    x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64
}

/// White noise with standard deviation `sigma`.
fn white(n: usize, seed: u64, sigma: f32) -> Vec<f32> {
    let mut g = WhiteNoise::new(seed);
    (0..n).map(|_| g.next() * sigma * 3f32.sqrt()).collect()
}

fn pink(n: usize, seed: u64, sigma: f32) -> Vec<f32> {
    let mut g = PinkNoise::new(SR, seed);
    // Let the filter settle, then scale to the requested RMS.
    for _ in 0..8192 {
        g.next();
    }
    (0..n).map(|_| g.next() * sigma / openplugin_dsp::PINK_RMS).collect()
}

fn tone(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.3 * (2.0 * std::f64::consts::PI * 1_000.37 * i as f64 / SR as f64).sin() as f32)
        .collect()
}

fn harmonic(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f64 / SR as f64;
            (1..=8)
                .map(|k| (2.0 * std::f64::consts::PI * 220.0 * k as f64 * t).sin() / k as f64)
                .sum::<f64>() as f32
                * 0.25
        })
        .collect()
}

fn add(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

fn run(r: &mut SpectralRepair, x: &[f32]) -> Vec<f32> {
    let mut y = vec![0.0; x.len()];
    for (xi, yi) in x.chunks(256).zip(y.chunks_mut(256)) {
        r.process(0, xi, yi);
    }
    y
}

fn learned(noise_kind: &str, sigma: f32, p: RepairParams) -> SpectralRepair {
    let mut r = SpectralRepair::new(RepairConfig::new(SR, 1, FRAME)).unwrap();
    r.set_params(p);
    let n = if noise_kind == "pink" { pink(96_000, 11, sigma) } else { white(96_000, 11, sigma) };
    r.start_learning();
    run(&mut r, &n);
    assert!(r.stop_learning());
    r
}

struct Measure {
    snr_in: f64,
    snr_out: f64,
    noise_red: f64,
    sdr: f64,
}

/// Total-error SNR of the mixture, noise reduction on the noise alone, and the
/// clean component's own distortion (clean alone through the same settings).
fn measure(kind: &str, clean: &[f32], p: RepairParams) -> Measure {
    let sigma = 0.1;
    let n = 192_000;
    let skip = 48_000;
    let noise = if kind == "pink" { pink(n, 77, sigma) } else { white(n, 77, sigma) };
    let mix = add(clean, &noise);
    let lat = FRAME;
    let seg = |v: &[f32], d: usize| -> Vec<f32> { v[skip + d..n - lat + d].to_vec() };
    let mut r = learned(kind, sigma, p);
    let y = run(&mut r, &mix);
    let c = seg(clean, 0);
    let yy = seg(&y, lat);
    let err: Vec<f32> = yy.iter().zip(&c).map(|(a, b)| a - b).collect();
    let nn = seg(&noise, 0);
    let snr_in = db(power(&c) / power(&nn));
    let snr_out = db(power(&c) / power(&err));
    r.reset();
    let yn = run(&mut r, &noise);
    let noise_red = db(power(&nn) / power(&seg(&yn, lat)));
    r.reset();
    let yc = run(&mut r, clean);
    let dist: Vec<f32> = seg(&yc, lat).iter().zip(&c).map(|(a, b)| a - b).collect();
    let sdr = db(power(&c) / power(&dist));
    Measure { snr_in, snr_out, noise_red, sdr }
}

fn settings() -> Vec<(&'static str, RepairParams)> {
    let d = RepairParams::default();
    vec![
        ("SS a=2 b=.02 30dB", RepairParams { amount_db: 30.0, ..d }),
        ("SS a=1 b=.05 12dB", RepairParams { amount_db: 12.0, over_subtraction: 1.0, spectral_floor: 0.05, ..d }),
        ("SS a=3 b=0 40dB", RepairParams { amount_db: 40.0, over_subtraction: 3.0, spectral_floor: 0.0, ..d }),
        ("Wiener a=1.5", RepairParams { amount_db: 30.0, rule: GainRule::Wiener, over_subtraction: 1.5, ..d }),
        ("Wiener DD", RepairParams { amount_db: 30.0, rule: GainRule::Wiener, decision_directed: true, ..d }),
        ("MMSE-STSA DD", RepairParams { amount_db: 30.0, rule: GainRule::MmseStsa, decision_directed: true, over_subtraction: 1.0, ..d }),
        ("Gate 6dB 30dB", RepairParams { amount_db: 30.0, mode: RepairMode::Gate, threshold_db: 6.0, ..d }),
    ]
}

#[test]
fn snr_improvement_table() {
    let n = 192_000;
    let cases: Vec<(&str, &str, Vec<f32>)> = vec![
        ("tone+white", "white", tone(n)),
        ("tone+pink", "pink", tone(n)),
        ("harmonic+white", "white", harmonic(n)),
    ];
    for (name, kind, clean) in &cases {
        let mut gains: Vec<(&str, f64)> = Vec::new();
        for (label, p) in settings() {
            let m = measure(kind, clean, p);
            println!(
                "{name:15} {label:20} in {:6.2} dB  out {:6.2} dB  gain {:+6.2} dB  noise -{:5.1} dB  clean SDR {:6.1} dB",
                m.snr_in, m.snr_out, m.snr_out - m.snr_in, m.noise_red, m.sdr
            );
            assert!(m.snr_out - m.snr_in > 3.0, "{name} {label}");
            assert!(m.sdr > 10.0, "{name} {label}: clean distortion");
            gains.push((label, m.snr_out - m.snr_in));
        }
        // More over-subtraction (settings 1 -> 0 -> 2) buys more SNR.
        assert!(gains[1].1 < gains[0].1 && gains[0].1 < gains[2].1, "{name}: {gains:?}");
        if name.starts_with("tone") {
            assert!(gains[6].1 > 10.0, "{name}: gate {gains:?}");
        }
    }
}

#[test]
fn identity_when_disabled_or_zero_amount() {
    let x = add(&tone(30_000), &white(30_000, 3, 0.1));
    let mut reference = Stft::new(1, SpectralRepair::stft_config(FRAME, HOP)).unwrap();
    let mut want = vec![0.0; x.len()];
    reference.process(0, &x, &mut want, &mut Identity);
    for (label, p) in [
        ("disabled", RepairParams { enabled: false, ..RepairParams::default() }),
        ("amount 0", RepairParams { amount_db: 0.0, ..RepairParams::default() }),
        ("amount 0 listen", RepairParams { amount_db: 0.0, listen_removed: true, ..RepairParams::default() }),
    ] {
        for mode in [RepairMode::Reduce, RepairMode::Gate] {
            let mut r = learned("white", 0.1, RepairParams { mode, ..p });
            r.reset();
            let y = run(&mut r, &x);
            assert!(y.iter().zip(&want).all(|(a, b)| a.to_bits() == b.to_bits()), "{label} {mode:?}");
        }
    }
    // No profile also passes through; and the delay is the latency.
    let mut r = SpectralRepair::new(RepairConfig::new(SR, 1, FRAME)).unwrap();
    let y = run(&mut r, &x);
    assert!(y.iter().zip(&want).all(|(a, b)| a.to_bits() == b.to_bits()));
    assert_eq!(r.latency(), FRAME);
    let mut worst = 0.0f32;
    for i in FRAME..x.len() {
        worst = worst.max((y[i] - x[i - FRAME]).abs());
    }
    assert!(worst < 1e-5, "{worst}");
}

#[test]
fn reduction_and_floor() {
    let sigma = 0.1;
    let noise = white(192_000, 5, sigma);
    let reduction = |p: RepairParams| {
        let mut r = learned("white", sigma, p);
        let y = run(&mut r, &noise);
        db(power(&noise[60_000..180_000]) / power(&y[60_000 + FRAME..180_000 + FRAME]))
    };
    // A gate set far above the noise closes every bin: the reduction is the
    // amount, not silence.
    for amount in [6.0f32, 12.0, 24.0, 40.0] {
        let red = reduction(RepairParams {
            mode: RepairMode::Gate,
            threshold_db: 25.0,
            amount_db: amount,
            freq_smooth_bins: 0,
            ..RepairParams::default()
        });
        println!("gate, amount {amount} dB: noise reduced by {red:.2} dB");
        assert!((red - amount as f64).abs() < 0.3, "{red} vs {amount}");
    }
    // Spectral subtraction never reduces more than the amount, and reduces more
    // when allowed more.
    let mut last = 0.0;
    for amount in [3.0f32, 6.0, 12.0, 24.0] {
        let red = reduction(RepairParams {
            amount_db: amount,
            over_subtraction: 3.0,
            spectral_floor: 0.0,
            ..RepairParams::default()
        });
        println!("spectral subtraction, amount {amount} dB: noise reduced by {red:.2} dB");
        assert!(red <= amount as f64 + 0.3 && red >= last, "{red} vs {amount}");
        last = red;
    }
}

#[test]
fn listen_removed_is_the_complement() {
    let x = add(&tone(60_000), &white(60_000, 9, 0.1));
    let p = RepairParams { amount_db: 30.0, ..RepairParams::default() };
    let mut a = learned("white", 0.1, p);
    let mut b = learned("white", 0.1, RepairParams { listen_removed: true, ..p });
    a.reset();
    b.reset();
    let ya = run(&mut a, &x);
    let yb = run(&mut b, &x);
    let mut reference = Stft::new(1, SpectralRepair::stft_config(FRAME, HOP)).unwrap();
    let mut id = vec![0.0; x.len()];
    reference.process(0, &x, &mut id, &mut Identity);
    let worst = (0..x.len()).map(|i| (ya[i] + yb[i] - id[i]).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1e-5, "{worst}");
    assert!(power(&yb[20_000..]) < power(&ya[20_000..]));
}

/// Time-frequency power statistics of `x` (Hann 1024, hop 256, bins 8..=500):
/// `(kurtosis, coefficient of variation)` of the bin power over all bins and
/// frames. Gaussian noise has exponentially distributed bin power: kurtosis 9
/// and coefficient of variation 1.
fn power_stats(x: &[f32]) -> (f64, f64) {
    let mut stft = Stft::new(1, StftConfig { frame: 1024, hop: 256, ..StftConfig::hann(1024) }).unwrap();
    let mut vals: Vec<f64> = Vec::new();
    let mut cb = |_: usize, b: &mut [Complex<f32>]| {
        for c in &b[8..=500] {
            vals.push((c.re * c.re + c.im * c.im) as f64);
        }
    };
    let mut sink = vec![0.0; x.len()];
    stft.process(0, x, &mut sink, &mut cb);
    let n = vals.len() as f64;
    let mean = vals.iter().sum::<f64>() / n;
    let m2 = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
    let m4 = vals.iter().map(|v| (v - mean).powi(4)).sum::<f64>() / n;
    (m4 / (m2 * m2), m2.sqrt() / mean)
}

/// Musical noise is a residual made of isolated time-frequency peaks that come
/// and go. Two measures of it, both on the residual of noise-only input:
/// the kurtosis ratio of Uemura et al. (2008), the kurtosis of the residual's
/// bin power over that of the input noise (1 for a quieter copy of Gaussian
/// noise, above 1 for peaks), and the coefficient of variation of the bin
/// power (the variance of the residual spectrum over time, normalised by its
/// mean; 1 for Gaussian noise).
///
/// Measured (see the table printed): smoothing the gain over frames lowers
/// both for every rule. Smoothing across bins alone does NOT in this measure:
/// it also deepens the reduction, which leaves a sparser residual. Radius 4
/// together with time smoothing is lower than none.
#[test]
fn smoothing_lowers_musical_noise() {
    let sigma = 0.1;
    let noise = white(288_000, 21, sigma);
    let (k_in, cv_in) = power_stats(&noise[48_000..]);
    println!("input: kurtosis {k_in:.2}, cv {cv_in:.2}");
    let rules = [
        ("SS", RepairMode::Reduce, GainRule::SpectralSubtraction, false, 2.0),
        ("Wiener", RepairMode::Reduce, GainRule::Wiener, false, 2.0),
        ("MMSE DD", RepairMode::Reduce, GainRule::MmseStsa, true, 1.0),
        ("gate", RepairMode::Gate, GainRule::Wiener, false, 1.0),
    ];
    for (name, mode, rule, dd, alpha) in rules {
        let base = RepairParams {
            mode,
            rule,
            decision_directed: dd,
            over_subtraction: alpha,
            threshold_db: 4.0,
            amount_db: 40.0,
            spectral_floor: 0.0,
            freq_smooth_bins: 0,
            attack_ms: 0.0,
            release_ms: 0.0,
            ..RepairParams::default()
        };
        let mut got = Vec::new();
        for (label, p) in [
            ("off", base),
            ("time 20/120 ms", RepairParams { attack_ms: 20.0, release_ms: 120.0, ..base }),
            ("freq 4 + time 40/200", RepairParams { freq_smooth_bins: 4, attack_ms: 40.0, release_ms: 200.0, ..base }),
        ] {
            let mut r = learned("white", sigma, p);
            let y = run(&mut r, &noise);
            let (k, cv) = power_stats(&y[48_000..]);
            println!("{name:8} {label:22} kurtosis ratio {:6.2}  cv {cv:5.2}", k / k_in);
            got.push((k / k_in, cv));
        }
        assert!(got[1].0 < got[0].0 && got[1].1 < got[0].1, "{name} time smoothing: {got:?}");
        assert!(got[2].0 < got[0].0 && got[2].1 < got[0].1, "{name} full smoothing: {got:?}");
        if name == "MMSE DD" {
            // Decision-directed MMSE-STSA with smoothing: essentially Gaussian residual.
            assert!(got[2].0 < 2.0 && got[2].1 < 1.15, "{got:?}");
        }
    }
}

#[test]
fn profile_converges() {
    let sigma = 0.1f64;
    // E|X_k|^2 = sigma^2 * sum(w^2); the sqrt-Hann window squared is Hann: sum = N/2.
    let truth = sigma * sigma * FRAME as f64 / 2.0;
    let mut last = f64::INFINITY;
    for seconds in [0.25f32, 1.0, 4.0, 16.0] {
        let n = (seconds * SR) as usize;
        let mut r = SpectralRepair::new(RepairConfig::new(SR, 1, FRAME)).unwrap();
        r.start_learning();
        run(&mut r, &white(n, 4, sigma as f32));
        r.stop_learning();
        let p = r.profile(0).unwrap();
        let rel: Vec<f64> = p[2..p.len() - 2].iter().map(|v| *v as f64 / truth - 1.0).collect();
        let rms = (rel.iter().map(|e| e * e).sum::<f64>() / rel.len() as f64).sqrt();
        let bias = rel.iter().sum::<f64>() / rel.len() as f64;
        let frames = r.profile_frames(0);
        println!(
            "{seconds:5} s: {frames:4} frames, rms error {rms:.3} (1/sqrt(frames) = {:.3}), bias {bias:+.4}",
            1.0 / (frames as f64).sqrt()
        );
        assert!(rms < last, "error must fall with learn time");
        assert!(rms < 2.0 / (frames as f64).sqrt(), "{rms}");
        assert!(bias.abs() < 0.03);
        last = rms;
    }
    // Pink noise: the profile follows the -3 dB per octave tilt.
    let mut r = SpectralRepair::new(RepairConfig::new(SR, 1, FRAME)).unwrap();
    r.start_learning();
    run(&mut r, &pink(192_000, 8, 0.1));
    r.stop_learning();
    let p = r.profile(0).unwrap();
    let bin = |hz: f32| (hz / SR * FRAME as f32).round() as usize;
    let avg = |hz: f32| {
        let b = bin(hz);
        let w = b / 5;
        p[b - w..=b + w].iter().map(|v| *v as f64).sum::<f64>() / (2 * w + 1) as f64
    };
    let tilt = db(avg(200.0) / avg(6_400.0));
    println!("pink profile tilt 200 Hz -> 6.4 kHz: {tilt:.1} dB (ideal 15.0)");
    assert!((tilt - 15.0).abs() < 2.5, "{tilt}");
}

#[test]
fn state_bytes_round_trip_and_refusals() {
    let mut a = SpectralRepair::new(RepairConfig::new(SR, 2, 512)).unwrap();
    a.start_learning();
    let n = white(20_000, 1, 0.2);
    run(&mut a, &n);
    let mut y = vec![0.0; n.len()];
    a.process(1, &white(20_000, 2, 0.05), &mut y);
    a.stop_learning();
    let bytes = a.save_profile();
    let mut b = SpectralRepair::new(RepairConfig::new(SR, 2, 512)).unwrap();
    assert_eq!(b.load_profile(&bytes), Ok(()));
    for ch in 0..2 {
        assert_eq!(a.profile_frames(ch), b.profile_frames(ch));
        let (pa, pb) = (a.profile(ch).unwrap(), b.profile(ch).unwrap());
        assert!(pa.iter().zip(pb).all(|(x, y)| x.to_bits() == y.to_bits()));
    }
    assert_eq!(b.save_profile(), bytes);
    // Identical behaviour afterwards.
    let x = white(8_000, 6, 0.1);
    let (mut ya, mut yb) = (vec![0.0; 8_000], vec![0.0; 8_000]);
    a.reset();
    b.reset();
    a.process(0, &x, &mut ya);
    b.process(0, &x, &mut yb);
    assert_eq!(ya, yb);

    // Refusals leave the loaded profile alone.
    let before = b.save_profile();
    assert_eq!(b.load_profile(&[]), Err(ProfileError::Truncated));
    assert_eq!(b.load_profile(&bytes[..bytes.len() - 1]), Err(ProfileError::Truncated));
    let mut bad = bytes.clone();
    bad[0] = b'X';
    assert_eq!(b.load_profile(&bad), Err(ProfileError::BadMagic));
    let mut bad = bytes.clone();
    bad[4] = 2;
    assert_eq!(b.load_profile(&bad), Err(ProfileError::UnsupportedVersion(2)));
    let mut bad = bytes.clone();
    bad[9] = 4; // frame 512 -> 1024
    assert_eq!(b.load_profile(&bad), Err(ProfileError::Mismatch));
    let mut bad = bytes.clone();
    bad[100] ^= 0x40;
    assert_eq!(b.load_profile(&bad), Err(ProfileError::Corrupt));
    let mut bad = bytes.clone();
    bad.push(0);
    assert_eq!(b.load_profile(&bad), Err(ProfileError::Corrupt));
    // A profile for another frame size or channel count.
    let other = SpectralRepair::new(RepairConfig::new(SR, 2, 1024)).unwrap().save_profile();
    assert_eq!(b.load_profile(&other), Err(ProfileError::Mismatch));
    let mono = SpectralRepair::new(RepairConfig::new(SR, 1, 512)).unwrap().save_profile();
    assert_eq!(b.load_profile(&mono), Err(ProfileError::Mismatch));
    // Well-formed checksum but a NaN power.
    let mut nan = bytes.clone();
    nan[24..28].copy_from_slice(&f32::NAN.to_le_bytes());
    let body = nan.len() - 4;
    let mut h = 0x811c_9dc5u32;
    for v in &nan[..body] {
        h = (h ^ *v as u32).wrapping_mul(0x0100_0193);
    }
    nan[body..].copy_from_slice(&h.to_le_bytes());
    assert_eq!(b.load_profile(&nan), Err(ProfileError::BadValue));
    assert_eq!(b.save_profile(), before);
    // Every truncation and every single-bit flip is refused without a panic.
    for cut in 0..bytes.len() {
        assert!(b.load_profile(&bytes[..cut]).is_err());
    }
    for i in 0..bytes.len() {
        let mut m = bytes.clone();
        m[i] ^= 0x01;
        assert!(b.load_profile(&m).is_err(), "flip at {i}");
    }
    assert_eq!(b.save_profile(), before);
}

#[test]
fn hostile_input_and_parameters() {
    let mut r = learned("white", 0.1, RepairParams::default());
    r.set_params(RepairParams {
        amount_db: f32::NAN,
        over_subtraction: f32::INFINITY,
        spectral_floor: -5.0,
        threshold_db: f32::NEG_INFINITY,
        attack_ms: f32::NAN,
        freq_smooth_bins: 10_000,
        ..RepairParams::default()
    });
    let p = r.params();
    assert!(p.amount_db.is_finite() && p.over_subtraction.is_finite());
    for rule in [GainRule::SpectralSubtraction, GainRule::Wiener, GainRule::MmseStsa] {
        for mode in [RepairMode::Reduce, RepairMode::Gate] {
            r.set_params(RepairParams { rule, mode, decision_directed: true, ..RepairParams::default() });
            let mut x = white(40_000, 2, 0.1);
            for i in (0..x.len()).step_by(997) {
                x[i] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, 0.0][i % 5];
            }
            let y = run(&mut r, &x);
            assert!(y.iter().all(|v| v.is_finite()), "{rule:?} {mode:?}");
        }
    }
    // Silence in, silence out.
    let y = run(&mut r, &vec![0.0; 20_000]);
    assert!(y[FRAME * 3..].iter().all(|v| *v == 0.0 || v.abs() < 1e-20));
    assert!(!r.set_profile(0, &[f32::NAN; 1025]));
    assert!(!r.set_profile(0, &[1.0; 3]));
    assert!(!r.set_profile(9, &[1.0; 1025]));
}
