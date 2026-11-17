//! Each "after" kernel of `docs/PERFORMANCE.md` computes what its "before" computes (exactly, or
//! within the stated tolerance), and the kernels that claim something about the conformance kit
//! are run through it.

use openplugin_core::Plugin;
use openplugin_testing::{conformance, Check, Options};
use perf_guide::kernels::*;
use perf_guide::{Kernel, Rig};

openplugin_testing::install_allocator!();

fn noise(frames: usize, channels: usize) -> Vec<Vec<f32>> {
    let mut seed = 0x1234_5678_u32;
    (0..channels)
        .map(|_| {
            (0..frames)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
                })
                .collect()
        })
        .collect()
}

/// Runs `blocks` blocks of `frames` frames of the same input through `k`, returns the last output.
fn run(k: &mut impl Kernel, blocks: usize, frames: usize) -> Vec<Vec<f32>> {
    k.prepare(48_000.0, frames, 2);
    let input = noise(frames, 2);
    let mut io = input.clone();
    for _ in 0..blocks {
        io.clone_from(&input);
        k.process(&mut openplugin_core::ChannelsMut::new(&mut io), frames);
    }
    io
}

fn max_diff(a: &[Vec<f32>], b: &[Vec<f32>]) -> f32 {
    a.iter().flatten().zip(b.iter().flatten()).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

#[test]
fn allocation_per_block_fails_the_kit_and_the_preallocated_scratch_passes() {
    let before = conformance(|| Rig(AllocPerBlock), Options::quick());
    assert_eq!(before.failed_checks(), vec![Check::NoAlloc], "{before}");
    let after = conformance(|| Rig(Preallocated::default()), Options::quick());
    assert!(after.passed(), "{after}");
    // Same arithmetic.
    let a = run(&mut AllocPerBlock, 1, 256);
    let b = run(&mut Preallocated::default(), 1, 256);
    assert_eq!(a, b);
}

#[test]
fn subnormal_output_fails_the_kit_and_the_flush_passes() {
    let before = conformance(|| Rig(Decay { start: 1.0e-36, flush: false }), Options::quick());
    assert_eq!(before.failed_checks(), vec![Check::NoDenormals], "{before}");
    let after = conformance(|| Rig(Decay { start: 1.0e-36, flush: true }), Options::quick());
    assert!(after.passed(), "{after}");
}

#[test]
fn sixteen_sample_coefficient_updates_follow_the_per_sample_sweep() {
    let a = run(&mut SweptLowpass::new(1), 40, 4096);
    let b = run(&mut SweptLowpass::new(16), 40, 4096);
    // The cutoff moves 3 Hz-slow; a stale coefficient for 16 samples barely shows.
    assert!(max_diff(&a, &b) < 2.0e-3, "differ by {}", max_diff(&a, &b));
}

#[test]
fn settled_skip_and_ramps_reach_the_same_gain() {
    let mut per_sample = SmoothedGain::new(SmoothMode::PerSample, false);
    let mut skip = SmoothedGain::new(SmoothMode::SettledSkip, false);
    // Not moving: the gain is already at its target, so all three are the same constant.
    let a = run(&mut per_sample, 4, 1024);
    let b = run(&mut skip, 4, 1024);
    let c = run(&mut SmoothedGain::new(SmoothMode::BlockRamp, false), 4, 1024);
    assert!(max_diff(&a, &b) < 1.0e-6);
    assert!(max_diff(&a, &c) < 1.0e-6);
    // Moving: after the ramp's block the ramp lands exactly on the target the one-pole is still
    // approaching, so they differ, but both stay between the two targets.
    for k in [SmoothMode::PerSample, SmoothMode::BlockRamp] {
        let out = run(&mut SmoothedGain::new(k, true), 9, 1024);
        let peak = out.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak <= 0.5 + 1.0e-4, "{k:?} peak {peak}");
    }
}

#[test]
fn structure_of_arrays_computes_what_the_array_of_structures_does() {
    let a = run(&mut PartialsAos::new(), 50, 64);
    let b = run(&mut PartialsSoa::new(), 50, 64);
    assert_eq!(a[0][0], b[0][0]);
}

#[test]
fn every_vectorisation_form_gives_the_same_samples() {
    let reference = run(&mut CurveIndexed::new(), 1, 1003);
    assert_eq!(reference, run(&mut CurveZip::default(), 1, 1003));
    assert_eq!(reference, run(&mut CurveChunks::default(), 1, 1003));
    assert!(max_diff(&reference, &run(&mut CurveZipF64::default(), 1, 1003)) < 1.0e-6);
}

#[test]
fn f32_and_f64_state_agree_for_a_one_kilohertz_low_pass() {
    let a = run(&mut Tdf2F32::new(), 10, 1024);
    let b = run(&mut Tdf2F64::new(), 10, 1024);
    assert!(max_diff(&a, &b) < 1.0e-4, "differ by {}", max_diff(&a, &b));
}

#[test]
fn oversampled_saturators_are_finite_and_bounded() {
    for mut k in [Box::new(Sat1x) as Box<dyn Kernel>, Box::new(SatIir2x::new()), Box::new(SatLinearPhase::new(2)), Box::new(SatLinearPhase::new(4))] {
        let out = run(&mut k, 8, 512);
        assert!(out.iter().flatten().all(|v| v.is_finite() && v.abs() < 2.0));
    }
}

/// Case study (docs/STABILITY.md): the tone filter of `examples/design_editor` before `e34f820`.
#[test]
fn the_unguarded_tone_filter_fails_the_kit_where_it_failed_in_the_example() {
    let before = conformance(|| Rig(ToneFilter::new(false)), Options::quick());
    println!("{before}");
    let failed = before.failed_checks();
    assert!(failed.contains(&Check::HostileInput), "{before}");
    assert!(failed.contains(&Check::NoDenormals), "{before}");
    let after = conformance(|| Rig(ToneFilter::new(true)), Options::quick());
    assert!(after.passed(), "{after}");
}

/// Case study (docs/STABILITY.md): `openplugin_dsp::Biquad` before `ed32781` kept a NaN in its state.
#[test]
fn an_unguarded_biquad_keeps_a_nan_for_good_and_the_sdk_biquad_does_not() {
    let before = conformance(|| Rig(BiquadLowpass::new(false)), Options::quick());
    println!("{before}");
    // The raw filter also lets its tail decay through the subnormal range, which the kit reports too.
    assert!(before.failed_checks().contains(&Check::HostileInput), "{before}");
    let after = conformance(|| Rig(BiquadLowpass::new(true)), Options::quick());
    assert!(after.passed(), "{after}");
}

#[test]
fn the_rig_reports_a_plugin_with_no_parameters() {
    let rig = Rig(Sat1x);
    assert_eq!(rig.info().params.len(), 0);
}
