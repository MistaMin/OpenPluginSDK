//! Fractional-delay interpolators: linear, Hermite, Lagrange and Thiran allpass.
//!
//! A delay of `d` samples that is not an integer needs a value of the signal
//! between samples. The choices here are the ones surveyed in
//! T. I. Laakso, V. Valimaki, M. Karjalainen and U. K. Laine, "Splitting the
//! Unit Delay: Tools for Fractional Delay Filter Design", IEEE Signal
//! Processing Magazine 13(1), 30-60, January 1996 (Laakso et al. 1996):
//!
//! - **Lagrange** (section on maximally flat FIR design). The `N + 1` taps
//!   `h(k) = prod_{m != k} (D - m) / (k - m)` make the response match the ideal
//!   `e^{-j w D}` to order `N` at DC. The taps are used on the central interval
//!   of the `N + 1` nodes, where the error is smallest. Order 1 is linear
//!   interpolation. The magnitude falls towards Nyquist (a low-pass that
//!   deepens as the fraction nears one half), and the delay is accurate at low
//!   frequency only.
//! - **Hermite**, the four-point cubic with central-difference tangents
//!   (the Catmull-Rom spline; see also O. Niemitalo, "Polynomial interpolators
//!   for high-quality resampling of oversampled audio", 2001). It has the same
//!   cost as third-order Lagrange but a continuous first derivative, which is
//!   kind to a delay that is swept. Its taps are linear interpolation followed
//!   by a real, symmetric three-tap correction `[c, 1 - 2c, c]` with
//!   `c = -t (1 - t) / 2`, so its phase and group delay are exactly those of
//!   linear interpolation (accurate at low frequency only) while its magnitude
//!   is far flatter (the tests print both). Choose Lagrange or Thiran when the
//!   delay itself must be accurate at high frequency.
//! - **Thiran allpass** (section on IIR design, after J.-P. Thiran, "Recursive
//!   digital filters with maximally flat group delay", IEEE Trans. Circuit
//!   Theory 18(6), 1971). An order `N` allpass
//!   `H(z) = z^-N A(1/z) / A(z)`, `a_k = (-1)^k C(N, k) prod_{n=0}^{N}
//!   (D - N + n) / (D - N + k + n)`, has exactly flat magnitude and a group
//!   delay equal to `D` at DC with the first `N - 1` derivatives zero. It is
//!   stable when `D > N - 1`; this module keeps `D` in `[N - 0.5, N + 0.5)`, where
//!   Laakso et al. recommend it. It is recursive, so it suits fixed or slowly
//!   changing delays (waveguide tuning); a delay that jumps by an integer
//!   restarts a transient in its state.
//!
//! Delay is counted so that 0 is the most recently written sample. Each kind
//! has a smallest delay it can honour ([`Interpolation::min_delay`]): the
//! taps may not reach into the future.

use std::f64::consts::PI;

/// Largest Lagrange order.
pub const MAX_LAGRANGE_ORDER: u8 = 8;
/// Largest Thiran order.
pub const MAX_THIRAN_ORDER: u8 = 4;

/// Interpolator for a fractional delay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    /// Two-point linear interpolation (Lagrange order 1).
    Linear,
    /// Four-point, third-order Hermite (Catmull-Rom).
    Hermite,
    /// Lagrange interpolation of the given order (clamped to 1..=8).
    Lagrange(u8),
    /// Thiran allpass of the given order (clamped to 1..=4).
    Thiran(u8),
}

/// FIR taps: `w[k]` multiplies the sample at integer delay `start + k`.
pub(crate) struct Taps {
    pub start: usize,
    pub len: usize,
    pub w: [f64; MAX_LAGRANGE_ORDER as usize + 1],
}

impl Interpolation {
    fn lagrange_order(self) -> Option<usize> {
        match self {
            Interpolation::Linear => Some(1),
            Interpolation::Lagrange(n) => Some(usize::from(n.clamp(1, MAX_LAGRANGE_ORDER))),
            _ => None,
        }
    }

    pub(crate) fn thiran_order(self) -> Option<usize> {
        match self {
            Interpolation::Thiran(n) => Some(usize::from(n.clamp(1, MAX_THIRAN_ORDER))),
            _ => None,
        }
    }

    /// Smallest delay, in samples, the interpolator can produce.
    ///
    /// Linear 0, Hermite 1, Lagrange `max(N / 2 - 0.5, 0)`, Thiran `N - 0.5`.
    pub fn min_delay(self) -> f32 {
        if let Some(n) = self.lagrange_order() {
            (n as f32 * 0.5 - 0.5).max(0.0)
        } else if let Some(n) = self.thiran_order() {
            n as f32 - 0.5
        } else {
            1.0
        }
    }

    /// FIR weights for delay `d` (already clamped to `>= min_delay`).
    /// `None` for the recursive Thiran type.
    #[allow(clippy::needless_range_loop)]
    pub(crate) fn taps(self, d: f64) -> Option<Taps> {
        let mut w = [0.0; MAX_LAGRANGE_ORDER as usize + 1];
        if let Some(n) = self.lagrange_order() {
            let start = (d - n as f64 * 0.5 + 0.5).floor().max(0.0);
            let x = d - start;
            for k in 0..=n {
                let mut h = 1.0;
                for m in 0..=n {
                    if m != k {
                        h *= (x - m as f64) / (k as f64 - m as f64);
                    }
                }
                w[k] = h;
            }
            Some(Taps { start: start as usize, len: n + 1, w })
        } else if self.thiran_order().is_none() {
            let i = d.floor().max(1.0);
            let t = d - i;
            let t2 = t * t;
            let t3 = t2 * t;
            w[0] = -0.5 * t3 + t2 - 0.5 * t;
            w[1] = 1.5 * t3 - 2.5 * t2 + 1.0;
            w[2] = -1.5 * t3 + 2.0 * t2 + 0.5 * t;
            w[3] = 0.5 * t3 - 0.5 * t2;
            Some(Taps { start: i as usize - 1, len: 4, w })
        } else {
            None
        }
    }

    /// Complex frequency response `(re, im)` of the whole delay `d` at angular
    /// frequency `omega` (radians per sample), from the filter coefficients.
    /// Its phase is `-omega * d` for an ideal fractional delay.
    #[allow(clippy::needless_range_loop)]
    pub fn frequency_response(self, delay: f32, omega: f32) -> (f64, f64) {
        let d = f64::from(delay).max(f64::from(self.min_delay()));
        let w = f64::from(omega);
        if let Some(taps) = self.taps(d) {
            let (mut re, mut im) = (0.0, 0.0);
            for k in 0..taps.len {
                let ph = -w * (taps.start + k) as f64;
                re += taps.w[k] * ph.cos();
                im += taps.w[k] * ph.sin();
            }
            (re, im)
        } else {
            let n = self.thiran_order().unwrap_or(1);
            let (m, df) = thiran_split(d, n);
            let a = thiran_coeffs(df, n);
            // H(e^jw) = (a_N + ... + z^-N) / (1 + a_1 z^-1 + ...), z^-k = e^{-jwk}.
            let (mut nr, mut ni, mut dr, mut di) = (0.0, 0.0, 0.0, 0.0);
            for k in 0..=n {
                let ph = -w * k as f64;
                dr += a[k] * ph.cos();
                di += a[k] * ph.sin();
                let b = a[n - k];
                nr += b * ph.cos();
                ni += b * ph.sin();
            }
            let den = dr * dr + di * di;
            let (hr, hi) = ((nr * dr + ni * di) / den, (ni * dr - nr * di) / den);
            let ph = -w * m as f64;
            (hr * ph.cos() - hi * ph.sin(), hr * ph.sin() + hi * ph.cos())
        }
    }
}

/// Integer part `M` and allpass delay `D` in `[N - 0.5, N + 0.5)` with `M + D = d`.
pub(crate) fn thiran_split(d: f64, n: usize) -> (usize, f64) {
    let m = (d - n as f64 + 0.5).floor().max(0.0);
    (m as usize, d - m)
}

/// Thiran coefficients `a_0..=a_N` (`a_0 = 1`) for allpass delay `D`
/// (Laakso et al. 1996, eq. 74). `a[k] = (-1)^k C(N,k) prod_n (D-N+n)/(D-N+k+n)`.
#[allow(clippy::needless_range_loop)]
pub(crate) fn thiran_coeffs(dd: f64, n: usize) -> [f64; MAX_THIRAN_ORDER as usize + 1] {
    let mut a = [0.0; MAX_THIRAN_ORDER as usize + 1];
    a[0] = 1.0;
    let mut binom = 1.0;
    for k in 1..=n {
        binom = binom * (n - k + 1) as f64 / k as f64;
        let mut prod = 1.0;
        for i in 0..=n {
            let top = dd - n as f64 + i as f64;
            let bottom = dd - n as f64 + k as f64 + i as f64;
            prod *= top / bottom;
        }
        let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
        a[k] = sign * binom * prod;
    }
    a
}

/// Order-`N` Thiran allpass in transposed direct form II, `f64` state.
#[derive(Clone, Debug)]
pub(crate) struct ThiranState {
    pub n: usize,
    pub a: [f64; MAX_THIRAN_ORDER as usize + 1],
    s: [f64; MAX_THIRAN_ORDER as usize],
    last_d: f64,
}

impl ThiranState {
    pub fn new() -> Self {
        let mut t = Self {
            n: 1,
            a: [0.0; MAX_THIRAN_ORDER as usize + 1],
            s: [0.0; MAX_THIRAN_ORDER as usize],
            last_d: f64::NAN,
        };
        t.set(1, 0.9);
        t
    }

    pub fn reset(&mut self) {
        self.s = [0.0; MAX_THIRAN_ORDER as usize];
    }

    /// Order and allpass delay `dd` (recomputes only when something changed).
    pub fn set(&mut self, n: usize, dd: f64) {
        if n != self.n || dd.to_bits() != self.last_d.to_bits() {
            if n != self.n {
                self.reset();
            }
            self.n = n;
            self.a = thiran_coeffs(dd, n);
            self.last_d = dd;
        }
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let n = self.n;
        let y = self.a[n] * x + self.s[0];
        for i in 0..n {
            let next = if i + 1 < n { self.s[i + 1] } else { 0.0 };
            // b_{i+1} = a_{N-i-1}
            self.s[i] = self.a[n - i - 1] * x + next - self.a[i + 1] * y;
            if self.s[i].abs() < 1e-30 {
                self.s[i] = 0.0;
            }
        }
        if y.is_finite() && y.abs() < 1e30 {
            y
        } else {
            self.reset();
            0.0
        }
    }
}

/// Phase `-arg H` divided by `omega`: the phase delay in samples.
pub fn phase_delay(interp: Interpolation, delay: f32, omega: f32) -> f64 {
    let (re, im) = interp.frequency_response(delay, omega);
    let w = f64::from(omega);
    // The delay can exceed one cycle of phase, so unwrap around the ideal.
    let ideal = -w * f64::from(delay);
    let mut ph = im.atan2(re);
    ph += (2.0 * PI) * ((ideal - ph) / (2.0 * PI)).round();
    -ph / w
}

/// Group delay in samples, `-d arg H / d omega`, by a central difference of the
/// unwrapped phase over a step of `1e-4` rad/sample.
pub fn group_delay(interp: Interpolation, delay: f32, omega: f32) -> f64 {
    let h = 1e-4;
    let w = f64::from(omega);
    let p = |x: f64| {
        let (re, im) = interp.frequency_response(delay, x as f32);
        im.atan2(re)
    };
    let mut d = p(w + h) - p(w - h);
    d -= 2.0 * PI * (d / (2.0 * PI)).round();
    -d / (2.0 * h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thiran_order_one_matches_closed_form() {
        // a1 = (1 - D) / (1 + D)
        let a = thiran_coeffs(0.8, 1);
        assert!((a[1] - (1.0 - 0.8) / (1.0 + 0.8)).abs() < 1e-14);
    }

    #[test]
    fn lagrange_and_hermite_weights_sum_to_one() {
        for interp in [
            Interpolation::Linear,
            Interpolation::Hermite,
            Interpolation::Lagrange(3),
            Interpolation::Lagrange(5),
        ] {
            for i in 0..20 {
                let d = 4.0 + f64::from(i) * 0.05;
                let t = interp.taps(d).unwrap();
                let sum: f64 = t.w[..t.len].iter().sum();
                assert!((sum - 1.0).abs() < 1e-12, "{interp:?} {d}");
            }
        }
    }

    #[test]
    fn integer_delay_is_a_pure_delay() {
        for interp in [Interpolation::Hermite, Interpolation::Lagrange(3), Interpolation::Linear] {
            let t = interp.taps(5.0).unwrap();
            let hot: Vec<usize> = (0..t.len).filter(|k| t.w[*k].abs() > 1e-12).collect();
            assert_eq!(hot.len(), 1);
            assert_eq!(t.start + hot[0], 5);
        }
    }
}
