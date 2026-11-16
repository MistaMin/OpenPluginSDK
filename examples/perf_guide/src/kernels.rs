//! The before/after kernel pairs of `docs/PERFORMANCE.md`. Each section says which section of the
//! guide quotes it. Every kernel is stereo-agnostic (any channel count) and allocation-free in
//! `process`; the first pair is the exception on purpose.
#![allow(deprecated)] // `Oversampler2x` is measured here beside its replacements

use std::f32::consts::TAU;

use openplugin_core::OnePole;
use openplugin_dsp::{LinearPhaseOversampler, Oversampler2x};

use crate::Kernel;

// ---------------------------------------------------------------------------------------------
// 1. Allocation on the audio thread
// ---------------------------------------------------------------------------------------------

/// Before: a `Vec` per channel per block. The conformance kit's `no_alloc` check fails this.
pub struct AllocPerBlock;

impl Kernel for AllocPerBlock {
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        for channel in io.iter_mut() {
            let copy: Vec<f32> = channel[..frames].to_vec();
            for (d, s) in channel[..frames].iter_mut().zip(&copy) {
                *d = *s * 0.5;
            }
        }
    }
}

/// After: the scratch is allocated in `prepare`, which is not on the audio thread.
#[derive(Default)]
pub struct Preallocated {
    scratch: Vec<f32>,
}

impl Kernel for Preallocated {
    fn prepare(&mut self, _: f32, max_block: usize, _: usize) {
        self.scratch = vec![0.0; max_block];
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        for channel in io.iter_mut() {
            let copy = &mut self.scratch[..frames];
            copy.copy_from_slice(&channel[..frames]);
            for (d, s) in channel[..frames].iter_mut().zip(copy.iter()) {
                *d = *s * 0.5;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 2. Denormals
// ---------------------------------------------------------------------------------------------

/// A decaying one-pole tail, re-excited at `start` every 512 samples. With a `start` near
/// `1e-36` most of each 512 samples run in the subnormal range (below `1.18e-38`), which is what a
/// reverb or filter tail does for the last few hundred milliseconds of its life.
pub struct Decay {
    pub start: f32,
    pub flush: bool,
}

impl Kernel for Decay {
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        for channel in io.iter_mut() {
            // A new excitation every 512 samples, however long the host's block is.
            for chunk in channel[..frames].chunks_mut(512) {
                let mut y = self.start;
                if self.flush {
                    for s in chunk {
                        y *= 0.97;
                        // Flush before the value gets near the subnormal range.
                        if y.abs() < 1.0e-30 {
                            y = 0.0;
                        }
                        *s = y;
                    }
                } else {
                    for s in chunk {
                        y *= 0.97;
                        *s = y;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 3. Block rate versus sample rate: a coefficient that depends on a modulated cutoff
// ---------------------------------------------------------------------------------------------

/// One-pole low-pass whose cutoff is swept by an LFO. `every` is how many samples pass between
/// coefficient updates: 1 recomputes `exp` for every sample (before), 16 recomputes it every 16th
/// (after). The sweep is 200 Hz to 1.8 kHz at 3 Hz.
pub struct SweptLowpass {
    every: usize,
    rate: f32,
    phase: f32,
    coeff: f32,
    z: Vec<f32>,
}

impl SweptLowpass {
    pub fn new(every: usize) -> Self {
        Self { every: every.max(1), rate: 48_000.0, phase: 0.0, coeff: 0.1, z: Vec::new() }
    }

    #[inline]
    fn coeff_at(&self, phase: f32) -> f32 {
        let cutoff = 1_000.0 + 800.0 * phase.sin();
        1.0 - (-TAU * cutoff / self.rate).exp()
    }
}

impl Kernel for SweptLowpass {
    fn prepare(&mut self, sample_rate: f32, _: usize, channels: usize) {
        self.rate = sample_rate;
        self.z = vec![0.0; channels];
    }
    fn reset(&mut self) {
        self.z.iter_mut().for_each(|z| *z = 0.0);
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let inc = TAU * 3.0 / self.rate;
        let start_phase = self.phase;
        let count = io.len();
        for (c, channel) in io.iter_mut().enumerate() {
            let mut phase = start_phase;
            let mut z = self.z[c];
            let mut coeff = self.coeff;
            for (i, s) in channel[..frames].iter_mut().enumerate() {
                if i % self.every == 0 {
                    coeff = self.coeff_at(phase);
                }
                phase += inc;
                z += coeff * (*s - z);
                *s = z;
            }
            self.z[c] = z;
            self.coeff = coeff;
            if c + 1 == count {
                self.phase = (phase % TAU + TAU) % TAU;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 4. Parameter smoothing
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SmoothMode {
    /// A one-pole smoother stepped for every sample, always.
    PerSample,
    /// The same one-pole, but once it has settled the block is a constant multiply.
    SettledSkip,
    /// A linear ramp from the last gain to the target across the block (no per-sample feedback).
    BlockRamp,
}

/// A smoothed gain. `moving` makes the target alternate between 0.25 and 1.0 every block (a
/// parameter under constant automation); otherwise it is constant (a parameter nobody touches).
pub struct SmoothedGain {
    mode: SmoothMode,
    moving: bool,
    pole: OnePole,
    gain: f32,
    block: u64,
    ramp: Vec<f32>,
}

impl SmoothedGain {
    pub fn new(mode: SmoothMode, moving: bool) -> Self {
        Self { mode, moving, pole: OnePole::new(), gain: 0.5, block: 0, ramp: Vec::new() }
    }
}

impl Kernel for SmoothedGain {
    fn prepare(&mut self, sample_rate: f32, max_block: usize, _: usize) {
        self.pole.set_time(sample_rate, 10.0);
        self.pole.reset(0.5);
        self.gain = 0.5;
        self.ramp = vec![0.0; max_block];
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let target = if self.moving && self.block % 2 == 1 { 1.0 } else if self.moving { 0.25 } else { 0.5 };
        self.block += 1;
        match self.mode {
            SmoothMode::PerSample => {
                for g in &mut self.ramp[..frames] {
                    *g = self.pole.next_value(target);
                }
                apply_curve(io, &self.ramp, frames);
            }
            SmoothMode::SettledSkip => {
                if (self.pole.current() - target).abs() < 1.0e-5 {
                    self.pole.reset(target);
                    for channel in io.iter_mut() {
                        for s in &mut channel[..frames] {
                            *s *= target;
                        }
                    }
                } else {
                    for g in &mut self.ramp[..frames] {
                        *g = self.pole.next_value(target);
                    }
                    apply_curve(io, &self.ramp, frames);
                }
            }
            SmoothMode::BlockRamp => {
                let step = (target - self.gain) / frames as f32;
                let start = self.gain;
                for channel in io.iter_mut() {
                    for (i, s) in channel[..frames].iter_mut().enumerate() {
                        *s *= start + step * i as f32;
                    }
                }
                self.gain = target;
            }
        }
    }
}

fn apply_curve(io: &mut [&mut [f32]], curve: &[f32], frames: usize) {
    for channel in io.iter_mut() {
        for (s, g) in channel[..frames].iter_mut().zip(&curve[..frames]) {
            *s *= *g;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 5. Memory layout: array of structures against structure of arrays
// ---------------------------------------------------------------------------------------------

/// Number of partials (or voices, or bands) in the layout benchmark. 16384 of them is a 1 MiB
/// array of structures: bigger than the level 1 cache, as a big additive or granular engine is.
pub const PARTIALS: usize = 16_384;

#[derive(Clone, Copy)]
struct Partial {
    amp: f32,
    decay: f32,
    gain: f32,
    // The rest of what a partial carries (phase, frequency, pan, filter state ...). The loop
    // below does not touch it, but an array of structures drags it through the cache anyway.
    #[allow(dead_code)]
    other: [f32; 13],
}

/// Before: `Vec<Partial>`. One pass decays each amplitude and adds a share of its gain; the first
/// amplitude goes to the first output sample so the work is observable.
pub struct PartialsAos {
    parts: Vec<Partial>,
}

impl PartialsAos {
    pub fn new() -> Self {
        let parts = (0..PARTIALS)
            .map(|i| Partial { amp: 1.0, decay: 0.999_99 - (i % 7) as f32 * 1.0e-6, gain: 0.5, other: [0.0; 13] })
            .collect();
        Self { parts }
    }
}

impl Default for PartialsAos {
    fn default() -> Self {
        Self::new()
    }
}

impl Kernel for PartialsAos {
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn process(&mut self, io: &mut [&mut [f32]], _frames: usize) {
        for p in &mut self.parts {
            p.amp = p.amp * p.decay + p.gain * 1.0e-6;
        }
        io[0][0] = self.parts[0].amp;
    }
}

/// After: the same fields as separate arrays.
pub struct PartialsSoa {
    amp: Vec<f32>,
    decay: Vec<f32>,
    gain: Vec<f32>,
}

impl PartialsSoa {
    pub fn new() -> Self {
        Self {
            amp: vec![1.0; PARTIALS],
            decay: (0..PARTIALS).map(|i| 0.999_99 - (i % 7) as f32 * 1.0e-6).collect(),
            gain: vec![0.5; PARTIALS],
        }
    }
}

impl Default for PartialsSoa {
    fn default() -> Self {
        Self::new()
    }
}

impl Kernel for PartialsSoa {
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn process(&mut self, io: &mut [&mut [f32]], _frames: usize) {
        for ((a, d), g) in self.amp.iter_mut().zip(&self.decay).zip(&self.gain) {
            *a = *a * *d + *g * 1.0e-6;
        }
        io[0][0] = self.amp[0];
    }
}

// ---------------------------------------------------------------------------------------------
// 6. Helping the compiler vectorise
// ---------------------------------------------------------------------------------------------

/// A per-sample coefficient curve times the signal plus an offset, clamped: `y = clamp(x c + 0.1)`.
fn curve(max_block: usize) -> Vec<f32> {
    (0..max_block).map(|i| 0.5 + 0.4 * ((i % 64) as f32 / 64.0)).collect()
}

/// Before: indexing through `io[c][i]` and `self.curve[i]`. Nothing tells the compiler that
/// `frames` is within either `Vec`, so every access keeps its bounds check and the loop cannot be
/// turned into vector code.
pub struct CurveIndexed {
    curve: Vec<f32>,
}

impl CurveIndexed {
    pub fn new() -> Self {
        Self { curve: Vec::new() }
    }
}

impl Default for CurveIndexed {
    fn default() -> Self {
        Self::new()
    }
}

impl Kernel for CurveIndexed {
    fn prepare(&mut self, _: f32, max_block: usize, _: usize) {
        self.curve = curve(max_block);
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        for c in 0..io.len() {
            for i in 0..frames {
                io[c][i] = (io[c][i] * self.curve[i] + 0.1).clamp(-1.0, 1.0);
            }
        }
    }
}

/// After, form 1: slice both to `frames` once, then zip. One length check per channel.
#[derive(Default)]
pub struct CurveZip {
    curve: Vec<f32>,
}

impl Kernel for CurveZip {
    fn prepare(&mut self, _: f32, max_block: usize, _: usize) {
        self.curve = curve(max_block);
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let curve = &self.curve[..frames];
        for channel in io.iter_mut() {
            for (s, c) in channel[..frames].iter_mut().zip(curve) {
                *s = (*s * *c + 0.1).clamp(-1.0, 1.0);
            }
        }
    }
}

/// After, form 2: `chunks_exact(8)` with the remainder handled separately. The chunk length is a
/// compile-time constant, so the inner loop has no length to check at all.
#[derive(Default)]
pub struct CurveChunks {
    curve: Vec<f32>,
}

impl Kernel for CurveChunks {
    fn prepare(&mut self, _: f32, max_block: usize, _: usize) {
        self.curve = curve(max_block);
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let curve = &self.curve[..frames];
        for channel in io.iter_mut() {
            let data = &mut channel[..frames];
            let mut cs = curve.chunks_exact(8);
            let mut ds = data.chunks_exact_mut(8);
            for (d, c) in (&mut ds).zip(&mut cs) {
                for k in 0..8 {
                    d[k] = (d[k] * c[k] + 0.1).clamp(-1.0, 1.0);
                }
            }
            for (d, c) in ds.into_remainder().iter_mut().zip(cs.remainder()) {
                *d = (*d * *c + 0.1).clamp(-1.0, 1.0);
            }
        }
    }
}

/// The same loop as [`CurveZip`], computed in `f64` and rounded back to `f32` per sample. Half as
/// many lanes fit in a vector register.
#[derive(Default)]
pub struct CurveZipF64 {
    curve: Vec<f32>,
}

impl Kernel for CurveZipF64 {
    fn prepare(&mut self, _: f32, max_block: usize, _: usize) {
        self.curve = curve(max_block);
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let curve = &self.curve[..frames];
        for channel in io.iter_mut() {
            for (s, c) in channel[..frames].iter_mut().zip(curve) {
                *s = (f64::from(*s) * f64::from(*c) + 0.1).clamp(-1.0, 1.0) as f32;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 7. Fast approximations
// ---------------------------------------------------------------------------------------------

/// Applies `f(x * pre)` to every sample. `f` is a generic parameter, so it is inlined and the loop
/// can vectorise when `f` is branch-free.
pub struct Map<F: Fn(f32) -> f32 + Send + 'static> {
    pub f: F,
    pub pre: f32,
}

impl<F: Fn(f32) -> f32 + Send + 'static> Kernel for Map<F> {
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        for channel in io.iter_mut() {
            for s in &mut channel[..frames] {
                *s = (self.f)(*s * self.pre);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 8. Oversampling
// ---------------------------------------------------------------------------------------------

#[inline]
fn drive(x: f32) -> f32 {
    (x * 4.0).tanh()
}

/// The saturator with no oversampling.
pub struct Sat1x;

impl Kernel for Sat1x {
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        for channel in io.iter_mut() {
            for s in &mut channel[..frames] {
                *s = drive(*s);
            }
        }
    }
}

/// The saturator at 2x through `Oversampler2x` (a minimum-phase half-band FIR).
pub struct SatIir2x {
    os: Vec<Oversampler2x>,
    up: Vec<f32>,
}

impl SatIir2x {
    pub fn new() -> Self {
        Self { os: Vec::new(), up: Vec::new() }
    }
}

impl Default for SatIir2x {
    fn default() -> Self {
        Self::new()
    }
}

impl Kernel for SatIir2x {
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.os = (0..channels).map(|_| Oversampler2x::new(sample_rate)).collect();
        self.up = vec![0.0; 2 * max_block];
    }
    fn reset(&mut self) {
        self.os.iter_mut().for_each(Oversampler2x::reset);
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        for (channel, os) in io.iter_mut().zip(&mut self.os) {
            let up = &mut self.up[..2 * frames];
            os.upsample_2x(&channel[..frames], up);
            for s in up.iter_mut() {
                *s = drive(*s);
            }
            os.downsample_2x(up, &mut channel[..frames]);
        }
    }
}

/// The saturator at 2x or 4x through the linear-phase `LinearPhaseOversampler` (64 samples of
/// latency). Stereo only.
pub struct SatLinearPhase {
    os: Option<LinearPhaseOversampler>,
    factor: usize,
    copy: [Vec<f32>; 2],
}

impl SatLinearPhase {
    pub fn new(factor: usize) -> Self {
        Self { os: None, factor, copy: [Vec::new(), Vec::new()] }
    }
}

impl Kernel for SatLinearPhase {
    fn prepare(&mut self, _: f32, max_block: usize, _: usize) {
        self.os = LinearPhaseOversampler::prepare(self.factor, 2, max_block).ok();
        self.copy = [vec![0.0; max_block], vec![0.0; max_block]];
    }
    fn reset(&mut self) {
        if let Some(os) = &mut self.os {
            os.reset();
        }
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let Some(os) = &mut self.os else { return };
        if io.len() < 2 {
            return;
        }
        self.copy[0][..frames].copy_from_slice(&io[0][..frames]);
        self.copy[1][..frames].copy_from_slice(&io[1][..frames]);
        let (left, right) = io.split_at_mut(1);
        let input = [&self.copy[0][..frames], &self.copy[1][..frames]];
        let mut output = [&mut left[0][..frames], &mut right[0][..frames]];
        let _ = os.process(&input, &mut output, |frame| {
            for s in frame.iter_mut() {
                *s = drive(*s);
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------
// 9. f32 against f64 state in a recursive filter
// ---------------------------------------------------------------------------------------------

macro_rules! tdf2_lowpass {
    ($name:ident, $t:ty) => {
        /// Transposed direct form II low-pass (1 kHz, Q 0.707) with state of this precision.
        pub struct $name {
            b: [$t; 3],
            a: [$t; 2],
            z: Vec<[$t; 2]>,
        }

        impl $name {
            pub fn new() -> Self {
                Self { b: [1.0, 0.0, 0.0], a: [0.0; 2], z: Vec::new() }
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl Kernel for $name {
            fn prepare(&mut self, sample_rate: f32, _: usize, channels: usize) {
                let (b, a) = crate::stability::rbj_lowpass(f64::from(sample_rate), 1_000.0, 0.707);
                self.b = [b[0] as $t, b[1] as $t, b[2] as $t];
                self.a = [a[0] as $t, a[1] as $t];
                self.z = vec![[0.0; 2]; channels];
            }
            fn reset(&mut self) {
                self.z.iter_mut().for_each(|z| *z = [0.0; 2]);
            }
            fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
                for (channel, z) in io.iter_mut().zip(&mut self.z) {
                    let [mut z1, mut z2] = *z;
                    for s in &mut channel[..frames] {
                        let x = *s as $t;
                        let y = self.b[0] * x + z1;
                        z1 = self.b[1] * x - self.a[0] * y + z2;
                        z2 = self.b[2] * x - self.a[1] * y;
                        *s = y as f32;
                    }
                    *z = [z1, z2];
                }
            }
        }
    };
}

tdf2_lowpass!(Tdf2F32, f32);
tdf2_lowpass!(Tdf2F64, f64);

// ---------------------------------------------------------------------------------------------
// 10. Case studies: the two bugs the conformance kit found in the SDK's own examples
// ---------------------------------------------------------------------------------------------

/// A 1 kHz low-pass in direct form I. `sdk: false` is the filter as `openplugin_dsp::Biquad` was
/// before commit `ed32781` (no guard: one NaN in the input stays in the state for good);
/// `sdk: true` is the SDK's `Biquad` today.
pub struct BiquadLowpass {
    sdk: bool,
    rate: f32,
    raw: Vec<crate::stability::Df1F64>,
    guarded: Vec<openplugin_dsp::Biquad>,
}

impl BiquadLowpass {
    pub fn new(sdk: bool) -> Self {
        Self { sdk, rate: 48_000.0, raw: Vec::new(), guarded: Vec::new() }
    }
}

impl Kernel for BiquadLowpass {
    fn prepare(&mut self, sample_rate: f32, _: usize, channels: usize) {
        use crate::stability::Lowpass;
        self.rate = sample_rate;
        self.raw = (0..channels).map(|_| crate::stability::Df1F64::default()).collect();
        self.raw.iter_mut().for_each(|f| f.configure(sample_rate, 1_000.0, 0.707));
        self.guarded = (0..channels).map(|_| openplugin_dsp::Biquad::new()).collect();
        self.guarded
            .iter_mut()
            .for_each(|f| f.configure(openplugin_dsp::FilterType::LowPass, sample_rate, 1_000.0, 0.707, 0.0));
    }
    fn reset(&mut self) {
        use crate::stability::Lowpass;
        self.guarded.iter_mut().for_each(openplugin_dsp::Biquad::reset);
        for f in &mut self.raw {
            // A fresh filter has no state; configure it again for the same cutoff.
            *f = crate::stability::Df1F64::default();
            f.configure(self.rate, 1_000.0, 0.707);
        }
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        use crate::stability::Lowpass;
        for (c, channel) in io.iter_mut().enumerate() {
            for s in &mut channel[..frames] {
                *s = if self.sdk { self.guarded[c].process_sample(*s) } else { self.raw[c].tick(*s) };
            }
        }
    }
}

/// The tone filter of `examples/design_editor` as it was before commit `e34f820`: a one-pole
/// low-pass whose state `low` keeps a NaN for good and passes through the subnormal range while a
/// tail decays. With `guard` it is the fixed code (`if !low.is_finite() || low.abs() < 1e-20`).
pub struct ToneFilter {
    pub guard: bool,
    low: Vec<f32>,
    a: f32,
}

impl ToneFilter {
    pub fn new(guard: bool) -> Self {
        Self { guard, low: Vec::new(), a: 0.1 }
    }
}

impl Kernel for ToneFilter {
    fn prepare(&mut self, sample_rate: f32, _: usize, channels: usize) {
        self.low = vec![0.0; channels];
        self.a = 1.0 - (-TAU * 2_000.0 / sample_rate).exp();
    }
    fn reset(&mut self) {
        self.low.iter_mut().for_each(|l| *l = 0.0);
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        for (channel, state) in io.iter_mut().zip(&mut self.low) {
            let mut low = *state;
            for s in &mut channel[..frames] {
                low += self.a * (*s - low);
                if self.guard && (!low.is_finite() || low.abs() < 1.0e-20) {
                    low = 0.0;
                }
                *s = low;
            }
            *state = low;
        }
    }
}
