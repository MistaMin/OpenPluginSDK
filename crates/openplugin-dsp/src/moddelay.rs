//! A delay line read at a fractional, per-sample delay.
//!
//! [`ModDelay`] is a circular buffer sized once, at construction or
//! [`ModDelay::prepare`], for the largest delay the plugin will ask for.
//! `write` and `read` never allocate. The read delay may change on every
//! sample (a chorus or flanger sweep) and uses one of the interpolators of
//! [`crate::interp`] (Laakso et al. 1996, "Splitting the Unit Delay", IEEE
//! Signal Processing Magazine 13(1)).
//!
//! Delay 0 is the sample written last. Call [`ModDelay::write`] and then
//! [`ModDelay::read`] once per sample (or [`ModDelay::process`], which does
//! both). The Thiran type is a recursive filter: call `read` exactly once per
//! sample so that its state advances in step.
//!
//! The policy matches `filters.rs`: a non-finite input sample is stored as
//! `0.0`, stored values below `1e-30` are flushed so a decaying tail never
//! becomes subnormal, and a non-finite or out-of-range delay is clamped into
//! `[min_delay, max_delay]` (NaN reads the minimum).
//!
//! [`Glide`] is a one-pole smoother for a delay time, so a jump in a parameter
//! moves the read point as a short sweep instead of a click.

use crate::interp::{thiran_split, Interpolation, ThiranState};
use crate::rate::effective_rate;

const FLUSH: f32 = 1e-30;

/// Circular delay with a fractional, modulated read position.
#[derive(Clone, Debug)]
pub struct ModDelay {
    buf: Vec<f32>,
    mask: usize,
    write: usize,
    max_delay: f32,
    interp: Interpolation,
    thiran: ThiranState,
    sample_rate: f32,
}

impl ModDelay {
    /// A line for delays up to `max_delay_ms` at `sample_rate` (made finite and
    /// clamped by [`effective_rate`]). Allocates.
    pub fn new(sample_rate: f32, max_delay_ms: f32, interp: Interpolation) -> Self {
        let mut line = Self {
            buf: Vec::new(),
            mask: 0,
            write: 0,
            max_delay: 0.0,
            interp,
            thiran: ThiranState::new(),
            sample_rate: 48_000.0,
        };
        line.prepare(sample_rate, max_delay_ms);
        line
    }

    /// Resize for a new rate or maximum delay and clear. Allocates, so call it
    /// from the plugin's `prepare`, not from `process`.
    pub fn prepare(&mut self, sample_rate: f32, max_delay_ms: f32) {
        self.sample_rate = effective_rate(sample_rate);
        let ms = if max_delay_ms.is_finite() { max_delay_ms.max(0.0) } else { 0.0 };
        let samples = (f64::from(ms) * f64::from(self.sample_rate) / 1000.0).ceil();
        // Cap at ten minutes so a silly argument cannot ask for gigabytes.
        let samples = samples.min(f64::from(self.sample_rate) * 600.0) as usize;
        self.max_delay = samples as f32;
        let cap = (samples + 16).next_power_of_two();
        self.buf.clear();
        self.buf.resize(cap, 0.0);
        self.mask = cap - 1;
        self.write = 0;
        self.thiran.reset();
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Largest delay in samples.
    pub fn max_delay(&self) -> f32 {
        self.max_delay
    }

    /// Smallest delay in samples for the current interpolator.
    pub fn min_delay(&self) -> f32 {
        self.interp.min_delay()
    }

    pub fn interpolation(&self) -> Interpolation {
        self.interp
    }

    /// Change the interpolator. Does not allocate.
    pub fn set_interpolation(&mut self, interp: Interpolation) {
        if interp != self.interp {
            self.interp = interp;
            self.thiran.reset();
        }
    }

    /// Clear the buffer and the interpolator state.
    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
        self.thiran.reset();
    }

    /// Store one sample. NaN and infinity are stored as zero.
    #[inline]
    pub fn write(&mut self, x: f32) {
        let x = if x.is_finite() && x.abs() >= FLUSH { x } else { 0.0 };
        self.write = (self.write + 1) & self.mask;
        self.buf[self.write] = x;
    }

    #[inline]
    fn tap(&self, delay: usize) -> f64 {
        f64::from(self.buf[self.write.wrapping_sub(delay) & self.mask])
    }

    fn clamp_delay(&self, delay: f32) -> f32 {
        let lo = self.min_delay();
        let hi = self.max_delay.max(lo);
        if delay.is_finite() { delay.clamp(lo, hi) } else { lo }
    }

    /// The signal `delay` samples ago (0 is the last `write`), interpolated.
    #[inline]
    pub fn read(&mut self, delay: f32) -> f32 {
        let d = f64::from(self.clamp_delay(delay));
        let y = if let Some(taps) = self.interp.taps(d) {
            let mut acc = 0.0;
            for k in 0..taps.len {
                acc += taps.w[k] * self.tap(taps.start + k);
            }
            acc
        } else {
            let n = self.interp.thiran_order().unwrap_or(1);
            let (m, dd) = thiran_split(d, n);
            self.thiran.set(n, dd);
            let x = self.tap(m);
            self.thiran.process(x)
        };
        let y = y as f32;
        if y.is_finite() && y.abs() >= FLUSH { y } else { 0.0 }
    }

    /// `write(x)` then `read(delay)`.
    #[inline]
    pub fn process(&mut self, x: f32, delay: f32) -> f32 {
        self.write(x);
        self.read(delay)
    }
}

/// One-pole glide for a control value such as a delay in samples.
///
/// `next(target)` moves a fraction `1 - exp(-1 / (tau * fs))` of the way to
/// `target` each call. A non-finite target is ignored.
#[derive(Clone, Copy, Debug)]
pub struct Glide {
    value: f32,
    coef: f32,
    primed: bool,
}

impl Glide {
    /// `ms` is the time constant; 0 jumps straight to the target.
    pub fn new(sample_rate: f32, ms: f32) -> Self {
        let mut g = Self { value: 0.0, coef: 1.0, primed: false };
        g.set_time(sample_rate, ms);
        g
    }

    pub fn set_time(&mut self, sample_rate: f32, ms: f32) {
        let fs = f64::from(effective_rate(sample_rate));
        let tau = if ms.is_finite() { f64::from(ms.max(0.0)) / 1000.0 } else { 0.0 };
        self.coef = if tau <= 0.0 { 1.0 } else { (1.0 - (-1.0 / (tau * fs)).exp()) as f32 };
    }

    /// Jump to `value` with no glide.
    pub fn set_immediate(&mut self, value: f32) {
        if value.is_finite() {
            self.value = value;
            self.primed = true;
        }
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    /// Advance one sample towards `target`. The first call lands on it.
    #[inline]
    pub fn next(&mut self, target: f32) -> f32 {
        if target.is_finite() {
            if self.primed {
                self.value += (target - self.value) * self.coef;
            } else {
                self.value = target;
                self.primed = true;
            }
        }
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_delays_are_exact_for_every_interpolator() {
        for interp in [
            Interpolation::Linear,
            Interpolation::Hermite,
            Interpolation::Lagrange(3),
            Interpolation::Lagrange(5),
        ] {
            let mut line = ModDelay::new(48_000.0, 10.0, interp);
            let mut out = Vec::new();
            for n in 0..32 {
                out.push(line.process(if n == 0 { 1.0 } else { 0.0 }, 7.0));
            }
            assert_eq!(out[7], 1.0, "{interp:?}");
            assert!(out.iter().enumerate().all(|(i, v)| i == 7 || v.abs() < 1e-6));
        }
    }

    #[test]
    fn hostile_input_and_delay_do_not_panic_or_poison() {
        let mut line = ModDelay::new(48_000.0, 5.0, Interpolation::Hermite);
        for d in [f32::NAN, f32::INFINITY, -5.0, 1e9, 0.0] {
            assert!(line.process(f32::NAN, d).is_finite());
            assert!(line.process(f32::INFINITY, d).is_finite());
        }
        for _ in 0..1000 {
            assert!(line.process(0.0, 100.0).abs() < 1e-30);
        }
    }

    #[test]
    fn glide_reaches_target() {
        let mut g = Glide::new(48_000.0, 5.0);
        assert_eq!(g.next(10.0), 10.0);
        for _ in 0..48_000 {
            g.next(20.0);
        }
        assert!((g.value() - 20.0).abs() < 1e-3);
        assert_eq!(g.next(f32::NAN), g.value());
    }
}
