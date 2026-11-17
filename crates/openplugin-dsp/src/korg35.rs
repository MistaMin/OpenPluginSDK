//! Sallen-Key / Korg-35 style two-pole filter with nonlinear feedback.
//!
//! The Korg MS-20 (Korg-35 chip) filter is a Sallen-Key topology: a one-pole
//! stage followed by a second one-pole stage whose input receives positive
//! feedback of the high-passed output. Sources for the structure and the idea of
//! a saturating feedback path: Vadim Zavalishin, "The Art of VA Filter Design"
//! (Native Instruments, rev. 2.1), the Sallen-Key discussion; Will Pirkle,
//! "Designing Software Synthesizer Plugins in C++" (Focal Press, 2nd ed. 2021),
//! the Korg35 LPF/HPF chapter. We did not rely on their exact block diagrams;
//! the equations below are derived by us and verified by the tests.
//!
//! # Low-pass
//!
//! With the cutoff normalised to 1 and `K` the feedback gain:
//!
//! ```text
//! y1 = LP(x)                    (one-pole)
//! u  = y1 + K * sat(h)          h = y - z,  z = LP(y)   (h is the one-pole high-pass of y)
//! y  = LP(u)
//! ```
//!
//! Linear (`sat` = identity): `y (s+1) = y1 + K y s/(s+1)` with `y1 = x/(s+1)`,
//! hence `H(s) = 1 / (s^2 + (2 - K) s + 1)`: `Q = 1/(2 - K)`, magnitude `Q` at the
//! cutoff, self-oscillation at `K = 2`.
//!
//! # High-pass
//!
//! ```text
//! y1 = x - LP(x)   u = y1 + K sat(l)   y = u - LP(u)   l = LP(y)
//! ```
//!
//! gives `H(s) = s^2 / (s^2 + (2 - K) s + 1)` (derived the same way).
//!
//! # Zero-delay solve
//!
//! With trapezoidal one-poles (`G = g/(1+g)`, `g = tan(pi fc / fs)`) the only
//! unknown is the scalar `h` (low-pass) or `l` (high-pass), which satisfies
//! `phi(v) = v - a tanh(v) - c = 0`, `a = G (1-G) K`, `c` known from the states.
//! Since `G(1-G) <= 1/4`, `phi' >= 1 - K/4 > 0` for `K < 4`, and the root lies in
//! `[c - a, c + a]`. Newton's method inside that bracket (bisection fallback) is
//! used, at most [`MAX_ITERATIONS`] iterations.
//!
//! The saturator `sat = tanh` has unit small-signal gain, so the linear analysis
//! holds for small signals. Real-time behaviour as for [`crate::TransistorLadder`].

use crate::rate::effective_rate;
use crate::svf::{finite_out, flush, sanitize_in};

/// Cap on Newton iterations per sample.
pub const MAX_ITERATIONS: u32 = 12;

/// Largest resonance argument (1.0 is `K = 2`, self-oscillation).
pub const MAX_RESONANCE: f32 = 1.1;

/// Response of a [`Korg35`] filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Korg35Mode {
    /// `1 / (s^2 + (2-K) s + 1)`.
    LowPass,
    /// `s^2 / (s^2 + (2-K) s + 1)`.
    HighPass,
}

/// Korg-35 style Sallen-Key two-pole. See the [module documentation](self).
#[derive(Debug, Clone)]
pub struct Korg35 {
    sample_rate: f64,
    mode: Korg35Mode,
    gg: f64,
    k: f64,
    drive: f64,
    /// LP: [s1, sy, sz]; HP: [s1, s3, sl].
    s: [f64; 3],
    h: f64,
    iterations: u32,
}

impl Korg35 {
    /// A low-pass at 1 kHz, no resonance.
    pub fn new(sample_rate: f32) -> Self {
        let mut f = Self {
            sample_rate: 48_000.0,
            mode: Korg35Mode::LowPass,
            gg: 0.0,
            k: 0.0,
            drive: 1.0,
            s: [0.0; 3],
            h: 0.0,
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

    /// Selects low- or high-pass (clears the state).
    pub fn set_mode(&mut self, mode: Korg35Mode) {
        if self.mode != mode {
            self.mode = mode;
            self.reset();
        }
    }

    /// Cutoff in Hz, clamped to 1 Hz ..= 0.49 fs.
    pub fn set_cutoff(&mut self, hz: f32) {
        if hz.is_finite() {
            let g = crate::ladder::warp(self.sample_rate, hz);
            self.gg = g / (1.0 + g);
        }
    }

    /// Resonance `0 ..= MAX_RESONANCE`; `K = 2 * resonance`.
    pub fn set_resonance(&mut self, resonance: f32) {
        if resonance.is_finite() {
            self.k = 2.0 * f64::from(resonance.clamp(0.0, MAX_RESONANCE));
        }
    }

    /// Linear input gain (applied before the filter), `0 ..= 64`.
    pub fn set_drive(&mut self, drive: f32) {
        if drive.is_finite() {
            self.drive = f64::from(drive.clamp(0.0, 64.0));
        }
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.s = [0.0; 3];
        self.h = 0.0;
        self.iterations = 0;
    }

    /// Newton iterations used by the last sample.
    pub fn last_iterations(&self) -> u32 {
        self.iterations
    }

    #[inline]
    fn one_pole(gg: f64, x: f64, s: &mut f64) -> f64 {
        let y = gg * x + (1.0 - gg) * *s;
        *s = 2.0 * y - *s;
        y
    }

    /// Root of `v - a tanh v - c` in `[c-a, c+a]`.
    fn solve(&mut self, a: f64, c: f64) -> f64 {
        let (mut lo, mut hi) = (c - a, c + a);
        let mut v = self.h.clamp(lo, hi);
        let mut n = 0;
        while n < MAX_ITERATIONS {
            n += 1;
            let t = v.tanh();
            let f = v - a * t - c;
            if f < 0.0 {
                lo = v;
            } else {
                hi = v;
            }
            let step = f / (1.0 - a * (1.0 - t * t));
            if step.abs() <= 1e-13 * (1.0 + v.abs()) {
                v -= step;
                break;
            }
            let mut next = v - step;
            if !(next > lo && next < hi) {
                next = 0.5 * (lo + hi);
            }
            v = next;
        }
        self.iterations = n;
        v
    }

    /// Processes one sample.
    pub fn process_sample(&mut self, input: f32) -> f32 {
        let x = self.drive * sanitize_in(input);
        let (gg, om, k) = (self.gg, 1.0 - self.gg, self.k);
        let a = gg * om * k;
        let mut s = self.s;
        let y = match self.mode {
            Korg35Mode::LowPass => {
                let y1 = Self::one_pole(gg, x, &mut s[0]);
                let c = om * (gg * y1 + om * s[1] - s[2]);
                let h = self.solve(a, c);
                self.h = h;
                let u = y1 + k * h.tanh();
                let y = Self::one_pole(gg, u, &mut s[1]);
                Self::one_pole(gg, y, &mut s[2]);
                y
            }
            Korg35Mode::HighPass => {
                let lp = Self::one_pole(gg, x, &mut s[0]);
                let y1 = x - lp;
                let c = gg * om * (y1 - s[1]) + om * s[2];
                let l = self.solve(a, c);
                self.h = l;
                let u = y1 + k * l.tanh();
                let y = u - Self::one_pole(gg, u, &mut s[1]);
                Self::one_pole(gg, y, &mut s[2]);
                y
            }
        };
        for v in &mut s {
            *v = flush(*v);
        }
        self.s = s;
        match finite_out(y) {
            Some(v) if s.iter().all(|v| v.is_finite()) => v,
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
    fn lowpass_dc_gain_one_highpass_zero() {
        let mut lp = Korg35::new(48_000.0);
        let mut hp = Korg35::new(48_000.0);
        hp.set_mode(Korg35Mode::HighPass);
        lp.set_resonance(0.5);
        hp.set_resonance(0.5);
        let (mut a, mut b) = (0.0, 1.0);
        for _ in 0..40_000 {
            a = lp.process_sample(0.05);
            b = hp.process_sample(0.05);
        }
        assert!((a - 0.05).abs() < 1e-5, "{a}");
        assert!(b.abs() < 1e-5, "{b}");
    }
}
