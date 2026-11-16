//! The shared sample-rate range and the default `Plugin::accepts_sample_rate`.

use openplugin_core::{
    sample_rate_supported, Category, ParamInfo, Plugin, PluginInfo, MAX_SAMPLE_RATE,
    MIN_SAMPLE_RATE,
};

static PARAMS: [ParamInfo; 0] = [];

static INFO: PluginInfo = PluginInfo::new(
    "com.opengrid.test.sample-rate",
    "Sample Rate Probe",
    "OpenGrid",
    "0.0.1",
    Category::Utility,
    &PARAMS,
);

struct Probe;

impl Plugin for Probe {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [&mut [f32]], _: usize) {}
}

struct Narrow;

impl Plugin for Narrow {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [&mut [f32]], _: usize) {}
    fn accepts_sample_rate(&self, sample_rate: f32) -> bool {
        sample_rate == 48_000.0
    }
}

#[test]
fn the_shared_bounds_are_one_kilohertz_and_768_kilohertz() {
    assert_eq!(MIN_SAMPLE_RATE, 1_000.0);
    assert_eq!(MAX_SAMPLE_RATE, 768_000.0);
}

#[test]
fn the_default_predicate_accepts_the_bounds_and_rejects_just_outside_them() {
    let probe = Probe;
    for rate in [
        MIN_SAMPLE_RATE,
        MIN_SAMPLE_RATE + 1.0,
        11_025.0,
        44_100.0,
        48_000.0,
        96_000.0,
        192_000.0,
        MAX_SAMPLE_RATE - 1.0,
        MAX_SAMPLE_RATE,
    ] {
        assert!(probe.accepts_sample_rate(rate), "{rate} Hz should be accepted");
    }
    for rate in [
        MIN_SAMPLE_RATE - 1.0,
        0.0,
        -0.0,
        -1.0,
        -48_000.0,
        MAX_SAMPLE_RATE + 1.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ] {
        assert!(!probe.accepts_sample_rate(rate), "{rate} Hz should be rejected");
    }
}

#[test]
fn the_host_side_predicate_uses_the_same_bounds() {
    for rate in [1_000.0, 1_001.0, 1_234.567_8, 48_000.0, 768_000.0] {
        assert!(sample_rate_supported(rate), "{rate} Hz should be accepted");
    }
    for rate in [
        999.0,
        768_000.5,
        0.0,
        -44_100.0,
        f64::NAN,
        f64::INFINITY,
        f64::MAX,
        f64::NEG_INFINITY,
        f64::from(f32::MAX),
    ] {
        assert!(!sample_rate_supported(rate), "{rate} Hz should be rejected");
    }
}

#[test]
fn the_trait_and_the_helper_agree_on_the_probed_rates() {
    let probe = Probe;
    for rate in [
        MIN_SAMPLE_RATE - 1.0,
        MIN_SAMPLE_RATE,
        MIN_SAMPLE_RATE + 1.0,
        1_234.567_8,
        48_000.0,
        MAX_SAMPLE_RATE - 1.0,
        MAX_SAMPLE_RATE,
        MAX_SAMPLE_RATE + 1.0,
        0.0,
        -1.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ] {
        assert_eq!(
            probe.accepts_sample_rate(rate),
            sample_rate_supported(f64::from(rate)),
            "{rate} Hz"
        );
    }
}

#[test]
fn a_plugin_can_narrow_the_shared_range() {
    assert!(Narrow.accepts_sample_rate(48_000.0));
    assert!(!Narrow.accepts_sample_rate(MIN_SAMPLE_RATE));
    assert!(!Narrow.accepts_sample_rate(MAX_SAMPLE_RATE));
}
