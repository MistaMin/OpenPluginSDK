//! Band-limited oscillators: saw, square, pulse (PWM) and triangle, with hard sync.
//!
//! A naive saw or square has a step in it, and a step has partials at every
//! multiple of the pitch with only a 1/n fall-off. Sampled, the partials above
//! Nyquist fold back as inharmonic aliases. The cure used here is to subtract
//! from the naive waveform what a band-limited step would have changed:
//!
//! * a **step** of size `J` at time `t0` gets `J * r(x)`, where `x = (k - t0)`
//!   is the distance in samples from the step to sample `k` and
//!   `r(x) = H(x) - u(x)` is the *residual* of the band-limited step `H`
//!   (the integral of a low-pass impulse) against the ideal step `u`;
//! * a **slope change** `D` (a corner, as in a triangle) gets `D * R(x)`, where
//!   `R` is the same residual one integral further (the *BLAMP*, band-limited
//!   ramp).
//!
//! The low-pass impulse is a piecewise polynomial, so `r` and `R` are
//! polynomials of `x`, cheap to evaluate with no table. This is PolyBLEP, from
//! Välimäki and Huovilainen, "Antialiasing oscillators in subtractive
//! synthesis", IEEE Signal Processing Magazine 24(2), 2007, and its
//! generalisation by Välimäki, Pekonen and Nam, "Perceptually informed
//! synthesis of bandlimited classical waveforms using integrated polynomial
//! interpolation", J. Acoust. Soc. Am. 131(1), 2012, which builds the residuals
//! by integrating polynomial interpolation kernels (BLEP, and BLAMP for
//! piecewise-linear waves). This module uses the uniform B-spline kernels
//! `M_n` of width `n` samples (Schoenberg's cardinal B-splines; `n` = 2 is the
//! triangle, `n` = 4 the cubic). With the truncated-power form
//!
//! ```text
//! H_n(x) = sum_k (-1)^k C(n,k) (x + n/2 - k)_+^n / n!        (integral of M_n)
//! G_n(x) = sum_k (-1)^k C(n,k) (x + n/2 - k)_+^(n+1) / (n+1)!  (integral of H_n)
//! ```
//!
//! the step residual is `H_n(x)` for `x < 0` and `-H_n(-x)` for `x > 0`, and
//! the ramp residual is `G_n(-|x|)`. `n` = 2 is the classic 2-point PolyBLEP
//! residual of Välimäki and Huovilainen. A wider kernel attenuates the aliased
//! partials more, at the price of more latency and more work per event:
//!
//! * [`BlepOrder::Two`]: width 2, latency 1 sample.
//! * [`BlepOrder::Four`]: width 4, latency 2. The default.
//! * [`BlepOrder::Six`]: width 6, latency 3.
//! * [`BlepOrder::Eight`]: width 8, latency 4.
//!
//! The output is delayed by [`BandlimitedOsc::latency`] samples. That is what
//! lets the corrections reach the samples before the step, which a zero-latency
//! PolyBLEP gets only by looking at a phase it can still predict; with the delay
//! the same code also handles **hard sync** (a reset at an arbitrary instant
//! between samples) and **pulse width modulation** without any prediction.
//!
//! The phase is `f64`, so a 20 Hz tone does not drift over hours. Events inside
//! one sample interval (wrap, pulse edge, sync) are processed in time order.
//!
//! Remaining aliasing: PolyBLEP lowers aliasing, it does not remove it. The
//! measured aliased power for each order and pitch up to 5 kHz at 44.1 kHz is
//! in `tests/synth_proof.rs` and in the cookbook; for demanding uses pick a
//! wider order, run at 2x oversampling, or use the wavetable oscillator, which
//! has its own, lower, measured floor.
//!
//! Nothing here allocates, and non-finite or negative frequencies are treated
//! as zero so the phase cannot become NaN.

/// Which residual kernel to use: the B-spline width in samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlepOrder {
    /// Width 2 (1 sample latency). Cheapest.
    Two,
    /// Width 4 (2 samples latency). Default.
    Four,
    /// Width 6 (3 samples latency).
    Six,
    /// Width 8 (4 samples latency).
    Eight,
}

impl BlepOrder {
    /// Kernel width in samples.
    pub const fn width(self) -> usize {
        match self {
            Self::Two => 2,
            Self::Four => 4,
            Self::Six => 6,
            Self::Eight => 8,
        }
    }

    const fn half_width(self) -> usize {
        self.width() / 2
    }
}

/// `sum_k (-1)^k C(n,k) (t - k)_+^p / p!` with `t = x + n/2`, the cardinal
/// B-spline integrals (`p = n` for the step, `n + 1` for the ramp).
fn truncated_power_sum(n: usize, p: i32, x: f64) -> f64 {
    let t = x + n as f64 * 0.5;
    let mut binom = 1.0f64;
    let mut sum = 0.0;
    for k in 0..=n {
        let d = t - k as f64;
        if d > 0.0 {
            let term = binom * d.powi(p);
            if k % 2 == 0 { sum += term } else { sum -= term }
        }
        binom = binom * (n - k) as f64 / (k + 1) as f64;
    }
    let mut fact = 1.0;
    for i in 2..=p {
        fact *= f64::from(i);
    }
    sum / fact
}

/// Step residual `r(x) = H(x) - u(x)` for a unit step at `x = 0`.
///
/// `x` is the sample position minus the step position, in samples. `H` is the
/// integral of the B-spline low-pass impulse of the given order. `r` is odd and
/// zero for `|x| >= width / 2`.
pub fn blep_residual(order: BlepOrder, x: f64) -> f64 {
    let n = order.width();
    let ax = x.abs();
    if !(ax < n as f64 * 0.5) {
        return 0.0;
    }
    // H(-ax); for x < 0 that is H(x), for x > 0, H(x) - 1 = -H(-x).
    let h = truncated_power_sum(n, n as i32, -ax);
    if x < 0.0 { h } else { -h }
}

/// Ramp residual `R(x) = Lambda(x) - x u(x)`: the band-limited ramp minus the
/// ideal ramp for a unit slope change at `x = 0`. Even in `x`, zero for
/// `|x| >= width / 2`.
pub fn blamp_residual(order: BlepOrder, x: f64) -> f64 {
    let n = order.width();
    let ax = x.abs();
    if !(ax < n as f64 * 0.5) {
        return 0.0;
    }
    truncated_power_sum(n, n as i32 + 1, -ax)
}

/// Waveform of a [`BandlimitedOsc`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlWave {
    /// Rising ramp, -1 to 1, falling edge at the wrap.
    Saw,
    /// Pulse with a fixed 50 % width.
    Square,
    /// Pulse with the width set by [`BandlimitedOsc::set_pulse_width`].
    Pulse,
    /// Triangle, starting at its peak (+1) at phase 0.
    Triangle,
}


const RING: usize = 16;

/// A band-limited oscillator. See the module documentation.
#[derive(Clone, Copy, Debug)]
pub struct BandlimitedOsc {
    wave: BlWave,
    order: BlepOrder,
    width: f64,
    phase: f64,
    acc: [f64; RING],
    n: usize,
    last_wrap: Option<f64>,
}

#[derive(Clone, Copy)]
enum Ev {
    Wrap,
    Edge(f64),
    Sync,
}

impl BandlimitedOsc {
    /// A new oscillator at phase 0 with 4-point residuals.
    pub const fn new(wave: BlWave) -> Self {
        Self {
            wave,
            order: BlepOrder::Four,
            width: 0.5,
            phase: 0.0,
            acc: [0.0; RING],
            n: 0,
            last_wrap: None,
        }
    }

    /// Choose the residual order. Changing it mid-note may click once.
    pub fn with_order(mut self, order: BlepOrder) -> Self {
        self.order = order;
        self
    }

    pub fn set_wave(&mut self, wave: BlWave) {
        self.wave = wave;
    }

    pub fn wave(&self) -> BlWave {
        self.wave
    }

    /// Pulse width for [`BlWave::Pulse`], clamped to 0.001..=0.999. NaN is ignored.
    /// Safe to change every sample (PWM).
    pub fn set_pulse_width(&mut self, width: f32) {
        if width.is_finite() {
            self.width = f64::from(width).clamp(0.001, 0.999);
        }
    }

    /// Delay in samples between the phase and the output.
    pub fn latency(&self) -> usize {
        self.order.half_width()
    }

    /// Phase in 0..1 of the (undelayed) oscillator.
    pub fn phase(&self) -> f64 {
        self.phase
    }

    /// Set the phase (wrapped into 0..1; NaN becomes 0). Clears the delay line.
    pub fn set_phase(&mut self, phase: f64) {
        self.phase = if phase.is_finite() { phase.rem_euclid(1.0) } else { 0.0 };
        self.acc = [0.0; RING];
    }

    /// Back to phase 0 with an empty delay line.
    pub fn reset(&mut self) {
        self.set_phase(0.0);
        self.last_wrap = None;
    }

    /// When the last step wrapped, the fraction (0..1] of that sample interval
    /// that had elapsed. Feed it to another oscillator's [`Self::next_sync`] to
    /// hard-sync it to this one.
    pub fn last_wrap(&self) -> Option<f32> {
        self.last_wrap.map(|t| t as f32)
    }

    fn pulse_width(&self) -> f64 {
        if self.wave == BlWave::Square { 0.5 } else { self.width }
    }

    /// Naive value at `phase`; `right` picks the limit from the right at a jump.
    fn value(&self, ph: f64, right: bool) -> f64 {
        match self.wave {
            BlWave::Saw => 2.0 * ph - 1.0,
            BlWave::Square | BlWave::Pulse => {
                let w = self.pulse_width();
                let high = if right { ph < w } else { ph <= w && ph > 0.0 };
                if high { 1.0 } else { -1.0 }
            }
            BlWave::Triangle => {
                if ph < 0.5 { 1.0 - 4.0 * ph } else { 4.0 * ph - 3.0 }
            }
        }
    }

    /// Naive slope per unit phase.
    fn slope(&self, ph: f64, right: bool) -> f64 {
        match self.wave {
            BlWave::Saw => 2.0,
            BlWave::Square | BlWave::Pulse => 0.0,
            BlWave::Triangle => {
                let up = if right { ph >= 0.5 } else { ph > 0.5 };
                if up { 4.0 } else { -4.0 }
            }
        }
    }

    fn second_threshold(&self) -> Option<f64> {
        match self.wave {
            BlWave::Saw => None,
            BlWave::Square | BlWave::Pulse => Some(self.pulse_width()),
            BlWave::Triangle => Some(0.5),
        }
    }

    /// Add the corrections for a discontinuity at `t` (fraction of the current interval).
    fn add_event(&mut self, t: f64, jump: f64, dslope: f64) {
        if jump == 0.0 && dslope == 0.0 {
            return;
        }
        let w = self.order.half_width() as isize;
        let frac = 1.0 - t;
        for j in -w..w {
            let x = j as f64 + frac;
            let c = jump * blep_residual(self.order, x) + dslope * blamp_residual(self.order, x);
            let idx = self.n.wrapping_add_signed(j) & (RING - 1);
            self.acc[idx] += c;
        }
    }

    /// One output sample at `hz` for sample rate `sample_rate`.
    ///
    /// The result is [`Self::latency`] samples late. A non-finite or negative
    /// `hz` holds the phase; frequencies above Nyquist are limited to Nyquist.
    pub fn next(&mut self, hz: f32, sample_rate: f32) -> f32 {
        self.next_sync(hz, sample_rate, None)
    }

    /// Like [`Self::next`], with a hard-sync reset `sync` (fraction 0..1] of the
    /// way through this sample interval; see [`Self::last_wrap`]. The phase
    /// jumps to 0 at that instant and the jump is band-limited too.
    pub fn next_sync(&mut self, hz: f32, sample_rate: f32, sync: Option<f32>) -> f32 {
        let ds = if hz.is_finite() && hz > 0.0 && sample_rate.is_finite() && sample_rate > 0.0 {
            f64::from(hz) / f64::from(sample_rate)
        } else {
            0.0
        };
        self.next_normalized(ds, sync)
    }

    /// The same with the frequency as a fraction of the sample rate (`hz / sample_rate`).
    pub fn next_normalized(&mut self, cycles_per_sample: f64, sync: Option<f32>) -> f32 {
        let ds = if cycles_per_sample.is_finite() {
            cycles_per_sample.clamp(0.0, 0.5)
        } else {
            0.0
        };
        let mut sync = sync.and_then(|s| {
            let s = f64::from(s);
            (s.is_finite() && s > 0.0 && s <= 1.0).then_some(s)
        });
        self.last_wrap = None;
        let mut t = 0.0f64;
        let mut ph = self.phase;
        for _ in 0..8 {
            let mut next_t = f64::INFINITY;
            let mut ev = None;
            if let Some(s) = sync {
                if s >= t {
                    next_t = s;
                    ev = Some(Ev::Sync);
                }
            }
            if ds > 0.0 {
                let tw = t + (1.0 - ph) / ds;
                if tw <= 1.0 && tw < next_t {
                    next_t = tw;
                    ev = Some(Ev::Wrap);
                }
                if let Some(th) = self.second_threshold() {
                    if th > ph {
                        let te = t + (th - ph) / ds;
                        if te <= 1.0 && te < next_t {
                            next_t = te;
                            ev = Some(Ev::Edge(th));
                        }
                    }
                }
            }
            let Some(ev) = ev else { break };
            match ev {
                Ev::Wrap => {
                    let j = self.value(0.0, true) - self.value(1.0, false);
                    let d = (self.slope(0.0, true) - self.slope(1.0, false)) * ds;
                    self.add_event(next_t, j, d);
                    self.last_wrap = Some(next_t);
                    ph = 0.0;
                }
                Ev::Edge(th) => {
                    let j = self.value(th, true) - self.value(th, false);
                    let d = (self.slope(th, true) - self.slope(th, false)) * ds;
                    self.add_event(next_t, j, d);
                    ph = th;
                }
                Ev::Sync => {
                    let at = (ph + (next_t - t) * ds).min(1.0 - 1e-12);
                    let j = self.value(0.0, true) - self.value(at, false);
                    let d = (self.slope(0.0, true) - self.slope(at, false)) * ds;
                    self.add_event(next_t, j, d);
                    ph = 0.0;
                    sync = None;
                }
            }
            t = next_t;
        }
        ph += (1.0 - t) * ds;
        if ph >= 1.0 {
            ph -= 1.0;
        }
        self.phase = ph;

        let naive = self.value(ph, true);
        self.acc[self.n & (RING - 1)] += naive;
        let out_idx = self.n.wrapping_sub(self.order.half_width()) & (RING - 1);
        let out = self.acc[out_idx];
        self.acc[out_idx] = 0.0;
        self.n = self.n.wrapping_add(1);
        let out = out as f32;
        if out.abs() < 1e-30 { 0.0 } else { out }
    }

    /// Fill `out` at a constant frequency.
    pub fn fill(&mut self, hz: f32, sample_rate: f32, out: &mut [f32]) {
        for v in out {
            *v = self.next(hz, sample_rate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORDERS: [BlepOrder; 4] = [BlepOrder::Two, BlepOrder::Four, BlepOrder::Six, BlepOrder::Eight];

    /// Cardinal B-spline of width `n` by the Cox-de Boor recursion (independent of the closed form).
    fn bspline(n: usize, x: f64) -> f64 {
        if n == 1 {
            return if (-0.5..0.5).contains(&x) { 1.0 } else { 0.0 };
        }
        let h = n as f64 * 0.5;
        ((h + x) * bspline(n - 1, x + 0.5) + (h - x) * bspline(n - 1, x - 0.5)) / (n - 1) as f64
    }

    #[test]
    fn residuals_match_numerical_integrals_of_the_kernel() {
        for order in ORDERS {
            let n = order.width();
            let steps = 200_000;
            let lo = -(n as f64) * 0.5 - 1.0;
            let dx = -2.0 * lo / steps as f64;
            let mut h = 0.0; // H(x), running
            let mut l = 0.0; // Lambda(x), running
            let mut checked = 0;
            for i in 0..steps {
                let x = lo + (i as f64 + 1.0) * dx; // right edge of the cell
                let mid = x - 0.5 * dx;
                let h_prev = h;
                h += bspline(n, mid) * dx;
                l += 0.5 * (h_prev + h) * dx;
                // Compare at a few points, away from the step itself.
                if i % 20_000 == 7_777 && x.abs() > 0.05 {
                    let u = if x > 0.0 { 1.0 } else { 0.0 };
                    let ramp = if x > 0.0 { x } else { 0.0 };
                    let r = blep_residual(order, x);
                    let rr = blamp_residual(order, x);
                    assert!((r - (h - u)).abs() < 1e-4, "{order:?} step x={x}: {r} vs {}", h - u);
                    assert!((rr - (l - ramp)).abs() < 1e-4, "{order:?} ramp x={x}: {rr} vs {}", l - ramp);
                    checked += 1;
                }
            }
            assert!(checked >= 8);
            assert!((h - 1.0).abs() < 1e-6, "kernel integrates to 1");
            // Odd step residual, even ramp residual, compact support.
            for x in [0.1, 0.7, 1.3] {
                assert!((blep_residual(order, x) + blep_residual(order, -x)).abs() < 1e-12);
                assert!((blamp_residual(order, x) - blamp_residual(order, -x)).abs() < 1e-12);
            }
            assert_eq!(blep_residual(order, n as f64 * 0.5), 0.0);
            assert_eq!(blamp_residual(order, -(n as f64) * 0.5 - 0.1), 0.0);
        }
    }

    fn render(wave: BlWave, order: BlepOrder, ds: f64, n: usize) -> Vec<f32> {
        let mut o = BandlimitedOsc::new(wave).with_order(order);
        (0..n).map(|_| o.next_normalized(ds, None)).collect()
    }

    #[test]
    fn amplitude_and_dc_are_right() {
        for order in ORDERS {
            // Exactly periodic: 100 cycles in 8192 samples is not an integer period, use 1/64.
            let ds = 1.0 / 64.0;
            let n = 64 * 400;
            for (wave, dc) in [(BlWave::Saw, 0.0), (BlWave::Square, 0.0), (BlWave::Triangle, 0.0)] {
                let x = render(wave, order, ds, n);
                let body = &x[64..];
                let mean = body.iter().map(|v| f64::from(*v)).sum::<f64>() / body.len() as f64;
                assert!((mean - dc).abs() < 1e-4, "{wave:?} {order:?} dc {mean}");
                let peak = body.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                assert!((0.9..1.12).contains(&peak), "{wave:?} {order:?} peak {peak}");
            }
            // A pulse's mean is 2w - 1.
            for w in [0.1f32, 0.25, 0.7] {
                let mut o = BandlimitedOsc::new(BlWave::Pulse).with_order(order);
                o.set_pulse_width(w);
                let x: Vec<f32> = (0..n).map(|_| o.next_normalized(ds, None)).collect();
                let mean = x[64..].iter().map(|v| f64::from(*v)).sum::<f64>() / (n - 64) as f64;
                assert!((mean - (2.0 * f64::from(w) - 1.0)).abs() < 1e-4, "pulse {w}: {mean}");
            }
        }
    }

    #[test]
    fn frequency_is_correct() {
        // 441 Hz at 44.1 kHz: 100 samples per period; zero crossings of a saw once per period.
        let mut o = BandlimitedOsc::new(BlWave::Saw);
        let x: Vec<f32> = (0..1000).map(|_| o.next(441.0, 44_100.0)).collect();
        let crossings = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((9..=10).contains(&crossings), "{crossings}");
    }

    #[test]
    fn hostile_inputs_stay_finite() {
        let mut o = BandlimitedOsc::new(BlWave::Triangle);
        for hz in [f32::NAN, f32::INFINITY, -5.0, 1e9, 0.0, 440.0] {
            for sr in [f32::NAN, 0.0, -1.0, 44_100.0] {
                for s in [None, Some(f32::NAN), Some(2.0), Some(0.5)] {
                    for _ in 0..50 {
                        assert!(o.next_sync(hz, sr, s).is_finite());
                    }
                }
            }
        }
        o.set_pulse_width(f32::NAN);
        o.set_phase(f64::NAN);
        assert_eq!(o.phase(), 0.0);
    }

    #[test]
    fn sync_to_a_master_resets_the_slave_each_master_cycle() {
        let mut master = BandlimitedOsc::new(BlWave::Saw);
        let mut slave = BandlimitedOsc::new(BlWave::Saw);
        let (fm, fs) = (0.0123f32, 0.0317f32);
        let mut out = Vec::new();
        for _ in 0..4000 {
            master.next(fm, 1.0);
            out.push(slave.next_sync(fs, 1.0, master.last_wrap()));
        }
        assert!(out.iter().all(|v| v.is_finite() && v.abs() < 1.3));
        // The waveform repeats with the master's period (1/fm samples), not the slave's.
        let period = 1.0 / f64::from(fm);
        let a = 1500usize;
        let b = (a as f64 + period * 10.0).round() as usize;
        let mut diff = 0.0f64;
        for k in 0..8 {
            diff += f64::from((out[a + k] - out[b + k]).abs());
        }
        assert!(diff < 0.5, "not periodic with the master: {diff}");
    }
}
