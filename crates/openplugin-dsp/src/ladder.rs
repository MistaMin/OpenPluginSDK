//! Zero-delay-feedback ladder filters: the transistor (Moog) ladder and the
//! diode ladder.
//!
//! # Transistor ladder
//!
//! Four cascaded one-pole low-passes with the output fed back to the input
//! through the resonance gain `k`, after Vadim Zavalishin, "The Art of VA
//! Filter Design" (Native Instruments, rev. 2.1), chapter 6 (the ladder
//! filter), which also gives the linear analysis used here: with the stage
//! cutoff normalised to 1, the closed loop is `H(s) = 1 / ((1 + s)^4 + k)`, so
//! at the cutoff `|H| = 1 / (4 - k)` (phase -180 degrees) and the filter
//! self-oscillates at `k = 4`, at the cutoff.
//!
//! Nonlinearity placement (our choice, in the spirit of Huovilainen 2004/2006,
//! "Non-linear digital implementation of the Moog ladder filter", DAFx):
//! a `tanh` at the input of every stage, including the feedback sum. Each stage
//! is the continuous system `y' = w (tanh(u) - y)` with `u0 = drive * x - k y4`
//! and `u_i = y_(i-1)`. The integrators use the trapezoidal rule, so with the
//! prewarped `g = tan(pi fc / fs)` and `G = g / (1 + g)` stage `i` is
//!
//! ```text
//! y_i = G tanh(u_i) + (1 - G) s_i,        s_i <- 2 y_i - s_i
//! ```
//!
//! Only `u0` is unknown (the delay-free loop). `F(u0) = u0 - drive x + k y4(u0)`
//! is strictly increasing with `F' >= 1` (every stage derivative is
//! non-negative), and `|y4| <= G + (1 - G)|s4|` because `|tanh| <= 1`, so the
//! root is bracketed in `drive x +- k (G + (1-G)|s4|)`. The solver is Newton's
//! method safeguarded by that bracket (a step leaving the bracket is replaced by
//! bisection), started from the previous sample's `u0`, with at most
//! [`MAX_ITERATIONS`] iterations; it normally needs two or three. The
//! derivation of this solver is our own.
//!
//! # Diode ladder
//!
//! The diode ladder (EMS Synthi, TB-303 class) couples neighbouring capacitors
//! through diode pairs, so each node is loaded by both neighbours. Zavalishin's
//! text also treats this filter; the equations here were derived by us from the
//! circuit idea, not copied. With every coupling current idealised as
//! `tanh(difference)` (a diode pair's `sinh`, normalised and bounded), the
//! normalised node equations are
//!
//! ```text
//! y1' = w [ T(u - y1)  + T(y2 - y1) ]      u = drive x - k y4
//! y2' = w [ T(y1 - y2) + T(y3 - y2) ]
//! y3' = w [ T(y2 - y3) + T(y4 - y3) ]      T = tanh
//! y4' = w [ T(y3 - y4) ]
//! ```
//!
//! The linearisation is `H(s) = 1 / (p(s) + k)` with
//! `p(s) = s^4 + 7 s^3 + 15 s^2 + 10 s + 1` (the characteristic polynomial of
//! the tridiagonal coupling matrix; its roots are `-(2 - 2 cos((2j-1) pi / 9))`).
//! At `s = jw` the imaginary part of `p` vanishes at `w^2 = 10/7` and then
//! `p = 1 - 150/7 + 100/49 = -901/49`, so the loop self-oscillates at
//! `k = 901/49 = 18.388` and `w = sqrt(10/7) = 1.195` times the cutoff
//! ([`DIODE_K_CRIT`]). The `resonance` argument is scaled so that 1.0 is that
//! point. The trapezoidal rule on all four nodes gives the 4x4 nonlinear system
//! `y = s + g F(y)` (state update `s <- s + 2 g F(y)`), which is solved by
//! Newton's method with a dense 4x4 Jacobian (Gaussian elimination, fixed size)
//! and step halving whenever the residual does not fall, at most
//! [`MAX_ITERATIONS`] iterations. If the cap is hit the best iterate so far is
//! used ([`DiodeLadder::last_residual`] reports the residual).
//!
//! # Real-time and error behaviour
//!
//! No allocation. State is `f64`. A non-finite input is treated as `0.0`; a
//! non-finite or `f32`-overflowing state resets the filter. States below
//! `1e-30` are flushed to zero. Setters ignore NaN or infinite arguments and
//! clamp the rest. The cutoff may change on every sample (the state variables
//! are integrator states, as in [`crate::Svf`]).

#![allow(clippy::needless_range_loop)]

use crate::rate::effective_rate;
use crate::svf::{finite_out, flush, sanitize_in};
use std::f64::consts::PI;

/// Cap on Newton iterations per sample for the ladders.
pub const MAX_ITERATIONS: u32 = 16;

/// Largest resonance argument accepted (1.0 is the self-oscillation point).
pub const MAX_RESONANCE: f32 = 1.25;

/// Linear self-oscillation gain of the diode ladder, `901 / 49`.
pub const DIODE_K_CRIT: f64 = 901.0 / 49.0;

/// Largest input drive (linear gain before the first nonlinearity).
pub const MAX_DRIVE: f32 = 64.0;

#[inline]
fn sech2(t: f64) -> f64 {
    // d/dx tanh(x), from tanh(x) itself.
    1.0 - t * t
}

#[inline]
pub(crate) fn warp(sample_rate: f64, hz: f32) -> f64 {
    let f = f64::from(hz).clamp(1.0, 0.49 * sample_rate);
    (PI * f / sample_rate).tan()
}

// ---------------------------------------------------------------------------
// Transistor ladder
// ---------------------------------------------------------------------------

/// Four-pole transistor ladder (Moog style). See the [module documentation](self).
#[derive(Debug, Clone)]
pub struct TransistorLadder {
    sample_rate: f64,
    gg: f64,
    k: f64,
    drive: f64,
    tap: usize,
    s: [f64; 4],
    u: f64,
    iterations: u32,
}

impl TransistorLadder {
    /// A 24 dB/oct low-pass at 1 kHz, no resonance, drive 1.
    pub fn new(sample_rate: f32) -> Self {
        let mut f = Self {
            sample_rate: 48_000.0,
            gg: 0.0,
            k: 0.0,
            drive: 1.0,
            tap: 4,
            s: [0.0; 4],
            u: 0.0,
            iterations: 0,
        };
        f.set_sample_rate(sample_rate);
        f.set_cutoff(1000.0);
        f
    }

    /// Sets the host rate (clamped by [`effective_rate`]); call [`Self::set_cutoff`] after.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = f64::from(effective_rate(sample_rate));
    }

    /// Cutoff in Hz, clamped to 1 Hz ..= 0.49 fs. This is where `|H| = 1/(4-k)`.
    pub fn set_cutoff(&mut self, hz: f32) {
        if !hz.is_finite() {
            return;
        }
        let g = warp(self.sample_rate, hz);
        self.gg = g / (1.0 + g);
    }

    /// Resonance, `0 ..= MAX_RESONANCE`; feedback gain `k = 4 * resonance`, so 1.0
    /// is the linear self-oscillation point.
    pub fn set_resonance(&mut self, resonance: f32) {
        if resonance.is_finite() {
            self.k = 4.0 * f64::from(resonance.clamp(0.0, MAX_RESONANCE));
        }
    }

    /// Linear gain before the first `tanh`, `0 ..= MAX_DRIVE`.
    pub fn set_drive(&mut self, drive: f32) {
        if drive.is_finite() {
            self.drive = f64::from(drive.clamp(0.0, MAX_DRIVE));
        }
    }

    /// Output tap, 1 ..= 4 poles (6 to 24 dB/oct). Feedback is always from stage 4.
    pub fn set_poles(&mut self, poles: usize) {
        self.tap = poles.clamp(1, 4);
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.s = [0.0; 4];
        self.u = 0.0;
        self.iterations = 0;
    }

    /// Newton iterations used by the last sample (bounded by [`MAX_ITERATIONS`]).
    pub fn last_iterations(&self) -> u32 {
        self.iterations
    }

    /// Stage outputs and d y4 / d u0 for a candidate `u0`.
    #[inline]
    fn stages(&self, u0: f64) -> ([f64; 4], f64) {
        let (gg, om) = (self.gg, 1.0 - self.gg);
        let mut y = [0.0; 4];
        let mut inp = u0;
        let mut d = 1.0;
        for i in 0..4 {
            let t = inp.tanh();
            d *= gg * sech2(t);
            y[i] = gg * t + om * self.s[i];
            inp = y[i];
        }
        (y, d)
    }

    /// Processes one sample, returning the selected tap.
    pub fn process_sample(&mut self, input: f32) -> f32 {
        let x = self.drive * sanitize_in(input);
        let om = 1.0 - self.gg;
        let b = self.gg + om * self.s[3].abs();
        let mut lo = x - self.k * b;
        let mut hi = x + self.k * b;
        let mut u = self.u.clamp(lo, hi);
        let mut iters = 0;
        if self.k > 0.0 {
            while iters < MAX_ITERATIONS {
                iters += 1;
                let (y, d4) = self.stages(u);
                let f = u - x + self.k * y[3];
                if f < 0.0 {
                    lo = u;
                } else {
                    hi = u;
                }
                let df = 1.0 + self.k * d4;
                let step = f / df;
                if step.abs() <= 1e-13 * (1.0 + u.abs()) {
                    u -= step;
                    break;
                }
                let mut next = u - step;
                if !(next > lo && next < hi) {
                    next = 0.5 * (lo + hi);
                }
                u = next;
            }
        } else {
            u = x;
            iters = 1;
        }
        self.iterations = iters;
        self.u = u;
        let (y, _) = self.stages(u);
        for i in 0..4 {
            self.s[i] = flush(2.0 * y[i] - self.s[i]);
        }
        match finite_out(y[self.tap - 1]) {
            Some(v) if self.s.iter().all(|s| s.is_finite()) => v,
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
}

// ---------------------------------------------------------------------------
// Diode ladder
// ---------------------------------------------------------------------------

/// Four-pole diode ladder (EMS / TB-303 class). See the [module documentation](self).
#[derive(Debug, Clone)]
pub struct DiodeLadder {
    sample_rate: f64,
    g: f64,
    k: f64,
    drive: f64,
    tap: usize,
    s: [f64; 4],
    y: [f64; 4],
    iterations: u32,
    residual: f64,
}

type Vec4 = [f64; 4];
type Mat4 = [[f64; 4]; 4];

/// `F(y)` (right-hand side over the cutoff) and its Jacobian.
#[inline]
fn diode_f(y: &Vec4, x: f64, k: f64) -> (Vec4, Mat4) {
    let u = x - k * y[3];
    let t0 = (u - y[0]).tanh();
    let t1 = (y[1] - y[0]).tanh();
    let t2 = (y[2] - y[1]).tanh();
    let t3 = (y[3] - y[2]).tanh();
    let (d0, e1, c2, e3) = (sech2(t0), sech2(t1), sech2(t2), sech2(t3));
    let f = [t0 + t1, -t1 + t2, -t2 + t3, -t3];
    let mut j = [[0.0; 4]; 4];
    j[0][0] = -d0 - e1;
    j[0][1] = e1;
    j[0][3] = -k * d0;
    j[1][0] = e1;
    j[1][1] = -e1 - c2;
    j[1][2] = c2;
    j[2][1] = c2;
    j[2][2] = -c2 - e3;
    j[2][3] = e3;
    j[3][2] = e3;
    j[3][3] = -e3;
    (f, j)
}

/// Solves `a x = b` (4x4) by Gaussian elimination with partial pivoting.
fn solve4(mut a: Mat4, mut b: Vec4) -> Option<Vec4> {
    for c in 0..4 {
        let mut p = c;
        for r in c + 1..4 {
            if a[r][c].abs() > a[p][c].abs() {
                p = r;
            }
        }
        if a[p][c].abs() < 1e-300 {
            return None;
        }
        a.swap(c, p);
        b.swap(c, p);
        for r in c + 1..4 {
            let m = a[r][c] / a[c][c];
            for cc in c..4 {
                a[r][cc] -= m * a[c][cc];
            }
            b[r] -= m * b[c];
        }
    }
    let mut x = [0.0; 4];
    for r in (0..4).rev() {
        let mut v = b[r];
        for c in r + 1..4 {
            v -= a[r][c] * x[c];
        }
        x[r] = v / a[r][r];
    }
    Some(x)
}

impl DiodeLadder {
    /// A low-pass at 1 kHz, no resonance, drive 1.
    pub fn new(sample_rate: f32) -> Self {
        let mut f = Self {
            sample_rate: 48_000.0,
            g: 0.0,
            k: 0.0,
            drive: 1.0,
            tap: 4,
            s: [0.0; 4],
            y: [0.0; 4],
            iterations: 0,
            residual: 0.0,
        };
        f.set_sample_rate(sample_rate);
        f.set_cutoff(1000.0);
        f
    }

    /// Sets the host rate (clamped by [`effective_rate`]); call [`Self::set_cutoff`] after.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = f64::from(effective_rate(sample_rate));
    }

    /// Stage cutoff in Hz (the linear loop resonates at 1.195 times this at
    /// `resonance` 1.0), clamped to 1 Hz ..= 0.49 fs.
    pub fn set_cutoff(&mut self, hz: f32) {
        if hz.is_finite() {
            self.g = warp(self.sample_rate, hz);
        }
    }

    /// Resonance, `0 ..= MAX_RESONANCE`; `k = resonance * DIODE_K_CRIT`.
    pub fn set_resonance(&mut self, resonance: f32) {
        if resonance.is_finite() {
            self.k = DIODE_K_CRIT * f64::from(resonance.clamp(0.0, MAX_RESONANCE));
        }
    }

    /// Linear gain before the first nonlinearity, `0 ..= MAX_DRIVE`.
    pub fn set_drive(&mut self, drive: f32) {
        if drive.is_finite() {
            self.drive = f64::from(drive.clamp(0.0, MAX_DRIVE));
        }
    }

    /// Output node, 1 ..= 4. Feedback is always from node 4.
    pub fn set_poles(&mut self, poles: usize) {
        self.tap = poles.clamp(1, 4);
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.s = [0.0; 4];
        self.y = [0.0; 4];
        self.iterations = 0;
        self.residual = 0.0;
    }

    /// Newton iterations used by the last sample.
    pub fn last_iterations(&self) -> u32 {
        self.iterations
    }

    /// Infinity norm of the trapezoidal residual after the last sample.
    pub fn last_residual(&self) -> f64 {
        self.residual
    }

    #[inline]
    fn residual_of(&self, y: &Vec4, x: f64) -> (Vec4, f64, Mat4) {
        let (f, j) = diode_f(y, x, self.k);
        let mut r = [0.0; 4];
        let mut n = 0.0f64;
        for i in 0..4 {
            r[i] = y[i] - self.s[i] - self.g * f[i];
            n = n.max(r[i].abs());
        }
        (r, n, j)
    }

    /// Processes one sample, returning the selected node.
    pub fn process_sample(&mut self, input: f32) -> f32 {
        let x = self.drive * sanitize_in(input);
        let g = self.g;
        let mut y = self.y;
        let (mut r, mut norm, mut jf) = self.residual_of(&y, x);
        let mut iters = 0;
        while iters < MAX_ITERATIONS && norm > 1e-12 {
            iters += 1;
            let mut m = [[0.0; 4]; 4];
            for (i, row) in m.iter_mut().enumerate() {
                for (c, v) in row.iter_mut().enumerate() {
                    *v = f64::from(u8::from(i == c)) - g * jf[i][c];
                }
            }
            let Some(d) = solve4(m, r) else { break };
            let mut step = 1.0;
            let mut accepted = false;
            for _ in 0..5 {
                let mut cand = y;
                for i in 0..4 {
                    cand[i] -= step * d[i];
                }
                let (r2, n2, j2) = self.residual_of(&cand, x);
                if n2 < norm {
                    y = cand;
                    r = r2;
                    norm = n2;
                    jf = j2;
                    accepted = true;
                    break;
                }
                step *= 0.5;
            }
            if !accepted {
                break;
            }
        }
        self.iterations = iters;
        self.residual = norm;
        let (f, _) = diode_f(&y, x, self.k);
        for i in 0..4 {
            self.s[i] = flush(self.s[i] + 2.0 * g * f[i]);
            y[i] = flush(y[i]);
        }
        self.y = y;
        match finite_out(y[self.tap - 1]) {
            Some(v) if y.iter().chain(self.s.iter()).all(|v| v.is_finite()) => v,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_gain_is_one_without_resonance() {
        let mut t = TransistorLadder::new(48_000.0);
        let mut d = DiodeLadder::new(48_000.0);
        let (mut a, mut b) = (0.0, 0.0);
        for _ in 0..20_000 {
            a = t.process_sample(0.01);
            b = d.process_sample(0.01);
        }
        assert!((a - 0.01).abs() < 1e-5, "{a}");
        assert!((b - 0.01).abs() < 1e-5, "{b}");
    }

    #[test]
    fn nan_input_is_zero_and_recovers() {
        let mut t = TransistorLadder::new(48_000.0);
        t.set_resonance(0.9);
        let mut d = DiodeLadder::new(48_000.0);
        d.set_resonance(0.9);
        for n in 0..2000 {
            let x = if n % 7 == 0 { f32::NAN } else { 0.3 };
            assert!(t.process_sample(x).is_finite());
            assert!(d.process_sample(x).is_finite());
        }
        t.set_cutoff(f32::NAN);
        d.set_resonance(f32::INFINITY);
        assert!(t.process_sample(f32::INFINITY).is_finite());
    }
}
