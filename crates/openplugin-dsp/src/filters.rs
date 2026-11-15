//! Simple filters: one-pole low/high-pass, DC blocker, first- and second-order
//! all-pass, and Butterworth cascades of order 1 to 8.
//!
//! All coefficients come from [`effective_rate`], so they follow the real host
//! rate. Everything is allocation-free with `f64` state, flushes values below
//! `1e-30` to zero, treats a non-finite input sample as `0.0` (it never reaches
//! the state) and resets the state if a result would leave the `f32` range.
//! `configure`/`set` calls ignore NaN or infinite parameters. Frequencies are
//! clamped to 1 Hz..0.499 fs.
//!
//! The one-pole and all-pass sections are the TPT (bilinear, pre-warped)
//! forms from Zavalishin, "The Art of VA Filter Design", chapter 3; the
//! Butterworth cascades factor the order-`N` polynomial into the standard
//! sections `Q_k = 1 / (2 cos((2k + 1) pi / (2N)))` (Butterworth 1930; any
//! filter-design text) and run them as [`Svf`]s plus one one-pole for odd
//! orders.

use crate::rate::effective_rate;
use crate::svf::{finite_out, flush, sanitize_in, Svf, SvfMode};
use std::f64::consts::PI;

fn tan_g(sample_rate: f32, freq: f32) -> Option<f64> {
    if !freq.is_finite() {
        return None;
    }
    let fs = f64::from(effective_rate(sample_rate));
    let f = f64::from(freq).clamp(1.0, 0.499 * fs);
    Some((PI * f / fs).tan())
}

/// Response of a [`OnePoleFilter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnePoleMode {
    /// `1 / (s + 1)` with `s` normalised to the corner.
    LowPass,
    /// `s / (s + 1)`.
    HighPass,
}

/// First-order low/high-pass, 6 dB/octave, -3 dB at the corner frequency.
///
/// Named `OnePoleFilter` to avoid confusion with the parameter smoother
/// `openplugin_core::OnePole`.
#[derive(Debug, Clone)]
pub struct OnePoleFilter {
    big_g: f64,
    s: f64,
    mode: OnePoleMode,
}

impl Default for OnePoleFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl OnePoleFilter {
    /// A 1 kHz-corner-at-1-rad low-pass placeholder: passes DC, set a real
    /// corner with [`OnePoleFilter::set`].
    pub fn new() -> Self {
        Self {
            big_g: 1.0,
            s: 0.0,
            mode: OnePoleMode::LowPass,
        }
    }

    pub fn reset(&mut self) {
        self.s = 0.0;
    }

    pub fn set(&mut self, mode: OnePoleMode, sample_rate: f32, freq: f32) {
        if let Some(g) = tan_g(sample_rate, freq) {
            self.big_g = g / (1.0 + g);
            self.mode = mode;
        }
    }

    /// `(low, high)` outputs for one sample; `low + high == x`.
    #[inline]
    pub(crate) fn split(&mut self, x: f64) -> (f64, f64) {
        let v = (x - self.s) * self.big_g;
        let lp = v + self.s;
        self.s = flush(lp + v);
        (lp, x - lp)
    }

    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let (lp, hp) = self.split(sanitize_in(x));
        let y = match self.mode {
            OnePoleMode::LowPass => lp,
            OnePoleMode::HighPass => hp,
        };
        finite_out(y).unwrap_or_else(|| {
            self.reset();
            0.0
        })
    }
}

/// First-order all-pass: unit gain, phase -90 degrees at the corner (+1 at DC,
/// -1 at Nyquist). `H(s) = (1 - s) / (1 + s)`.
#[derive(Debug, Clone, Default)]
pub struct Allpass1 {
    pole: OnePoleFilter,
}

impl Allpass1 {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.pole.reset();
    }

    pub fn set(&mut self, sample_rate: f32, freq: f32) {
        self.pole.set(OnePoleMode::LowPass, sample_rate, freq);
    }

    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let x = sanitize_in(x);
        let (lp, _) = self.pole.split(x);
        finite_out(2.0 * lp - x).unwrap_or_else(|| {
            self.reset();
            0.0
        })
    }
}

/// Second-order all-pass: unit gain, phase -180 degrees at the centre
/// frequency, sharper for higher `q`. `H(s) = (s^2 - s/Q + 1) / (s^2 + s/Q + 1)`.
#[derive(Debug, Clone, Default)]
pub struct Allpass2 {
    svf: Svf,
}

impl Allpass2 {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.svf.reset();
    }

    pub fn set(&mut self, sample_rate: f32, freq: f32, q: f32) {
        self.svf.configure(SvfMode::AllPass, sample_rate, freq, q, 0.0);
    }

    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        self.svf.process_sample(x)
    }
}

/// First-order DC blocker: a bilinear high-pass with its -3 dB corner at
/// `freq` (20 Hz by default), unit gain at Nyquist.
///
/// `y[n] = (x[n] - x[n-1]) / (1 + g) + R y[n-1]`, `g = tan(pi f / fs)`,
/// `R = (1 - g) / (1 + g)`.
#[derive(Debug, Clone)]
pub struct DcBlocker {
    gain: f64,
    r: f64,
    x1: f64,
    y1: f64,
}

impl Default for DcBlocker {
    fn default() -> Self {
        Self::new(48_000.0, 20.0)
    }
}

impl DcBlocker {
    pub fn new(sample_rate: f32, freq: f32) -> Self {
        let mut s = Self {
            gain: 1.0,
            r: 0.0,
            x1: 0.0,
            y1: 0.0,
        };
        s.set(sample_rate, freq);
        s
    }

    pub fn set(&mut self, sample_rate: f32, freq: f32) {
        if let Some(g) = tan_g(sample_rate, freq) {
            self.gain = 1.0 / (1.0 + g);
            self.r = (1.0 - g) / (1.0 + g);
        }
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }

    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let x = sanitize_in(x);
        let y = self.gain * (x - self.x1) + self.r * self.y1;
        match finite_out(y) {
            Some(out) => {
                self.x1 = x;
                self.y1 = flush(y);
                out
            }
            None => {
                self.reset();
                0.0
            }
        }
    }
}

/// Low-pass or high-pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButterworthKind {
    LowPass,
    HighPass,
}

/// Highest supported Butterworth order.
pub const MAX_BUTTERWORTH_ORDER: usize = 8;

/// Butterworth low/high-pass of order 1 to 8 (6 to 48 dB/octave), -3 dB at the
/// cutoff, maximally flat. Odd orders use one [`OnePoleFilter`]; each pair of
/// poles is one [`Svf`] section with its Butterworth Q.
#[derive(Debug, Clone)]
pub struct Butterworth {
    order: usize,
    pole: OnePoleFilter,
    sections: [Svf; 4],
}

impl Default for Butterworth {
    fn default() -> Self {
        Self::new()
    }
}

impl Butterworth {
    /// Order 2, pass-through until [`Butterworth::set`].
    pub fn new() -> Self {
        Self {
            order: 2,
            pole: OnePoleFilter::new(),
            sections: [Svf::new(), Svf::new(), Svf::new(), Svf::new()],
        }
    }

    pub fn reset(&mut self) {
        self.pole.reset();
        for s in &mut self.sections {
            s.reset();
        }
    }

    pub fn order(&self) -> usize {
        self.order
    }

    /// `Q` of the `k`-th second-order section of an order-`order` Butterworth.
    pub fn section_q(order: usize, k: usize) -> f64 {
        let offset = if order % 2 == 1 { 1.0 } else { 0.5 };
        let phi = PI * (k as f64 + offset) / order as f64; // angle from the negative real axis
        1.0 / (2.0 * phi.cos())
    }

    /// Sets kind, order (clamped to 1..=8), rate and cutoff. Changing the order
    /// clears the state; changing only the cutoff does not.
    pub fn set(&mut self, kind: ButterworthKind, order: usize, sample_rate: f32, freq: f32) {
        if !freq.is_finite() {
            return;
        }
        let order = order.clamp(1, MAX_BUTTERWORTH_ORDER);
        if order != self.order {
            self.order = order;
            self.reset();
        }
        let (omode, smode) = match kind {
            ButterworthKind::LowPass => (OnePoleMode::LowPass, SvfMode::LowPass),
            ButterworthKind::HighPass => (OnePoleMode::HighPass, SvfMode::HighPass),
        };
        if order % 2 == 1 {
            self.pole.set(omode, sample_rate, freq);
        }
        for (k, section) in self.sections.iter_mut().enumerate().take(order / 2) {
            let q = Self::section_q(order, k) as f32;
            section.configure(smode, sample_rate, freq, q, 0.0);
        }
    }

    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let mut y = x;
        if self.order % 2 == 1 {
            y = self.pole.process_sample(y);
        }
        for k in 0..self.order / 2 {
            y = self.sections[k].process_sample(y);
        }
        y
    }
}
