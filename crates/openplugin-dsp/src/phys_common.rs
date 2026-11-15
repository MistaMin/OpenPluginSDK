//! Shared pieces of the physical models: the loop filter (dispersion
//! allpasses, tuning allpass, loss filter and loop gain) and the exact phase
//! formulas used to design it.
//!
//! Sources: Jaffe and Smith, "Extensions of the Karplus-Strong Plucked-String
//! Algorithm", Computer Music J. 7(2), 1983 (loss filter `(1-S) + S z^-1`,
//! allpass tuning); J. O. Smith, "Physical Audio Signal Processing" (online
//! book, CCRMA), chapters on digital waveguides, loop-filter design and
//! delay-line tuning; Van Duyne and Smith, "A Simplified Approach to Modeling
//! Dispersion Caused by Stiffness in Strings and Plates", ICMC 1994
//! (cascaded first-order allpasses); Fletcher and Rossing, "The Physics of
//! Musical Instruments" 2nd ed., section 2.18 (stiff-string partials
//! `f_k = k f_1 sqrt((1 + B k^2) / (1 + B))`).
//!
//! The design is exact at the first resonance: the loop's total phase delay
//! at `f_1` equals the requested delay, whatever the loss filter and the
//! dispersion contribute there. Everything here runs in `f64`.

use core::f64::consts::PI;

/// State below this magnitude is flushed to zero (denormal safety).
pub(crate) const FLUSH: f64 = 1e-30;
/// Largest number of dispersion allpass sections.
pub const MAX_DISPERSION_SECTIONS: usize = 8;

/// Phase delay in samples of the allpass `(a + z^-1) / (1 + a z^-1)` at `w`
/// (radians per sample, `0 < w < pi`).
pub(crate) fn allpass_tau(a: f64, w: f64) -> f64 {
    let (s, c) = w.sin_cos();
    let num = (-s).atan2(a + c);
    let den = (-a * s).atan2(1.0 + a * c);
    (den - num) / w
}

/// Coefficient of the first-order allpass whose phase delay at `w` is exactly
/// `delta` samples (J. O. Smith, PASP, "Tuning with a first-order allpass":
/// `a = sin(w (1 - delta) / 2) / sin(w (1 + delta) / 2)`).
pub(crate) fn allpass_coeff(delta: f64, w: f64) -> f64 {
    let d = (w * (1.0 + delta) * 0.5).sin();
    if d.abs() < 1e-9 {
        return 0.0;
    }
    (w * (1.0 - delta) * 0.5).sin() / d
}

/// Phase delay of the loss filter `(1 - s) + s z^-1` at `w`.
pub(crate) fn loss_tau(s: f64, w: f64) -> f64 {
    let (sn, c) = w.sin_cos();
    (s * sn).atan2((1.0 - s) + s * c) / w
}

/// Magnitude of the loss filter `(1 - s) + s z^-1` at `w`.
pub(crate) fn loss_mag(s: f64, w: f64) -> f64 {
    ((1.0 - s) * (1.0 - s) + s * s + 2.0 * s * (1.0 - s) * w.cos()).max(0.0).sqrt()
}

/// Stiff-string partial frequency, with `f1` the frequency of partial 1.
pub fn stiff_partial_hz(f1: f64, b: f64, k: f64) -> f64 {
    k * f1 * ((1.0 + b * k * k) / (1.0 + b)).sqrt()
}

/// One first-order allpass, direct form I.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Ap1 {
    pub a: f64,
    x1: f64,
    y1: f64,
}

impl Ap1 {
    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.a * x + self.x1 - self.a * self.y1;
        self.x1 = x;
        self.y1 = if y.abs() < FLUSH { 0.0 } else { y };
        y
    }
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}

/// What the loop has to do at its first resonance.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LoopSpec {
    pub fs: f64,
    /// Frequency of the first resonance.
    pub f1: f64,
    /// Loop delay in samples at `f1` (the whole loop, all parts).
    pub loop_delay: f64,
    /// Loss filter parameter `S` in `[0, 1]`.
    pub s: f64,
    pub t60: f64,
    pub tuned: bool,
    /// Inharmonicity coefficient `B` (0 = none).
    pub stiffness: f64,
    pub sections: usize,
    /// Loop sign (`1.0` or `-1.0`), applied in [`LoopFilter::process`].
    pub sign: f64,
}

/// Dispersion, tuning allpass, loss filter and gain: everything in the loop
/// except the integer delay line.
#[derive(Clone, Debug)]
pub(crate) struct LoopFilter {
    disp: [Ap1; MAX_DISPERSION_SECTIONS],
    m: usize,
    a_disp: f64,
    tune: Ap1,
    tuned: bool,
    s: f64,
    rho: f64,
    lx1: f64,
    sign: f64,
}

impl LoopFilter {
    pub fn new() -> Self {
        Self {
            disp: [Ap1::default(); MAX_DISPERSION_SECTIONS],
            m: 0,
            a_disp: 0.0,
            tune: Ap1::default(),
            tuned: false,
            s: 0.0,
            rho: 1.0,
            lx1: 0.0,
            sign: 1.0,
        }
    }

    pub fn reset(&mut self) {
        for d in &mut self.disp {
            d.reset();
        }
        self.tune.reset();
        self.lx1 = 0.0;
    }

    /// Chooses the allpass coefficients and the gain for `spec` and returns the
    /// integer delay (at least 1) the delay line must supply.
    pub fn design(&mut self, spec: &LoopSpec) -> usize {
        let w1 = 2.0 * PI * spec.f1 / spec.fs;
        let m = if spec.stiffness > 1e-9 { spec.sections.min(MAX_DISPERSION_SECTIONS) } else { 0 };
        let s = spec.s.clamp(0.0, 1.0);
        let a_d = if m > 0 {
            fit_dispersion(spec.stiffness, m, spec.f1, spec.fs, s)
        } else {
            0.0
        };
        let tau_disp = m as f64 * allpass_tau(a_d, w1);
        let r = spec.loop_delay - loss_tau(s, w1) - if m > 0 { tau_disp } else { 0.0 };
        let n = if spec.tuned {
            let n = (r - 1.0).round().max(1.0);
            let delta = (r - n).clamp(0.1, 3.0);
            self.tune.a = allpass_coeff(delta, w1);
            n
        } else {
            self.tune.a = 0.0;
            r.round().max(1.0)
        };
        self.tuned = spec.tuned;
        self.m = m;
        self.a_disp = a_d;
        for d in self.disp.iter_mut().take(m) {
            d.a = a_d;
        }
        self.s = s;
        self.sign = if spec.sign < 0.0 { -1.0 } else { 1.0 };
        let g = 10f64.powf(-3.0 * (spec.loop_delay / spec.fs) / spec.t60.max(1e-3));
        let h = loss_mag(s, w1).max(1e-6);
        self.rho = (g / h).clamp(0.0, 1.0);
        n as usize
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let mut v = x;
        for d in self.disp.iter_mut().take(self.m) {
            v = d.process(v);
        }
        if self.tuned {
            v = self.tune.process(v);
        }
        let l = (1.0 - self.s) * v + self.s * self.lx1;
        self.lx1 = if v.abs() < FLUSH { 0.0 } else { v };
        let out = self.sign * self.rho * l;
        if out.abs() < FLUSH {
            0.0
        } else {
            out
        }
    }

    /// Phase delay in samples of the filter chain at `w`.
    pub fn tau(&self, w: f64) -> f64 {
        let mut t = loss_tau(self.s, w) + self.m as f64 * allpass_tau(self.a_disp, w);
        if self.tuned {
            t += allpass_tau(self.tune.a, w);
        }
        t
    }

    pub fn rho(&self) -> f64 {
        self.rho
    }

    /// Frequency (Hz) of resonance `j` (1 = lowest): the root of
    /// `w (n + tau(w)) = phase_j`, where `phase_j` is `2 pi j` for a loop of
    /// sign `+1` and `pi (2 j - 1)` for a loop of sign `-1`.
    pub fn resonance_hz(&self, n: usize, j: usize, fs: f64) -> f64 {
        let j = j.max(1) as f64;
        let target = if self.sign < 0.0 { PI * (2.0 * j - 1.0) } else { 2.0 * PI * j };
        let phase = |w: f64| w * (n as f64 + self.tau(w));
        let (mut lo, mut hi) = (1e-6, PI * 0.98);
        if phase(hi) < target {
            return fs * 0.49;
        }
        for _ in 0..80 {
            let mid = 0.5 * (lo + hi);
            if phase(mid) < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi) * fs / (2.0 * PI)
    }
}

/// Least-squares dispersion allpass coefficient for inharmonicity `b`:
/// matches the loop phase delay at partials `k = 2..=12` (below `0.3 fs`,
/// relative to partial 1, weighted by `1/k^2` so the low partials, which the
/// ear hears as pitch, are fitted best) to what `f_k = k f_1 sqrt((1 + B k^2) / (1 + B))` requires.
fn fit_dispersion(b: f64, m: usize, f1: f64, fs: f64, s: f64) -> f64 {
    let w1 = 2.0 * PI * f1 / fs;
    let mut ks = [0.0f64; 12];
    let mut count = 0;
    for k in 2..=12usize {
        let kf = k as f64;
        if stiff_partial_hz(f1, b, kf) < 0.3 * fs {
            ks[count] = kf;
            count += 1;
        }
    }
    if count == 0 {
        return 0.0;
    }
    let base_loss = loss_tau(s, w1);
    let err = |a: f64| -> f64 {
        let t1 = allpass_tau(a, w1);
        let mut e = 0.0;
        for &kf in &ks[..count] {
            let wk = 2.0 * PI * stiff_partial_hz(f1, b, kf) / fs;
            let model = m as f64 * (allpass_tau(a, wk) - t1) + (loss_tau(s, wk) - base_loss);
            let req = (fs / f1) * (((1.0 + b) / (1.0 + b * kf * kf)).sqrt() - 1.0);
            e += (model - req) * (model - req) / (kf * kf);
        }
        e
    };
    let (mut best, mut best_e) = (0.0, err(0.0));
    for i in 1..=400 {
        let a = -0.995 * i as f64 / 400.0;
        let e = err(a);
        if e < best_e {
            best_e = e;
            best = a;
        }
    }
    // Golden-section refinement around the grid minimum.
    let step = 0.995 / 400.0;
    let (mut lo, mut hi) = ((best - step).max(-0.995), (best + step).min(0.0));
    let gr = 0.618_033_988_749_894_8;
    for _ in 0..40 {
        let c = hi - gr * (hi - lo);
        let d = lo + gr * (hi - lo);
        if err(c) < err(d) {
            hi = d;
        } else {
            lo = c;
        }
    }
    0.5 * (lo + hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allpass_coefficient_gives_the_requested_phase_delay() {
        for &delta in &[0.5, 0.8, 1.0, 1.3, 1.5] {
            for &w in &[0.01, 0.05, 0.3, 0.8, 1.4] {
                let a = allpass_coeff(delta, w);
                assert!((allpass_tau(a, w) - delta).abs() < 1e-9, "{delta} {w}");
            }
        }
    }

    #[test]
    fn loss_filter_formulas_match_direct_evaluation() {
        for &s in &[0.0, 0.2, 0.5, 0.9] {
            let w: f64 = 0.7;
            let (re, im) = ((1.0 - s) + s * w.cos(), -s * w.sin());
            assert!(((re * re + im * im).sqrt() - loss_mag(s, w)).abs() < 1e-12);
            assert!((-(im.atan2(re)) / w - loss_tau(s, w)).abs() < 1e-12);
        }
    }
}
