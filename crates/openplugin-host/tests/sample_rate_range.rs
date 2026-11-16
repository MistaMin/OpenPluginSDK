//! Sample-rate boundaries through the shell create path and `HostSession`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use openplugin_core::{
    Category, ParamInfo, Plugin, PluginInfo, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE,
};
use openplugin_host::{shell, HostSession};

static PARAMS: [ParamInfo; 0] = [];

static INFO: PluginInfo = PluginInfo::new(
    "com.opengrid.test.sample-rate",
    "Sample Rate Probe",
    "OpenGrid",
    "0.0.1",
    Category::Utility,
    &PARAMS,
);

struct RateProbe {
    prepared: Arc<AtomicU32>,
}

impl Default for RateProbe {
    fn default() -> Self {
        Self { prepared: Arc::new(AtomicU32::new(0)) }
    }
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
    fn process(&mut self, _: &mut [&mut [f32]], _: usize) {}
}

#[test]
fn shell_create_accepts_the_shared_range_and_rejects_just_outside_it() {
    for rate in [MIN_SAMPLE_RATE, 1_234.567_8, 48_000.0, MAX_SAMPLE_RATE] {
        let instance = shell::create::<RateProbe>(f64::from(rate), 64, 2);
        assert!(!instance.is_null(), "{rate} Hz should create");
        unsafe { shell::destroy(instance) };
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
        assert!(shell::create::<RateProbe>(rate, 64, 2).is_null(), "{rate} Hz should be rejected");
    }
}

#[test]
fn ffi_new_keeps_an_accepted_rate_and_prepare_rejects_the_rest() {
    let shared = Arc::new(AtomicU32::new(0));
    let instance = HostSession::new(
        Box::new(RateProbe { prepared: shared.clone() }),
        f64::from(MIN_SAMPLE_RATE),
        64,
        2,
    ).unwrap();
    assert_eq!(
        f32::from_bits(shared.load(Ordering::Relaxed)),
        MIN_SAMPLE_RATE,
        "new must not substitute 48 kHz for an accepted rate"
    );
    assert!(instance.prepare(f64::from(MAX_SAMPLE_RATE), 64, 2));
    assert!(!instance.prepare(f64::from(MIN_SAMPLE_RATE) - 1.0, 64, 2));
    assert!(!instance.prepare(f64::from(MAX_SAMPLE_RATE) + 1.0, 64, 2));
    assert!(!instance.prepare(0.0, 64, 2));
    assert!(!instance.prepare(-44_100.0, 64, 2));
    assert!(!instance.prepare(f64::NAN, 64, 2));
    assert!(!instance.prepare(f64::INFINITY, 64, 2));
}
