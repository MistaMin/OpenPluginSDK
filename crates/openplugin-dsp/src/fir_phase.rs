//! Linear-phase and minimum-phase FIR from a magnitude curve.
//!
//! Design only: these functions allocate and belong on a worker thread. Play
//! the taps through [`crate::LongConvolver`] (see the cookbook).
//!
//! # Linear phase (frequency sampling with a window)
//!
//! The curve (gain in dB at frequencies in Hz, linear in dB over log frequency
//! between points, flat beyond the ends) is sampled on `L/2 + 1` FFT bins, given
//! the phase `exp(-j w M)` with `M = (N-1)/2`, and transformed back. The first
//! `N` samples, multiplied by a symmetric window, are the taps (Oppenheim and
//! Schafer, sec. 7.5.1 "frequency sampling", and Rabiner and Gold, "Theory and
//! Application of Digital Signal Processing", 1975, sec. 3.10). Taps are made
//! exactly symmetric, `h[n] == h[N-1-n]`, so the group delay is exactly
//! `(N-1)/2` samples at every frequency. Resolution: the window smooths the
//! response over about `4 fs / N` for a Hann-like window, so a feature narrower
//! than that is not reproduced.
//!
//! # Minimum phase (real cepstrum, homomorphic method)
//!
//! For a magnitude `|H|` with `ln|H|` sampled on `L` points, the real cepstrum is
//! `c[n] = IDFT(ln|H|)`. The minimum-phase response has the causal (folded)
//! cepstrum `c_hat[0] = c[0]`, `c_hat[n] = 2 c[n]` for `0 < n < L/2`,
//! `c_hat[L/2] = c[L/2]`, `c_hat[n] = 0` otherwise, and
//! `H_min = exp(DFT(c_hat))` (Oppenheim and Schafer, 3rd ed., sec. 13.1.3 and
//! 13.2, "Minimum-phase and causal"). Sampling the cepstrum on `L` points
//! aliases it; the error shrinks as `L` grows, so the curve entry points use
//! `L >= 32 N` (at least 8192) and the error is verified in the tests. The
//! magnitude is floored at `1e-9` (-180 dB) before the logarithm. The result
//! is truncated to `N` taps: check [`partial_energy`] for the part of the
//! energy kept.
//!
//! # Mixed phase
//!
//! [`mixed_phase_eq`] scales the minimum phase by `amount` and adds the
//! remainder as linear phase: `phase = amount phi_min - (1 - amount) w M`.
//! `amount = 1` is minimum phase, `0` is linear phase with delay `M`. Pre-ringing
//! shrinks in proportion to `amount`. The result is not windowed, so use enough
//! taps that the tails are negligible.
//!
//! # Linear to minimum phase
//!
//! [`minimum_phase_from_linear`] turns a linear-phase design with zero-phase
//! amplitude `A(w)` into a minimum-phase filter of half the order whose
//! *squared* magnitude is `A(w) + delta`, where `delta` lifts the stopband
//! ripple so the sum is positive (spectral factorisation, Smith, "Spectral
//! Audio Signal Processing", "Minimum phase filter design from a linear phase
//! one"; Vaidyanathan, "Multirate Systems and Filter Banks", 1993, sec. 4.6).
//! Because the magnitude is a square root, ripple and stopband dB are halved:
//! ask the prototype for about twice the attenuation wanted.

use crate::fir_design::FirError;
use crate::windows::WindowKind;
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use std::f64::consts::PI;

type C = Complex<f64>;

/// A point of an EQ curve: `(frequency in Hz, gain in dB)`.
pub type EqPoint = (f64, f64);

const MAG_FLOOR: f64 = 1e-9;

/// Latency of a linear-phase filter of `taps` coefficients, in samples.
/// An integer when `taps` is odd.
pub fn linear_phase_latency(taps: usize) -> f32 {
    (taps as f32 - 1.0) / 2.0
}

fn fft(buf: &mut [C], inverse: bool) {
    let mut planner = FftPlanner::<f64>::new();
    let f = if inverse {
        planner.plan_fft_inverse(buf.len())
    } else {
        planner.plan_fft_forward(buf.len())
    };
    f.process(buf);
}

fn fft_size_for(taps: usize) -> usize {
    (taps * 32).next_power_of_two().max(8192)
}

/// Samples `curve` on bins `0..=fft_size/2` as linear magnitudes.
pub fn sample_curve(
    curve: &[EqPoint],
    sample_rate: f64,
    fft_size: usize,
) -> Result<Vec<f64>, FirError> {
    if curve.is_empty()
        || !(sample_rate.is_finite() && sample_rate > 0.0)
        || fft_size < 8
        || !fft_size.is_power_of_two()
    {
        return Err(FirError::BadCurve);
    }
    for (i, &(f, g)) in curve.iter().enumerate() {
        if !(f.is_finite() && g.is_finite() && f > 0.0 && f <= sample_rate / 2.0) {
            return Err(FirError::BadCurve);
        }
        if i > 0 && f <= curve[i - 1].0 {
            return Err(FirError::BadCurve);
        }
    }
    let bins = fft_size / 2 + 1;
    let mut out = Vec::with_capacity(bins);
    let mut seg = 0;
    for k in 0..bins {
        let f = k as f64 * sample_rate / fft_size as f64;
        let db = if f <= curve[0].0 {
            curve[0].1
        } else if f >= curve[curve.len() - 1].0 {
            curve[curve.len() - 1].1
        } else {
            while curve[seg + 1].0 < f {
                seg += 1;
            }
            let (f0, g0) = curve[seg];
            let (f1, g1) = curve[seg + 1];
            g0 + (g1 - g0) * (f / f0).ln() / (f1 / f0).ln()
        };
        out.push(10f64.powf(db / 20.0));
    }
    Ok(out)
}

/// Linear-phase FIR from an EQ curve. `taps` should be odd (an even length has
/// a half-sample delay and a zero at Nyquist); `window` shapes the truncation
/// (Kaiser with `beta` near 6 to 9, or Blackman, are good defaults).
pub fn linear_phase_eq(
    curve: &[EqPoint],
    sample_rate: f64,
    taps: usize,
    window: WindowKind,
) -> Result<Vec<f32>, FirError> {
    if taps < 3 {
        return Err(FirError::TooShort);
    }
    let l = fft_size_for(taps);
    let mag = sample_curve(curve, sample_rate, l)?;
    let h = linear_phase_from_magnitude(&mag, taps, window)?;
    Ok(h)
}

/// Linear-phase FIR from magnitudes on bins `0..=L/2` (`mag.len() = L/2 + 1`,
/// `L` a power of two at least `2 taps`).
pub fn linear_phase_from_magnitude(
    mag: &[f64],
    taps: usize,
    window: WindowKind,
) -> Result<Vec<f32>, FirError> {
    let l = 2 * mag.len().saturating_sub(1);
    if taps < 3 || l < 2 * taps || !l.is_power_of_two() {
        return Err(FirError::TooShort);
    }
    let m = (taps as f64 - 1.0) / 2.0;
    let mut spec = vec![C::new(0.0, 0.0); l];
    for k in 0..=l / 2 {
        let ph = -2.0 * PI * k as f64 * m / l as f64;
        let v = C::from_polar(mag[k], ph);
        spec[k] = v;
        if k > 0 && k < l / 2 {
            spec[l - k] = v.conj();
        }
    }
    spec[0].im = 0.0;
    spec[l / 2].im = 0.0;
    fft(&mut spec, true);
    let w = WindowKind::to_vec(window, taps, false);
    let mut h: Vec<f64> = (0..taps)
        .map(|n| spec[n].re / l as f64 * w[n] as f64)
        .collect();
    // Exact symmetry.
    for n in 0..taps / 2 {
        let a = 0.5 * (h[n] + h[taps - 1 - n]);
        h[n] = a;
        h[taps - 1 - n] = a;
    }
    Ok(h.into_iter().map(|v| v as f32).collect())
}

/// Folded cepstrum of `ln|H|`, then `ln H_min` on all `L` bins.
fn log_min_phase(mag: &[f64]) -> Vec<C> {
    let l = 2 * (mag.len() - 1);
    let mut s = vec![C::new(0.0, 0.0); l];
    for k in 0..=l / 2 {
        let v = C::new(mag[k].max(MAG_FLOOR).ln(), 0.0);
        s[k] = v;
        if k > 0 && k < l / 2 {
            s[l - k] = v;
        }
    }
    fft(&mut s, true);
    for v in s.iter_mut() {
        *v /= l as f64;
    }
    for v in s.iter_mut().take(l / 2).skip(1) {
        *v *= 2.0;
    }
    for v in s.iter_mut().skip(l / 2 + 1) {
        *v = C::new(0.0, 0.0);
    }
    fft(&mut s, false);
    s
}

fn shaped(mag: &[f64], taps: usize, amount: f64) -> Result<Vec<f32>, FirError> {
    let l = 2 * mag.len().saturating_sub(1);
    if taps < 2 || l < 2 * taps || !l.is_power_of_two() {
        return Err(FirError::TooShort);
    }
    let amount = amount.clamp(0.0, 1.0);
    let m = (taps as f64 - 1.0) / 2.0;
    let mut s = log_min_phase(mag);
    for (k, v) in s.iter_mut().enumerate() {
        let w = if k <= l / 2 {
            2.0 * PI * k as f64 / l as f64
        } else {
            -2.0 * PI * (l - k) as f64 / l as f64
        };
        let phase = amount * v.im - (1.0 - amount) * w * m;
        *v = C::from_polar(v.re.exp(), phase);
    }
    fft(&mut s, true);
    Ok((0..taps).map(|n| (s[n].re / l as f64) as f32).collect())
}

/// Minimum-phase FIR from magnitudes on bins `0..=L/2` (see the module docs).
pub fn minimum_phase_from_magnitude(mag: &[f64], taps: usize) -> Result<Vec<f32>, FirError> {
    shaped(mag, taps, 1.0)
}

/// Blend of minimum and linear phase from magnitudes (`amount` 1 = minimum).
pub fn mixed_phase_from_magnitude(
    mag: &[f64],
    taps: usize,
    amount: f64,
) -> Result<Vec<f32>, FirError> {
    shaped(mag, taps, amount)
}

/// Minimum-phase FIR from an EQ curve.
pub fn minimum_phase_eq(
    curve: &[EqPoint],
    sample_rate: f64,
    taps: usize,
) -> Result<Vec<f32>, FirError> {
    mixed_phase_eq(curve, sample_rate, taps, 1.0)
}

/// Mixed-phase FIR from an EQ curve; delay about `(1 - amount)(taps - 1)/2`.
pub fn mixed_phase_eq(
    curve: &[EqPoint],
    sample_rate: f64,
    taps: usize,
    amount: f64,
) -> Result<Vec<f32>, FirError> {
    if taps < 2 {
        return Err(FirError::TooShort);
    }
    let mag = sample_curve(curve, sample_rate, fft_size_for(taps))?;
    shaped(&mag, taps, amount)
}

/// Fraction of the energy of `taps` that lies in its first `n` samples.
pub fn partial_energy(taps: &[f32], n: usize) -> f64 {
    let total: f64 = taps.iter().map(|&v| (v as f64).powi(2)).sum();
    if total == 0.0 {
        return 1.0;
    }
    taps.iter().take(n).map(|&v| (v as f64).powi(2)).sum::<f64>() / total
}

/// Minimum-phase spectral factor of a linear-phase design (odd length `N`):
/// `(N + 1) / 2` taps whose squared magnitude is the prototype's zero-phase
/// amplitude plus a small lift. Gain is normalised to about 1 in the passband.
pub fn minimum_phase_from_linear(linear: &[f32]) -> Result<Vec<f32>, FirError> {
    let h: Vec<f64> = linear.iter().map(|&v| v as f64).collect();
    Ok(minphase_from_linear_f64(&h)?
        .into_iter()
        .map(|v| v as f32)
        .collect())
}

pub(crate) fn minphase_from_linear_f64(linear: &[f64]) -> Result<Vec<f64>, FirError> {
    let n = linear.len();
    if n < 3 || n % 2 == 0 {
        return Err(FirError::EvenLength);
    }
    let m = (n - 1) / 2;
    let l = (n * 64).next_power_of_two().max(8192);
    // Zero-phase amplitude A(w) = sum h[n] cos(w (n - M)), via the FFT of the
    // prototype shifted so its centre is at index 0.
    let mut a = vec![C::new(0.0, 0.0); l];
    for (i, &v) in linear.iter().enumerate() {
        let idx = (i + l - m) % l;
        a[idx] = C::new(v, 0.0);
    }
    fft(&mut a, false);
    let amp: Vec<f64> = (0..=l / 2).map(|k| a[k].re).collect();
    let min = amp.iter().cloned().fold(f64::MAX, f64::min);
    let lift = (-min).max(0.0) * 1.05 + 1e-15;
    let mag: Vec<f64> = amp
        .iter()
        .map(|&v| ((v + lift).max(0.0)).sqrt() / (1.0 + lift).sqrt())
        .collect();
    // The factor of a degree-M trigonometric polynomial has M + 1 taps.
    let taps = m + 1;
    let s = {
        let mut s = log_min_phase(&mag);
        for v in s.iter_mut() {
            *v = C::from_polar(v.re.exp(), v.im);
        }
        fft(&mut s, true);
        s
    };
    Ok((0..taps).map(|i| s[i].re / l as f64).collect())
}
