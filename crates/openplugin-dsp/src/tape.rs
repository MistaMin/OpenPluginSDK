//! Tape saturation by Jiles-Atherton hysteresis, after Jatin Chowdhury,
//! "Real-time Physical Modelling for Analog Tape Machines", DAFx-19 (2019).
//!
//! Chowdhury models the magnetic tape with the Jiles-Atherton (JA) hysteresis
//! model (D. C. Jiles, D. L. Atherton, "Theory of ferromagnetic hysteresis",
//! J. Magn. Magn. Mater. 61, 1986) and integrates it per sample with a
//! Runge-Kutta scheme at an oversampled rate. This module is written from the
//! JA equations; no code from the ChowDSP projects was consulted. We did not
//! check the paper's exact parameter values, so the parameter set is our own
//! (below). The head-gap, spacing and playback-loss filters of the paper are
//! not included.
//!
//! # Model (normalised)
//!
//! With `h = H / a` (field over the domain-wall scale `a`) and `m = M / Ms`
//! (magnetisation over saturation):
//!
//! ```text
//! he    = h + alpha m                       (effective field)
//! m_an  = L(he),  L(x) = coth x - 1/x       (Langevin anhysteretic curve)
//! dm/dh = S / (1 - alpha S)
//! S     = dm * |m_an - m| / k  +  c L'(he)  (irreversible + reversible part)
//! ```
//!
//! `dm` is 1 when the field moves towards the anhysteretic curve and 0 otherwise
//! (so the susceptibility is never negative, as in the paper's `delta_M`).
//! This follows from `M = (1-c) M_irr + c M_an`, `dM_irr/dHe = dm (M_an - M_irr) /
//! (k delta)` and `dHe = dH + alpha dM`; we re-derived the closed form, using
//! `M_irr = (M - c M_an)/(1-c)` so only `M` is a state. `L'(x) = 1/x^2 - 1/sinh^2 x`.
//!
//! The equation is rate independent: it is an ODE in `h`. Over one sample the
//! input field moves linearly from `h[n-1]` to `h[n]` and `m` is advanced by the
//! classical fourth-order Runge-Kutta method in the variable `h` with
//! `ceil(|dh| / 0.25)` substeps (1 to 32). There is no latency. Because the
//! shape of the curve is sharp, the loop is better resolved at a higher rate:
//! run the whole model at 2x or 4x with [`crate::LinearPhaseOversampler`]
//! (see `docs/DSP_COOKBOOK.md`) to cut aliasing.
//!
//! # Parameters
//!
//! * `drive`: linear gain on the input before the field, `0 ..= 32`.
//! * `saturation`, `0 ..= 1`: field scale `h = 6 saturation' x`, between
//!   0.5 and 6 (more saturation reaches the knee at lower input levels).
//! * `width`, `0 ..= 1`: pinning `k = 0.3 + 1.7 width`; wider loops dissipate more
//!   energy per cycle.
//!
//! Fixed: `alpha = 0.1`, `c = 0.17` (reversibility; after the typical tape fit),
//! `a = 1`. `alpha S < 1` is guaranteed for all states (|m| is limited to 1.2) and
//! the denominator is additionally floored at 0.05, so the derivative is finite.
//! The output is `m` (range about +-1).
//!
//! # Real-time and error behaviour
//!
//! No allocation, f64 state, non-finite input treated as 0, a non-finite state
//! resets the model, magnetisation flushed below `1e-30`. The input field is
//! clamped to +-200. An optional one-pole DC blocker (default off) uses the real
//! sample rate.

use crate::rate::effective_rate;
use crate::svf::{finite_out, flush, sanitize_in};
use std::f64::consts::PI;

const ALPHA: f64 = 0.1;
const C_REV: f64 = 0.17;
const MAX_SUBSTEPS: usize = 32;
const SUBSTEP_H: f64 = 0.25;
const M_LIMIT: f64 = 1.2;

/// Langevin function `coth x - 1/x`.
#[inline]
fn langevin(x: f64) -> f64 {
    let ax = x.abs();
    if ax < 1e-3 {
        x / 3.0 - x * x * x / 45.0
    } else if ax > 20.0 {
        x.signum() - 1.0 / x
    } else {
        let e = (-2.0 * ax).exp();
        x.signum() * ((1.0 + e) / (1.0 - e) - 1.0 / ax)
    }
}

/// Derivative of the Langevin function.
#[inline]
fn langevin_d(x: f64) -> f64 {
    let ax = x.abs();
    if ax < 1e-2 {
        1.0 / 3.0 - x * x / 15.0
    } else if ax > 20.0 {
        1.0 / (x * x)
    } else {
        let s = ax.sinh();
        1.0 / (ax * ax) - 1.0 / (s * s)
    }
}

/// Jiles-Atherton tape hysteresis. See the [module documentation](self).
#[derive(Debug, Clone)]
pub struct TapeHysteresis {
    sample_rate: f64,
    drive: f64,
    saturation: f64,
    width: f64,
    k: f64,
    m: f64,
    h_prev: f64,
    dc_hz: f64,
    dc_a: f64,
    dc_x: f64,
    dc_y: f64,
}

impl TapeHysteresis {
    /// A model running at `sample_rate` (give the oversampled rate when oversampling).
    /// Defaults: drive 1, saturation 0.5, width 0.5, DC blocker off.
    pub fn new(sample_rate: f32) -> Self {
        let mut t = Self {
            sample_rate: f64::from(effective_rate(sample_rate)),
            drive: 1.0,
            saturation: 0.5,
            width: 0.5,
            k: 0.0,
            m: 0.0,
            h_prev: 0.0,
            dc_hz: 0.0,
            dc_a: 0.0,
            dc_x: 0.0,
            dc_y: 0.0,
        };
        t.update();
        t
    }

    fn update(&mut self) {
        self.k = 0.3 + 1.7 * self.width;
        self.dc_a = if self.dc_hz > 0.0 {
            (-2.0 * PI * self.dc_hz / self.sample_rate).exp()
        } else {
            0.0
        };
    }

    /// Sets the (possibly oversampled) rate.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = f64::from(effective_rate(sample_rate));
        self.update();
    }

    /// Input gain, `0 ..= 32`.
    pub fn set_drive(&mut self, drive: f32) {
        if drive.is_finite() {
            self.drive = f64::from(drive.clamp(0.0, 32.0));
        }
    }

    /// Saturation amount, `0 ..= 1`.
    pub fn set_saturation(&mut self, saturation: f32) {
        if saturation.is_finite() {
            self.saturation = f64::from(saturation.clamp(0.0, 1.0));
        }
    }

    /// Hysteresis width, `0 ..= 1`.
    pub fn set_width(&mut self, width: f32) {
        if width.is_finite() {
            self.width = f64::from(width.clamp(0.0, 1.0));
            self.update();
        }
    }

    /// Output DC blocker corner in Hz (0 disables; at most 100 Hz).
    pub fn set_dc_block(&mut self, hz: f32) {
        if hz.is_finite() {
            self.dc_hz = f64::from(hz.clamp(0.0, 100.0));
            self.update();
        }
    }

    /// Clears the magnetisation and the field memory.
    pub fn reset(&mut self) {
        self.m = 0.0;
        self.h_prev = 0.0;
        self.dc_x = 0.0;
        self.dc_y = 0.0;
    }

    /// `dm/dh` at `(m, h)` moving in direction `delta` (+1 or -1).
    #[inline]
    fn slope(&self, m: f64, h: f64, delta: f64) -> f64 {
        let he = h + ALPHA * m;
        let man = langevin(he);
        let d = man - m;
        let irr = if d * delta > 0.0 { d.abs() / self.k } else { 0.0 };
        let s = irr + C_REV * langevin_d(he);
        s / (1.0 - ALPHA * s).max(0.05)
    }

    /// The field scale factor from the saturation parameter.
    #[inline]
    fn field_scale(&self) -> f64 {
        0.5 + 5.5 * self.saturation
    }

    /// Processes one sample.
    pub fn process_sample(&mut self, input: f32) -> f32 {
        let h1 = (self.drive * self.field_scale() * sanitize_in(input)).clamp(-200.0, 200.0);
        let dh = h1 - self.h_prev;
        if dh != 0.0 {
            let n = ((dh.abs() / SUBSTEP_H).ceil() as usize).clamp(1, MAX_SUBSTEPS);
            let delta = dh.signum();
            let step = dh / n as f64;
            let mut h = self.h_prev;
            let mut m = self.m;
            for _ in 0..n {
                let k1 = self.slope(m, h, delta);
                let k2 = self.slope(m + 0.5 * step * k1, h + 0.5 * step, delta);
                let k3 = self.slope(m + 0.5 * step * k2, h + 0.5 * step, delta);
                let k4 = self.slope(m + step * k3, h + step, delta);
                m += step * (k1 + 2.0 * k2 + 2.0 * k3 + k4) / 6.0;
                m = m.clamp(-M_LIMIT, M_LIMIT);
                h += step;
            }
            self.m = flush(m);
        }
        self.h_prev = h1;
        let mut y = self.m;
        if self.dc_hz > 0.0 {
            // y[n] = x[n] - x[n-1] + a y[n-1]
            let out = y - self.dc_x + self.dc_a * self.dc_y;
            self.dc_x = y;
            self.dc_y = flush(out);
            y = self.dc_y;
        }
        match finite_out(y) {
            Some(v) if self.m.is_finite() => v,
            _ => {
                self.reset();
                0.0
            }
        }
    }

    /// Processes a buffer in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        for v in buf {
            *v = self.process_sample(*v);
        }
    }

    /// Magnetisation `m` after the last sample (before the DC blocker).
    pub fn magnetisation(&self) -> f64 {
        self.m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn langevin_is_continuous_across_branches() {
        for &x in &[1e-3, 1e-2, 20.0, -1e-3, -20.0] {
            let e = 1e-9;
            assert!((langevin(x * (1.0 + e)) - langevin(x * (1.0 - e))).abs() < 1e-6);
            assert!((langevin_d(x * (1.0 + e)) - langevin_d(x * (1.0 - e))).abs() < 1e-6);
        }
    }

    #[test]
    fn stays_bounded_and_finite() {
        let mut t = TapeHysteresis::new(48_000.0);
        t.set_drive(32.0);
        for n in 0..10_000 {
            let x = if n % 1000 == 3 { f32::NAN } else { ((n as f32) * 0.37).sin() * 100.0 };
            let y = t.process_sample(x);
            assert!(y.is_finite() && y.abs() <= 1.21);
        }
    }
}
