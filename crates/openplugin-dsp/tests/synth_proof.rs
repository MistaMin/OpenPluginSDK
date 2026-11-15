//! Measured proofs for the synthesis blocks: aliasing against the naive
//! oscillator, noise spectra, resampler quality.
//!
//! Coherent sampling: a frequency of `k / N` cycles per sample with `N = 2^16`
//! and odd `k` makes every harmonic land on a bin that is a multiple of `k`;
//! aliased partials fold onto bins that are not (the folding of `h k` mod `N`
//! hits a harmonic bin only for `h >= N - 32`, where the partials are 1e-5 of
//! the fundamental). So the power in non-multiples of `k` is the aliased power,
//! with no window leakage.

use openplugin_dsp::{
    BandlimitedOsc, BlWave, BlepOrder, Oscillator, PinkNoise, Quality, SincKernel, Wave, WavetableOsc,
    WavetableSet,
};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

const N: usize = 1 << 16;
const SR: f64 = 44_100.0;

/// Odd bin count for `hz` at 44.1 kHz.
fn bin_for(hz: f64) -> usize {
    let k = (hz * N as f64 / SR).round() as usize;
    k | 1
}

fn spectrum(x: &[f32]) -> Vec<f64> {
    let mut buf: Vec<Complex<f64>> = x.iter().map(|v| Complex::new(f64::from(*v), 0.0)).collect();
    FftPlanner::<f64>::new().plan_fft_forward(x.len()).process(&mut buf);
    buf.iter().map(|c| c.norm_sqr() / (x.len() as f64 * x.len() as f64)).collect()
}

/// Aliased power relative to the fundamental, dB, for a signal of `N` samples at `k` cycles.
fn alias_db(x: &[f32], k: usize) -> f64 {
    let p = spectrum(x);
    let fundamental = p[k];
    let mut alias = 0.0;
    for (j, v) in p.iter().enumerate().take(N / 2 + 1).skip(1) {
        if j % k != 0 {
            alias += 2.0 * v;
        }
    }
    10.0 * (alias / (2.0 * fundamental)).log10()
}

fn naive(wave: &str, k: usize) -> Vec<f32> {
    // Oscillator::next takes hz and sample_rate; sr = N makes hz = k an exact k / N step.
    let n = N as f32;
    match wave {
        "saw" | "square" => {
            let mut o = Oscillator::new(if wave == "saw" { Wave::Saw } else { Wave::Square });
            (0..N).map(|_| o.next(k as f32, n)).collect()
        }
        "tri" => {
            let mut o = Oscillator::new(Wave::Saw);
            (0..N)
                .map(|_| {
                    let s = o.next(k as f32, n); // -1..1 ramp
                    // ramp phase p = (s+1)/2 ; triangle peak at 0.
                    let p = (s + 1.0) * 0.5;
                    4.0 * (p - 0.5).abs() - 1.0
                })
                .collect()
        }
        _ => unreachable!(),
    }
}

fn blep(wave: BlWave, order: BlepOrder, k: usize, width: f32) -> Vec<f32> {
    let mut o = BandlimitedOsc::new(wave).with_order(order);
    o.set_pulse_width(width);
    let ds = k as f64 / N as f64;
    // Run a few periods first so the delay line is full; N is a whole number of periods.
    for _ in 0..8 {
        o.next_normalized(ds, None);
    }
    let x: Vec<f32> = (0..N).map(|_| o.next_normalized(ds, None)).collect();
    // The 8 warm-up samples shifted the phase by 8 samples; the sequence is still periodic in N.
    x
}

#[test]
fn aliasing_of_band_limited_oscillators_against_naive() {
    let pitches = [100.0, 440.0, 1000.0, 2000.0, 3000.0, 5000.0];
    let mut report = String::new();
    report.push_str("aliased power re fundamental (dB), 44.1 kHz\n  wave     f0(Hz)   naive   blep2   blep4   blep6   blep8  wavetable\n");
    let saw_table = WavetableSet::saw(2048).unwrap();
    for (name, wave) in [("saw", BlWave::Saw), ("square", BlWave::Square), ("tri", BlWave::Triangle)] {
        for hz in pitches {
            let k = bin_for(hz);
            let f0 = k as f64 * SR / N as f64;
            let n = alias_db(&naive(name, k), k);
            let b2 = alias_db(&blep(wave, BlepOrder::Two, k, 0.5), k);
            let b4 = alias_db(&blep(wave, BlepOrder::Four, k, 0.5), k);
            let b6 = alias_db(&blep(wave, BlepOrder::Six, k, 0.5), k);
            let b8 = alias_db(&blep(wave, BlepOrder::Eight, k, 0.5), k);
            let wt = if name == "saw" {
                let mut o = WavetableOsc::new();
                let x: Vec<f32> = (0..N).map(|_| o.next_normalized(&saw_table, k as f64 / N as f64)).collect();
                alias_db(&x, k)
            } else {
                f64::NAN
            };
            report.push_str(&format!("  {name:<7} {f0:>7.0} {n:>7.1} {b2:>7.1} {b4:>7.1} {b6:>7.1} {b8:>7.1} {wt:>8.1}\n"));
            // Each wider kernel buys about 9 dB; the 4-point PolyBLEP is 20 dB or more
            // below the naive oscillator at every pitch. Floors have 3 dB of margin.
            assert!(b4 < n - 20.0, "{name} {hz}: blep4 {b4} vs naive {n}");
            assert!(b4 <= b2 + 0.5 && b6 <= b4 + 0.5 && b8 <= b6 + 0.5, "{name} {hz}: orders not monotone");
            let floors = if name == "tri" { [-35.0, -45.0, -55.0, -65.0] } else { [-20.0, -30.0, -40.0, -50.0] };
            for (v, f, o) in [(b2, floors[0], 2), (b4, floors[1], 4), (b6, floors[2], 6), (b8, floors[3], 8)] {
                assert!(v < f, "{name} {hz} order {o}: {v} dB (limit {f})");
            }
            if name == "saw" {
                assert!(wt < -85.0, "wavetable saw {hz}: {wt}");
            }
        }
    }
    // PWM: a pulse at several widths.
    for w in [0.1f32, 0.3, 0.5, 0.8] {
        let k = bin_for(2000.0);
        let b4 = alias_db(&blep(BlWave::Pulse, BlepOrder::Four, k, w), k);
        report.push_str(&format!("  pulse w={w:.1} 2 kHz blep4 {b4:.1}\n"));
        assert!(b4 < -30.0, "pulse {w}: {b4}");
    }
    println!("{report}");
}

#[test]
fn aliasing_of_hard_sync() {
    // Master at k_m / N, slave at 2.71x. The result repeats with the master period.
    let km = bin_for(500.0);
    let dm = km as f64 / N as f64;
    let ds = dm * 2.71;
    let render = |sync_on: bool, band_limited: bool| -> Vec<f32> {
        let mut master = BandlimitedOsc::new(BlWave::Saw);
        let mut slave = BandlimitedOsc::new(BlWave::Saw);
        let mut naive_phase = 0.0f64;
        let mut master_phase = 0.0f64;
        let mut out = Vec::with_capacity(N);
        for i in 0..N + 64 {
            master.next_normalized(dm, None);
            let v = if band_limited {
                slave.next_normalized(ds, if sync_on { master.last_wrap().map(|t| t) } else { None })
            } else {
                // Naive: reset at the sample after the master wraps.
                master_phase += dm;
                let wrapped = master_phase >= 1.0;
                if wrapped {
                    master_phase -= 1.0;
                }
                naive_phase += ds;
                if naive_phase >= 1.0 {
                    naive_phase -= 1.0;
                }
                if sync_on && wrapped {
                    naive_phase = 0.0;
                }
                (2.0 * naive_phase - 1.0) as f32
            };
            if i >= 64 {
                out.push(v);
            }
        }
        out
    };
    let naive = alias_db(&render(true, false), km);
    let bl = alias_db(&render(true, true), km);
    println!("hard sync saw, master 500 Hz, slave x2.71: aliased power naive {naive:.1} dB, band-limited {bl:.1} dB");
    assert!(bl < naive - 15.0, "{bl} vs {naive}");
}

/// Welch PSD of a noise signal and the least-squares slope in dB per octave over
/// half-octave bands from `lo` to `hi` Hz.
fn psd_slope(x: &[f32], sr: f64, lo: f64, hi: f64) -> (f64, f64) {
    let seg = 1usize << 17;
    let hop = seg / 2;
    let win: Vec<f64> = (0..seg)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / seg as f64).cos())
        .collect();
    let fft = FftPlanner::<f64>::new().plan_fft_forward(seg);
    let mut psd = vec![0.0f64; seg / 2];
    let mut count = 0.0;
    let mut start = 0;
    while start + seg <= x.len() {
        let mut buf: Vec<Complex<f64>> =
            (0..seg).map(|i| Complex::new(f64::from(x[start + i]) * win[i], 0.0)).collect();
        fft.process(&mut buf);
        for (p, c) in psd.iter_mut().zip(&buf) {
            *p += c.norm_sqr();
        }
        count += 1.0;
        start += hop;
    }
    let bin_hz = sr / seg as f64;
    let mut pts: Vec<(f64, f64)> = Vec::new();
    let mut f = lo;
    while f * 2f64.sqrt() <= hi {
        let (a, b) = (f, f * 2f64.sqrt());
        let (ia, ib) = ((a / bin_hz).ceil() as usize, (b / bin_hz).floor() as usize);
        if ib >= ia {
            let mean = psd[ia..=ib].iter().sum::<f64>() / (ib - ia + 1) as f64 / count;
            pts.push(((a * b).sqrt().log2(), 10.0 * mean.log10()));
        }
        f = b;
    }
    let n = pts.len() as f64;
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |(a, b), p| (a + p.0, b + p.1));
    let (mx, my) = (sx / n, sy / n);
    let slope = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum::<f64>()
        / pts.iter().map(|p| (p.0 - mx) * (p.0 - mx)).sum::<f64>();
    let worst = pts.iter().map(|p| (p.1 - (my + slope * (p.0 - mx))).abs()).fold(0.0, f64::max);
    (slope, worst)
}

#[test]
fn pink_noise_slope_is_minus_three_db_per_octave_over_four_decades() {
    // 384 kHz: 10 Hz to 100 kHz is four decades. 96 kHz covers 10 Hz to 25 kHz.
    for (sr, hi) in [(384_000.0f32, 100_000.0f64), (96_000.0, 25_000.0), (48_000.0, 13_000.0)] {
        let mut p = PinkNoise::new(sr, 0xC0FFEE);
        let n = 1usize << 22;
        let x: Vec<f32> = (0..n).map(|_| p.next()).collect();
        let (slope, worst) = psd_slope(&x, f64::from(sr), 10.0, hi);
        println!("pink @ {sr} Hz: 10 Hz..{hi} Hz slope {slope:.3} dB/oct, worst band deviation {worst:.2} dB");
        assert!((slope + 3.0).abs() < 0.5, "slope {slope} at {sr}");
        assert!(worst < 1.5, "band deviation {worst} at {sr}");
    }
}

#[test]
fn white_noise_is_flat() {
    let mut w = openplugin_dsp::WhiteNoise::new(5);
    let x: Vec<f32> = (0..1 << 21).map(|_| w.next()).collect();
    let (slope, _) = psd_slope(&x, 48_000.0, 100.0, 20_000.0);
    assert!(slope.abs() < 0.1, "{slope}");
}

#[test]
fn resampler_removes_what_would_fold_and_keeps_what_does_not() {
    // A 0.30 cycles/sample tone read at ratio 1.7 would land at 0.51 (above Nyquist) -> must be gone.
    // A 0.10 tone lands at 0.17 -> must stay at unit level.
    for q in [Quality::Low, Quality::Medium, Quality::High, Quality::Best] {
        let k = SincKernel::new(q);
        let tone = |f: f64, len: usize| -> Vec<f32> {
            (0..len).map(|i| (2.0 * std::f64::consts::PI * f * i as f64).sin() as f32).collect()
        };
        let rms = |v: &[f32]| (v.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>() / v.len() as f64).sqrt();
        let mut out = vec![0.0f32; 4000];
        let input = tone(0.10, 20_000);
        assert!(k.resample(&input, 1.7, &mut out) >= 4000);
        let kept = rms(&out[400..3600]) / (0.5f64).sqrt();
        let input = tone(0.30, 20_000);
        k.resample(&input, 1.7, &mut out);
        let folded = rms(&out[400..3600]) / (0.5f64).sqrt();
        println!("{q:?}: in-band gain {kept:.5}, out-of-band residue {:.1} dB", 20.0 * folded.log10());
        assert!((kept - 1.0).abs() < 5e-3, "{q:?} {kept}");
        let limit = match q {
            Quality::Low => -50.0,
            Quality::Medium => -70.0,
            Quality::High => -90.0,
            Quality::Best => -100.0,
        };
        assert!(20.0 * folded.log10() < limit, "{q:?} residue {}", 20.0 * folded.log10());
    }
}
