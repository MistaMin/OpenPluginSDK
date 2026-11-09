//! Antiderivative Anti-Aliasing (ADAA) algorithms.
//!
//! Provides first-order Residual ADAA (ADAA1) and second-order ADAA (ADAA2)
//! to suppress non-linear harmonic foldover without requiring extreme oversampling.

use std::f32;

/// First-order Residual ADAA processor.
///
/// Splits the transfer function into linear pass-through and non-linear residual:
/// ```text
/// y = slope * x + r(x),   where r(x) = f(x) - slope * x
/// ```
/// The residual receives first-order ADAA averaging, while the linear path
/// uses a first-order all-pass (a = 1/3) to match the half-sample phase delay.
#[derive(Debug, Clone)]
pub struct ResidualAdaa1 {
    x_prev: f32,
    allpass_state: f32,
}

impl Default for ResidualAdaa1 {
    fn default() -> Self {
        Self::new()
    }
}

impl ResidualAdaa1 {
    pub fn new() -> Self {
        Self {
            x_prev: 0.0,
            allpass_state: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.x_prev = 0.0;
        self.allpass_state = 0.0;
    }

    /// Evaluates ADAA1 for a smooth `tanh` saturation curve with gain `k`.
    ///
    /// - `f(x) = tanh(k*x)`
    /// - Antiderivative `F(x) = ln(cosh(k*x)) / k`
    #[inline]
    pub fn process_tanh(&mut self, x: f32, k: f32) -> f32 {
        let slope = k; // small-signal slope at x=0
        let delta = x - self.x_prev;

        // Linear path through all-pass (a = 1/3)
        let a = 1.0 / 3.0;
        let lin_in = slope * x;
        let lin = a * lin_in + self.allpass_state;
        self.allpass_state = lin_in - a * lin;

        // Non-linear residual evaluation
        let r_curr = (k * x).tanh() - lin_in;
        let r_prev = (k * self.x_prev).tanh() - slope * self.x_prev;

        let mean_r = if delta.abs() <= 1e-5 {
            // Small-step fallback to avoid 0/0 division
            r_curr
        } else {
            let f_curr = log_cosh(k * x) / k;
            let f_prev = log_cosh(k * self.x_prev) / k;
            let f_r_curr = f_curr - 0.5 * slope * x * x;
            let f_r_prev = f_prev - 0.5 * slope * self.x_prev * self.x_prev;
            let calculated = (f_r_curr - f_r_prev) / delta;
            // Safety clamp: Mean residual must remain bounded by endpoint values
            let lo = r_curr.min(r_prev) - 1e-5;
            let hi = r_curr.max(r_prev) + 1e-5;
            calculated.clamp(lo, hi)
        };

        self.x_prev = x;

        // Compression blending weight: w = c^8 / (c^8 + 0.3^8)
        let f_val = (k * x).tanh();
        let c = if lin_in.abs() > 1e-6 {
            (1.0 - f_val / lin_in).abs().clamp(0.0, 1.0)
        } else {
            0.0
        };
        let c8 = c.powi(8);
        let w = c8 / (c8 + 0.3_f32.powi(8));

        let avg_lin = 0.5 * slope * (x + self.x_prev);
        lin + mean_r + w * (avg_lin - lin)
    }

    /// Evaluates ADAA1 on a polynomial soft saturator:
    /// `f(x) = x - x^3 / 6.75` for `|x| <= 1.5`, clamped to `±1.0` outside.
    #[inline]
    pub fn process_soft_clip(&mut self, x: f32) -> f32 {
        let delta = x - self.x_prev;
        let y = if delta.abs() <= 1e-5 {
            soft_clip_f(x)
        } else {
            let f_curr = soft_clip_antiderivative(x);
            let f_prev = soft_clip_antiderivative(self.x_prev);
            (f_curr - f_prev) / delta
        };
        self.x_prev = x;
        y
    }
}

/// Numerically stable `ln(cosh(x))` for ADAA calculations.
#[inline]
pub fn log_cosh(x: f32) -> f32 {
    let ax = x.abs();
    if ax < 1e-4 {
        0.5 * x * x
    } else if ax > 12.0 {
        ax - f32::consts::LN_2
    } else {
        x.cosh().ln()
    }
}

#[inline]
fn soft_clip_f(x: f32) -> f32 {
    if x > 1.5 {
        1.0
    } else if x < -1.5 {
        -1.0
    } else {
        x - (x * x * x) / 6.75
    }
}

#[inline]
fn soft_clip_antiderivative(x: f32) -> f32 {
    let ax = x.abs();
    if ax > 1.5 {
        ax - 0.5625 // continuous boundary match at 1.5
    } else {
        0.5 * x * x - (x * x * x * x) / 27.0
    }
}

/// Second-order Antiderivative Anti-Aliasing (ADAA2) with Taylor series fallback.
#[derive(Debug, Clone)]
pub struct Adaa2Taylor {
    x1: f32,
    x2: f32,
}

impl Default for Adaa2Taylor {
    fn default() -> Self {
        Self::new()
    }
}

impl Adaa2Taylor {
    pub fn new() -> Self {
        Self { x1: 0.0, x2: 0.0 }
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
    }

    /// Evaluates ADAA2 for a hard-clipping stage (-1.0 .. +1.0).
    #[inline]
    pub fn process_hard_clip(&mut self, x: f32) -> f32 {
        let a = x;
        let b = self.x1;
        let c = self.x2;

        let j_ab = evaluate_j(a, b);
        let j_cb = evaluate_j(c, b);

        self.x2 = self.x1;
        self.x1 = x;

        j_ab + j_cb
    }
}

#[inline]
fn q1_clip(u: f32) -> f32 {
    let au = u.abs();
    if au <= 1.0 {
        0.75 * u * u - 0.125 * u.powi(4)
    } else {
        au - 0.375
    }
}

#[inline]
fn q2_clip(u: f32) -> f32 {
    let au = u.abs();
    if au <= 1.0 {
        0.25 * u.powi(3) - 0.025 * u.powi(5)
    } else {
        u.signum() * (0.5 * u * u - 0.375 * au + 0.1)
    }
}

#[inline]
fn evaluate_j(a: f32, b: f32) -> f32 {
    let d = a - b;
    if d.abs() < 1e-4 {
        // Taylor expansion about b to eliminate 0/0 cancellation
        let mid = (a + 2.0 * b) / 3.0;
        let q_val = if mid.abs() <= 1.0 {
            1.5 * (mid - mid.powi(3) / 3.0)
        } else {
            mid.signum()
        };
        let q_d2 = if b.abs() <= 1.0 { -3.0 * b } else { 0.0 };
        0.5 * q_val + q_d2 * d * d / 72.0
    } else {
        (q2_clip(a) - q2_clip(b) - d * q1_clip(b)) / (d * d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adaa1_tanh_smoothness() {
        let mut adaa1 = ResidualAdaa1::new();
        let y1 = adaa1.process_tanh(0.1, 2.0);

        let mut adaa2 = ResidualAdaa1::new();
        let y2 = adaa2.process_tanh(0.10001, 2.0);

        assert!(!y1.is_nan());
        assert!(!y2.is_nan());
        assert!((y1 - y2).abs() < 1e-3);
    }

    #[test]
    fn test_adaa2_hard_clip() {
        let mut adaa2 = Adaa2Taylor::new();
        let y1 = adaa2.process_hard_clip(0.5);
        let y2 = adaa2.process_hard_clip(1.5);
        assert!(!y1.is_nan());
        assert!(!y2.is_nan());
    }
}
