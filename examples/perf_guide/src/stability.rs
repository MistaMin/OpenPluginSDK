//! Numbers for `docs/STABILITY.md`: filter topologies under modulation, `f32` against `f64` state
//! at low cutoff, and why a parameter needs smoothing. Everything here is deterministic: the noise
//! comes from the kit's own generator with a fixed seed, so a run prints the same numbers.

use std::f64::consts::TAU;

use openplugin_core::OnePole;
use openplugin_dsp::{Biquad, FilterType, Svf, SvfMode};
use openplugin_testing::rng::Rng;

/// RBJ low-pass coefficients, normalised so `a0 = 1`: `([b0, b1, b2], [a1, a2])`.
pub fn rbj_lowpass(fs: f64, f0: f64, q: f64) -> ([f64; 3], [f64; 2]) {
    let f0 = f0.clamp(1.0, fs * 0.499);
    let w0 = TAU * f0 / fs;
    let (sin, cos) = w0.sin_cos();
    let alpha = sin / (2.0 * q.max(0.01));
    let a0 = 1.0 + alpha;
    let b1 = (1.0 - cos) / a0;
    ([b1 * 0.5, b1, b1 * 0.5], [-2.0 * cos / a0, (1.0 - alpha) / a0])
}

/// A low-pass whose cutoff and Q can be changed between any two samples.
pub trait Lowpass {
    fn name(&self) -> &'static str;
    fn configure(&mut self, fs: f32, f0: f32, q: f32);
    fn tick(&mut self, x: f32) -> f32;
}

macro_rules! sections {
    ($df1:ident, $tdf2:ident, $t:ty, $label1:expr, $label2:expr) => {
        /// Direct form I: two input and two output delays.
        #[derive(Default)]
        pub struct $df1 {
            b: [$t; 3],
            a: [$t; 2],
            x: [$t; 2],
            y: [$t; 2],
        }
        impl Lowpass for $df1 {
            fn name(&self) -> &'static str {
                $label1
            }
            fn configure(&mut self, fs: f32, f0: f32, q: f32) {
                let (b, a) = rbj_lowpass(f64::from(fs), f64::from(f0), f64::from(q));
                self.b = [b[0] as $t, b[1] as $t, b[2] as $t];
                self.a = [a[0] as $t, a[1] as $t];
            }
            fn tick(&mut self, x: f32) -> f32 {
                let x = x as $t;
                let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
                    - self.a[0] * self.y[0]
                    - self.a[1] * self.y[1];
                self.x = [x, self.x[0]];
                self.y = [y, self.y[0]];
                y as f32
            }
        }

        /// Transposed direct form II: two state variables.
        #[derive(Default)]
        pub struct $tdf2 {
            b: [$t; 3],
            a: [$t; 2],
            z: [$t; 2],
        }
        impl Lowpass for $tdf2 {
            fn name(&self) -> &'static str {
                $label2
            }
            fn configure(&mut self, fs: f32, f0: f32, q: f32) {
                let (b, a) = rbj_lowpass(f64::from(fs), f64::from(f0), f64::from(q));
                self.b = [b[0] as $t, b[1] as $t, b[2] as $t];
                self.a = [a[0] as $t, a[1] as $t];
            }
            fn tick(&mut self, x: f32) -> f32 {
                let x = x as $t;
                let y = self.b[0] * x + self.z[0];
                self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
                self.z[1] = self.b[2] * x - self.a[1] * y;
                y as f32
            }
        }
    };
}

sections!(Df1F32, Tdf2F32, f32, "DF1, f32 state", "TDF2, f32 state");
sections!(Df1F64, Tdf2F64, f64, "DF1, f64 state", "TDF2, f64 state");

/// The SDK's `Biquad` (direct form I, `f32`, with the non-finite guard).
#[derive(Default)]
pub struct SdkBiquad(pub Biquad);

impl Lowpass for SdkBiquad {
    fn name(&self) -> &'static str {
        "SDK Biquad (DF1, f32)"
    }
    fn configure(&mut self, fs: f32, f0: f32, q: f32) {
        self.0.configure(FilterType::LowPass, fs, f0, q, 0.0);
    }
    fn tick(&mut self, x: f32) -> f32 {
        self.0.process_sample(x)
    }
}

/// The SDK's `Svf` (zero-delay-feedback state variable filter, `f64` state).
#[derive(Default)]
pub struct SdkSvf(pub Svf);

impl Lowpass for SdkSvf {
    fn name(&self) -> &'static str {
        "SDK Svf (TPT, f64 state)"
    }
    fn configure(&mut self, fs: f32, f0: f32, q: f32) {
        self.0.configure(SvfMode::LowPass, fs, f0, q, 0.0);
    }
    fn tick(&mut self, x: f32) -> f32 {
        self.0.process_sample(x)
    }
}

/// Every topology the guide compares, freshly built.
pub fn all_topologies() -> Vec<Box<dyn Lowpass>> {
    vec![
        Box::new(Df1F32::default()),
        Box::new(Df1F64::default()),
        Box::new(Tdf2F32::default()),
        Box::new(Tdf2F64::default()),
        Box::new(SdkBiquad::default()),
        Box::new(SdkSvf::default()),
    ]
}

/// What a modulation run produced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Modulated {
    /// Largest output magnitude.
    pub peak: f64,
    /// Number of non-finite output samples.
    pub non_finite: usize,
}

/// Runs `filter` for `seconds` at 48 kHz on white noise of amplitude 0.5, drawing a new cutoff
/// (log-uniform, 50 Hz to 12 kHz) every `every` samples at the given Q.
pub fn modulation_run(filter: &mut dyn Lowpass, q: f32, every: usize, seconds: usize) -> Modulated {
    let fs = 48_000.0f32;
    let mut rng = Rng::new(0x0057_AB1E);
    let mut peak = 0.0f64;
    let mut non_finite = 0;
    for i in 0..seconds * fs as usize {
        if i % every == 0 {
            let t = rng.unit();
            let f0 = 50.0 * (12_000.0f32 / 50.0).powf(t);
            filter.configure(fs, f0, q);
        }
        let y = filter.tick(0.5 * rng.bipolar());
        if y.is_finite() {
            peak = peak.max(f64::from(y.abs()));
        } else {
            non_finite += 1;
        }
    }
    Modulated { peak, non_finite }
}

/// The same noise through a cutoff that sweeps exponentially between 50 Hz and 12 kHz at `lfo_hz`
/// (a filter-sweep LFO), with the coefficients recomputed for every sample. This is what an
/// automated cutoff knob or an envelope follower does to a filter.
pub fn sweep_run(filter: &mut dyn Lowpass, q: f32, lfo_hz: f32, seconds: usize) -> Modulated {
    let fs = 48_000.0f32;
    let mut rng = Rng::new(0x0057_AB1E);
    let mut peak = 0.0f64;
    let mut non_finite = 0;
    for i in 0..seconds * fs as usize {
        let lfo = 0.5 + 0.5 * (std::f32::consts::TAU * lfo_hz * i as f32 / fs).sin();
        filter.configure(fs, 50.0 * (12_000.0f32 / 50.0).powf(lfo), q);
        let y = filter.tick(0.5 * rng.bipolar());
        if y.is_finite() {
            peak = peak.max(f64::from(y.abs()));
        } else {
            non_finite += 1;
        }
    }
    Modulated { peak, non_finite }
}

/// A unit step into a low-pass at `f0` Hz and `fs` Hz sampling, `seconds` long. Returns the
/// final output; the exact answer is 1.0 (unity DC gain).
pub fn step_settles_to(filter: &mut dyn Lowpass, fs: f32, f0: f32, seconds: usize) -> f64 {
    filter.configure(fs, f0, 0.707);
    let mut y = 0.0;
    for _ in 0..seconds * fs as usize {
        y = filter.tick(1.0);
    }
    f64::from(y)
}

/// Largest sample-to-sample change of a 1 kHz sine at 48 kHz whose gain goes from 0 to 1 at
/// sample 1000, once unsmoothed and once through a 10 ms one-pole.
/// Returns `(unsmoothed, smoothed, the sine's own largest step)`.
pub fn zipper() -> (f32, f32, f32) {
    let fs = 48_000.0f32;
    let mut pole = OnePole::new();
    pole.set_time(fs, 10.0);
    pole.reset(0.0);
    let (mut raw_prev, mut smooth_prev) = (0.0f32, 0.0f32);
    let (mut raw_max, mut smooth_max, mut sine_max) = (0.0f32, 0.0f32, 0.0f32);
    let mut sine_prev = 0.0f32;
    for i in 0..4000 {
        let x = (std::f32::consts::TAU * 1000.0 * i as f32 / fs).sin();
        let target = if i < 1000 { 0.0 } else { 1.0 };
        let raw = x * target;
        let smooth = x * pole.next(target);
        raw_max = raw_max.max((raw - raw_prev).abs());
        smooth_max = smooth_max.max((smooth - smooth_prev).abs());
        sine_max = sine_max.max((x - sine_prev).abs());
        (raw_prev, smooth_prev, sine_prev) = (raw, smooth, x);
    }
    (raw_max, smooth_max, sine_max)
}
