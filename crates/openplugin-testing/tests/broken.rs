//! Deliberately broken plugins. Each must fail exactly the check it breaks,
//! and the control plugin must pass all of them.

use openplugin::prelude::*;
use openplugin_testing::{bench, conformance, Check, Options, Status};

openplugin_testing::install_allocator!();

static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", 0.0, 2.0, 1.0, Unit::None)];

/// The same parameter in a group path no host can show (an empty name).
static BAD_GROUP_PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", 0.0, 2.0, 1.0, Unit::None).with_group("Filter//Env")];
static BAD_GROUP_INFO: PluginInfo = PluginInfo { params: &BAD_GROUP_PARAMS, ..INFO };

static INFO: PluginInfo = PluginInfo {
    id: "test.faulty",
    name: "Faulty",
    vendor: "test",
    url: "",
    email: "",
    version: "0",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

#[derive(Clone, Copy, PartialEq)]
enum Fault {
    None,
    Alloc,
    Nan,
    Panic,
    LatencyFlaps,
    LatencyLies,
    WrongState,
    NeverDecays,
    PoisonedByNan,
    Slow(u32),
    BadGroup,
}

struct Faulty {
    fault: Fault,
    gain: f32,
    calls: u64,
    state: f32,
    delay: Vec<f32>,
    pos: usize,
}

impl Faulty {
    fn new(fault: Fault) -> Self {
        Self { fault, gain: 1.0, calls: 0, state: 0.0, delay: vec![0.0; 32], pos: 0 }
    }
}

impl Plugin for Faulty {
    fn info(&self) -> &'static PluginInfo {
        if self.fault == Fault::BadGroup {
            &BAD_GROUP_INFO
        } else {
            &INFO
        }
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {
        self.calls = 0;
        self.state = 0.0;
        self.pos = 0;
        self.delay.iter_mut().for_each(|s| *s = 0.0);
    }
    fn reset(&mut self) {
        self.state = 0.0;
        self.pos = 0;
        self.delay.iter_mut().for_each(|s| *s = 0.0);
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id == "gain" {
            self.gain = PARAMS[0].clamp(value);
            true
        } else {
            false
        }
    }
    fn param(&self, id: &str) -> Option<f32> {
        (id == "gain").then_some(self.gain)
    }
    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.calls += 1;
        match self.fault {
            Fault::Alloc => {
                let v = vec![0u8; 16];
                std::hint::black_box(&v);
            }
            Fault::Panic if frames == 7 => panic!("deliberate panic"),
            Fault::Slow(n) => {
                for ch in io.iter_mut() {
                    for s in &mut ch[..frames] {
                        let mut x = *s;
                        for _ in 0..n {
                            x = (x * 1.0001).sin();
                        }
                        *s = std::hint::black_box(x);
                    }
                }
            }
            Fault::NeverDecays => {
                // A leaky integrator: the tail it reports (1000) is far shorter than its real decay.
                for ch in io.iter_mut().take(1) {
                    for s in &mut ch[..frames] {
                        let x = if s.is_finite() { *s } else { 0.0 };
                        self.state = 0.01 * x + 0.9995 * self.state;
                        if self.state.abs() < 1e-20 {
                            self.state = 0.0;
                        }
                        *s = self.state;
                    }
                }
                for ch in io.iter_mut().skip(1) {
                    ch[..frames].fill(0.0);
                }
                return;
            }
            Fault::PoisonedByNan => {
                for ch in io.iter_mut().take(1) {
                    for s in &mut ch[..frames] {
                        self.state = *s + 0.5 * self.state;
                        if self.state.abs() < 1e-20 {
                            self.state = 0.0;
                        }
                        *s = self.state;
                    }
                }
                for ch in io.iter_mut().skip(1) {
                    ch[..frames].fill(0.0);
                }
                return;
            }
            Fault::LatencyLies => {
                // Delays by 32 samples and says 0.
                for ch in io.iter_mut().take(1) {
                    for s in &mut ch[..frames] {
                        let out = self.delay[self.pos];
                        self.delay[self.pos] = *s;
                        self.pos = (self.pos + 1) % 32;
                        *s = out;
                    }
                }
                for ch in io.iter_mut().skip(1) {
                    ch[..frames].fill(0.0);
                }
                return;
            }
            _ => {}
        }
        for ch in io.iter_mut() {
            for s in &mut ch[..frames] {
                *s = (*s * self.gain).clamp(-8.0, 8.0);
            }
        }
        if self.fault == Fault::Nan && self.calls == 50 {
            io[0][0] = f32::NAN;
        }
    }
    fn latency(&self) -> usize {
        match self.fault {
            Fault::LatencyFlaps if self.calls > 3 => 64,
            _ => 0,
        }
    }
    fn tail_samples(&self) -> usize {
        match self.fault {
            Fault::NeverDecays => 1000,
            Fault::PoisonedByNan => 64,
            Fault::LatencyLies => 32,
            _ => 0,
        }
    }
    fn save_state(&mut self) -> Option<Vec<u8>> {
        (self.fault == Fault::WrongState).then(|| self.gain.to_le_bytes().to_vec())
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        if self.fault != Fault::WrongState {
            return false;
        }
        // Bug: reads the chunk wrongly and zeroes the gain.
        let _ = data;
        self.gain = 0.0;
        true
    }
}

fn run(fault: Fault, options: Options) -> openplugin_testing::Report {
    conformance(move || Faulty::new(fault), options)
}

fn only(fault: Fault, check: Check) {
    only_with(fault, check, Options::quick());
}

fn only_with(fault: Fault, check: Check, options: Options) {
    let report = run(fault, options);
    println!("{report}");
    assert_eq!(report.failed_checks(), vec![check], "{report}");
}

#[test]
fn control_plugin_passes_everything() {
    let report = run(Fault::None, Options::quick());
    println!("{report}");
    assert!(report.passed(), "{report}");
    assert_eq!(report.status(Check::NoAlloc), Status::Pass);
    assert_eq!(report.status(Check::Locks), Status::Skip);
    assert_eq!(report.status(Check::Tail), Status::Pass);
    assert_eq!(report.status(Check::Reset), Status::Pass);
    assert_eq!(report.status(Check::State), Status::Pass);
}

#[test]
fn allocation_in_process_fails_no_alloc() {
    only(Fault::Alloc, Check::NoAlloc);
}

#[test]
fn nan_output_fails_finite_output() {
    only(Fault::Nan, Check::FiniteOutput);
}

#[test]
fn panic_fails_no_panic() {
    only(Fault::Panic, Check::NoPanic);
}

#[test]
fn changing_latency_fails_latency() {
    only(Fault::LatencyFlaps, Check::Latency);
}

#[test]
fn lying_latency_fails_latency_when_measured() {
    // Without the impulse option a constant lie cannot be seen.
    let report = run(Fault::LatencyLies, Options::quick());
    assert!(report.passed(), "{report}");
    only_with(Fault::LatencyLies, Check::Latency, Options { verify_latency_by_impulse: true, ..Options::quick() });
}

#[test]
fn an_invalid_group_path_fails_param_fuzz() {
    only(Fault::BadGroup, Check::ParamFuzz);
    let report = run(Fault::BadGroup, Options::quick());
    let failure = report.result(Check::ParamFuzz).first_failure.clone().expect("a failure");
    assert_eq!(failure.case, "param-groups");
    assert!(failure.message.contains("Filter//Env") && failure.message.contains("EmptyName"), "{}", failure.message);
}

#[test]
fn wrong_state_round_trip_fails_state() {
    only(Fault::WrongState, Check::State);
}

#[test]
fn tail_longer_than_reported_fails_tail() {
    only(Fault::NeverDecays, Check::Tail);
}

#[test]
fn poisoned_by_nan_fails_hostile_input() {
    only(Fault::PoisonedByNan, Check::HostileInput);
}

#[test]
fn failure_is_reproducible_from_seed_and_case() {
    let first = run(Fault::Panic, Options::quick());
    let failure = first.result(Check::NoPanic).first_failure.clone().expect("a failure");
    let again = run(Fault::Panic, Options { filter: Some(failure.case.clone()), ..Options::quick() });
    let repeated = again.result(Check::NoPanic).first_failure.clone().expect("the same failure");
    assert_eq!(repeated.case, failure.case);
    assert_eq!(repeated.case_seed, failure.case_seed);
    assert_eq!(repeated.message, failure.message);
    let other_seed = run(Fault::Nan, Options::quick().with_seed(7));
    let same_seed = run(Fault::Nan, Options::quick().with_seed(7));
    assert_eq!(other_seed, same_seed, "same seed, same report");
}

#[test]
fn skipped_rates_are_reported() {
    struct Narrow(Faulty);
    impl Plugin for Narrow {
        fn info(&self) -> &'static PluginInfo {
            self.0.info()
        }
        fn prepare(&mut self, a: f32, b: usize, c: usize) {
            self.0.prepare(a, b, c)
        }
        fn reset(&mut self) {
            self.0.reset()
        }
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            self.0.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.0.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.0.process(io, frames)
        }
        fn note_ports(&self) -> NotePorts {
            NotePorts { input: false, output: false }
        }
        fn accepts_sample_rate(&self, rate: f32) -> bool {
            rate <= 96_000.0
        }
    }
    let report = conformance(|| Narrow(Faulty::new(Fault::None)), Options::default());
    assert!(report.passed(), "{report}");
    assert_eq!(report.rates_skipped, vec![176_400.0, 192_000.0, 384_000.0]);
}

#[test]
fn a_deliberate_slowdown_is_detected_by_compare() {
    let options = bench::BenchOptions { sample_rates: vec![48_000.0], block_sizes: vec![64, 512], iterations: 300, warmup_blocks: 50, ..Default::default() };
    let base = bench::bench(|| Faulty::new(Fault::None), &options);
    let same = bench::bench(|| Faulty::new(Fault::None), &options);
    let slow = bench::bench(|| Faulty::new(Fault::Slow(40)), &options);
    assert!(!base.cases.is_empty());
    let cmp = bench::compare(&base, &slow, 0.25);
    println!("{}", cmp.to_text());
    assert!(cmp.has_regression(), "a 40x slowdown must be flagged");
    assert_eq!(cmp.regressions().len(), cmp.rows.len());
    // Two runs of the same plugin are not flagged by an ordinary tolerance.
    let calm = bench::compare(&base, &same, 3.0);
    assert!(!calm.has_regression(), "{}", calm.to_text());
    // The JSON report reads back.
    let back = bench::BenchReport::from_json(&base.to_json()).expect("json");
    assert_eq!(back.cases.len(), base.cases.len());
    assert_eq!(back.to_json(), base.to_json());
}
