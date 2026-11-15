//! FIR design: windowed sinc, Kaiser's design formulas and least squares.
//!
//! Everything here allocates and is meant for a worker thread or prepare time,
//! never the audio callback. Hand the taps to a running convolver with
//! `openplugin_core::worker` (see `docs/DSP_COOKBOOK.md`, "FIR design and phase").
//!
//! # Windowed sinc
//!
//! The ideal response of a low-pass with cutoff `fc` (cycles per sample) is
//! `h[m] = 2 fc sinc(2 fc m)` for `m = n - (N-1)/2`; the high-pass is
//! `delta[m] - lowpass`, the band-pass is the difference of two low-passes and the
//! band-stop is `delta[m] - bandpass` (Oppenheim and Schafer, "Discrete-Time
//! Signal Processing", 3rd ed., sec. 7.5, and Smith, "Spectral Audio Signal
//! Processing", "Window method"). The truncated sinc is multiplied by a
//! symmetric window from [`crate::windows`]. The cutoff is the -6 dB point, in
//! the middle of the transition band. High-pass and band-stop need an odd
//! length (a type I filter); an even symmetric filter has a zero at Nyquist.
//!
//! # Kaiser's formulas
//!
//! For a stopband attenuation `A` dB (`A = -20 log10(delta)`, with `delta` the
//! smaller of passband and stopband deviation) and a transition width
//! `d_omega = 2 pi d_f / fs`, Kaiser (1974, "Nonrecursive digital filter design
//! using the I0-sinh window function", Proc. IEEE ISCAS; given in Oppenheim and
//! Schafer eqs. 7.62 and 7.63) estimates
//!
//! ```text
//! beta = 0.1102 (A - 8.7)                              A > 50
//!      = 0.5842 (A - 21)^0.4 + 0.07886 (A - 21)         21 <= A <= 50
//!      = 0                                              A < 21
//! M    = (A - 8) / (2.285 d_omega),    taps = M + 1
//! ```
//!
//! [`kaiser_taps`] rounds up to an odd length. The estimate is empirical: the
//! tests record how far achieved attenuation and transition width fall from it.
//!
//! # Least squares
//!
//! [`design_least_squares`] fits the zero-phase amplitude of a symmetric
//! filter to a desired gain on a weighted frequency grid by solving the normal
//! equations `G a = b`, `G = sum w c c^T`, `b = sum w D c` in the cosine basis
//! (Parks and Burrus, "Digital Filter Design", 1987, ch. 2; Smith, "Spectral
//! Audio Signal Processing", "Least squares linear-phase FIR"). For an odd
//! length `N = 2M+1` the basis is `cos(k w)`, `k = 0..=M`; for an even length it
//! is `cos((k - 1/2) w)`, `k = 1..=N/2`.
//!
//! *Conditioning.* If the grid covers the whole band `0..=0.5`, `G` is close to
//! diagonal and the problem is benign. Leaving a transition gap un-weighted
//! (the point of a "don't care" band) makes `G` the Gram matrix of a prolate
//! spheroidal problem whose smallest eigenvalue falls roughly exponentially with
//! the length times the gap: narrow gaps with hundreds of taps give condition
//! numbers beyond `1e12`, past what `f64` can solve. The solver uses Cholesky
//! with a ridge of `1e-13` times the mean diagonal and reports a pivot-ratio
//! estimate of the condition number in [`LsFit::condition_estimate`]; it fails
//! with [`FirError::Singular`] when a pivot is not positive. Wide transition
//! bands or a small positive weight in the gap keep it well behaved.
//!
//! Parks-McClellan (Remez exchange) is not implemented here. The least-squares
//! and Kaiser designs cover the same needs for audio; an equiripple design is
//! left for later rather than shipped half-checked.

use crate::windows::WindowKind;
use std::f64::consts::PI;

/// Why a design was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirError {
    /// Fewer than 3 taps, or a size too small for the request.
    TooShort,
    /// High-pass and band-stop designs need an odd number of taps.
    EvenLength,
    /// A frequency outside `0..=Nyquist`, edges out of order, or a non-finite value.
    BadFrequency,
    /// Attenuation or transition width unusable.
    BadSpec,
    /// An empty or unsorted curve, or a non-finite point.
    BadCurve,
    /// The normal equations were not positive definite (see the module docs).
    Singular,
}

impl std::fmt::Display for FirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooShort => "too few taps",
            Self::EvenLength => "this filter type needs an odd number of taps",
            Self::BadFrequency => "frequency out of range or out of order",
            Self::BadSpec => "unusable attenuation or transition width",
            Self::BadCurve => "empty, unsorted or non-finite curve",
            Self::Singular => "least-squares system is singular (ill-conditioned)",
        })
    }
}

impl std::error::Error for FirError {}

/// The response shape of a windowed-sinc design. Frequencies are in Hz and are
/// the -6 dB points (the middle of the transition).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FirKind {
    LowPass { cutoff_hz: f64 },
    HighPass { cutoff_hz: f64 },
    BandPass { low_hz: f64, high_hz: f64 },
    BandStop { low_hz: f64, high_hz: f64 },
}

fn lowpass_ideal(fc: f64, m: f64) -> f64 {
    if m.abs() < 1e-12 {
        2.0 * fc
    } else {
        (2.0 * PI * fc * m).sin() / (PI * m)
    }
}

fn delta(m: f64) -> f64 {
    if m.abs() < 1e-12 {
        1.0
    } else {
        0.0
    }
}

fn check_kind(kind: FirKind, sample_rate: f64, taps: usize) -> Result<(), FirError> {
    if taps < 3 {
        return Err(FirError::TooShort);
    }
    if !(sample_rate.is_finite() && sample_rate > 0.0) {
        return Err(FirError::BadFrequency);
    }
    let nyq = sample_rate / 2.0;
    let in_range = |f: f64| f.is_finite() && f > 0.0 && f < nyq;
    let ok = match kind {
        FirKind::LowPass { cutoff_hz } | FirKind::HighPass { cutoff_hz } => in_range(cutoff_hz),
        FirKind::BandPass { low_hz, high_hz } | FirKind::BandStop { low_hz, high_hz } => {
            in_range(low_hz) && in_range(high_hz) && low_hz < high_hz
        }
    };
    if !ok {
        return Err(FirError::BadFrequency);
    }
    if matches!(kind, FirKind::HighPass { .. } | FirKind::BandStop { .. }) && taps % 2 == 0 {
        return Err(FirError::EvenLength);
    }
    Ok(())
}

/// Windowed-sinc design with an explicit `f64` window.
fn windowed_with(
    kind: FirKind,
    sample_rate: f64,
    taps: usize,
    w: &[f64],
) -> Result<Vec<f64>, FirError> {
    check_kind(kind, sample_rate, taps)?;
    let centre = (taps as f64 - 1.0) / 2.0;
    let n = |hz: f64| hz / sample_rate;
    Ok((0..taps)
        .map(|i| {
            let m = i as f64 - centre;
            let ideal = match kind {
                FirKind::LowPass { cutoff_hz } => lowpass_ideal(n(cutoff_hz), m),
                FirKind::HighPass { cutoff_hz } => delta(m) - lowpass_ideal(n(cutoff_hz), m),
                FirKind::BandPass { low_hz, high_hz } => {
                    lowpass_ideal(n(high_hz), m) - lowpass_ideal(n(low_hz), m)
                }
                FirKind::BandStop { low_hz, high_hz } => {
                    delta(m) - (lowpass_ideal(n(high_hz), m) - lowpass_ideal(n(low_hz), m))
                }
            };
            ideal * w[i]
        })
        .collect())
}

/// Symmetric window in `f64`. Kaiser is evaluated directly so that the design
/// does not lose accuracy to the `f32` window table.
fn window_f64(window: WindowKind, n: usize) -> Vec<f64> {
    match window {
        WindowKind::Kaiser { beta } => kaiser_window_f64((beta as f64).max(0.0), n),
        other => other.to_vec(n, false).into_iter().map(f64::from).collect(),
    }
}

pub(crate) fn kaiser_window_f64(beta: f64, n: usize) -> Vec<f64> {
    if n == 1 {
        return vec![1.0];
    }
    let denom = crate::windows::bessel_i0(beta);
    (0..n)
        .map(|i| {
            let r = 2.0 * i as f64 / (n - 1) as f64 - 1.0;
            crate::windows::bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / denom
        })
        .collect()
}

/// Windowed-sinc FIR of `taps` coefficients with any symmetric window.
/// `taps` must be odd for `HighPass` and `BandStop`.
pub fn design_windowed(
    kind: FirKind,
    sample_rate: f64,
    taps: usize,
    window: WindowKind,
) -> Result<Vec<f32>, FirError> {
    let w = window_f64(window, taps);
    Ok(windowed_with(kind, sample_rate, taps, &w)?
        .into_iter()
        .map(|v| v as f32)
        .collect())
}

/// Kaiser's `beta` for a stopband attenuation in dB (O&S eq. 7.62).
pub fn kaiser_beta(atten_db: f64) -> f64 {
    let a = atten_db;
    if a > 50.0 {
        0.1102 * (a - 8.7)
    } else if a >= 21.0 {
        0.5842 * (a - 21.0).powf(0.4) + 0.07886 * (a - 21.0)
    } else {
        0.0
    }
}

/// Attenuation in dB that meets both a passband ripple (peak deviation in dB,
/// `+-ripple`) and a stopband attenuation: `-20 log10(min(dp, ds))`. A Kaiser
/// window gives equal deviation in both bands, so the stricter one rules.
pub fn kaiser_attenuation_db(passband_ripple_db: f64, stopband_atten_db: f64) -> f64 {
    let dp = 10f64.powf(passband_ripple_db.abs() / 20.0) - 1.0;
    let ds = 10f64.powf(-stopband_atten_db.abs() / 20.0);
    -20.0 * dp.min(ds).log10()
}

/// Kaiser's length estimate (O&S eq. 7.63), rounded up to an odd count.
pub fn kaiser_taps(atten_db: f64, transition_hz: f64, sample_rate: f64) -> usize {
    let d_omega = 2.0 * PI * transition_hz / sample_rate;
    let m = ((atten_db - 8.0) / (2.285 * d_omega)).ceil().max(2.0) as usize;
    (m + 1) | 1
}

/// Kaiser design from an attenuation (dB) and a transition width (Hz): the
/// length and `beta` come from Kaiser's formulas. The result has an odd length.
pub fn design_kaiser(
    kind: FirKind,
    sample_rate: f64,
    atten_db: f64,
    transition_hz: f64,
) -> Result<Vec<f32>, FirError> {
    Ok(design_kaiser_f64(kind, sample_rate, atten_db, transition_hz)?
        .into_iter()
        .map(|v| v as f32)
        .collect())
}

/// As [`design_kaiser`], in `f64` (the half-band design needs more than `f32`
/// holds).
pub(crate) fn design_kaiser_f64(
    kind: FirKind,
    sample_rate: f64,
    atten_db: f64,
    transition_hz: f64,
) -> Result<Vec<f64>, FirError> {
    if !(atten_db.is_finite() && atten_db > 0.0 && transition_hz.is_finite() && transition_hz > 0.0)
        || !(sample_rate.is_finite() && sample_rate > 0.0)
    {
        return Err(FirError::BadSpec);
    }
    let taps = kaiser_taps(atten_db, transition_hz, sample_rate);
    let w = kaiser_window_f64(kaiser_beta(atten_db), taps);
    windowed_with(kind, sample_rate, taps, &w)
}

// ---------------------------------------------------------------------------
// Analysis helpers (direct evaluation of the DTFT, f64).
// ---------------------------------------------------------------------------

/// `H(e^{jw})` of `taps` at `freq` cycles per sample, as `(re, im)`.
pub fn fir_response(taps: &[f32], freq: f64) -> (f64, f64) {
    let w = 2.0 * PI * freq;
    let (mut re, mut im) = (0.0, 0.0);
    for (n, &h) in taps.iter().enumerate() {
        let a = w * n as f64;
        re += h as f64 * a.cos();
        im -= h as f64 * a.sin();
    }
    (re, im)
}

/// Magnitude in dB at `freq` cycles per sample.
pub fn fir_magnitude_db(taps: &[f32], freq: f64) -> f64 {
    let (re, im) = fir_response(taps, freq);
    10.0 * (re * re + im * im).max(1e-300).log10()
}

/// Group delay in samples at `freq` cycles per sample:
/// `Re{ sum(n h[n] e^{-jwn}) conj(H) } / |H|^2`.
pub fn fir_group_delay(taps: &[f32], freq: f64) -> f64 {
    let w = 2.0 * PI * freq;
    let (mut re, mut im, mut nre, mut nim) = (0.0, 0.0, 0.0, 0.0);
    for (n, &h) in taps.iter().enumerate() {
        let a = w * n as f64;
        let (c, s) = (a.cos(), a.sin());
        re += h as f64 * c;
        im -= h as f64 * s;
        nre += n as f64 * h as f64 * c;
        nim -= n as f64 * h as f64 * s;
    }
    let mag2 = re * re + im * im;
    if mag2 < 1e-300 {
        return 0.0;
    }
    (nre * re + nim * im) / mag2
}

// ---------------------------------------------------------------------------
// Least squares.
// ---------------------------------------------------------------------------

/// One point of the weighted frequency grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LsPoint {
    pub freq_hz: f64,
    /// Desired linear gain (zero-phase amplitude; may be negative).
    pub gain: f64,
    pub weight: f64,
}

/// A band with a gain that varies linearly from `gain_lo` to `gain_hi`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LsBand {
    pub lo_hz: f64,
    pub hi_hz: f64,
    pub gain_lo: f64,
    pub gain_hi: f64,
    pub weight: f64,
}

impl LsBand {
    pub fn flat(lo_hz: f64, hi_hz: f64, gain: f64, weight: f64) -> Self {
        Self {
            lo_hz,
            hi_hz,
            gain_lo: gain,
            gain_hi: gain,
            weight,
        }
    }
}

/// A dense uniform grid over `bands` for a design of `taps` coefficients
/// (spacing of 1/16 of the finest cosine basis function's period).
pub fn ls_grid(bands: &[LsBand], sample_rate: f64, taps: usize) -> Vec<LsPoint> {
    let dof = taps.div_ceil(2).max(1) as f64;
    let step = sample_rate / 2.0 / (16.0 * dof);
    let mut grid = Vec::new();
    for b in bands {
        let n = (((b.hi_hz - b.lo_hz) / step).ceil() as usize).max(1);
        for i in 0..=n {
            let t = i as f64 / n as f64;
            grid.push(LsPoint {
                freq_hz: b.lo_hz + t * (b.hi_hz - b.lo_hz),
                gain: b.gain_lo + t * (b.gain_hi - b.gain_lo),
                weight: b.weight,
            });
        }
    }
    grid
}

/// A least-squares design and how it was solved.
#[derive(Clone, Debug)]
pub struct LsFit {
    pub taps: Vec<f32>,
    /// Weighted RMS error of the fit over the grid.
    pub rms_error: f64,
    /// `(max pivot / min pivot)^2` of the Cholesky factor: a rough estimate of
    /// the condition number of the normal equations.
    pub condition_estimate: f64,
}

fn basis(odd: bool, i: usize, w: f64) -> f64 {
    if odd {
        (i as f64 * w).cos()
    } else {
        ((i as f64 + 0.5) * w).cos()
    }
}

/// Least-squares symmetric FIR of `taps` coefficients to a weighted grid.
///
/// With an even `taps` the response has a zero at Nyquist, so do not ask for
/// gain there. Linear phase, delay `(taps-1)/2`.
pub fn design_least_squares(
    taps: usize,
    sample_rate: f64,
    grid: &[LsPoint],
) -> Result<LsFit, FirError> {
    if taps < 2 {
        return Err(FirError::TooShort);
    }
    if !(sample_rate.is_finite() && sample_rate > 0.0) {
        return Err(FirError::BadFrequency);
    }
    let odd = taps % 2 == 1;
    let k = if odd { taps / 2 + 1 } else { taps / 2 };
    if grid.len() < k {
        return Err(FirError::TooShort);
    }
    for p in grid {
        if !(p.freq_hz.is_finite()
            && (0.0..=sample_rate / 2.0).contains(&p.freq_hz)
            && p.gain.is_finite()
            && p.weight.is_finite()
            && p.weight >= 0.0)
        {
            return Err(FirError::BadFrequency);
        }
    }
    // Normal equations (lower triangle of G).
    let mut g = vec![0.0f64; k * k];
    let mut b = vec![0.0f64; k];
    let mut c = vec![0.0f64; k];
    for p in grid {
        let w = 2.0 * PI * p.freq_hz / sample_rate;
        for (i, ci) in c.iter_mut().enumerate() {
            *ci = basis(odd, i, w);
        }
        for i in 0..k {
            b[i] += p.weight * p.gain * c[i];
            for j in 0..=i {
                g[i * k + j] += p.weight * c[i] * c[j];
            }
        }
    }
    let mean_diag = (0..k).map(|i| g[i * k + i]).sum::<f64>() / k as f64;
    for i in 0..k {
        g[i * k + i] += 1e-13 * mean_diag;
    }
    // Cholesky G = L L^T, in place.
    for j in 0..k {
        let mut d = g[j * k + j];
        for s in 0..j {
            d -= g[j * k + s] * g[j * k + s];
        }
        if d <= 0.0 || !d.is_finite() {
            return Err(FirError::Singular);
        }
        let d = d.sqrt();
        g[j * k + j] = d;
        for i in j + 1..k {
            let mut v = g[i * k + j];
            for s in 0..j {
                v -= g[i * k + s] * g[j * k + s];
            }
            g[i * k + j] = v / d;
        }
    }
    let (mut pmax, mut pmin) = (0.0f64, f64::MAX);
    for i in 0..k {
        pmax = pmax.max(g[i * k + i]);
        pmin = pmin.min(g[i * k + i]);
    }
    // Solve L y = b, then L^T a = y.
    let mut a = b;
    for i in 0..k {
        for s in 0..i {
            a[i] -= g[i * k + s] * a[s];
        }
        a[i] /= g[i * k + i];
    }
    for i in (0..k).rev() {
        for s in i + 1..k {
            a[i] -= g[s * k + i] * a[s];
        }
        a[i] /= g[i * k + i];
    }
    // Basis coefficients to taps.
    let mut h = vec![0.0f64; taps];
    if odd {
        let m = taps / 2;
        h[m] = a[0];
        for (i, &ai) in a.iter().enumerate().skip(1) {
            h[m - i] = ai / 2.0;
            h[m + i] = ai / 2.0;
        }
    } else {
        let half = taps / 2;
        for (i, &ai) in a.iter().enumerate() {
            let kk = i + 1;
            h[half - kk] = ai / 2.0;
            h[half + kk - 1] = ai / 2.0;
        }
    }
    let (mut num, mut den) = (0.0, 0.0);
    for p in grid {
        let w = 2.0 * PI * p.freq_hz / sample_rate;
        let amp: f64 = a.iter().enumerate().map(|(i, &ai)| ai * basis(odd, i, w)).sum();
        num += p.weight * (amp - p.gain).powi(2);
        den += p.weight;
    }
    Ok(LsFit {
        taps: h.into_iter().map(|v| v as f32).collect(),
        rms_error: if den > 0.0 { (num / den).sqrt() } else { 0.0 },
        condition_estimate: (pmax / pmin).powi(2),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windowed_lowpass_is_symmetric_with_unit_dc() {
        let h = design_windowed(
            FirKind::LowPass { cutoff_hz: 5000.0 },
            48_000.0,
            101,
            WindowKind::Blackman,
        )
        .unwrap();
        for i in 0..h.len() {
            assert_eq!(h[i], h[h.len() - 1 - i]);
        }
        let dc: f32 = h.iter().sum();
        assert!((dc - 1.0).abs() < 1e-3);
        let edge = fir_magnitude_db(&h, 5000.0 / 48_000.0);
        assert!((-7.0..-5.0).contains(&edge), "{edge}");
    }

    #[test]
    fn invalid_requests_are_refused() {
        assert_eq!(
            design_windowed(
                FirKind::HighPass { cutoff_hz: 1000.0 },
                48_000.0,
                64,
                WindowKind::Hann
            ),
            Err(FirError::EvenLength)
        );
        assert_eq!(
            design_windowed(
                FirKind::LowPass { cutoff_hz: 30_000.0 },
                48_000.0,
                65,
                WindowKind::Hann
            ),
            Err(FirError::BadFrequency)
        );
        assert_eq!(
            design_kaiser(FirKind::LowPass { cutoff_hz: 1000.0 }, 48_000.0, 60.0, 0.0),
            Err(FirError::BadSpec)
        );
    }

    #[test]
    fn kaiser_beta_matches_published_points() {
        assert!((kaiser_beta(60.0) - 5.6533).abs() < 1e-3);
        assert!((kaiser_beta(80.0) - 7.8573).abs() < 1e-3);
        assert!((kaiser_beta(40.0) - 3.3953).abs() < 1e-2);
        assert_eq!(kaiser_beta(10.0), 0.0);
    }

    #[test]
    fn ripple_to_attenuation() {
        let a = kaiser_attenuation_db(0.01, 40.0);
        assert!((a - 58.7).abs() < 0.1, "{a}");
        assert!((kaiser_attenuation_db(1.0, 60.0) - 60.0).abs() < 1e-9);
    }

    #[test]
    fn least_squares_recovers_an_exact_filter() {
        // The desired amplitude is itself a 21-tap symmetric filter's: fit exactly.
        let target = design_windowed(
            FirKind::LowPass { cutoff_hz: 4000.0 },
            48_000.0,
            21,
            WindowKind::Hamming,
        )
        .unwrap();
        let grid: Vec<LsPoint> = (0..=400)
            .map(|i| {
                let f = 0.5 * i as f64 / 400.0;
                let amp: f64 = target
                    .iter()
                    .enumerate()
                    .map(|(n, &h)| h as f64 * (2.0 * PI * f * (n as f64 - 10.0)).cos())
                    .sum();
                LsPoint {
                    freq_hz: f * 48_000.0,
                    gain: amp,
                    weight: 1.0,
                }
            })
            .collect();
        let fit = design_least_squares(21, 48_000.0, &grid).unwrap();
        for (a, b) in fit.taps.iter().zip(&target) {
            assert!((a - b).abs() < 1e-5, "{a} vs {b}");
        }
    }
}
