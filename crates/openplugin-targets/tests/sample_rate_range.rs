//! Sample-rate boundaries end to end: the CLAP activate path and the LV2
//! instantiate path.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use openplugin_core::{
    Category, ParamInfo, Plugin, PluginInfo, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE,
};
use openplugin_targets::{lv2, ClapAdapter};

static PARAMS: [ParamInfo; 0] = [];

static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.test.sample-rate",
    name: "Sample Rate Probe",
    vendor: "OpenGrid",
    url: "",
    email: "",
    version: "0.0.1",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

struct RateProbe {
    prepared: Arc<AtomicU32>,
}

impl Plugin for RateProbe {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.prepared.store(sample_rate.to_bits(), Ordering::Relaxed);
    }
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
}

struct NarrowProbe {
    prepared: Arc<AtomicU32>,
}

impl Plugin for NarrowProbe {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.prepared.store(sample_rate.to_bits(), Ordering::Relaxed);
    }
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
    fn accepts_sample_rate(&self, sample_rate: f32) -> bool {
        sample_rate == 48_000.0
    }
}

#[test]
fn clap_activate_accepts_the_shared_range_and_rejects_just_outside_it() {
    let shared = Arc::new(AtomicU32::new(0));
    let mut adapter = ClapAdapter::new(RateProbe { prepared: shared.clone() });

    for rate in [MIN_SAMPLE_RATE, 1_001.0, 1_234.567_8, 44_100.0, 48_000.0, MAX_SAMPLE_RATE] {
        shared.store(0, Ordering::Relaxed);
        assert!(adapter.activate(f64::from(rate), 1, 64), "{rate} Hz should activate");
        assert_eq!(
            f32::from_bits(shared.load(Ordering::Relaxed)),
            rate,
            "the plugin must be prepared at the host rate, not a substitute"
        );
    }

    for rate in [
        f64::from(MIN_SAMPLE_RATE) - 1.0,
        0.0,
        -1.0,
        f64::from(MAX_SAMPLE_RATE) + 1.0,
        f64::NAN,
        f64::INFINITY,
        f64::MAX,
        f64::NEG_INFINITY,
    ] {
        shared.store(0, Ordering::Relaxed);
        assert!(!adapter.activate(rate, 1, 64), "{rate} Hz should be rejected");
        assert_eq!(shared.load(Ordering::Relaxed), 0, "prepare must not run for {rate}");
    }
}

#[test]
fn clap_still_honours_a_plugin_that_narrows_the_range() {
    let shared = Arc::new(AtomicU32::new(0));
    let mut adapter = ClapAdapter::new(NarrowProbe { prepared: shared.clone() });

    assert!(!adapter.activate(f64::from(MIN_SAMPLE_RATE), 1, 64));
    assert_eq!(shared.load(Ordering::Relaxed), 0, "a refused rate must not prepare");
    assert!(adapter.activate(48_000.0, 1, 64));
    assert_eq!(f32::from_bits(shared.load(Ordering::Relaxed)), 48_000.0);
}

static LV2_PREPARED: AtomicU32 = AtomicU32::new(0);

struct Lv2Rate;

impl Plugin for Lv2Rate {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        LV2_PREPARED.store(sample_rate.to_bits(), Ordering::Relaxed);
    }
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
}

fn lv2_probe() -> Box<dyn Plugin> {
    Box::new(Lv2Rate)
}

#[test]
fn lv2_instantiates_at_the_shared_range_without_a_substitute_rate() {
    lv2::set_creator(lv2_probe);
    let descriptor = lv2::descriptor(0);
    assert!(!descriptor.is_null());
    let descriptor = unsafe { &*descriptor };

    for rate in [MIN_SAMPLE_RATE, 1_001.0, 1_234.567_8, 48_000.0, MAX_SAMPLE_RATE] {
        LV2_PREPARED.store(0, Ordering::Relaxed);
        let raw = (descriptor.instantiate)(descriptor, f64::from(rate), std::ptr::null(), std::ptr::null());
        assert!(!raw.is_null(), "{rate} Hz should instantiate");
        assert_eq!(
            f32::from_bits(LV2_PREPARED.load(Ordering::Relaxed)),
            rate,
            "the plugin must be prepared at the host rate, not a substitute"
        );
        (descriptor.cleanup)(raw);
    }

    // LV2 passes 0.0 when the host does not know the rate yet.
    LV2_PREPARED.store(0, Ordering::Relaxed);
    let raw = (descriptor.instantiate)(descriptor, 0.0, std::ptr::null(), std::ptr::null());
    assert!(!raw.is_null(), "an unknown rate should instantiate");
    assert_eq!(
        f32::from_bits(LV2_PREPARED.load(Ordering::Relaxed)),
        48_000.0,
        "an unknown rate uses the 48 kHz default"
    );
    (descriptor.cleanup)(raw);

    for rate in [
        f64::from(MIN_SAMPLE_RATE) - 1.0,
        f64::from(MAX_SAMPLE_RATE) + 1.0,
        -1.0,
        f64::NAN,
        f64::INFINITY,
        f64::MAX,
        f64::NEG_INFINITY,
    ] {
        LV2_PREPARED.store(0, Ordering::Relaxed);
        let raw = (descriptor.instantiate)(descriptor, rate, std::ptr::null(), std::ptr::null());
        assert!(raw.is_null(), "{rate} Hz should fail instantiation");
        assert_eq!(LV2_PREPARED.load(Ordering::Relaxed), 0, "prepare must not run for {rate}");
    }
}
