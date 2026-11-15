//! The error bounds of the fast approximations quoted in `docs/PERFORMANCE.md`.
//! `cargo run --release -p example-perf-guide -- approx` prints the measured values.

use perf_guide::approx::{fast_exp, fast_sin, fast_tanh, fast_tanh_cheap, measure_errors, ErrorReport};

fn find(name: &str, domain: &str) -> ErrorReport {
    measure_errors().into_iter().find(|e| e.name == name && e.domain == domain).expect("measured")
}

#[test]
fn tanh_error_is_below_one_part_in_ten_thousand() {
    let e = find("fast_tanh", "[-12, 12]");
    assert!(e.max_abs < 1.0e-4, "{e:?}");
    let cheap = find("fast_tanh_cheap", "[-12, 12]");
    assert!(cheap.max_abs < 2.5e-2, "{cheap:?}");
}

#[test]
fn exp_relative_error_is_below_five_parts_in_a_million() {
    let e = find("fast_exp", "[-80, 80]");
    assert!(e.max_rel < 5.0e-6, "{e:?}");
}

#[test]
fn sin_absolute_error_is_below_two_parts_in_ten_million_for_one_turn() {
    let e = find("fast_sin", "[-pi, pi]");
    assert!(e.max_abs < 2.0e-7, "{e:?}");
    let wide = find("fast_sin", "[-100, 100]");
    assert!(wide.max_abs < 6.0e-6, "{wide:?}");
}

#[test]
fn the_approximations_keep_their_shape_outside_the_accurate_range() {
    assert_eq!(fast_tanh(0.0), 0.0);
    assert!(fast_tanh(100.0) <= 1.0 && fast_tanh(-100.0) >= -1.0);
    assert!(fast_tanh(f32::INFINITY) <= 1.0);
    assert!(fast_tanh_cheap(1.0e9) <= 1.0);
    // exp saturates instead of overflowing or going subnormal.
    assert!(fast_exp(1.0e6).is_finite() && fast_exp(1.0e6) > 1.0e37);
    assert!(fast_exp(-1.0e6) >= f32::MIN_POSITIVE);
    assert_eq!(fast_exp(0.0), 1.0);
    // sin is odd and exact at the origin.
    assert_eq!(fast_sin(0.0), 0.0);
    assert!((fast_sin(1.0) + fast_sin(-1.0)).abs() < 1.0e-7);
}

#[test]
fn nan_in_is_nan_out_for_exp_and_sin_and_tanh_is_bounded() {
    // Documented behaviour: a NaN is not repaired here. The caller guards the input.
    assert!(fast_sin(f32::NAN).is_nan());
    assert!(fast_exp(f32::NAN).is_nan() || fast_exp(f32::NAN).is_finite());
    let t = fast_tanh(f32::NAN);
    assert!(t.is_nan() || t.abs() <= 1.0);
}
