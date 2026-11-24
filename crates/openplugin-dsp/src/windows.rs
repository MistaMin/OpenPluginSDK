//! Spectral analysis windows with their coherent gain and equivalent noise bandwidth.
//!
//! The cosine-sum family follows F. J. Harris, "On the use of windows for
//! harmonic analysis with the discrete Fourier transform", Proc. IEEE 66(1),
//! 1978, pp. 51-83: `w[n] = sum_m (-1)^m a_m cos(2 pi m n / N)`. The Kaiser
//! window is J. F. Kaiser's `I0(beta sqrt(1 - x^2)) / I0(beta)`, with the zeroth
//! order modified Bessel function summed from its power series here.
//!
//! The five-term flat-top uses the coefficients common in measurement
//! software (ISO 18431-2 style, a0 = 0.21557895). Flat-top is for amplitude
//! accuracy of a tone; its main lobe is wide.
//!
//! Two figures describe a window `w` of length `N`:
//! - coherent gain `sum(w) / N`: the amplitude a bin-centred sine loses;
//! - equivalent noise bandwidth `N sum(w^2) / sum(w)^2`, in bins.
//!
//! `periodic = true` gives the DFT-even form (period `N`), the right one for
//! STFT analysis and overlap-add. `false` gives the symmetric form (period
//! `N - 1`), the right one for FIR design. This module does not touch
//! [`crate::window::hann`].

use std::f64::consts::PI;

/// Window shape.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum WindowKind {
    /// All ones.
    Rectangular,
    /// Raised cosine, `0.5 - 0.5 cos`.
    Hann,
    /// Classic 0.54 / 0.46 Hamming.
    Hamming,
    /// Classic 0.42 / 0.5 / 0.08 Blackman.
    Blackman,
    /// Four-term minimum-sidelobe window (-92 dB), Harris 1978.
    BlackmanHarris,
    /// Five-term flat-top window, for amplitude-accurate readings.
    FlatTop,
    /// Kaiser with shape `beta` (0 is rectangular; about 8.6 gives -90 dB sidelobes).
    Kaiser {
        /// Shape parameter, 0 or more.
        beta: f32,
    },
    /// Square root of the periodic Hann window. Used as both the analysis and
    /// synthesis window, the product is Hann.
    SqrtHann,
}

/// Zeroth-order modified Bessel function of the first kind, by its power series
/// `sum ((x/2)^k / k!)^2`. Converges for every finite `x`.
pub fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut k = 1.0;
    while term > sum * 1e-17 {
        term *= q / (k * k);
        sum += term;
        k += 1.0;
        if k > 500.0 {
            break;
        }
    }
    sum
}

impl WindowKind {
    /// Cosine-sum coefficients, if this is a cosine-sum window.
    fn cosine_sum(self) -> Option<&'static [f64]> {
        Some(match self {
            Self::Rectangular => &[1.0],
            Self::Hann => &[0.5, 0.5],
            Self::Hamming => &[0.54, 0.46],
            Self::Blackman => &[0.42, 0.5, 0.08],
            Self::BlackmanHarris => &[0.35875, 0.48829, 0.14128, 0.01168],
            Self::FlatTop => &[
                0.21557895,
                0.41663158,
                0.277263158,
                0.083578947,
                0.006947368,
            ],
            _ => return None,
        })
    }

    /// Value at position `n` of a window with `denominator` (`N` periodic, `N - 1` symmetric).
    fn value(self, n: usize, denominator: f64) -> f64 {
        let x = n as f64 / denominator;
        if let Some(a) = self.cosine_sum() {
            let mut sum = 0.0;
            let mut sign = 1.0;
            for (m, a) in a.iter().enumerate() {
                sum += sign * a * (2.0 * PI * m as f64 * x).cos();
                sign = -sign;
            }
            return sum;
        }
        match self {
            Self::Kaiser { beta } => {
                let beta = (beta as f64).max(0.0);
                let r = 2.0 * x - 1.0;
                bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(beta)
            }
            Self::SqrtHann => (0.5 - 0.5 * (2.0 * PI * x).cos()).max(0.0).sqrt(),
            _ => unreachable!(),
        }
    }

    /// Fill `out` with the window. Allocation free.
    pub fn fill(self, out: &mut [f32], periodic: bool) {
        let n = out.len();
        if n == 0 {
            return;
        }
        if n == 1 {
            out[0] = 1.0;
            return;
        }
        let denominator = if periodic { n as f64 } else { (n - 1) as f64 };
        for (i, v) in out.iter_mut().enumerate() {
            *v = self.value(i, denominator) as f32;
        }
    }

    /// The window as a new `Vec`. Allocates; call at prepare time.
    pub fn to_vec(self, len: usize, periodic: bool) -> Vec<f32> {
        let mut v = vec![0.0; len];
        self.fill(&mut v, periodic);
        v
    }
}

/// Coherent gain `sum(w) / N`.
pub fn coherent_gain(w: &[f32]) -> f64 {
    if w.is_empty() {
        return 0.0;
    }
    w.iter().map(|&v| v as f64).sum::<f64>() / w.len() as f64
}

/// Equivalent noise bandwidth in bins, `N sum(w^2) / sum(w)^2` (Harris 1978, eq. 11).
pub fn enbw_bins(w: &[f32]) -> f64 {
    let s: f64 = w.iter().map(|&v| v as f64).sum();
    let s2: f64 = w.iter().map(|&v| (v as f64) * (v as f64)).sum();
    if s == 0.0 {
        return 0.0;
    }
    w.len() as f64 * s2 / (s * s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(k: WindowKind) -> (f64, f64) {
        let w = k.to_vec(4096, true);
        (coherent_gain(&w), enbw_bins(&w))
    }

    #[test]
    fn bessel_i0_known_values() {
        assert!((bessel_i0(0.0) - 1.0).abs() < 1e-15);
        // Abramowitz & Stegun table 9.8.
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008).abs() < 1e-13);
        assert!((bessel_i0(5.0) - 27.239_871_823_604_44).abs() < 1e-10);
    }

    #[test]
    fn matches_harris_1978_table() {
        let (cg, e) = stats(WindowKind::Rectangular);
        assert!((cg - 1.0).abs() < 1e-9 && (e - 1.0).abs() < 1e-9);
        let (cg, e) = stats(WindowKind::Hann);
        assert!((cg - 0.5).abs() < 1e-6 && (e - 1.5).abs() < 1e-6);
        let (cg, e) = stats(WindowKind::Hamming);
        assert!((cg - 0.54).abs() < 1e-6 && (e - 1.3628).abs() < 5e-4);
        let (cg, e) = stats(WindowKind::Blackman);
        assert!((cg - 0.42).abs() < 1e-6 && (e - 1.7268).abs() < 5e-4);
        let (cg, e) = stats(WindowKind::BlackmanHarris);
        assert!((cg - 0.35875).abs() < 1e-6 && (e - 2.0044).abs() < 5e-4);
    }

    #[test]
    fn flat_top_gain_and_bandwidth() {
        let (cg, e) = stats(WindowKind::FlatTop);
        assert!((cg - 0.21557895).abs() < 1e-6);
        assert!((e - 3.77).abs() < 0.01, "enbw {e}");
    }

    #[test]
    fn kaiser_limits_and_gain() {
        let (cg, e) = stats(WindowKind::Kaiser { beta: 0.0 });
        assert!((cg - 1.0).abs() < 1e-6 && (e - 1.0).abs() < 1e-6);
        // Continuous-window mean is sinh(beta) / (beta I0(beta)).
        for beta in [2.0f64, 6.0, 9.0] {
            let (cg, _) = stats(WindowKind::Kaiser { beta: beta as f32 });
            let want = beta.sinh() / (beta * bessel_i0(beta));
            assert!((cg - want).abs() < 1e-4, "beta {beta}: {cg} vs {want}");
        }
        let (_, e4) = stats(WindowKind::Kaiser { beta: 4.0 });
        let (_, e9) = stats(WindowKind::Kaiser { beta: 9.0 });
        assert!(e4 < e9);
    }

    #[test]
    fn periodic_and_symmetric_forms() {
        let p = WindowKind::Hann.to_vec(8, true);
        let s = WindowKind::Hann.to_vec(8, false);
        assert!(p[0].abs() < 1e-7 && s[0].abs() < 1e-7 && s[7].abs() < 1e-7);
        assert!((p[4] - 1.0).abs() < 1e-6);
        for i in 0..8 {
            assert!((s[i] - s[7 - i]).abs() < 1e-6);
        }
        let sq = WindowKind::SqrtHann.to_vec(8, true);
        for i in 0..8 {
            assert!((sq[i] * sq[i] - p[i]).abs() < 1e-6);
        }
    }
}
