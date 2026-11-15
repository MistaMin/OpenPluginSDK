//! Topology-preserving-transform (TPT) state-variable filter.
//!
//! The structure is the zero-delay-feedback trapezoidal-integrator SVF of
//! Vadim Zavalishin, "The Art of VA Filter Design" (Native Instruments, 2012,
//! rev. 2.1+), chapter 4, in the state/coefficient arrangement of Andrew Simper,
//! "Linear Trapezoidal Integrated State Variable Filter with Low Noise
//! Optimisation" (Cytomic technical paper, 2013). The equations here were
//! re-derived from those papers; no third-party code was consulted.
//!
//! Two integrators hold the state (`ic1eq`, `ic2eq`). With `g = tan(pi f / fs)`
//! and `k = 1 / Q`:
//!
//! ```text
//! a1 = 1 / (1 + g (g + k)),  a2 = g a1,  a3 = g a2
//! v3 = x - ic2;  v1 = a1 ic1 + a2 v3;  v2 = ic2 + a2 ic1 + a3 v3
//! ic1 = 2 v1 - ic1;  ic2 = 2 v2 - ic2
//! y = m0 x + m1 v1 + m2 v2
//! ```
//!
//! `v1` is the band-pass (peak gain `1 / k`) and `v2` the low-pass; the
//! high-pass is `x - k v1 - v2`. The modes differ only in the mix `m0, m1, m2`
//! (and for the shelves in `g`). The filter is the bilinear transform of the
//! analog prototype, pre-warped so the analog frequency `f` lands exactly on the
//! digital `f`; its magnitude at digital frequency `w` therefore equals the
//! analog prototype at `tan(w/2) / g`. It cramps toward Nyquist like the RBJ
//! biquad (use [`crate::MatchedBiquad`] when that matters).
//!
//! Unlike a direct-form biquad, the coefficients may change on every sample:
//! the state variables are the integrator outputs (energy-like quantities), so
//! a jump in cutoff or Q does not inject the transient a direct-form state does
//! and the filter stays stable for any `g > 0`, `k > 0`.
//!
//! # Real-time and error behaviour
//!
//! No allocation. The state is `f64`. A non-finite input sample is treated as
//! `0.0` (so NaN or infinity never reaches the state); a result outside the
//! `f32` range (only possible from absurdly large finite input) resets the
//! state and outputs `0.0`. State and output below `1e-30` are flushed to zero
//! (denormal protection). [`Svf::configure`] ignores a call whose frequency, Q
//! or gain is NaN or infinite and keeps the previous coefficients.

use crate::rate::effective_rate;
use std::f64::consts::PI;

/// Magnitudes below this are flushed to zero (about -600 dB).
pub(crate) const FLUSH: f64 = 1e-30;

#[inline]
pub(crate) fn flush(v: f64) -> f64 {
    if v.abs() < FLUSH {
        0.0
    } else {
        v
    }
}

#[inline]
pub(crate) fn sanitize_in(x: f32) -> f64 {
    if x.is_finite() {
        f64::from(x)
    } else {
        0.0
    }
}

/// `Some(y as f32)` when `y` is finite and fits in an `f32`.
#[inline]
pub(crate) fn finite_out(y: f64) -> Option<f32> {
    let y = flush(y);
    if y.abs() <= f64::from(f32::MAX) {
        Some(y as f32)
    } else {
        None
    }
}

/// Response of an [`Svf`]. `f` is the corner or centre frequency, `Q` the
/// quality, `A = 10^(gain_db / 40)`. Transfer functions use `s` normalised to
/// `2 pi f`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SvfMode {
    /// `1 / (s^2 + s/Q + 1)`.
    LowPass,
    /// `s^2 / (s^2 + s/Q + 1)`.
    HighPass,
    /// `(s/Q) / (s^2 + s/Q + 1)`; gain 1 at `f`.
    BandPass,
    /// `(s^2 + 1) / (s^2 + s/Q + 1)`.
    Notch,
    /// `(s^2 - 1) / (s^2 + s/Q + 1)` (high-pass minus low-pass); magnitude `2Q` at `f`.
    Peak,
    /// `(s^2 - s/Q + 1) / (s^2 + s/Q + 1)`.
    AllPass,
    /// RBJ-form bell, `(s^2 + s A/Q + 1) / (s^2 + s/(A Q) + 1)`; `gain_db` at `f`.
    Bell,
    /// Low shelf, `A (s^2 + s sqrt(A)/Q + A) / (A s^2 + s sqrt(A)/Q + 1)`; `gain_db` below `f`.
    LowShelf,
    /// High shelf, `A (A s^2 + s sqrt(A)/Q + 1) / (s^2 + s sqrt(A)/Q + A)`; `gain_db` above `f`.
    HighShelf,
}

/// TPT state-variable filter. See the [module documentation](self).
#[derive(Debug, Clone)]
pub struct Svf {
    a1: f64,
    a2: f64,
    a3: f64,
    k: f64,
    m0: f64,
    m1: f64,
    m2: f64,
    ic1: f64,
    ic2: f64,
}

impl Default for Svf {
    fn default() -> Self {
        Self::new()
    }
}

impl Svf {
    /// A pass-through filter until [`Svf::configure`] is called.
    pub fn new() -> Self {
        Self {
            a1: 1.0,
            a2: 0.0,
            a3: 0.0,
            k: 1.0,
            m0: 1.0,
            m1: 0.0,
            m2: 0.0,
            ic1: 0.0,
            ic2: 0.0,
        }
    }

    /// Clears the state; keeps the coefficients.
    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    /// Sets the response. Cheap enough to call every sample (one `tan`, plus a
    /// `powf` for the gain modes). `sample_rate` goes through
    /// [`effective_rate`]; `freq` is clamped to 1 Hz..0.499 fs, `q` to
    /// 0.01..1000 and `gain_db` to +-60 dB (used only by the bell and shelves).
    /// A NaN or infinite `freq`, `q` or `gain_db` leaves the filter unchanged.
    pub fn configure(&mut self, mode: SvfMode, sample_rate: f32, freq: f32, q: f32, gain_db: f32) {
        if !(freq.is_finite() && q.is_finite() && gain_db.is_finite()) {
            return;
        }
        let fs = f64::from(effective_rate(sample_rate));
        let f = f64::from(freq).clamp(1.0, 0.499 * fs);
        let q = f64::from(q).clamp(0.01, 1000.0);
        let g0 = (PI * f / fs).tan();
        let k0 = 1.0 / q;
        let amp = |db: f32| 10f64.powf(f64::from(db).clamp(-60.0, 60.0) / 40.0);
        let (g, k, m0, m1, m2) = match mode {
            SvfMode::LowPass => (g0, k0, 0.0, 0.0, 1.0),
            SvfMode::HighPass => (g0, k0, 1.0, -k0, -1.0),
            SvfMode::BandPass => (g0, k0, 0.0, k0, 0.0),
            SvfMode::Notch => (g0, k0, 1.0, -k0, 0.0),
            SvfMode::Peak => (g0, k0, 1.0, -k0, -2.0),
            SvfMode::AllPass => (g0, k0, 1.0, -2.0 * k0, 0.0),
            SvfMode::Bell => {
                let a = amp(gain_db);
                let k = k0 / a;
                (g0, k, 1.0, k * (a * a - 1.0), 0.0)
            }
            SvfMode::LowShelf => {
                let a = amp(gain_db);
                (g0 / a.sqrt(), k0, 1.0, k0 * (a - 1.0), a * a - 1.0)
            }
            SvfMode::HighShelf => {
                let a = amp(gain_db);
                (g0 * a.sqrt(), k0, a * a, k0 * (1.0 - a) * a, 1.0 - a * a)
            }
        };
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
        self.k = k;
        self.m0 = m0;
        self.m1 = m1;
        self.m2 = m2;
    }

    /// One integrator step. Returns `(v1, v2)`: band-pass (peak gain `1/k`) and
    /// low-pass. `x` must be finite.
    #[inline]
    fn step(&mut self, x: f64) -> (f64, f64) {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = flush(2.0 * v1 - self.ic1);
        self.ic2 = flush(2.0 * v2 - self.ic2);
        (v1, v2)
    }

    /// Filters one sample with the configured mode.
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let x = sanitize_in(x);
        let (v1, v2) = self.step(x);
        match finite_out(self.m0 * x + self.m1 * v1 + self.m2 * v2) {
            Some(y) => y,
            None => {
                self.reset();
                0.0
            }
        }
    }

    /// Filters one sample and returns the three basic outputs at the configured
    /// cutoff and Q regardless of the mode: `(low_pass, band_pass, high_pass)`,
    /// with the band-pass normalised to unit peak gain. They satisfy
    /// `low + band + high == x` exactly (up to rounding).
    #[inline]
    pub fn process_split(&mut self, x: f32) -> (f32, f32, f32) {
        let x = sanitize_in(x);
        let (v1, v2) = self.step(x);
        let hp = x - self.k * v1 - v2;
        match (finite_out(v2), finite_out(self.k * v1), finite_out(hp)) {
            (Some(l), Some(b), Some(h)) => (l, b, h),
            _ => {
                self.reset();
                (0.0, 0.0, 0.0)
            }
        }
    }
}
