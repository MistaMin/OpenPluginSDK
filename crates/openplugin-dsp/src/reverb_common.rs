//! Shared plumbing for the reverb modules: a power-of-two delay ring with
//! cubic interpolation, a gliding pre-delay, a prime search, and the
//! denormal / NaN guards.
//!
//! Guard policy (as `delay.rs`): a non-finite input sample is treated as `0.0`;
//! every value written back into a recursive path goes through [`flush`], which
//! maps NaN, infinities, denormals and absurd magnitudes to `0.0`, so a bad
//! value can never circulate in a feedback loop.

/// Input sanitiser: non-finite becomes `0.0`.
#[inline]
pub(crate) fn clean(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// State guard: flushes denormals (below 1e-20) and non-finite or absurdly
/// large values (above 1e18) to zero.
#[inline]
pub(crate) fn flush(x: f32) -> f32 {
    let a = x.abs();
    if (1.0e-20..1.0e18).contains(&a) { x } else { 0.0 }
}

/// `f64` twin of [`flush`] for filter states.
#[inline]
pub(crate) fn flush64(x: f64) -> f64 {
    let a = x.abs();
    if (1.0e-30..1.0e18).contains(&a) { x } else { 0.0 }
}

/// Smallest prime that is `>= n` (trial division; `n` is a few thousand at most).
pub(crate) fn next_prime(n: usize) -> usize {
    let mut c = n.max(2);
    loop {
        let mut prime = true;
        let mut d = 2;
        while d * d <= c {
            if c % d == 0 {
                prime = false;
                break;
            }
            d += 1;
        }
        if prime {
            return c;
        }
        c += 1;
    }
}

/// Power-of-two ring buffer. `at(d)` is read-before-push: it returns the value
/// pushed `d` calls ago, so `at(D)` followed by `push` forms a loop of exactly
/// `D` samples.
#[derive(Clone, Debug)]
pub(crate) struct Ring {
    buf: Vec<f32>,
    mask: usize,
    w: usize,
}

impl Ring {
    /// A ring that can be read at any delay up to `max_delay` (the interpolating
    /// reader needs two more samples of history, which the padding provides).
    pub(crate) fn new(max_delay: usize) -> Self {
        let size = (max_delay + 8).next_power_of_two();
        Self {
            buf: vec![0.0; size],
            mask: size - 1,
            w: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.buf.fill(0.0);
        self.w = 0;
    }

    #[inline]
    pub(crate) fn push(&mut self, v: f32) {
        self.buf[self.w] = v;
        self.w = (self.w + 1) & self.mask;
    }

    #[inline]
    pub(crate) fn at(&self, d: usize) -> f32 {
        self.buf[self.w.wrapping_sub(d) & self.mask]
    }

    /// First-order allpass (Thiran) fractional-delay read at delay `d >= 2`,
    /// with the interpolator's one state in `state`. Unlike a polynomial
    /// interpolator it has exactly unit gain at every frequency, so a
    /// modulated line does not shorten the high band's decay (J. Dattorro,
    /// JAES 45(9), 1997, section on modulated delays; Thiran, "Recursive
    /// digital filters with maximally flat group delay", IEEE Trans. Circuit
    /// Theory 18(6), 1971). The fraction is kept in `[0.5, 1.5)`, where the
    /// pole stays inside `(-0.34, 0.2]`.
    #[inline]
    pub(crate) fn at_allpass(&self, d: f32, state: &mut f32) -> f32 {
        let d = d.clamp(2.0, (self.buf.len() - 6) as f32);
        let i = (d - 0.5) as usize;
        let delta = d - i as f32;
        let eta = (1.0 - delta) / (1.0 + delta);
        let y = eta * self.at(i) + self.at(i + 1) - eta * *state;
        *state = flush(y);
        y
    }

    /// Linear read for a ring that was pushed first: delay 0 is the value just
    /// pushed.
    #[inline]
    pub(crate) fn after_push_lin(&self, d: f32) -> f32 {
        let d = d.clamp(0.0, (self.buf.len() - 6) as f32);
        let i = d as usize;
        let f = d - i as f32;
        self.at(i + 1) * (1.0 - f) + self.at(i + 2) * f
    }
}

/// Stereo pre-delay. The delay glides toward a new setting at most 0.05 sample
/// per sample (a 5 % pitch bend while it moves) so changes do not click.
#[derive(Clone, Debug)]
pub(crate) struct PreDelay {
    l: Ring,
    r: Ring,
    cur: f32,
    target: f32,
    max: f32,
}

impl PreDelay {
    pub(crate) fn new(sample_rate: f32, max_ms: f32, ms: f32) -> Self {
        let max = (max_ms * 0.001 * sample_rate).ceil().max(1.0);
        let cap = max as usize + 4;
        let target = (ms * 0.001 * sample_rate).clamp(0.0, max);
        Self {
            l: Ring::new(cap),
            r: Ring::new(cap),
            cur: target,
            target,
            max,
        }
    }

    pub(crate) fn set_ms(&mut self, sample_rate: f32, ms: f32) {
        if ms.is_finite() {
            self.target = (ms * 0.001 * sample_rate).clamp(0.0, self.max);
        }
    }

    pub(crate) fn target_samples(&self) -> usize {
        self.target.ceil() as usize
    }

    pub(crate) fn reset(&mut self) {
        self.l.clear();
        self.r.clear();
        self.cur = self.target;
    }

    #[inline]
    pub(crate) fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let step = (self.target - self.cur).clamp(-0.05, 0.05);
        self.cur += step;
        self.l.push(l);
        self.r.push(r);
        (self.l.after_push_lin(self.cur), self.r.after_push_lin(self.cur))
    }
}

/// Mid/side width: `1.0` leaves the signal, `0.0` is mono, `2.0` doubles the side.
#[inline]
pub(crate) fn apply_width(l: f32, r: f32, width: f32) -> (f32, f32) {
    let m = 0.5 * (l + r);
    let s = 0.5 * (l - r) * width;
    (m + s, m - s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primes() {
        assert_eq!(next_prime(0), 2);
        assert_eq!(next_prime(24), 29);
        assert_eq!(next_prime(31), 31);
    }

    #[test]
    fn ring_integer_and_allpass_reads() {
        let mut r = Ring::new(64);
        for i in 0..40 {
            r.push(i as f32);
        }
        assert_eq!(r.at(1), 39.0);
        assert_eq!(r.at(10), 30.0);
        let mut st = 0.0;
        assert!((r.at_allpass(10.0, &mut st) - 30.0).abs() < 1e-5);

        // A slow sine read at a fractional delay comes out delayed by that amount.
        let mut r = Ring::new(64);
        let mut st = 0.0;
        let w = 0.05f32;
        let mut worst = 0.0f32;
        for n in 0..400 {
            r.push((w * n as f32).sin());
            let y = r.at_allpass(10.5 + 1.0, &mut st); // push-first: delay d reads n+1-d
            if n > 100 {
                let want = (w * (n as f32 + 1.0 - 11.5)).sin();
                worst = worst.max((y - want).abs());
            }
        }
        assert!(worst < 0.01, "{worst}");
    }

    #[test]
    fn flush_kills_bad_values() {
        assert_eq!(flush(f32::NAN), 0.0);
        assert_eq!(flush(f32::INFINITY), 0.0);
        assert_eq!(flush(1e-30), 0.0);
        assert_eq!(flush(0.5), 0.5);
        assert_eq!(clean(f32::NAN), 0.0);
    }

    #[test]
    fn predelay_delays_by_the_setting() {
        let mut p = PreDelay::new(1000.0, 100.0, 10.0);
        let mut out = Vec::new();
        out.push(p.process(1.0, 1.0).0);
        for _ in 0..20 {
            out.push(p.process(0.0, 0.0).0);
        }
        assert_eq!(out.iter().position(|v| *v > 0.99), Some(10));
    }
}
