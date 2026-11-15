//! Fast approximations of `tanh`, `exp` and `sin` in `f32`, with the error measured by
//! `tests/approx_error.rs` and quoted in `docs/PERFORMANCE.md`.
//!
//! They are plain Rust (no intrinsics), allocate nothing, have no branches the optimiser cannot
//! turn into selects, and so let a loop over a buffer vectorise. Each documents its domain and what
//! it does outside it. None of them is a substitute for the standard function when exactness
//! matters (a filter coefficient computed once per block should use `tan` and `exp` from `std`).

use std::f32::consts::{LN_2, LOG2_E, PI, TAU};

/// `tanh(x)`, a [7/6] rational (the continued-fraction approximant) clamped to the output range
/// `[-1, 1]`. Odd, monotonic, exact at 0, saturates cleanly. Max absolute error over all `x` is
/// the figure in `tests/approx_error.rs` (about 1e-4 at the clamp, much lower elsewhere).
#[inline]
pub fn fast_tanh(x: f32) -> f32 {
    let x = x.clamp(-4.97, 4.97);
    let x2 = x * x;
    let num = x * (135_135.0 + x2 * (17_325.0 + x2 * (378.0 + x2)));
    let den = 135_135.0 + x2 * (62_370.0 + x2 * (3_150.0 + x2 * 28.0));
    (num / den).clamp(-1.0, 1.0)
}

/// `tanh(x)`, a cheaper [3/2] rational: `x (27 + x^2) / (27 + 9 x^2)` clamped to `[-1, 1]`. Smooth
/// and odd, but about two orders of magnitude less accurate than [`fast_tanh`]; for drive stages
/// where the exact curve does not matter.
#[inline]
pub fn fast_tanh_cheap(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    let x2 = x * x;
    (x * (27.0 + x2) / (27.0 + 9.0 * x2)).clamp(-1.0, 1.0)
}

/// `e^x`. Range reduction `x = n ln 2 + g` with `|g| <= ln 2 / 2`, a degree-6 polynomial for `e^g`,
/// and `2^n` built from the exponent bits. `x` is clamped to `[-87, 88]`, so the result stays a
/// normal `f32` and never overflows or goes subnormal: `fast_exp(100.0)` is `e^88`, not infinity.
#[inline]
pub fn fast_exp(x: f32) -> f32 {
    let x = x.clamp(-87.0, 88.0);
    let n = (x * LOG2_E).round();
    let g = x - n * LN_2;
    let p = 1.0
        + g * (1.0 + g * (0.5 + g * (1.0 / 6.0 + g * (1.0 / 24.0 + g * (1.0 / 120.0 + g * (1.0 / 720.0))))));
    let scale = f32::from_bits(((n as i32 + 127) as u32) << 23);
    p * scale
}

/// `sin(x)` for any finite `x` of moderate size. Reduces to `[-pi, pi]`, folds to `[-pi/2, pi/2]`
/// and evaluates the odd Taylor polynomial to `x^11`. The reduction is done in `f32`, so the
/// absolute error grows by about `6e-8` per radian of `|x|`: use it for phases below a few hundred
/// radians, or keep the phase wrapped to one turn (the better habit).
#[inline]
pub fn fast_sin(x: f32) -> f32 {
    let r = x - (x * (1.0 / TAU)).round() * TAU;
    let folded = PI / 2.0 - (PI / 2.0 - r.abs()).abs();
    let t2 = folded * folded;
    let s = folded
        * (1.0
            + t2 * (-1.0 / 6.0
                + t2 * (1.0 / 120.0
                    + t2 * (-1.0 / 5_040.0 + t2 * (1.0 / 362_880.0 + t2 * (-1.0 / 39_916_800.0))))));
    s.copysign(r)
}

/// The measured error of one approximation over a dense grid of `f32` inputs, against the `f64`
/// standard function evaluated at the same `f32` input.
#[derive(Clone, Debug)]
pub struct ErrorReport {
    pub name: &'static str,
    pub domain: &'static str,
    pub max_abs: f64,
    pub at_abs: f64,
    /// Largest `|approx - exact| / |exact|`, over inputs where `|exact| > 1e-30`.
    pub max_rel: f64,
}

fn scan(
    name: &'static str,
    domain: &'static str,
    (lo, hi, step): (f64, f64, f64),
    f: impl Fn(f32) -> f32,
    exact: impl Fn(f64) -> f64,
) -> ErrorReport {
    let (mut max_abs, mut at_abs, mut max_rel) = (0.0f64, 0.0f64, 0.0f64);
    let n = ((hi - lo) / step) as usize;
    for i in 0..=n {
        let x = (lo + i as f64 * step) as f32;
        let want = exact(f64::from(x));
        let got = f64::from(f(x));
        let abs = (got - want).abs();
        if abs > max_abs {
            (max_abs, at_abs) = (abs, f64::from(x));
        }
        if want.abs() > 1.0e-30 {
            max_rel = max_rel.max(abs / want.abs());
        }
    }
    ErrorReport { name, domain, max_abs, at_abs, max_rel }
}

/// Measures every approximation in this module. `tests/approx_error.rs` asserts the bounds quoted
/// in the guide; `perf_guide approx` prints them.
pub fn measure_errors() -> Vec<ErrorReport> {
    let pi = std::f64::consts::PI;
    vec![
        scan("fast_tanh", "[-12, 12]", (-12.0, 12.0, 1.0e-4), fast_tanh, f64::tanh),
        scan("fast_tanh_cheap", "[-12, 12]", (-12.0, 12.0, 1.0e-4), fast_tanh_cheap, f64::tanh),
        scan("fast_exp", "[-80, 80]", (-80.0, 80.0, 1.0e-3), fast_exp, f64::exp),
        scan("fast_sin", "[-pi, pi]", (-pi, pi, 1.0e-4), fast_sin, f64::sin),
        scan("fast_sin", "[-100, 100]", (-100.0, 100.0, 1.0e-4), fast_sin, f64::sin),
        scan("f32::tanh (reference)", "[-12, 12]", (-12.0, 12.0, 1.0e-4), f32::tanh, f64::tanh),
        scan("f32::exp (reference)", "[-80, 80]", (-80.0, 80.0, 1.0e-3), f32::exp, f64::exp),
        scan("f32::sin (reference)", "[-100, 100]", (-100.0, 100.0, 1.0e-4), f32::sin, f64::sin),
    ]
}
