//! Karplus-Strong plucked string, with the Jaffe-Smith extensions.
//!
//! The basic algorithm (Karplus and Strong, "Digital Synthesis of
//! Plucked-String and Drum Timbres", Computer Music J. 7(2), 1983) is a delay
//! line of `N` samples holding a burst of noise, fed back through a two-point
//! average. The extensions (Jaffe and Smith, "Extensions of the
//! Karplus-Strong Plucked-String Algorithm", Computer Music J. 7(2), 1983)
//! are all here:
//!
//! - **Tuning.** The delay is an integer, so the pitch is quantised
//!   (`fs / (N + 1/2)`). With [`Tuning::Allpass`] a first-order allpass in the
//!   loop supplies the fractional part. Its coefficient is designed for the
//!   phase delay at the fundamental (`a = sin(w (1 - d) / 2) / sin(w (1 + d) / 2)`,
//!   J. O. Smith, "Physical Audio Signal Processing"), taking into account the
//!   delay of the loss filter, so the loop's phase delay at `f` is exactly
//!   `fs / f`.
//! - **Loss filter and decay.** `H(z) = (1 - S) + S z^-1` with a loop gain
//!   `rho`. `S = 1/2` is the original average. `rho` is chosen so the
//!   fundamental falls 60 dB in the requested T60; `rho` never exceeds 1, so
//!   when the loss filter alone already decays faster than asked (high notes
//!   with a large `S`) the T60 is shorter than asked for the same reason as in
//!   the paper. Lower `S` for longer decays at high pitches.
//! - **Pick position.** The excitation is comb filtered by `1 - z^-D`, with
//!   `D = round(beta N)`, which notches partials `k = m / beta`.
//! - **Dynamic level.** The excitation is low-passed by a one-pole filter
//!   whose cutoff rises with the pluck velocity, so a soft pluck is duller.
//! - **Stiffness.** Up to eight first-order allpasses with one negative
//!   coefficient (Van Duyne and Smith, ICMC 1994) bend the partials to
//!   `f_k = k f_1 sqrt((1 + B k^2) / (1 + B))`. The coefficient is the least
//!   squares fit of the loop phase delay at partials 2 to 12 (below `0.3 fs`),
//!   weighted towards the low partials; the error grows with `B` and with
//!   the partial number, and falls with more sections (measured in
//!   `docs/DSP_COOKBOOK.md`).
//!
//! `set_frequency` is the frequency of the **first partial**. Nothing
//! allocates after `new`/`prepare`: the excitation is built inside the line.
//! The state is `f64`, flushed below `1e-30`, and non-finite settings are
//! ignored.

use crate::noise::WhiteNoise;
use crate::phys_common::{LoopFilter, LoopSpec, FLUSH, MAX_DISPERSION_SECTIONS};
use crate::rate::effective_rate;
use core::f64::consts::PI;

/// How the loop is brought to the requested pitch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tuning {
    /// Integer delay only (the original algorithm; the pitch is quantised).
    Integer,
    /// Integer delay plus a first-order allpass for the fraction.
    Allpass,
}

/// Karplus-Strong / Jaffe-Smith plucked string.
#[derive(Clone, Debug)]
pub struct PluckedString {
    fs: f64,
    buf: Vec<f64>,
    scratch: Vec<f64>,
    n: usize,
    pos: usize,
    filter: LoopFilter,
    rng: WhiteNoise,
    freq: f64,
    t60: f64,
    stretch: f64,
    tuning: Tuning,
    stiffness: f64,
    sections: usize,
    pick: f64,
    min_hz: f64,
}

impl PluckedString {
    /// A string that can sound down to `min_hz` at `sample_rate`. Allocates.
    pub fn new(sample_rate: f32, min_hz: f32) -> Self {
        let mut s = Self {
            fs: 48_000.0,
            buf: Vec::new(),
            scratch: Vec::new(),
            n: 1,
            pos: 0,
            filter: LoopFilter::new(),
            rng: WhiteNoise::new(0x5EED),
            freq: 220.0,
            t60: 2.0,
            stretch: 0.5,
            tuning: Tuning::Allpass,
            stiffness: 0.0,
            sections: 4,
            pick: 0.0,
            min_hz: 20.0,
        };
        s.prepare(sample_rate, min_hz);
        s
    }

    /// Resize for a new rate or lowest pitch and clear. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, min_hz: f32) {
        self.fs = f64::from(effective_rate(sample_rate));
        let min = if min_hz.is_finite() { f64::from(min_hz).clamp(1.0, 2000.0) } else { 20.0 };
        self.min_hz = min;
        let cap = (self.fs / min).ceil() as usize + 8;
        self.buf.clear();
        self.buf.resize(cap, 0.0);
        self.scratch.clear();
        self.scratch.resize(cap, 0.0);
        self.pos = 0;
        self.filter.reset();
        self.redesign();
    }

    /// Silence the string.
    pub fn reset(&mut self) {
        self.buf.iter_mut().for_each(|v| *v = 0.0);
        self.filter.reset();
        self.pos = 0;
    }

    /// Frequency of the first partial in Hz (clamped to the prepared range,
    /// at most a quarter of the sample rate).
    pub fn set_frequency(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.freq = f64::from(hz);
            self.redesign();
        }
    }

    /// Time for the first partial to fall 60 dB, in seconds (0.01 to 600).
    pub fn set_decay(&mut self, t60_seconds: f32) {
        if t60_seconds.is_finite() {
            self.t60 = f64::from(t60_seconds).clamp(0.01, 600.0);
            self.redesign();
        }
    }

    /// Jaffe-Smith loss-filter parameter `S` in `[0, 1]` (`0.5` is the
    /// original two-point average and darkens the most; `0` or `1` leaves all
    /// partials decaying together).
    pub fn set_stretch(&mut self, s: f32) {
        if s.is_finite() {
            self.stretch = f64::from(s).clamp(0.0, 1.0);
            self.redesign();
        }
    }

    pub fn set_tuning(&mut self, tuning: Tuning) {
        self.tuning = tuning;
        self.redesign();
    }

    /// Inharmonicity coefficient `B >= 0` of the stiff-string formula.
    pub fn set_stiffness(&mut self, b: f32) {
        if b.is_finite() {
            self.stiffness = f64::from(b).clamp(0.0, 0.1);
            self.redesign();
        }
    }

    /// Number of dispersion allpasses used for stiffness (1 to 8).
    pub fn set_dispersion_sections(&mut self, n: usize) {
        self.sections = n.clamp(1, MAX_DISPERSION_SECTIONS);
        self.redesign();
    }

    /// Pick position as a fraction of the string, `0` (off) to `0.5`.
    pub fn set_pick_position(&mut self, beta: f32) {
        if beta.is_finite() {
            self.pick = f64::from(beta).clamp(0.0, 0.5);
        }
    }

    fn redesign(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let cap = self.buf.len();
        let f_min = self.fs / (cap as f64 - 6.0);
        let f1 = self.freq.clamp(f_min, self.fs * 0.25);
        let spec = LoopSpec {
            fs: self.fs,
            f1,
            loop_delay: self.fs / f1,
            s: self.stretch,
            t60: self.t60,
            tuned: self.tuning == Tuning::Allpass,
            stiffness: self.stiffness,
            sections: self.sections,
            sign: 1.0,
        };
        self.n = self.filter.design(&spec).clamp(1, cap);
        if self.pos >= self.n {
            self.pos = 0;
        }
    }

    /// Length in samples of the integer delay line.
    pub fn delay_samples(&self) -> usize {
        self.n
    }

    /// Frequency in Hz of partial `k` (1 = first) as the designed loop
    /// produces it: the root of the loop phase condition.
    pub fn partial_frequency(&self, k: usize) -> f64 {
        self.filter.resonance_hz(self.n, k, self.fs)
    }

    /// Loop gain `rho` (at most 1).
    pub fn loop_gain(&self) -> f64 {
        self.filter.rho()
    }

    /// Strike: fill the line with a noise burst shaped by the pick-position
    /// comb and the velocity-dependent low-pass (`velocity` in `0..=1`).
    pub fn pluck(&mut self, velocity: f32) {
        let v = if velocity.is_finite() { f64::from(velocity).clamp(0.0, 1.0) } else { 0.0 };
        let n = self.n;
        for i in 0..n {
            self.buf[i] = f64::from(self.rng.next());
        }
        // Pick position: y[i] = x[i] - x[i - D] on the circular line.
        let d = (self.pick * n as f64).round() as usize;
        if self.pick > 0.0 && d >= 1 && d < n {
            self.scratch[..n].copy_from_slice(&self.buf[..n]);
            for i in 0..n {
                let j = (i + n - d) % n;
                self.buf[i] = self.scratch[i] - self.scratch[j];
            }
        }
        // Dynamic level: one-pole low-pass, cutoff rising with velocity.
        let f1 = self.freq.clamp(self.min_hz, self.fs * 0.25);
        let fc_lo = (2.0 * f1).min(0.4 * self.fs);
        let fc_hi = 0.45 * self.fs;
        let fc = fc_lo * (fc_hi / fc_lo).powf(v);
        if fc < 0.44 * self.fs {
            let c = (-2.0 * PI * fc / self.fs).exp();
            let mut y = 0.0;
            for pass in 0..2 {
                for i in 0..n {
                    y = (1.0 - c) * self.buf[i] + c * y;
                    if pass == 1 {
                        self.buf[i] = y;
                    }
                }
            }
        }
        // Zero mean, then a fixed RMS proportional to the velocity.
        let mean = self.buf[..n].iter().sum::<f64>() / n as f64;
        let mut e = 0.0;
        for x in &mut self.buf[..n] {
            *x -= mean;
            e += *x * *x;
        }
        let rms = (e / n as f64).sqrt();
        let g = if rms > 1e-9 { 0.5 * v / rms } else { 0.0 };
        for x in &mut self.buf[..n] {
            *x *= g;
        }
        self.filter.reset();
        self.pos = 0;
    }

    /// One output sample, with `input` added into the loop (an excitation
    /// from outside, for example a bow or an exciter signal).
    #[inline]
    pub fn next_with(&mut self, input: f32) -> f32 {
        let x = if input.is_finite() { f64::from(input) } else { 0.0 };
        let y = self.buf[self.pos];
        let mut nv = self.filter.process(y) + x;
        if !nv.is_finite() {
            nv = 0.0;
            self.filter.reset();
        } else if nv.abs() < FLUSH {
            nv = 0.0;
        }
        self.buf[self.pos] = nv;
        self.pos += 1;
        if self.pos >= self.n {
            self.pos = 0;
        }
        y as f32
    }

    /// One output sample.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        self.next_with(0.0)
    }

    /// Fill `out` with samples.
    pub fn process(&mut self, out: &mut [f32]) {
        for o in out {
            *o = self.next();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_then_decays_and_stays_finite() {
        let mut s = PluckedString::new(48_000.0, 20.0);
        s.set_frequency(220.0);
        s.set_decay(0.5);
        s.pluck(1.0);
        let mut peak = 0.0f32;
        for _ in 0..4800 {
            peak = peak.max(s.next().abs());
        }
        assert!(peak > 0.05);
        for _ in 0..96_000 {
            assert!(s.next().is_finite());
        }
        assert!(s.next().abs() < 1e-3);
    }

    #[test]
    fn hostile_settings_do_not_panic() {
        let mut s = PluckedString::new(44_100.0, 30.0);
        for hz in [f32::NAN, -1.0, 0.0, 1e9, 1.0, f32::INFINITY, 20_000.0] {
            s.set_frequency(hz);
            s.set_decay(f32::NAN);
            s.set_stiffness(1e9);
            s.pluck(f32::NAN);
            s.pluck(1.0);
            for _ in 0..2000 {
                assert!(s.next_with(f32::NAN).is_finite());
            }
        }
    }
}
