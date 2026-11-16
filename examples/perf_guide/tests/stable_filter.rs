//! `StableFilter` is written to the rules of `docs/STABILITY.md`. These tests are the proof: the
//! conformance kit passes it, and the host behaviours the kit does not drive are driven by hand.

use openplugin_core::Plugin;
use openplugin_testing::{conformance, Options};
use perf_guide::stable_filter::StableFilter;

openplugin_testing::install_allocator!();

fn options() -> Options {
    if cfg!(debug_assertions) { Options::quick() } else { Options::default() }
}

fn sine(frames: usize, offset: usize) -> Vec<f32> {
    (0..frames).map(|i| (std::f32::consts::TAU * 440.0 * (i + offset) as f32 / 48_000.0).sin() * 0.5).collect()
}

fn finite_and_bounded(io: &[Vec<f32>]) {
    assert!(io.iter().flatten().all(|v| v.is_finite() && v.abs() < 4.0));
}

#[test]
fn passes_the_conformance_kit() {
    let report = conformance(StableFilter::default, options());
    println!("{report}");
    report.assert_pass();
}

#[test]
fn process_before_prepare_is_defined() {
    let mut f = StableFilter::default();
    let mut io = vec![sine(256, 0), sine(256, 0)];
    f.process(&mut openplugin_core::ChannelsMut::new(&mut io), 256);
    finite_and_bounded(&io);
    // A low-pass at 1 kHz passes 440 Hz nearly unchanged.
    assert!(io[0].iter().any(|v| v.abs() > 0.3));
}

#[test]
fn a_rate_change_without_a_reset_keeps_working() {
    let mut f = StableFilter::default();
    f.prepare(44_100.0, 512, 2);
    let mut io = vec![sine(512, 0), sine(512, 0)];
    f.process(&mut openplugin_core::ChannelsMut::new(&mut io), 512);
    for rate in [96_000.0, 22_050.0, 384_000.0, 48_000.0] {
        f.prepare(rate, 512, 2);
        let mut io = vec![sine(512, 512), sine(512, 512)];
        f.process(&mut openplugin_core::ChannelsMut::new(&mut io), 512);
        finite_and_bounded(&io);
    }
}

#[test]
fn block_sizes_that_change_on_every_call_are_fine() {
    let mut f = StableFilter::default();
    f.prepare(48_000.0, 4096, 2);
    let mut at = 0;
    for frames in [1, 7, 4096, 2, 333, 64, 1, 1000, 3, 4096] {
        let mut io = vec![sine(frames, at), sine(frames, at)];
        f.process(&mut openplugin_core::ChannelsMut::new(&mut io), frames);
        finite_and_bounded(&io);
        at += frames;
    }
}

#[test]
fn a_frame_count_larger_than_the_buffers_is_clamped_not_indexed() {
    let mut f = StableFilter::default();
    f.prepare(48_000.0, 64, 2);
    let mut io = vec![sine(64, 0), sine(64, 0)];
    f.process(&mut openplugin_core::ChannelsMut::new(&mut io), 100_000);
    finite_and_bounded(&io);
    let mut none: Vec<Vec<f32>> = Vec::new();
    f.process(&mut openplugin_core::ChannelsMut::new(&mut none), 64);
}

#[test]
fn nan_and_infinity_at_the_input_do_not_reach_the_state() {
    let mut f = StableFilter::default();
    f.prepare(48_000.0, 512, 1);
    let mut dirty = vec![sine(512, 0)];
    dirty[0][10] = f32::NAN;
    dirty[0][11] = f32::INFINITY;
    dirty[0][12] = f32::NEG_INFINITY;
    f.process(&mut openplugin_core::ChannelsMut::new(&mut dirty), 512);
    finite_and_bounded(&dirty);
    let mut clean = vec![sine(512, 512)];
    f.process(&mut openplugin_core::ChannelsMut::new(&mut clean), 512);
    finite_and_bounded(&clean);
    assert!(clean[0].iter().any(|v| v.abs() > 0.3), "the filter went silent after the burst");
}

#[test]
fn a_tail_decays_to_exact_zero_without_subnormals() {
    let mut f = StableFilter::default();
    f.prepare(48_000.0, 512, 1);
    f.set_param("q", 5.0);
    let mut io = vec![vec![1.0f32; 512]];
    f.process(&mut openplugin_core::ChannelsMut::new(&mut io), 512);
    let mut subnormal = 0;
    let mut last = 1.0f32;
    for _ in 0..2000 {
        let mut silent = vec![vec![0.0f32; 512]];
        f.process(&mut openplugin_core::ChannelsMut::new(&mut silent), 512);
        subnormal += silent[0].iter().filter(|v| v.is_subnormal()).count();
        last = silent[0][511];
    }
    assert_eq!(subnormal, 0);
    assert_eq!(last, 0.0);
}

#[test]
fn hostile_parameters_are_clamped() {
    let mut f = StableFilter::default();
    f.prepare(48_000.0, 512, 1);
    for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0e30, -1.0e30, 0.0] {
        assert!(f.set_param("cutoff", v));
        assert!(f.set_param("q", v));
        let mut io = vec![sine(512, 0)];
        f.process(&mut openplugin_core::ChannelsMut::new(&mut io), 512);
        finite_and_bounded(&io);
        assert!(f.param("cutoff").unwrap().is_finite());
    }
}

/// Soak: the kit is deterministic per seed, so a soak is many seeds. Eight seeds run in every
/// `cargo test`; `-- --ignored` runs two hundred.
#[test]
fn eight_seeds_with_extra_parameter_fuzz_all_pass() {
    for seed in 1..=8u64 {
        let options = Options { fuzz_sessions: 6, ..Options::quick() }.with_seed(seed);
        let report = conformance(StableFilter::default, options);
        assert!(report.passed(), "seed {seed}: {report}");
    }
}

#[test]
#[ignore = "soak: about 40 seconds in release"]
fn two_hundred_seeds_all_pass() {
    for seed in 1..=200u64 {
        let options = Options { fuzz_sessions: 12, ..options() }.with_seed(seed);
        let report = conformance(StableFilter::default, options);
        assert!(report.passed(), "seed {seed}: {report}");
    }
}
