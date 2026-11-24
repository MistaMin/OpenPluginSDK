//! Analog-matched second-order filters (no cramping near Nyquist).
//!
//! After Martin Vicanek, "Matched Second Order Digital Filters" (2016). The RBJ
//! cookbook biquad ([`crate::Biquad`]) is a bilinear transform: the analog
//! response is squeezed into the band below Nyquist, so a high-frequency bell,
//! shelf or low-pass sits visibly off its analog prototype above about fs/4.
//! The matched design instead
//!
//! 1. puts the poles where impulse invariance puts them: with `zeta = 1/(2Q)`
//!    and `w = 2 pi f_p / fs`, `a1 = -2 e^(-zeta w) cos(w sqrt(1 - zeta^2))`
//!    (`cosh(w sqrt(zeta^2 - 1))` when `zeta > 1`), `a2 = e^(-2 zeta w)`; and
//! 2. chooses the numerator so the digital magnitude equals the analog one at
//!    DC, at `f0` and at Nyquist.
//!
//! The numerator is found from the squared magnitude, which for a real
//! second-order polynomial is `B0 phi0 + B1 phi1 + B2 phi2` with
//! `phi0 = 1 - s`, `phi1 = s`, `phi2 = 4 s (1 - s)`, `s = sin^2(w/2)`,
//! `B0 = (b0+b1+b2)^2`, `B1 = (b0-b1+b2)^2`, `B2 = -4 b0 b2` (and the same for
//! the denominator). Evaluating that at `w = 0`, `pi` and `w0` gives `B0`,
//! `B1`, `B2` directly; the `b`s follow from `b1 = (sqrt(B0) - sqrt(B1)) / 2`,
//! `b0 + b2 = (sqrt(B0) + sqrt(B1)) / 2`, `b0 b2 = -B2 / 4`. This module is
//! derived from the paper's idea and the maths above; the band-pass uses the
//! `b = [b0, 0, -b0]` form with only the gain at `f0` matched.
//!
//! Analog prototypes are the RBJ ones (the same `f0`, `Q`, `gain_db` meaning as
//! [`crate::Biquad`]), so a matched filter drops in where a `Biquad` was used.
//! If the numerator has no real factorisation (the radicand `S^2 + B2` would be
//! negative, which does not occur for the ranges in the tests) it is clamped to
//! zero and the match at `f0` degrades slightly.
//!
//! The filter runs in transposed direct form II with `f64` state. Changing
//! the coefficients every sample is allowed but, unlike [`crate::Svf`], is not
//! guaranteed bounded or smooth: use the SVF for fast modulation. As a last
//! resort an output beyond +-1e6 (+120 dB) resets the state and returns `0.0`.
//!
//! Non-finite input is treated as `0.0`; an output outside the `f32` range
//! resets the state and returns `0.0`; tiny values are flushed to zero.
//! [`MatchedBiquad::configure`] ignores NaN or infinite parameters.

use crate::rate::effective_rate;
use crate::svf::{finite_out, flush, sanitize_in};
use std::f64::consts::PI;

/// Filter shape; parameters mean the same as in [`crate::FilterType`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MatchedKind {
    /// Second-order low-pass with resonance `q`.
    LowPass,
    /// Second-order high-pass with resonance `q`.
    HighPass,
    /// Constant 0 dB peak gain at `f0`.
    BandPass,
    /// Notch at `f0` with width set by `q`.
    Notch,
    /// Bell of `gain_db` at `f0`.
    Peaking,
    /// Low shelf of `gain_db` with its corner at `f0`.
    LowShelf,
    /// High shelf of `gain_db` with its corner at `f0`.
    HighShelf,
}

/// Analog-matched biquad. See the [module documentation](self).
/// Plain state, no allocation; realtime-safe.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{MatchedBiquad, MatchedKind};
///
/// let mut eq = MatchedBiquad::new();
/// eq.configure(MatchedKind::Peaking, 48_000.0, 16_000.0, 1.0, 6.0);
/// // Matched to the analogue bell: about +6 dB at 16 kHz, no cramping.
/// let db = 20.0 * eq.magnitude_at(48_000.0, 16_000.0).log10();
/// assert!((db - 6.0).abs() < 0.5);
/// let y = eq.process_sample(0.25);
/// assert!(y.is_finite());
/// ```
#[derive(Debug, Clone)]
pub struct MatchedBiquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Default for MatchedBiquad {
    fn default() -> Self {
        Self::new()
    }
}

/// Output magnitude (+120 dB re full scale) above which the state is judged to
/// have run away and is reset. A time-varying direct-form filter is not
/// energy-bounded under arbitrary per-sample coefficient jumps; this keeps the
/// worst case finite and bounded. A static filter fed a sane signal never gets
/// near it.
const RUNAWAY: f32 = 1.0e6;

/// Pole frequency (rad/sample) above which impulse-invariant poles are replaced
/// by pre-warped bilinear ones.
const POLE_LIMIT: f64 = 0.99 * PI;

/// `|N(j w)|^2 / |D(j w)|^2` for `N = n[0] s^2 + n[1] s + n[2]`, in normalised units.
fn analog_power(n: [f64; 3], d: [f64; 3], w: f64) -> f64 {
    let w2 = w * w;
    let nr = n[2] - n[0] * w2;
    let ni = n[1] * w;
    let dr = d[2] - d[0] * w2;
    let di = d[1] * w;
    (nr * nr + ni * ni) / (dr * dr + di * di)
}

impl MatchedBiquad {
    /// A pass-through filter until configured.
    pub fn new() -> Self {
        Self {
            b: [1.0, 0.0, 0.0],
            a: [0.0, 0.0],
            z: [0.0; 2],
        }
    }

    /// Clears the state; keeps the coefficients.
    pub fn reset(&mut self) {
        self.z = [0.0; 2];
    }

    /// Feed-forward `[b0, b1, b2]` and feedback `[a1, a2]` coefficients
    /// (`a0 = 1`), for inspection and for evaluating the response.
    pub fn coefficients(&self) -> ([f64; 3], [f64; 2]) {
        (self.b, self.a)
    }

    /// Computes the coefficients. `freq` is clamped to 1 Hz..0.49 fs, `q` to
    /// 0.01..1000, `gain_db` to +-40 dB; `sample_rate` goes through
    /// [`effective_rate`]. Shelf gains large enough to push the shelf's own
    /// pole above Nyquist (a high shelf near fs/2 with a big boost) are
    /// accepted but less accurate. NaN or infinite parameters leave the filter
    /// unchanged.
    pub fn configure(&mut self, kind: MatchedKind, sample_rate: f32, freq: f32, q: f32, gain_db: f32) {
        if !(freq.is_finite() && q.is_finite() && gain_db.is_finite()) {
            return;
        }
        let fs = f64::from(effective_rate(sample_rate));
        let f0 = f64::from(freq).clamp(1.0, 0.49 * fs);
        let q = f64::from(q).clamp(0.01, 1000.0);
        let a = 10f64.powf(f64::from(gain_db).clamp(-40.0, 40.0) / 40.0);
        let sa = a.sqrt();
        let iq = 1.0 / q;

        // Analog prototype in s normalised to 2 pi f0: numerator and denominator
        // as [s^2, s^1, s^0] coefficients.
        let (n, d): ([f64; 3], [f64; 3]) = match kind {
            MatchedKind::LowPass => ([0.0, 0.0, 1.0], [1.0, iq, 1.0]),
            MatchedKind::HighPass => ([1.0, 0.0, 0.0], [1.0, iq, 1.0]),
            MatchedKind::BandPass => ([0.0, iq, 0.0], [1.0, iq, 1.0]),
            MatchedKind::Notch => ([1.0, 0.0, 1.0], [1.0, iq, 1.0]),
            MatchedKind::Peaking => ([1.0, a * iq, 1.0], [1.0, iq / a, 1.0]),
            MatchedKind::LowShelf => ([a, a * sa * iq, a * a], [a, sa * iq, 1.0]),
            MatchedKind::HighShelf => ([a * a, a * sa * iq, a], [1.0, sa * iq, a]),
        };

        // Poles by impulse invariance.
        let wp = (d[2] / d[0]).sqrt(); // pole frequency relative to f0
        let zeta = d[1] / (2.0 * (d[0] * d[2]).sqrt());
        let w0 = 2.0 * PI * f0 / fs; // rad/sample at f0
        let wpt = wp * w0;
        let (a1, a2) = if wpt <= POLE_LIMIT {
            let e = (-zeta * wpt).exp();
            let a1 = if zeta < 1.0 {
                -2.0 * e * (wpt * (1.0 - zeta * zeta).sqrt()).cos()
            } else {
                -2.0 * e * (wpt * (zeta * zeta - 1.0).sqrt()).cosh()
            };
            (a1, e * e)
        } else {
            // A pole this close to (or beyond) Nyquist would alias under
            // impulse invariance; use the bilinear pole pair pre-warped to the
            // pole frequency instead (stable, exact at the pole frequency).
            let g = (0.5 * wpt.min(3.0)).tan();
            let a0 = 1.0 + 2.0 * zeta * g + g * g;
            (2.0 * (g * g - 1.0) / a0, (1.0 - 2.0 * zeta * g + g * g) / a0)
        };

        // Digital denominator power at phi weights.
        let big_a0 = (1.0 + a1 + a2).powi(2);
        let big_a1 = (1.0 - a1 + a2).powi(2);
        let big_a2 = -4.0 * a2;
        let s = (0.5 * w0).sin().powi(2);
        let (p0, p1, p2) = (1.0 - s, s, 4.0 * s * (1.0 - s));
        let den_mid = big_a0 * p0 + big_a1 * p1 + big_a2 * p2;

        let (big_b0, big_b1, big_b2) = if kind == MatchedKind::BandPass {
            // b = [b0, 0, -b0]: |b|^2 = B2 phi2; unit gain at f0.
            (0.0, 0.0, analog_power(n, d, 1.0) * den_mid / p2)
        } else {
            let t_dc = analog_power(n, d, 0.0);
            let t_ny = analog_power(n, d, fs * 0.5 / f0);
            let t_mid = analog_power(n, d, 1.0);
            let b0_ = t_dc * big_a0;
            let b1_ = t_ny * big_a1;
            let b2_ = (t_mid * den_mid - b0_ * p0 - b1_ * p1) / p2;
            (b0_, b1_, b2_)
        };

        let w0s = big_b0.sqrt();
        let w1s = big_b1.sqrt();
        let sum = 0.5 * (w0s + w1s);
        let b1 = 0.5 * (w0s - w1s);
        let rad = (sum * sum + big_b2).max(0.0).sqrt();
        let (b0, b2) = (0.5 * (sum + rad), 0.5 * (sum - rad));
        let (b0, b1, b2) = match kind {
            MatchedKind::BandPass => {
                let g = (0.25 * big_b2).max(0.0).sqrt();
                (g, 0.0, -g)
            }
            MatchedKind::Notch => {
                // Zeros exactly on the unit circle at f0 (the squared magnitude
                // touches zero there, so the three-point fit would not factor);
                // the gain is matched at DC.
                let c = w0.cos();
                let k = (1.0 + a1 + a2) / (2.0 - 2.0 * c);
                (k, -2.0 * k * c, k)
            }
            _ => (b0, b1, b2),
        };
        if [b0, b1, b2, a1, a2].iter().all(|v| v.is_finite()) {
            self.b = [b0, b1, b2];
            self.a = [a1, a2];
        }
    }

    /// Filters one sample (transposed direct form II).
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let x = sanitize_in(x);
        let y = self.b[0] * x + self.z[0];
        let z0 = self.b[1] * x - self.a[0] * y + self.z[1];
        let z1 = self.b[2] * x - self.a[1] * y;
        match finite_out(y).filter(|v| v.abs() <= RUNAWAY) {
            Some(out) => {
                self.z = [flush(z0), flush(z1)];
                out
            }
            None => {
                self.reset();
                0.0
            }
        }
    }

    /// Magnitude of the configured digital response at `freq` Hz for a filter
    /// configured at `sample_rate`. Evaluates the coefficients, not the state.
    pub fn magnitude_at(&self, sample_rate: f32, freq: f32) -> f64 {
        let fs = f64::from(effective_rate(sample_rate));
        let w = 2.0 * PI * f64::from(freq) / fs;
        let s = (0.5 * w).sin().powi(2);
        let (p0, p1, p2) = (1.0 - s, s, 4.0 * s * (1.0 - s));
        let pw = |c: [f64; 3]| {
            (c[0] + c[1] + c[2]).powi(2) * p0
                + (c[0] - c[1] + c[2]).powi(2) * p1
                - 4.0 * c[0] * c[2] * p2
        };
        let num = pw(self.b);
        let den = pw([1.0, self.a[0], self.a[1]]);
        (num / den).max(0.0).sqrt()
    }
}
