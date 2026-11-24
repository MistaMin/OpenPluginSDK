//! Digital waveguides: a string and a bore.
//!
//! Both follow J. O. Smith, "Physical Audio Signal Processing" (CCRMA online
//! book), chapters "Digital Waveguide Models" and "Simplified Digital
//! Waveguide Models of Strings and Acoustic Tubes", and Smith's "Efficient
//! Simulation of the Reed-Bore and Bow-String Mechanisms" (ICMC 1986) for the
//! tube. A wave travelling right and a wave travelling left each live in a
//! delay line; the ends reflect them through filters. The physical quantity at
//! a point is the sum of the two waves there.
//!
//! The reflection filters are the Jaffe-Smith loss filter and loop gain, a
//! tuning allpass and optional dispersion allpasses, all from
//! the crate's private `phys_common` module (see [`crate::pluck`] for the design). They are
//! lumped at the bridge (string) or at the right end (bore), which is allowed
//! because the model is linear and time invariant. The integer delay is shared
//! between the two lines, so the loop phase delay at the fundamental is exact.
//!
//! - [`WaveguideString`]: rigid nut (`-1`), lossy bridge (`-1` and the loop
//!   filter), pluck as an initial triangular displacement split into two
//!   half-amplitude travelling waves, an excitation input injected at the pluck
//!   position and a pickup at its own position (both with linear interpolation
//!   between samples). Positions are fractions of the string length
//!   `Ls = (loop delay) / 2`, and put comb notches at partials `k = m / beta`
//!   (the spectrum of a triangle pluck is `sin(pi k beta) / k^2`).
//! - [`WaveguideBore`]: a cylinder whose ends are [`BoreEnd::Closed`]
//!   (pressure reflection `+1`) or [`BoreEnd::Open`] (`-1`, with the loss
//!   filter standing for radiation). Open-open and closed-closed tubes
//!   resonate at `f, 2f, 3f...`; closed-open at `f, 3f, 5f...` (a quarter-wave
//!   resonator, whose loop sign `-1` makes the loop half as long for the same
//!   pitch). The excitation is added to the wave entering at the left end; the
//!   output is the pressure at the right end, `p = p+ + p-`.
//!
//! Nothing allocates after `new`/`prepare`. Non-finite input is read as 0 and
//! a non-finite state resets the filters.

use crate::phys_common::{LoopFilter, LoopSpec, FLUSH, MAX_DISPERSION_SECTIONS};
use crate::rate::effective_rate;
use core::f64::consts::PI;

fn flush(x: f64) -> f64 {
    if x.abs() < FLUSH {
        0.0
    } else {
        x
    }
}

/// Bidirectional string with a rigid nut and a lossy bridge.
/// `new` and `prepare` allocate the delay lines; everything else is
/// allocation-free and realtime-safe.
///
/// # Example
///
/// ```
/// use openplugin_dsp::WaveguideString;
///
/// let mut string = WaveguideString::new(48_000.0, 40.0);
/// string.set_frequency(110.0);
/// string.set_decay(3.0);
/// string.pluck(1.0);
/// let mut out = vec![0.0f32; 4_800];
/// string.process(&mut out);
/// assert!(out.iter().any(|s| s.abs() > 0.01));
/// assert!(out.iter().all(|s| s.is_finite()));
/// ```
#[derive(Clone, Debug)]
pub struct WaveguideString {
    fs: f64,
    r: Vec<f64>,
    l: Vec<f64>,
    nr: usize,
    nl: usize,
    wr: usize,
    wl: usize,
    filter: LoopFilter,
    freq: f64,
    t60: f64,
    stretch: f64,
    tuned: bool,
    stiffness: f64,
    sections: usize,
    pluck_pos: f64,
    inject_pos: f64,
    pickup_pos: f64,
    inject: Tap,
    pickup: Tap,
    ls: f64,
}

/// A position on the string as slots in both lines with a linear fraction.
#[derive(Clone, Copy, Debug, Default)]
struct Tap {
    r: usize,
    l: usize,
    w: f64,
}

impl WaveguideString {
    /// A string that can sound down to `min_hz`. Allocates.
    pub fn new(sample_rate: f32, min_hz: f32) -> Self {
        let mut s = Self {
            fs: 48_000.0,
            r: Vec::new(),
            l: Vec::new(),
            nr: 1,
            nl: 1,
            wr: 0,
            wl: 0,
            filter: LoopFilter::new(),
            freq: 220.0,
            t60: 2.0,
            stretch: 0.0,
            tuned: true,
            stiffness: 0.0,
            sections: 4,
            pluck_pos: 0.2,
            inject_pos: 0.2,
            pickup_pos: 0.35,
            inject: Tap::default(),
            pickup: Tap::default(),
            ls: 1.0,
        };
        s.prepare(sample_rate, min_hz);
        s
    }

    /// Resize for a new rate or lowest pitch and clear. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, min_hz: f32) {
        self.fs = f64::from(effective_rate(sample_rate));
        let min = if min_hz.is_finite() { f64::from(min_hz).clamp(1.0, 2000.0) } else { 20.0 };
        let cap = (self.fs / min).ceil() as usize / 2 + 8;
        self.r.clear();
        self.r.resize(cap, 0.0);
        self.l.clear();
        self.l.resize(cap, 0.0);
        self.filter.reset();
        self.redesign();
    }

    /// Silences the string. Does not allocate.
    pub fn reset(&mut self) {
        self.r.iter_mut().for_each(|v| *v = 0.0);
        self.l.iter_mut().for_each(|v| *v = 0.0);
        self.filter.reset();
        self.wr = 0;
        self.wl = 0;
    }

    /// Frequency of the first partial in Hz.
    pub fn set_frequency(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.freq = f64::from(hz);
            self.redesign();
        }
    }

    /// T60 of the first partial, seconds (0.01 to 600).
    pub fn set_decay(&mut self, t60_seconds: f32) {
        if t60_seconds.is_finite() {
            self.t60 = f64::from(t60_seconds).clamp(0.01, 600.0);
            self.redesign();
        }
    }

    /// Bridge loss-filter parameter `S` in `[0, 1]`: 0 decays all partials
    /// together, 0.5 darkens the most.
    pub fn set_stretch(&mut self, s: f32) {
        if s.is_finite() {
            self.stretch = f64::from(s).clamp(0.0, 1.0);
            self.redesign();
        }
    }

    /// Fractional tuning allpass on (default) or off.
    pub fn set_tuned(&mut self, on: bool) {
        self.tuned = on;
        self.redesign();
    }

    /// Inharmonicity coefficient `B >= 0`.
    pub fn set_stiffness(&mut self, b: f32) {
        if b.is_finite() {
            self.stiffness = f64::from(b).clamp(0.0, 0.1);
            self.redesign();
        }
    }

    /// Number of dispersion allpass sections used for the stiffness, clamped to
    /// 1..=[`crate::MAX_DISPERSION_SECTIONS`].
    pub fn set_dispersion_sections(&mut self, n: usize) {
        self.sections = n.clamp(1, MAX_DISPERSION_SECTIONS);
        self.redesign();
    }

    /// Where `pluck` plucks, as a fraction of the string (0.02 to 0.5).
    pub fn set_pluck_position(&mut self, beta: f32) {
        if beta.is_finite() {
            self.pluck_pos = f64::from(beta).clamp(0.02, 0.5);
        }
    }

    /// Where the excitation input enters, as a fraction of the string.
    pub fn set_input_position(&mut self, x: f32) {
        if x.is_finite() {
            self.inject_pos = f64::from(x).clamp(0.0, 0.98);
            self.update_taps();
        }
    }

    /// Where the output is read, as a fraction of the string.
    pub fn set_pickup_position(&mut self, x: f32) {
        if x.is_finite() {
            self.pickup_pos = f64::from(x).clamp(0.0, 0.98);
            self.update_taps();
        }
    }

    fn redesign(&mut self) {
        if self.r.is_empty() {
            return;
        }
        let cap = self.r.len();
        let f_min = self.fs / (2.0 * cap as f64 - 20.0);
        let f1 = self.freq.clamp(f_min, self.fs * 0.2);
        let spec = LoopSpec {
            fs: self.fs,
            f1,
            loop_delay: self.fs / f1,
            s: self.stretch,
            t60: self.t60,
            tuned: self.tuned,
            stiffness: self.stiffness,
            sections: self.sections,
            sign: 1.0,
        };
        let n = self.filter.design(&spec).clamp(2, 2 * cap - 2);
        self.nr = n / 2;
        self.nl = n - self.nr;
        self.wr %= self.nr;
        self.wl %= self.nl;
        self.ls = 0.5 * ((self.nr + self.nl) as f64 + self.filter.tau(2.0 * PI * f1 / self.fs));
        self.update_taps();
    }

    /// Position `frac` of the string (0 nut, 1 bridge) as slots. A right-going
    /// slot `i` sits at `x = i` samples from the nut, a left-going slot `j`
    /// at `x = j + 1` (one tick before it reflects), and the string is
    /// `Ls = (nr + nl + chain delay) / 2` samples long.
    fn tap_at(&self, frac: f64) -> Tap {
        let hi = (self.nr.min(self.nl) as f64 - 1.5).max(1.0);
        let x = (frac * self.ls).clamp(1.0, hi);
        let i = x.floor();
        Tap { r: i as usize, l: (x - 1.0).floor().max(0.0) as usize, w: x - i }
    }

    fn update_taps(&mut self) {
        if self.r.is_empty() {
            return;
        }
        self.inject = self.tap_at(self.inject_pos);
        self.pickup = self.tap_at(self.pickup_pos);
    }

    /// Length of the string in samples (half the loop delay).
    pub fn string_length_samples(&self) -> f64 {
        self.ls
    }

    /// Total delay in the two lines.
    pub fn delay_samples(&self) -> usize {
        self.nr + self.nl
    }

    /// Frequency of partial `k` as designed.
    pub fn partial_frequency(&self, k: usize) -> f64 {
        self.filter.resonance_hz(self.nr + self.nl, k, self.fs)
    }

    #[inline]
    fn r_at(&self, p: usize) -> usize {
        (self.wr + self.nr - 1 - p.min(self.nr - 1)) % self.nr
    }

    #[inline]
    fn l_at(&self, p: usize) -> usize {
        (self.wl + p.min(self.nl - 1)) % self.nl
    }

    /// Pluck: an initial triangular displacement peaking at the pluck
    /// position, zero initial velocity (each travelling wave carries half).
    pub fn pluck(&mut self, velocity: f32) {
        let v = if velocity.is_finite() { f64::from(velocity).clamp(0.0, 1.0) } else { 0.0 };
        self.reset();
        let ls = self.ls;
        let xp = self.pluck_pos * ls;
        let shape = |x: f64| if x <= xp { x / xp } else { ((ls - x) / (ls - xp)).max(0.0) };
        for i in 0..self.nr {
            let idx = self.nr - 1 - i;
            self.r[idx] = 0.5 * v * shape(i as f64);
        }
        for j in 0..self.nl {
            self.l[j] = 0.5 * v * shape(j as f64 + 1.0);
        }
    }

    /// One output sample (the pickup), with `input` injected at the input
    /// position.
    #[inline]
    pub fn next_with(&mut self, input: f32) -> f32 {
        let x = if input.is_finite() { f64::from(input) } else { 0.0 };
        let r_end = self.r[self.wr];
        let l_end = self.l[self.wl];
        let mut new_l = -self.filter.process(r_end);
        let mut new_r = -l_end;
        if !new_l.is_finite() || !new_r.is_finite() {
            self.filter.reset();
            new_l = 0.0;
            new_r = 0.0;
        }
        self.r[self.wr] = flush(new_r);
        self.l[self.wl] = flush(new_l);
        self.wr = (self.wr + 1) % self.nr;
        self.wl = (self.wl + 1) % self.nl;
        if x != 0.0 {
            let t = self.inject;
            let (a0, a1) = (0.5 * x * (1.0 - t.w), 0.5 * x * t.w);
            let (r0, r1) = (self.r_at(t.r), self.r_at(t.r + 1));
            let (l0, l1) = (self.l_at(t.l), self.l_at(t.l + 1));
            self.r[r0] += a0;
            self.r[r1] += a1;
            self.l[l0] += a0;
            self.l[l1] += a1;
        }
        let t = self.pickup;
        let right = (1.0 - t.w) * self.r[self.r_at(t.r)] + t.w * self.r[self.r_at(t.r + 1)];
        let left = (1.0 - t.w) * self.l[self.l_at(t.l)] + t.w * self.l[self.l_at(t.l + 1)];
        (right + left) as f32
    }

    /// One output sample with no excitation input.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        self.next_with(0.0)
    }

    /// Fills `out` with output samples, no excitation input.
    pub fn process(&mut self, out: &mut [f32]) {
        for o in out {
            *o = self.next();
        }
    }
}

/// How a bore ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BoreEnd {
    /// Pressure reflection `+1` (a wall).
    Closed,
    /// Pressure reflection `-1` (an open end).
    Open,
}

impl BoreEnd {
    fn sign(self) -> f64 {
        match self {
            BoreEnd::Closed => 1.0,
            BoreEnd::Open => -1.0,
        }
    }
}

/// A cylindrical bore with an excitation input.
/// `new` and `prepare` allocate the delay lines; everything else is
/// allocation-free and realtime-safe.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{BoreEnd, WaveguideBore};
///
/// // Closed at the mouthpiece, open at the bell: a clarinet-like bore.
/// let mut bore = WaveguideBore::new(48_000.0, 50.0, BoreEnd::Closed, BoreEnd::Open);
/// bore.set_frequency(147.0);
/// let mut io = vec![0.0f32; 2_400];
/// io[0] = 1.0; // an impulse of breath
/// bore.process(&mut io);
/// assert!(io.iter().all(|s| s.is_finite()));
/// ```
#[derive(Clone, Debug)]
pub struct WaveguideBore {
    fs: f64,
    r: Vec<f64>,
    l: Vec<f64>,
    nr: usize,
    nl: usize,
    wr: usize,
    wl: usize,
    filter: LoopFilter,
    left: BoreEnd,
    right: BoreEnd,
    freq: f64,
    t60: f64,
    stretch: f64,
    dc_x1: f64,
    dc_y1: f64,
}

impl WaveguideBore {
    /// A bore that can sound down to `min_hz`. Allocates.
    pub fn new(sample_rate: f32, min_hz: f32, left: BoreEnd, right: BoreEnd) -> Self {
        let mut b = Self {
            fs: 48_000.0,
            r: Vec::new(),
            l: Vec::new(),
            nr: 1,
            nl: 1,
            wr: 0,
            wl: 0,
            filter: LoopFilter::new(),
            left,
            right,
            freq: 220.0,
            t60: 1.0,
            stretch: 0.2,
            dc_x1: 0.0,
            dc_y1: 0.0,
        };
        b.prepare(sample_rate, min_hz);
        b
    }

    /// Resize and clear. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, min_hz: f32) {
        self.fs = f64::from(effective_rate(sample_rate));
        let min = if min_hz.is_finite() { f64::from(min_hz).clamp(1.0, 2000.0) } else { 20.0 };
        let cap = (self.fs / min).ceil() as usize / 2 + 8;
        self.r.clear();
        self.r.resize(cap, 0.0);
        self.l.clear();
        self.l.resize(cap, 0.0);
        self.filter.reset();
        self.redesign();
    }

    /// Silences the bore. Does not allocate.
    pub fn reset(&mut self) {
        self.r.iter_mut().for_each(|v| *v = 0.0);
        self.l.iter_mut().for_each(|v| *v = 0.0);
        self.filter.reset();
        self.wr = 0;
        self.wl = 0;
        self.dc_x1 = 0.0;
        self.dc_y1 = 0.0;
    }

    /// Changes how the two ends reflect.
    pub fn set_ends(&mut self, left: BoreEnd, right: BoreEnd) {
        self.left = left;
        self.right = right;
        self.redesign();
    }

    /// Frequency of the lowest resonance in Hz.
    pub fn set_frequency(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 {
            self.freq = f64::from(hz);
            self.redesign();
        }
    }

    /// T60 of the lowest resonance in seconds (0.01 to 600).
    pub fn set_decay(&mut self, t60_seconds: f32) {
        if t60_seconds.is_finite() {
            self.t60 = f64::from(t60_seconds).clamp(0.01, 600.0);
            self.redesign();
        }
    }

    /// End-loss filter parameter `S` in `[0, 1]` (higher modes die faster
    /// around 0.5; this is the radiation low-pass of an open end).
    pub fn set_stretch(&mut self, s: f32) {
        if s.is_finite() {
            self.stretch = f64::from(s).clamp(0.0, 1.0);
            self.redesign();
        }
    }

    fn net_sign(&self) -> f64 {
        self.left.sign() * self.right.sign()
    }

    fn redesign(&mut self) {
        if self.r.is_empty() {
            return;
        }
        let cap = self.r.len();
        let sign = self.net_sign();
        let f_min = self.fs / (2.0 * cap as f64 - 20.0) * if sign < 0.0 { 0.5 } else { 1.0 };
        let f1 = self.freq.clamp(f_min, self.fs * 0.2);
        let loop_delay = if sign < 0.0 { 0.5 * self.fs / f1 } else { self.fs / f1 };
        let spec = LoopSpec {
            fs: self.fs,
            f1,
            loop_delay,
            s: self.stretch,
            t60: self.t60,
            tuned: true,
            stiffness: 0.0,
            sections: 0,
            sign,
        };
        let n = self.filter.design(&spec).clamp(2, 2 * cap - 2);
        self.nr = n / 2;
        self.nl = n - self.nr;
        self.wr %= self.nr;
        self.wl %= self.nl;
    }

    /// Total delay in the two lines.
    pub fn delay_samples(&self) -> usize {
        self.nr + self.nl
    }

    /// Frequency of resonance `j` (1 = lowest) as designed.
    pub fn resonance_frequency(&self, j: usize) -> f64 {
        self.filter.resonance_hz(self.nr + self.nl, j, self.fs)
    }

    /// Tube length in metres for speed of sound `c` (about 343 m/s): `c / 2f`
    /// for open-open and closed-closed, `c / 4f` for closed-open.
    pub fn physical_length(&self, c: f64) -> f64 {
        let f1 = self.freq;
        if self.net_sign() < 0.0 {
            c / (4.0 * f1)
        } else {
            c / (2.0 * f1)
        }
    }

    /// One output sample (pressure at the right end) for an excitation
    /// sample `input` entering at the left end.
    #[inline]
    pub fn next_with(&mut self, input: f32) -> f32 {
        let x = if input.is_finite() { f64::from(input) } else { 0.0 };
        let r_end = self.r[self.wr];
        let l_end = self.l[self.wl];
        let mut new_l = self.filter.process(r_end);
        let mut new_r = l_end + x;
        if !new_l.is_finite() || !new_r.is_finite() {
            self.filter.reset();
            new_l = 0.0;
            new_r = 0.0;
        }
        // Pressure at the right end: incoming wave plus its reflection, where
        // the reflection alone carries the right end's sign (the left sign is
        // part of the lumped loop filter).
        let p = r_end + self.left.sign() * new_l;
        self.r[self.wr] = flush(new_r);
        self.l[self.wl] = flush(new_l);
        self.wr = (self.wr + 1) % self.nr;
        self.wl = (self.wl + 1) % self.nl;
        // DC blocker (5 Hz) so closed or open pairs do not hold an offset.
        let c = 1.0 - 2.0 * PI * 5.0 / self.fs;
        let y = p - self.dc_x1 + c * self.dc_y1;
        self.dc_x1 = flush(p);
        self.dc_y1 = flush(y);
        y as f32
    }

    /// One output sample with no excitation input.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        self.next_with(0.0)
    }

    /// Process `io` in place: excitation in, pressure out.
    pub fn process(&mut self, io: &mut [f32]) {
        for s in io {
            *s = self.next_with(*s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_rings_and_decays() {
        let mut s = WaveguideString::new(48_000.0, 20.0);
        s.set_frequency(200.0);
        s.set_decay(0.3);
        s.pluck(1.0);
        let mut peak = 0.0f32;
        for _ in 0..2400 {
            peak = peak.max(s.next().abs());
        }
        assert!(peak > 0.05, "{peak}");
        for _ in 0..96_000 {
            assert!(s.next().is_finite());
        }
        assert!(s.next().abs() < 1e-3);
    }

    #[test]
    fn hostile_settings_do_not_panic() {
        let mut s = WaveguideString::new(44_100.0, 30.0);
        let mut b = WaveguideBore::new(44_100.0, 30.0, BoreEnd::Closed, BoreEnd::Open);
        for hz in [f32::NAN, -1.0, 0.0, 1e9, 1.0, f32::INFINITY] {
            s.set_frequency(hz);
            b.set_frequency(hz);
            s.set_pickup_position(f32::NAN);
            s.set_input_position(5.0);
            s.pluck(f32::NAN);
            for _ in 0..1000 {
                assert!(s.next_with(f32::NAN).is_finite());
                assert!(b.next_with(f32::INFINITY).is_finite());
            }
        }
    }
}
