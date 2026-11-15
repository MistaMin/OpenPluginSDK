//! A tiny `Plugin` around `FdnReverb` and `PlateReverb`, driven through the
//! trait the way a host (or the conformance checks of docs/TESTING_PLUGINS.md)
//! would: prepare, set parameters, process blocks, read the reported tail.
//!
//! Checks that the tail the plugin reports covers the audible decay (energy
//! after it is below -60 dB of the total) without being a gross overstatement,
//! that freeze reports an infinite tail, that a mono layout works, that
//! unknown parameters are refused, and that NaN input leaves the output finite.

use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Tail, Unit};
use openplugin_dsp::{FdnReverb, PlateReverb};

static PARAMS: [ParamInfo; 3] = [
    ParamInfo::new("decay", "Decay", 0.2, 10.0, 2.0, Unit::Seconds),
    ParamInfo::new("freeze", "Freeze", 0.0, 1.0, 0.0, Unit::Toggle),
    ParamInfo::new("engine", "Engine", 0.0, 1.0, 0.0, Unit::Toggle),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.test.reverb",
    name: "Test Reverb",
    vendor: "OpenGrid",
    url: "https://github.com/MistaMin/OpenPluginSDK",
    email: "contact@mistamin.com",
    version: "0.4.0-beta",
    category: Category::Reverb,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

struct TestReverb {
    fdn: FdnReverb,
    plate: PlateReverb,
    plate_engine: bool,
    decay: f32,
    freeze: bool,
}

impl TestReverb {
    fn new() -> Self {
        Self {
            fdn: FdnReverb::new(48_000.0, 8),
            plate: PlateReverb::new(48_000.0),
            plate_engine: false,
            decay: 2.0,
            freeze: false,
        }
    }
}

impl Plugin for TestReverb {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.fdn = FdnReverb::new(sample_rate, 8);
        self.plate = PlateReverb::new(sample_rate);
        self.apply();
    }

    fn reset(&mut self) {
        self.fdn.reset();
        self.plate.reset();
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "decay" => self.decay = PARAMS[0].clamp(value),
            "freeze" => self.freeze = PARAMS[1].clamp(value) > 0.5,
            "engine" => self.plate_engine = PARAMS[2].clamp(value) > 0.5,
            _ => return false,
        }
        self.apply();
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "decay" => Some(self.decay),
            "freeze" => Some(f32::from(u8::from(self.freeze))),
            "engine" => Some(f32::from(u8::from(self.plate_engine))),
            _ => None,
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        match io {
            [l, r, ..] => {
                let (l, r) = (&mut l[..frames], &mut r[..frames]);
                if self.plate_engine {
                    self.plate.process(l, r);
                } else {
                    self.fdn.process(l, r);
                }
            }
            [m] => {
                for s in &mut m[..frames] {
                    let (a, b) = if self.plate_engine {
                        self.plate.process_frame(*s, *s)
                    } else {
                        self.fdn.process_frame(*s, *s)
                    };
                    *s = 0.5 * (a + b);
                }
            }
            [] => {}
        }
    }

    fn tail_samples(&self) -> usize {
        if self.plate_engine { self.plate.tail_samples() } else { self.fdn.tail_samples() }
    }

    fn tail(&self) -> Tail {
        if self.freeze {
            Tail::Infinite
        } else {
            Tail::Finite(self.tail_samples().min(u32::MAX as usize) as u32)
        }
    }
}

impl TestReverb {
    fn apply(&mut self) {
        self.fdn.set_rt60(self.decay * 1.2, self.decay, self.decay * 0.6);
        self.fdn.set_freeze(self.freeze);
        self.plate.set_rt60(self.decay);
        self.plate.set_freeze(self.freeze);
    }
}

fn render(p: &mut TestReverb, seconds: f32, fs: f32) -> Vec<f32> {
    let block = 480;
    let total = (seconds * fs) as usize;
    let mut out = Vec::with_capacity(total);
    let mut first = true;
    while out.len() < total {
        let mut io = vec![vec![0.0f32; block], vec![0.0f32; block]];
        if first {
            io[0][0] = 1.0;
            io[1][0] = 1.0;
            first = false;
        }
        p.process(&mut io, block);
        out.extend_from_slice(&io[0]);
    }
    out
}

#[test]
fn reported_tail_covers_the_decay_for_both_engines() {
    let fs = 48_000.0;
    for plate in [false, true] {
        for decay in [0.5f32, 2.0, 6.0] {
            let mut p = TestReverb::new();
            p.prepare(fs, 480, 2);
            assert!(p.set_param("decay", decay));
            assert!(p.set_param("engine", f32::from(u8::from(plate))));
            let tail = match p.tail() {
                Tail::Finite(n) => n as usize,
                other => panic!("expected a finite tail, got {other:?}"),
            };
            let out = render(&mut p, 1.0 + decay * 3.0 + 1.0, fs);
            let total: f64 = out.iter().map(|v| f64::from(*v).powi(2)).sum();
            let after: f64 = out[tail.min(out.len())..].iter().map(|v| f64::from(*v).powi(2)).sum();
            let db_after = 10.0 * (after / total).log10();
            // Time at which the remaining energy first drops 60 dB.
            let mut rem = total;
            let mut t60 = out.len();
            for (i, v) in out.iter().enumerate() {
                rem -= f64::from(*v).powi(2);
                if rem < total * 1e-6 {
                    t60 = i;
                    break;
                }
            }
            println!(
                "plate {plate} decay {decay}: reported tail {:.2} s, -60 dB of energy at {:.2} s, energy after tail {:.1} dB",
                tail as f32 / fs,
                t60 as f32 / fs,
                db_after
            );
            assert!(after < total * 1e-6, "energy after the reported tail: {db_after} dB");
            assert!(tail <= 4 * t60.max(1), "tail {tail} grossly overstates {t60}");
            assert!(tail as f32 >= 0.9 * t60 as f32);
        }
    }
}

#[test]
fn freeze_reports_an_infinite_tail_and_holds() {
    let fs = 48_000.0;
    let mut p = TestReverb::new();
    p.prepare(fs, 480, 2);
    let _ = render(&mut p, 1.0, fs);
    assert!(p.set_param("freeze", 1.0));
    assert_eq!(p.tail(), Tail::Infinite);
    let out = render(&mut p, 3.0, fs);
    let rms = |s: &[f32]| (s.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / s.len() as f64).sqrt();
    assert!(rms(&out[fs as usize..]) > 1e-4, "frozen tail must keep sounding");
}

#[test]
fn trait_surface_behaves() {
    let mut p = TestReverb::new();
    p.prepare(44_100.0, 256, 1);
    assert!(!p.set_param("nope", 1.0));
    assert_eq!(p.param("nope"), None);
    assert_eq!(p.param("decay"), Some(2.0));
    // NaN and infinities in, finite and bounded out; mono layout works.
    let mut io = vec![vec![f32::NAN; 256]];
    io[0][3] = f32::INFINITY;
    for _ in 0..10 {
        p.process(&mut io, 256);
        assert!(io[0].iter().all(|v| v.is_finite()));
        io[0].fill(f32::NAN);
    }
    // A parameter NaN is clamped to the default by `ParamInfo::clamp`.
    assert!(p.set_param("decay", f32::NAN));
    assert_eq!(p.param("decay"), Some(2.0));
    p.reset();
    let mut io = vec![vec![0.0f32; 256]; 2];
    p.process(&mut io, 256);
    assert!(io.iter().flatten().all(|v| *v == 0.0), "silence in, silence out after reset");
}
