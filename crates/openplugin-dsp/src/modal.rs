//! Modal resonator bank.
//!
//! A struck bar, plate or membrane rings as a sum of decaying sinusoids, its
//! modes (Fletcher and Rossing, "The Physics of Musical Instruments", 2nd ed.,
//! 1998; Cook, "Real Sound Synthesis for Interactive Applications", 2002,
//! chapter on modal synthesis; Adrien, "The missing link: modal synthesis",
//! in De Poli et al. (eds.), Representations of Musical Signals, 1991). Each
//! mode is a two-pole resonator
//!
//! `y[n] = 2 r cos(w) y[n-1] - r^2 y[n-2] + g sin(w) x[n]`
//!
//! with `w = 2 pi f / fs` and `r = 10^(-3 / (T60 fs))`. Its impulse response
//! is exactly `g r^n sin(w (n + 1))`: frequency `f`, amplitude `g`, and an
//! amplitude that falls 60 dB in `T60`. The bank stores the coefficients and
//! states as structure-of-arrays in `f64` (a low mode at 192 kHz sits close
//! to `z = 1`, where `f32` would detune it) and sums the modes in groups of
//! four. States are flushed against denormals every 256 samples.
//!
//! **Presets** give frequency ratios from formulas:
//!
//! - [`ModalPreset::BarFreeFree`], [`ModalPreset::BarClampedFree`]: an ideal
//!   Euler-Bernoulli bar, `f_n` proportional to `(beta_n L)^2` where
//!   `beta_n L` are the roots of `cos x cosh x = 1` (free-free, 4.7300,
//!   7.8532, 10.9956, ... so the ratios are 1, 2.757, 5.404, 8.933) or
//!   `cos x cosh x = -1` (clamped-free, 1.8751, 4.6941, 7.8548, ... ratios 1,
//!   6.267, 17.55). The roots are found by Newton's method (Rayleigh, "The
//!   Theory of Sound" vol. 1 section 170; Fletcher and Rossing section 2.12).
//! - [`ModalPreset::BarSimplySupported`]: ratios `n^2`.
//! - [`ModalPreset::Plate`]: a rectangular plate with simply supported edges,
//!   `f_mn` proportional to `m^2 + (n a/b)^2` (Leissa, "Vibration of Plates",
//!   NASA SP-160, 1969; Fletcher and Rossing section 13.1).
//! - [`ModalPreset::Membrane`]: the ideal circular membrane with a fixed rim,
//!   `f_mn` proportional to the zeros `j_mn` of the Bessel function `J_m`,
//!   tabulated in [`MEMBRANE_ZEROS`] from Abramowitz and Stegun, "Handbook of
//!   Mathematical Functions" (1964), table 9.5. The table holds every zero up
//!   to 14.93 (28 modes: 1, 1.593, 2.136, 2.296, ...).
//!
//! [`Exciter`] makes the driving signal: an impulse, a decaying noise burst,
//! or a mallet (a half-sine force pulse of unit area, after the contact-time
//! model of Chaigne and Askenfelt, "Numerical simulations of piano strings.
//! I", J. Acoust. Soc. Am. 95(2), 1994; a harder mallet is a shorter pulse
//! and excites higher modes).

use crate::noise::WhiteNoise;
use crate::rate::effective_rate;
use core::f64::consts::PI;

const FLUSH_EVERY: u32 = 256;
const FLUSH: f64 = 1e-30;

/// Zeros `j_{m,n}` of the Bessel functions `J_m`, `m = 0..=10`, up to 14.93,
/// as `(m, n, zero)`, ascending. Abramowitz and Stegun (1964), table 9.5.
pub const MEMBRANE_ZEROS: [(u8, u8, f64); 28] = [
    (0, 1, 2.404_825_557_7),
    (1, 1, 3.831_705_970_2),
    (2, 1, 5.135_622_301_8),
    (0, 2, 5.520_078_110_3),
    (3, 1, 6.380_161_895_9),
    (1, 2, 7.015_586_669_8),
    (4, 1, 7.588_342_434_5),
    (2, 2, 8.417_244_140_4),
    (0, 3, 8.653_727_912_9),
    (5, 1, 8.771_483_815_96),
    (3, 2, 9.761_023_129_98),
    (6, 1, 9.936_109_524_2),
    (1, 3, 10.173_468_135_1),
    (4, 2, 11.064_709_488_5),
    (7, 1, 11.086_370_019_2),
    (2, 3, 11.619_841_172_1),
    (0, 4, 11.791_534_439_0),
    (8, 1, 12.225_092_264_0),
    (5, 2, 12.338_604_197_5),
    (3, 3, 13.015_200_721_7),
    (1, 4, 13.323_691_936_3),
    (9, 1, 13.354_300_477_4),
    (6, 2, 13.589_290_170_0),
    (4, 3, 14.372_536_671_6),
    (10, 1, 14.475_500_686_6),
    (2, 4, 14.795_951_782_4),
    (7, 2, 14.821_268_727_0),
    (0, 5, 14.930_917_708_5),
];

/// A family of mode frequencies.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum ModalPreset {
    /// Free-free ideal bar (xylophone, marimba, glockenspiel bars).
    BarFreeFree,
    /// Clamped-free ideal bar (tuning fork tine, kalimba tongue).
    BarClampedFree,
    /// Simply supported bar: ratios `n^2`.
    BarSimplySupported,
    /// Rectangular simply supported plate with side ratio `a / b`.
    Plate {
        /// Side ratio `a / b` of the plate.
        aspect: f32,
    },
    /// Ideal circular membrane with a fixed rim (up to 28 modes).
    Membrane,
}

/// Largest number of modes a preset can produce at once.
pub const MAX_PRESET_MODES: usize = 64;

fn bar_root(n: usize, free_free: bool) -> f64 {
    // cos x cosh x = +-1, written as g(x) = cos x -+ 1/cosh x so that large
    // arguments do not overflow.
    let sgn = if free_free { 1.0 } else { -1.0 };
    let mut x = if free_free { (2 * n + 1) as f64 } else { (2 * n - 1) as f64 } * PI * 0.5;
    for _ in 0..60 {
        let ch = x.cosh();
        let g = x.cos() - sgn / ch;
        let dg = -x.sin() + sgn * x.sinh() / (ch * ch);
        if dg.abs() < 1e-300 {
            break;
        }
        let step = g / dg;
        x -= step;
        if step.abs() < 1e-14 * x.abs() {
            break;
        }
    }
    x
}

/// Fill `out` with ascending frequency ratios (first mode = 1) for `preset`
/// and return how many were written (at most `out.len()` and
/// [`MAX_PRESET_MODES`]; a membrane yields at most 28).
pub fn mode_ratios(preset: ModalPreset, out: &mut [f64]) -> usize {
    let cap = out.len().min(MAX_PRESET_MODES);
    match preset {
        ModalPreset::BarFreeFree | ModalPreset::BarClampedFree => {
            let ff = preset == ModalPreset::BarFreeFree;
            let first = bar_root(1, ff);
            for (i, o) in out.iter_mut().enumerate().take(cap) {
                let x = bar_root(i + 1, ff);
                *o = (x / first) * (x / first);
            }
            cap
        }
        ModalPreset::BarSimplySupported => {
            for (i, o) in out.iter_mut().enumerate().take(cap) {
                *o = ((i + 1) * (i + 1)) as f64;
            }
            cap
        }
        ModalPreset::Plate { aspect } => {
            let a = if aspect.is_finite() { f64::from(aspect).clamp(0.05, 20.0) } else { 1.0 };
            let mut all = [0.0f64; 100];
            let mut c = 0;
            for m in 1..=10 {
                for n in 1..=10 {
                    all[c] = (m * m) as f64 + (n as f64 * a) * (n as f64 * a);
                    c += 1;
                }
            }
            all.sort_unstable_by(|x, y| x.total_cmp(y));
            let count = cap.min(c);
            for i in 0..count {
                out[i] = all[i] / all[0];
            }
            count
        }
        ModalPreset::Membrane => {
            let count = cap.min(MEMBRANE_ZEROS.len());
            let mut z = [0.0f64; 28];
            for (i, e) in MEMBRANE_ZEROS.iter().enumerate() {
                z[i] = e.2;
            }
            z.sort_unstable_by(|x, y| x.total_cmp(y));
            for i in 0..count {
                out[i] = z[i] / z[0];
            }
            count
        }
    }
}

/// Settings for [`ModalBank::load_preset`].
/// # Example
///
/// ```
/// use openplugin_dsp::PresetSettings;
///
/// let marimba_bar = PresetSettings { f1: 220.0, t60: 1.5, modes: 4, ..PresetSettings::default() };
/// assert_eq!(marimba_bar.decay_tilt, 1.0);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct PresetSettings {
    /// Frequency of the first mode.
    pub f1: f32,
    /// T60 of the first mode, seconds.
    pub t60: f32,
    /// Higher modes decay `T60_k = T60_1 (f_1 / f_k)^decay_tilt`.
    pub decay_tilt: f32,
    /// Mode gains `g_k = (f_1 / f_k)^gain_tilt`.
    pub gain_tilt: f32,
    /// Modes wanted.
    pub modes: usize,
}

impl Default for PresetSettings {
    fn default() -> Self {
        Self { f1: 440.0, t60: 1.0, decay_tilt: 1.0, gain_tilt: 0.0, modes: 8 }
    }
}

/// A bank of two-pole resonators.
/// `new` allocates; `next`, `process` and the setters do not.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{Exciter, ModalBank, ModalPreset, PresetSettings, Strike};
///
/// let rate = 48_000.0;
/// let mut bank = ModalBank::new(rate, 16);
/// let loaded = bank.load_preset(ModalPreset::BarFreeFree, &PresetSettings::default());
/// assert!(loaded > 0 && loaded <= 8);
///
/// let mut mallet = Exciter::new(rate, Strike::Mallet { contact_ms: 2.0 });
/// mallet.trigger(0.8);
/// let mut peak = 0.0f32;
/// for _ in 0..4_800 {
///     peak = peak.max(bank.next(mallet.next()).abs());
/// }
/// assert!(peak > 0.0 && peak.is_finite());
/// ```
#[derive(Clone, Debug)]
pub struct ModalBank {
    fs: f64,
    count: usize,
    a1: Vec<f64>,
    a2: Vec<f64>,
    b: Vec<f64>,
    y1: Vec<f64>,
    y2: Vec<f64>,
    freq: Vec<f64>,
    t60: Vec<f64>,
    flush_in: u32,
}

impl ModalBank {
    /// A bank of up to `capacity` modes. Allocates.
    pub fn new(sample_rate: f32, capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            fs: f64::from(effective_rate(sample_rate)),
            count: 0,
            a1: vec![0.0; cap],
            a2: vec![0.0; cap],
            b: vec![0.0; cap],
            y1: vec![0.0; cap],
            y2: vec![0.0; cap],
            freq: vec![0.0; cap],
            t60: vec![1.0; cap],
            flush_in: FLUSH_EVERY,
        }
    }

    /// Change the sample rate (existing modes are recomputed). Does not
    /// allocate.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.fs = f64::from(effective_rate(sample_rate));
        for i in 0..self.capacity() {
            self.recompute(i, self.freq[i], self.t60[i], self.gain_of(i));
        }
    }

    fn gain_of(&self, i: usize) -> f64 {
        let w = 2.0 * PI * self.freq[i] / self.fs;
        let s = w.sin();
        if s.abs() < 1e-12 {
            0.0
        } else {
            self.b[i] / s
        }
    }

    /// The most modes the bank holds (the `capacity` given to `new`, at least 1).
    pub fn capacity(&self) -> usize {
        self.a1.len()
    }

    /// Number of modes in use.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether no modes are active.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Silence all modes (keeps their settings).
    pub fn reset(&mut self) {
        self.y1.iter_mut().for_each(|v| *v = 0.0);
        self.y2.iter_mut().for_each(|v| *v = 0.0);
    }

    /// Use the first `n` modes (clamped to the capacity).
    pub fn set_len(&mut self, n: usize) {
        self.count = n.min(self.capacity());
    }

    fn recompute(&mut self, i: usize, freq: f64, t60: f64, gain: f64) {
        self.freq[i] = freq;
        self.t60[i] = t60;
        let ok = freq.is_finite()
            && freq > 0.0
            && freq < 0.49 * self.fs
            && t60.is_finite()
            && t60 > 0.0
            && gain.is_finite();
        if !ok {
            self.a1[i] = 0.0;
            self.a2[i] = 0.0;
            self.b[i] = 0.0;
            self.y1[i] = 0.0;
            self.y2[i] = 0.0;
            return;
        }
        let w = 2.0 * PI * freq / self.fs;
        let r = 10f64.powf(-3.0 / (t60 * self.fs));
        self.a1[i] = 2.0 * r * w.cos();
        self.a2[i] = r * r;
        self.b[i] = gain * w.sin();
    }

    /// Set mode `i` to frequency `freq_hz`, 60 dB decay time `t60_s` and
    /// gain `gain` (the amplitude of its response to a unit impulse). Modes at
    /// or above 0.49 of the sample rate, and non-finite or non-positive
    /// settings, silence the mode. Returns `false` if `i` is out of range.
    /// Grows `len` to include `i`.
    pub fn set_mode(&mut self, i: usize, freq_hz: f32, t60_s: f32, gain: f32) -> bool {
        if i >= self.capacity() {
            return false;
        }
        self.recompute(i, f64::from(freq_hz), f64::from(t60_s), f64::from(gain));
        self.count = self.count.max(i + 1);
        true
    }

    /// Frequency of mode `i` in Hz.
    pub fn mode_frequency(&self, i: usize) -> f32 {
        self.freq.get(i).copied().unwrap_or(0.0) as f32
    }

    /// Fill the bank from a preset; returns the number of modes loaded
    /// (limited by the capacity). Does not allocate.
    pub fn load_preset(&mut self, preset: ModalPreset, s: &PresetSettings) -> usize {
        let mut ratios = [0.0f64; MAX_PRESET_MODES];
        let want = s.modes.min(self.capacity()).min(MAX_PRESET_MODES);
        let n = mode_ratios(preset, &mut ratios[..want]);
        let f1 = f64::from(s.f1);
        let t1 = f64::from(s.t60);
        let (dt, gt) = (f64::from(s.decay_tilt), f64::from(s.gain_tilt));
        self.count = 0;
        for (i, &ratio) in ratios[..n].iter().enumerate() {
            let f = f1 * ratio;
            let t = t1 * (1.0 / ratio).powf(if dt.is_finite() { dt } else { 0.0 });
            let g = (1.0 / ratio).powf(if gt.is_finite() { gt } else { 0.0 });
            self.recompute(i, f, t, g);
            self.count = i + 1;
        }
        n
    }

    /// One output sample for one input (excitation) sample.
    #[inline]
    #[allow(clippy::should_implement_trait, clippy::needless_range_loop)]
    pub fn next(&mut self, input: f32) -> f32 {
        let x = if input.is_finite() { f64::from(input) } else { 0.0 };
        let n = self.count;
        let (a1, a2, b) = (&self.a1[..n], &self.a2[..n], &self.b[..n]);
        let (y1, y2) = (&mut self.y1[..n], &mut self.y2[..n]);
        let mut acc = [0.0f64; 4];
        let mut i = 0;
        while i + 4 <= n {
            for j in 0..4 {
                let k = i + j;
                let y = a1[k] * y1[k] - a2[k] * y2[k] + b[k] * x;
                y2[k] = y1[k];
                y1[k] = y;
                acc[j] += y;
            }
            i += 4;
        }
        while i < n {
            let y = a1[i] * y1[i] - a2[i] * y2[i] + b[i] * x;
            y2[i] = y1[i];
            y1[i] = y;
            acc[0] += y;
            i += 1;
        }
        self.flush_in -= 1;
        if self.flush_in == 0 {
            self.flush_in = FLUSH_EVERY;
            self.flush_states();
        }
        ((acc[0] + acc[1]) + (acc[2] + acc[3])) as f32
    }

    fn flush_states(&mut self) {
        for i in 0..self.count {
            let bad = !(self.y1[i].is_finite() && self.y2[i].is_finite());
            if bad || (self.y1[i].abs() < FLUSH && self.y2[i].abs() < FLUSH) {
                self.y1[i] = 0.0;
                self.y2[i] = 0.0;
            }
        }
    }

    /// Process a block in place: excitation in, bank output out.
    pub fn process(&mut self, io: &mut [f32]) {
        for s in io {
            *s = self.next(*s);
        }
    }
}

/// What an [`Exciter`] produces on a strike.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Strike {
    /// One sample of amplitude `velocity`.
    Impulse,
    /// White noise of RMS `velocity / sqrt 3`, fading linearly to zero over `ms`.
    NoiseBurst {
        /// Length of the burst, milliseconds (clamped to 0.01..=2000).
        ms: f32,
    },
    /// Half-sine force pulse of unit area (times `velocity`) lasting `contact_ms`.
    Mallet {
        /// Contact time, milliseconds (clamped to 0.01..=2000).
        contact_ms: f32,
    },
}

/// Excitation generator for a [`ModalBank`] (or any resonator).
/// Plain state, no allocation; see the [`ModalBank`] example.
#[derive(Clone, Debug)]
pub struct Exciter {
    fs: f64,
    kind: Strike,
    len: usize,
    n: usize,
    amp: f64,
    rng: WhiteNoise,
}

impl Exciter {
    /// An idle exciter for `sample_rate` (Hz, clamped to the SDK range) producing
    /// `kind`. Does not allocate.
    pub fn new(sample_rate: f32, kind: Strike) -> Self {
        Self {
            fs: f64::from(effective_rate(sample_rate)),
            kind,
            len: 0,
            n: 0,
            amp: 0.0,
            rng: WhiteNoise::new(0xE1C17E),
        }
    }

    /// Changes the sample rate in Hz; takes effect at the next trigger.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.fs = f64::from(effective_rate(sample_rate));
    }

    /// Changes what the next trigger produces.
    pub fn set_kind(&mut self, kind: Strike) {
        self.kind = kind;
    }

    /// Start a strike (`velocity` clamped to `0..=1`).
    pub fn trigger(&mut self, velocity: f32) {
        let v = if velocity.is_finite() { f64::from(velocity).clamp(0.0, 1.0) } else { 0.0 };
        let ms = |m: f32| if m.is_finite() { f64::from(m).clamp(0.01, 2000.0) } else { 1.0 };
        self.n = 0;
        self.amp = v;
        self.len = match self.kind {
            Strike::Impulse => 1,
            Strike::NoiseBurst { ms: m } => ((ms(m) * 1e-3 * self.fs).round() as usize).max(1),
            Strike::Mallet { contact_ms } => ((ms(contact_ms) * 1e-3 * self.fs).round() as usize).max(2),
        };
    }

    /// Whether a strike is still being produced.
    pub fn is_active(&self) -> bool {
        self.n < self.len
    }

    /// Next excitation sample (0 when the strike is over).
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        if self.n >= self.len {
            return 0.0;
        }
        let i = self.n;
        self.n += 1;
        let len = self.len as f64;
        let y = match self.kind {
            Strike::Impulse => self.amp,
            Strike::NoiseBurst { .. } => f64::from(self.rng.next()) * self.amp * (1.0 - i as f64 / len),
            Strike::Mallet { .. } => {
                // sin(pi (i + 1/2) / len) sums to ~ 2 len / pi: unit area.
                self.amp * (PI / (2.0 * len)) * (PI * (i as f64 + 0.5) / len).sin()
            }
        };
        y as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_free_bar_ratios_match_the_textbook() {
        let mut r = [0.0; 5];
        mode_ratios(ModalPreset::BarFreeFree, &mut r);
        for (got, want) in r.iter().zip([1.0, 2.757, 5.404, 8.933, 13.344]) {
            assert!((got - want).abs() < 2e-3 * want, "{got} {want}");
        }
        mode_ratios(ModalPreset::BarClampedFree, &mut r);
        for (got, want) in r.iter().zip([1.0, 6.267, 17.547, 34.386, 56.844]) {
            assert!((got - want).abs() < 2e-3 * want, "{got} {want}");
        }
    }

    #[test]
    fn impulse_response_is_an_exact_damped_sine() {
        let mut m = ModalBank::new(48_000.0, 4);
        m.set_mode(0, 1000.0, 0.5, 0.7);
        let w = 2.0 * PI * 1000.0 / 48_000.0;
        let r = 10f64.powf(-3.0 / (0.5 * 48_000.0));
        for n in 0..200 {
            let y = m.next(if n == 0 { 1.0 } else { 0.0 });
            let want = 0.7 * r.powi(n) * (w * (f64::from(n) + 1.0)).sin();
            assert!((f64::from(y) - want).abs() < 1e-6, "{n}");
        }
    }

    #[test]
    fn hostile_modes_are_silent() {
        let mut m = ModalBank::new(44_100.0, 4);
        assert!(!m.set_mode(9, 100.0, 1.0, 1.0));
        m.set_mode(0, f32::NAN, 1.0, 1.0);
        m.set_mode(1, 30_000.0, 1.0, 1.0);
        m.set_mode(2, 100.0, -1.0, 1.0);
        m.set_mode(3, 100.0, 1.0, f32::INFINITY);
        for n in 0..100 {
            assert_eq!(m.next(if n == 0 { f32::NAN } else { 1.0 }), 0.0);
        }
    }
}
