//! The measurements behind `docs/STABILITY.md`, asserted so they cannot drift.
//! `cargo run --release -p example-perf-guide -- stability` prints them.

use perf_guide::stability::*;

fn by_name(name: &str) -> Box<dyn Lowpass> {
    all_topologies().into_iter().find(|f| f.name().starts_with(name)).expect(name)
}

#[test]
fn the_svf_stays_calm_under_a_fast_resonant_sweep_and_the_direct_forms_ring_harder() {
    let svf = sweep_run(by_name("SDK Svf").as_mut(), 40.0, 50.0, 5);
    let df1 = sweep_run(by_name("DF1, f64").as_mut(), 40.0, 50.0, 5);
    let tdf2 = sweep_run(by_name("TDF2, f64").as_mut(), 40.0, 50.0, 5);
    assert_eq!(svf.non_finite, 0);
    assert!(svf.peak < 7.0, "svf peak {}", svf.peak);
    assert!(df1.peak > 4.0 * svf.peak, "df1 {} svf {}", df1.peak, svf.peak);
    assert!(tdf2.peak > 4.0 * svf.peak, "tdf2 {} svf {}", tdf2.peak, svf.peak);
}

#[test]
fn state_precision_does_not_change_the_modulation_peak() {
    // Going to f64 does not cure a topology that rings under modulation: the same peak, to 3 digits.
    let a = sweep_run(by_name("DF1, f32").as_mut(), 10.0, 50.0, 5);
    let b = sweep_run(by_name("DF1, f64").as_mut(), 10.0, 50.0, 5);
    assert!((a.peak - b.peak).abs() / b.peak < 1.0e-2, "{a:?} {b:?}");
}

#[test]
fn random_cutoff_jumps_at_high_q_blow_up_the_direct_forms_but_not_the_svf() {
    for name in ["DF1, f32", "DF1, f64", "TDF2, f32", "TDF2, f64"] {
        let r = modulation_run(by_name(name).as_mut(), 10.0, 16, 3);
        assert!(r.non_finite > 0 || r.peak > 1.0e6, "{name} did not blow up: {r:?}");
    }
    let svf = modulation_run(by_name("SDK Svf").as_mut(), 10.0, 16, 3);
    assert_eq!(svf.non_finite, 0);
    assert!(svf.peak < 10.0, "{svf:?}");
}

#[test]
fn f32_state_cannot_hold_a_very_low_cutoff_and_f64_can() {
    let f32_df1 = step_settles_to(by_name("DF1, f32").as_mut(), 192_000.0, 5.0, 8);
    let f32_tdf2 = step_settles_to(by_name("TDF2, f32").as_mut(), 192_000.0, 5.0, 8);
    assert!((f32_df1 - 1.0).abs() > 0.1, "{f32_df1}");
    assert!((f32_tdf2 - 1.0).abs() > 0.1, "{f32_tdf2}");
    for name in ["DF1, f64", "TDF2, f64", "SDK Svf"] {
        let y = step_settles_to(by_name(name).as_mut(), 192_000.0, 5.0, 8);
        assert!((y - 1.0).abs() < 1.0e-6, "{name}: {y}");
    }
}

#[test]
fn smoothing_removes_the_step_a_gain_jump_puts_into_the_signal() {
    let (raw, smooth, sine) = zipper();
    assert!(raw > 5.0 * sine, "raw {raw} sine {sine}");
    assert!(smooth <= sine * 1.01, "smooth {smooth} sine {sine}");
}

/// Documented behaviour of the SDK filters when a parameter, not the signal, is NaN.
#[test]
fn a_nan_cutoff_silences_biquad_until_it_is_configured_again_and_svf_ignores_it() {
    use openplugin_dsp::{Biquad, FilterType, Svf, SvfMode};
    let mut biquad = Biquad::new();
    biquad.configure(FilterType::LowPass, 48_000.0, 1_000.0, 0.707, 0.0);
    biquad.configure(FilterType::LowPass, 48_000.0, f32::NAN, 0.707, 0.0);
    let out: Vec<f32> = (0..200).map(|_| biquad.process_sample(0.5)).collect();
    assert!(out.iter().all(|v| *v == 0.0), "Biquad with a NaN cutoff outputs silence, not NaN");
    biquad.configure(FilterType::LowPass, 48_000.0, 1_000.0, 0.707, 0.0);
    assert!((0..2000).map(|_| biquad.process_sample(0.5)).last().unwrap() > 0.4);

    let mut svf = Svf::new();
    svf.configure(SvfMode::LowPass, 48_000.0, 1_000.0, 0.707, 0.0);
    svf.configure(SvfMode::LowPass, 48_000.0, f32::NAN, 0.707, 0.0);
    // The NaN call was ignored: the filter still passes DC at its old settings.
    assert!((0..2000).map(|_| svf.process_sample(0.5)).last().unwrap() > 0.4);
}
