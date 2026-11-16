//! A small plugin written to the rules of `docs/STABILITY.md`, used by the guide as the worked
//! example and by `tests/stable_filter.rs` to prove each rule with the conformance kit and with
//! host behaviours the kit does not drive.
//!
//! A resonant low-pass with `f64` state in transposed direct form II. What makes it hard to break:
//!
//! - non-finite input is replaced by silence before it reaches the state, and a non-finite or
//!   enormous result clears the state;
//! - state below `1e-30` is flushed to zero, so a decaying tail never enters the subnormal range;
//! - cutoff and Q are smoothed per sample and the coefficients are recomputed every 32 samples, in
//!   `f64`, from the smoothed values;
//! - no field depends on `prepare` having run: the state is a fixed array, the rate has a default,
//!   and a block longer than the buffers (or shorter than the channel count) is clamped instead of
//!   indexed past its end;
//! - `prepare` again, with a different rate, and no `reset` in between, keeps the state (it is a
//!   valid state of the same filter) and only recomputes what depends on the rate.

use std::f64::consts::TAU;

use openplugin_core::{
    Category, OnePole, ParamInfo, Plugin, PluginInfo, Taper, Unit, MAX_AUDIO_CHANNELS,
};

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("cutoff", "Cutoff", 20.0, 20_000.0, 1_000.0, Unit::Hz).with_taper(Taper::Log),
    ParamInfo::new("q", "Resonance", 0.1, 20.0, 0.707, Unit::None).with_taper(Taper::Log),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.example.stable-filter",
    name: "Stable filter",
    vendor: "OpenGrid",
    url: "https://github.com/MistaMin/OpenPluginSDK",
    email: "contact@mistamin.com",
    version: "1.0.0-beta",
    category: Category::Eq,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

/// Samples between coefficient updates.
const CONTROL: usize = 32;
/// State magnitudes below this are flushed to zero (about -600 dB).
const FLUSH: f64 = 1.0e-30;

pub struct StableFilter {
    rate: f32,
    cutoff: f32,
    q: f32,
    cutoff_smooth: OnePole,
    q_smooth: OnePole,
    b: [f64; 3],
    a: [f64; 2],
    z: [[f64; 2]; MAX_AUDIO_CHANNELS],
}

impl Default for StableFilter {
    fn default() -> Self {
        let mut f = Self {
            rate: 48_000.0,
            cutoff: 1_000.0,
            q: 0.707,
            cutoff_smooth: OnePole::new(),
            q_smooth: OnePole::new(),
            b: [1.0, 0.0, 0.0],
            a: [0.0; 2],
            z: [[0.0; 2]; MAX_AUDIO_CHANNELS],
        };
        f.set_rate(48_000.0);
        f
    }
}

impl StableFilter {
    fn set_rate(&mut self, rate: f32) {
        self.rate = if rate.is_finite() { rate.clamp(1_000.0, 768_000.0) } else { 48_000.0 };
        self.cutoff_smooth.set_time(self.rate, 10.0);
        self.q_smooth.set_time(self.rate, 10.0);
        self.cutoff_smooth.reset(self.cutoff);
        self.q_smooth.reset(self.q);
        self.update(self.cutoff, self.q);
    }

    /// Coefficients in `f64` from the (smoothed) cutoff and Q. Not per sample.
    fn update(&mut self, cutoff: f32, q: f32) {
        let (b, a) = crate::stability::rbj_lowpass(f64::from(self.rate), f64::from(cutoff), f64::from(q));
        // A coefficient set that is not finite is never installed.
        if b.iter().chain(&a).all(|c| c.is_finite()) {
            self.b = b;
            self.a = a;
        }
    }
}

impl Plugin for StableFilter {
    fn state_is_parameters_only(&self) -> bool {
        true
    }
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.set_rate(sample_rate);
    }
    fn reset(&mut self) {
        self.z = [[0.0; 2]; MAX_AUDIO_CHANNELS];
        self.cutoff_smooth.reset(self.cutoff);
        self.q_smooth.reset(self.q);
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "cutoff" => self.cutoff = PARAMS[0].clamp(value),
            "q" => self.q = PARAMS[1].clamp(value),
            _ => return false,
        }
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "cutoff" => Some(self.cutoff),
            "q" => Some(self.q),
            _ => None,
        }
    }
    fn tail_samples(&self) -> usize {
        // The ring-down of a resonator: time constant 2 Q / w0, to -95 dB (about 11 time constants).
        let w0 = TAU * f64::from(self.cutoff.max(20.0));
        let seconds = (11.0 * 2.0 * f64::from(self.q) / w0).min(10.0);
        (seconds * f64::from(self.rate)).ceil() as usize
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let channels = io.len().min(MAX_AUDIO_CHANNELS);
        let frames = io.iter().map(Vec::len).fold(frames, usize::min);
        let mut done = 0;
        while done < frames {
            let n = CONTROL.min(frames - done);
            // Advance the smoothers over the sub-block, then install coefficients for its end.
            let mut cutoff = self.cutoff_smooth.current();
            let mut q = self.q_smooth.current();
            for _ in 0..n {
                cutoff = self.cutoff_smooth.next(self.cutoff);
                q = self.q_smooth.next(self.q);
            }
            self.update(cutoff, q);
            for (channel, z) in io[..channels].iter_mut().zip(self.z.iter_mut()) {
                let [mut z1, mut z2] = *z;
                for s in &mut channel[done..done + n] {
                    let x = if s.is_finite() { f64::from(*s) } else { 0.0 };
                    let y = self.b[0] * x + z1;
                    let next1 = self.b[1] * x - self.a[0] * y + z2;
                    let next2 = self.b[2] * x - self.a[1] * y;
                    if y.abs() > f64::from(f32::MAX) || !(y.is_finite() && next1.is_finite() && next2.is_finite()) {
                        // Out of range: clear the state rather than carry it.
                        (z1, z2) = (0.0, 0.0);
                        *s = 0.0;
                        continue;
                    }
                    z1 = if next1.abs() < FLUSH { 0.0 } else { next1 };
                    z2 = if next2.abs() < FLUSH { 0.0 } else { next2 };
                    *s = if y.abs() < FLUSH { 0.0 } else { y as f32 };
                }
                *z = [z1, z2];
            }
            done += n;
        }
    }
}
