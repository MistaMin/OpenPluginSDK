//! Dither and noise-shaped requantisation to a shorter word length.
//!
//! Use [`Dither`] as the last stage of a plugin whose output is going to a
//! shorter word (16 bit for a CD master, 24 bit for a file, or 8..=24 bit for a
//! lo-fi effect). Rounding a signal to a grid is a nonlinear step: for a low
//! level input the rounding error is correlated with the signal and shows up as
//! harmonic distortion. Adding a small random signal (dither) before rounding
//! turns that distortion into a steady, signal-independent noise floor.
//!
//! Terms: the grid step is `Δ = 2^(1-bits)` (full scale is [-1, 1), so there
//! are `2^bits` levels).
//!
//! - RPDF dither is one uniform value in (-Δ/2, Δ/2). It removes the
//!   distortion but the noise power still moves with the signal (noise
//!   modulation). Total error power is Δ²/12 (rounding) + Δ²/12 (dither) = Δ²/6.
//! - TPDF dither is the sum of two such values, a triangular density over
//!   (-Δ, Δ). It also fixes the noise modulation: the first two moments of the
//!   error no longer depend on the signal. Total error power is
//!   Δ²/12 + Δ²/6 = Δ²/4. This is the standard choice.
//!   (Lipshitz, Wannamaker, Vanderkooy, "Quantization and Dither: A Theoretical
//!   Survey", JAES 40(5), 1992.)
//!
//! # Noise shaping
//!
//! Error feedback moves the requantisation noise in frequency without changing
//! the signal. With `v[n] = x[n] + Σ a_k e[n-k]`, `y[n] = Q(v[n] + d[n])` and
//! `e[n] = y[n] - v[n]`, the output is `y = x + A(z) e` with
//! `A(z) = 1 + Σ a_k z^-k`: the noise `e` is filtered by the noise transfer
//! function `A`. The Gerzon-Craven theorem says the log-average of `|A|` over
//! frequency cannot go below zero, so noise removed from one band is added in
//! another, and total noise power grows by `Σ a_k²` plus one.
//!
//! - [`NoiseShaping::FirstOrder`]: `A(z) = 1 - z^-1`. Zero at DC, +6 dB at
//!   Nyquist; moves the noise toward high frequency.
//! - [`NoiseShaping::Perceptual`]: an order 9 filter *designed here*, at the
//!   session rate, from a stated hearing weighting. It is not a published
//!   coefficient table. The published curves (Lipshitz, Vanderkooy, Wannamaker,
//!   "Minimally Audible Noise Shaping", JAES 39(11), 1991, and Wannamaker,
//!   "Psychoacoustically Optimal Noise Shaping", JAES 40(7/8), 1992) are
//!   tables defined at 44.1/48 kHz; this module does not reproduce them.
//!   Instead it uses the same underlying principle those papers use: choose
//!   a monic minimum-phase `A` that minimises the audibility-weighted noise
//!   power `∫ W(f) |A(f)|² df`. For a monic FIR `A` of order `M` this problem
//!   is exactly linear prediction on the autocorrelation of `W` (Levinson-
//!   Durbin). `W(f)` is the inverse of Terhardt's approximation of the
//!   threshold of hearing in quiet (E. Terhardt, "Calculating virtual pitch",
//!   Hearing Research 1, 1979):
//!   `T(f) = 3.64 f^-0.8 - 6.5 e^(-0.6 (f-3.3)²) + 1e-3 f^4` dB SPL, `f` in
//!   kHz, with `T` limited to -5..=25 dB so that the design stays moderate
//!   (about 30 dB of weighting range). The result puts a dip in the noise
//!   around 3-4 kHz, where hearing is most sensitive, and the noise rises
//!   above about 10 kHz. Because it is a design and not a table, it works at
//!   every rate: at 44.1/48 kHz the noise is pushed up toward the top of the
//!   band; at high rates (88.2 kHz and above) the same weighting leaves the
//!   gain mostly in the ultrasonic range, where hearing ignores it.
//!
//! # Auto-blank
//!
//! When a channel's input has been exactly `0.0` for `blank_after` samples the
//! output is exact zero (no dither noise, error history cleared), and the
//! dither resumes on the very next non-zero input sample.
//!
//! # Realtime
//!
//! [`Dither::new`] allocates; nothing after it does. `process*` never panics:
//! a channel index that does not exist leaves the buffer untouched, and
//! NaN and infinite input count as `0.0`.

use crate::rate::effective_rate;

/// Highest noise-shaping order held.
const MAX_ORDER: usize = 16;
/// Order of the designed [`NoiseShaping::Perceptual`] filter.
const PERCEPTUAL_ORDER: usize = 9;
/// Frequency grid cells used to integrate the weighting.
const DESIGN_CELLS: usize = 1024;

/// Which random density is added before rounding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DitherKind {
    /// Round only. Shows the distortion that dither exists to prevent.
    None,
    /// Rectangular density, 1 LSB wide.
    Rpdf,
    /// Triangular density, 2 LSB wide.
    Tpdf,
}

/// Error-feedback noise shaping. See the module documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum NoiseShaping {
    /// Flat (white) requantisation noise.
    None,
    /// `A(z) = 1 - z^-1`.
    FirstOrder,
    /// Order 9, designed at the session rate from an audibility weighting.
    Perceptual,
}

/// A small PCG32 generator (O'Neill 2014, "PCG: A Family of Simple Fast
/// Space-Efficient Statistically Good Algorithms for Random Number
/// Generation"), the XSH-RR output variant. Each channel owns an odd stream
/// constant so channels do not repeat one another.
#[derive(Debug, Clone, Copy)]
struct Pcg32 {
    state: u64,
    inc: u64,
}

fn splitmix64(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Pcg32 {
    fn new(seed: u64, channel: usize) -> Self {
        let mut s = seed ^ (channel as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
        let state = splitmix64(&mut s);
        let inc = splitmix64(&mut s) | 1;
        let mut rng = Self { state: 0, inc };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(state);
        rng.next_u32();
        rng
    }

    #[inline]
    fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform in [-0.5, 0.5).
    #[inline]
    fn uniform(&mut self) -> f64 {
        f64::from(self.next_u32()) * (1.0 / 4_294_967_296.0) - 0.5
    }
}

#[derive(Debug, Clone)]
struct Channel {
    rng: Pcg32,
    /// Past requantisation errors in LSB, newest first.
    err: [f64; MAX_ORDER],
    zeros: u32,
}

/// Dither, noise shaping and auto-blank for any number of channels.
#[derive(Debug, Clone)]
pub struct Dither {
    channels: Vec<Channel>,
    bits: u32,
    kind: DitherKind,
    shaping: NoiseShaping,
    sample_rate: f32,
    /// `a_1..=a_order` of the noise transfer function.
    coefs: [f64; MAX_ORDER],
    order: usize,
    blank_after: u32,
}

impl Dither {
    /// `bits` is clamped to 8..=24. TPDF, no shaping, auto-blank after 4096
    /// samples. `seed` makes the noise reproducible; channels get unrelated
    /// streams from it.
    pub fn new(channels: usize, sample_rate: f32, bits: u32, seed: u64) -> Self {
        let channels = (0..channels.clamp(1, 64))
            .map(|c| Channel {
                rng: Pcg32::new(seed, c),
                err: [0.0; MAX_ORDER],
                zeros: 0,
            })
            .collect();
        let mut d = Self {
            channels,
            bits: bits.clamp(8, 24),
            kind: DitherKind::Tpdf,
            shaping: NoiseShaping::None,
            sample_rate: effective_rate(sample_rate),
            coefs: [0.0; MAX_ORDER],
            order: 0,
            blank_after: 4096,
        };
        d.redesign();
        d
    }

    /// Target word length, clamped to 8..=24.
    pub fn set_bits(&mut self, bits: u32) {
        self.bits = bits.clamp(8, 24);
    }

    pub fn bits(&self) -> u32 {
        self.bits
    }

    pub fn set_kind(&mut self, kind: DitherKind) {
        self.kind = kind;
    }

    pub fn set_shaping(&mut self, shaping: NoiseShaping) {
        self.shaping = shaping;
        self.redesign();
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = effective_rate(sample_rate);
        self.redesign();
    }

    /// Samples of exact digital silence before the output is blanked. `0`
    /// turns auto-blank off.
    pub fn set_blank_after(&mut self, samples: u32) {
        self.blank_after = samples;
    }

    /// Clear the error history and silence counters (not the random streams).
    pub fn reset(&mut self) {
        for c in &mut self.channels {
            c.err = [0.0; MAX_ORDER];
            c.zeros = 0;
        }
    }

    /// The noise transfer function `1 + a_1 z^-1 + ... + a_M z^-M`, as
    /// `a_1..=a_M`.
    pub fn noise_transfer(&self) -> &[f64] {
        &self.coefs[..self.order]
    }

    fn redesign(&mut self) {
        self.coefs = [0.0; MAX_ORDER];
        self.order = 0;
        for c in &mut self.channels {
            c.err = [0.0; MAX_ORDER];
        }
        match self.shaping {
            NoiseShaping::None => {}
            NoiseShaping::FirstOrder => {
                self.coefs[0] = -1.0;
                self.order = 1;
            }
            NoiseShaping::Perceptual => {
                match design_weighted(f64::from(self.sample_rate), PERCEPTUAL_ORDER) {
                    Some(a) => {
                        self.coefs = a;
                        self.order = PERCEPTUAL_ORDER;
                    }
                    None => {
                        self.coefs[0] = -1.0;
                        self.order = 1;
                    }
                }
            }
        }
    }

    /// One sample to the grid of `bits`, returning the value in LSB units
    /// (integer valued, in -2^(bits-1)..=2^(bits-1)-1); 0 when blanked.
    #[inline]
    fn quantise(&mut self, channel: usize, x: f32, bits: u32) -> f64 {
        let kind = self.kind;
        let order = self.order;
        let blank_after = self.blank_after;
        let coefs = self.coefs;
        let c = &mut self.channels[channel];

        let x = if x.is_finite() { f64::from(x) } else { 0.0 };
        if x == 0.0 {
            c.zeros = c.zeros.saturating_add(1);
            if blank_after != 0 && c.zeros >= blank_after {
                c.err = [0.0; MAX_ORDER];
                return 0.0;
            }
        } else {
            c.zeros = 0;
        }

        let scale = f64::from(1u32 << (bits - 1));
        let mut v = x.clamp(-2.0, 2.0) * scale;
        for (a, e) in coefs.iter().zip(&c.err).take(order) {
            v += a * e;
        }
        let d = match kind {
            DitherKind::None => 0.0,
            DitherKind::Rpdf => c.rng.uniform(),
            DitherKind::Tpdf => c.rng.uniform() + c.rng.uniform(),
        };
        let y = (v + d).round().clamp(-scale, scale - 1.0);
        // Bounded so a clipped output cannot wind the feedback up, and flushed
        // so the history never holds a denormal.
        let mut e = (y - v).clamp(-2.0, 2.0);
        if e.abs() < 1e-30 {
            e = 0.0;
        }
        if order > 0 {
            c.err.copy_within(0..MAX_ORDER - 1, 1);
            c.err[0] = e;
        }
        y
    }

    /// Requantise `buf` in place to `bits` on the grid, as `f32` values.
    /// Does nothing when `channel` does not exist.
    pub fn process(&mut self, channel: usize, buf: &mut [f32]) {
        if channel >= self.channels.len() {
            return;
        }
        let bits = self.bits;
        let inv = 1.0 / f64::from(1u32 << (bits - 1));
        for s in buf.iter_mut() {
            *s = (self.quantise(channel, *s, bits) * inv) as f32;
        }
    }

    /// Requantise to 16 bits and write `i16` (ignores [`Self::set_bits`]).
    /// Writes `min(input.len(), out.len())` samples.
    pub fn process_i16(&mut self, channel: usize, input: &[f32], out: &mut [i16]) {
        if channel >= self.channels.len() {
            return;
        }
        for (o, &s) in out.iter_mut().zip(input) {
            *o = self.quantise(channel, s, 16) as i16;
        }
    }

    /// Requantise to 24 bits and write the value in the low 24 bits of an
    /// `i32` (sign-extended), as [`crate::pcm::f32_to_i24`] does.
    pub fn process_i24(&mut self, channel: usize, input: &[f32], out: &mut [i32]) {
        if channel >= self.channels.len() {
            return;
        }
        for (o, &s) in out.iter_mut().zip(input) {
            *o = self.quantise(channel, s, 24) as i32;
        }
    }

    /// Requantise to [`Self::bits`] and write left-justified in an `i32`
    /// (the grid value times `2^(32-bits)`), the layout of a 24-bit sample in
    /// a 32-bit container.
    pub fn process_i32(&mut self, channel: usize, input: &[f32], out: &mut [i32]) {
        if channel >= self.channels.len() {
            return;
        }
        let bits = self.bits;
        let shift = 32 - bits;
        for (o, &s) in out.iter_mut().zip(input) {
            *o = (self.quantise(channel, s, bits) as i32) << shift;
        }
    }
}

/// Terhardt's threshold in quiet in dB SPL, `f` in Hz.
fn threshold_in_quiet_db(f_hz: f64) -> f64 {
    let k = f_hz.max(20.0) / 1000.0;
    3.64 * k.powf(-0.8) - 6.5 * (-0.6 * (k - 3.3) * (k - 3.3)).exp() + 1e-3 * k.powi(4)
}

/// Minimum-phase monic FIR `A` of the given order minimising
/// `∫ W |A|²`, with `W = 10^(-T/10)` and `T` the clamped threshold in quiet.
/// Returns `a_1..=a_order`, or `None` if the numerics fail.
fn design_weighted(rate: f64, order: usize) -> Option<[f64; MAX_ORDER]> {
    let nyquist = rate * 0.5;
    let mut r = [0.0f64; MAX_ORDER + 1];
    for i in 0..=DESIGN_CELLS {
        let frac = i as f64 / DESIGN_CELLS as f64;
        let t = threshold_in_quiet_db(frac * nyquist).clamp(-5.0, 25.0);
        let w = 10f64.powf(-t / 10.0);
        let trap = if i == 0 || i == DESIGN_CELLS { 0.5 } else { 1.0 };
        let theta = std::f64::consts::PI * frac;
        for (k, rk) in r.iter_mut().enumerate().take(order + 1) {
            *rk += trap * w * (theta * k as f64).cos();
        }
    }
    r[0] *= 1.0 + 1e-9;

    // Levinson-Durbin.
    let mut a = [0.0f64; MAX_ORDER + 1];
    a[0] = 1.0;
    let mut err = r[0];
    if !(err.is_finite() && err > 0.0) {
        return None;
    }
    for m in 1..=order {
        let mut acc = r[m];
        for j in 1..m {
            acc += a[j] * r[m - j];
        }
        let k = -acc / err;
        let prev = a;
        for j in 1..m {
            a[j] = prev[j] + k * prev[m - j];
        }
        a[m] = k;
        err *= 1.0 - k * k;
        if !(err.is_finite() && err > 0.0) {
            return None;
        }
    }
    let mut out = [0.0f64; MAX_ORDER];
    out[..order].copy_from_slice(&a[1..=order]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    fn noise_input(n: usize, seed: u64) -> Vec<f32> {
        // Dense in-range values with no grid structure.
        let mut r = Pcg32::new(seed, 7);
        (0..n).map(|_| (r.uniform() * 1.2) as f32).collect()
    }

    #[test]
    fn pcg_uniform_moments() {
        let mut r = Pcg32::new(1, 0);
        let n = 400_000;
        let (mut s, mut s2) = (0.0, 0.0);
        for _ in 0..n {
            let u = r.uniform();
            assert!((-0.5..0.5).contains(&u));
            s += u;
            s2 += u * u;
        }
        assert!((s / n as f64).abs() < 0.005);
        assert!((s2 / n as f64 - 1.0 / 12.0).abs() < 0.002);
    }

    #[test]
    fn error_power_matches_theory() {
        let x = noise_input(400_000, 3);
        for bits in [8u32, 12, 16] {
            let delta = 2f64.powi(1 - bits as i32);
            for (kind, factor) in [
                (DitherKind::None, 1.0 / 12.0),
                (DitherKind::Rpdf, 1.0 / 6.0),
                (DitherKind::Tpdf, 1.0 / 4.0),
            ] {
                let mut d = Dither::new(1, FS, bits, 42);
                d.set_kind(kind);
                let mut y = x.clone();
                d.process(0, &mut y);
                let p: f64 = x
                    .iter()
                    .zip(&y)
                    .map(|(a, b)| (f64::from(*b) - f64::from(*a)).powi(2))
                    .sum::<f64>()
                    / x.len() as f64;
                let want = factor * delta * delta;
                assert!(
                    (p / want - 1.0).abs() < 0.03,
                    "bits {bits} {kind:?}: {p:e} vs {want:e}"
                );
            }
        }
    }

    #[test]
    fn dither_signal_variance() {
        let mut r = Pcg32::new(9, 1);
        let n = 400_000;
        let (mut rp, mut tp) = (0.0, 0.0);
        for _ in 0..n {
            rp += r.uniform().powi(2);
            let t = r.uniform() + r.uniform();
            tp += t * t;
        }
        // In units of delta squared: RPDF 1/12, TPDF 1/6.
        assert!((rp / n as f64 * 12.0 - 1.0).abs() < 0.02);
        assert!((tp / n as f64 * 6.0 - 1.0).abs() < 0.02);
    }

    #[test]
    fn outputs_lie_on_the_grid() {
        let x = noise_input(20_000, 5);
        for bits in [8u32, 13, 16, 20, 24] {
            for shaping in [NoiseShaping::None, NoiseShaping::FirstOrder, NoiseShaping::Perceptual] {
                let mut d = Dither::new(1, FS, bits, 1);
                d.set_shaping(shaping);
                let mut y = x.clone();
                d.process(0, &mut y);
                let scale = f64::from(1u32 << (bits - 1));
                for v in y {
                    let g = f64::from(v) * scale;
                    assert_eq!(g, g.round(), "bits {bits} value {v}");
                    assert!(g >= -scale && g <= scale - 1.0);
                }
            }
        }
    }

    #[test]
    fn integer_helpers_match_grid() {
        let x = noise_input(2000, 6);
        let mut d = Dither::new(1, FS, 16, 5);
        let mut f = x.clone();
        d.process(0, &mut f);
        let mut d2 = Dither::new(1, FS, 16, 5);
        let mut i = vec![0i16; x.len()];
        d2.process_i16(0, &x, &mut i);
        for (a, b) in f.iter().zip(&i) {
            assert_eq!(crate::pcm::f32_to_i16(*a), *b);
        }
        let mut d3 = Dither::new(1, FS, 24, 5);
        let mut f = x.clone();
        d3.process(0, &mut f);
        let mut d4 = Dither::new(1, FS, 24, 5);
        let mut i = vec![0i32; x.len()];
        d4.process_i24(0, &x, &mut i);
        for (a, b) in f.iter().zip(&i) {
            assert_eq!(crate::pcm::f32_to_i24(*a), *b);
            assert_eq!(crate::pcm::i24_to_f32(*b), *a);
        }
        let mut d5 = Dither::new(1, FS, 24, 5);
        let mut j = vec![0i32; x.len()];
        d5.process_i32(0, &x, &mut j);
        for (a, b) in i.iter().zip(&j) {
            assert_eq!(*a << 8, *b);
        }
    }

    #[test]
    fn silence_gives_exact_zero_and_resumes() {
        for shaping in [NoiseShaping::None, NoiseShaping::Perceptual] {
            let mut d = Dither::new(2, FS, 16, 3);
            d.set_shaping(shaping);
            d.set_blank_after(64);
            let mut buf = vec![0.0f32; 10_000];
            d.process(0, &mut buf);
            assert!(buf[63..].iter().all(|&s| s.to_bits() == 0));
            // A signal comes back instantly with dither on it.
            let mut sig = vec![0.25f32; 256];
            d.process(0, &mut sig);
            assert!(sig.iter().any(|&s| s != 0.25 && s != 0.0));
            assert!((sig[0] - 0.25).abs() < 1e-3);
            // And silence blanks again after the count.
            let mut tail = vec![0.0f32; 200];
            d.process(0, &mut tail);
            assert!(tail[64..].iter().all(|&s| s == 0.0));
        }
    }

    #[test]
    fn same_seed_same_output_and_channels_differ() {
        let x = noise_input(4096, 8);
        let run = |seed| {
            let mut d = Dither::new(2, FS, 16, seed);
            d.set_shaping(NoiseShaping::Perceptual);
            let (mut a, mut b) = (x.clone(), x.clone());
            d.process(0, &mut a);
            d.process(1, &mut b);
            (a, b)
        };
        let (a1, b1) = run(11);
        let (a2, b2) = run(11);
        assert_eq!(a1, a2);
        assert_eq!(b1, b2);
        assert_ne!(a1, b1);
        let (a3, _) = run(12);
        assert_ne!(a1, a3);
        // Decorrelated dither: the two channels' errors have no correlation.
        let (mut sab, mut saa, mut sbb) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..x.len() {
            let ea = f64::from(a1[i] - x[i]);
            let eb = f64::from(b1[i] - x[i]);
            sab += ea * eb;
            saa += ea * ea;
            sbb += eb * eb;
        }
        assert!((sab / (saa * sbb).sqrt()).abs() < 0.06);
    }

    #[test]
    fn bad_input_is_safe() {
        let mut d = Dither::new(1, FS, 16, 1);
        d.set_shaping(NoiseShaping::Perceptual);
        let mut buf = [f32::NAN, f32::INFINITY, -f32::INFINITY, 5.0, -5.0, 1.0, -1.0];
        d.process(0, &mut buf);
        assert!(buf.iter().all(|s| s.is_finite() && (-1.0..1.0).contains(s)));
        let mut other = [0.5f32; 4];
        d.process(9, &mut other);
        assert_eq!(other, [0.5; 4]);
    }

    // Single-bin DFT power of `x` at an integer bin.
    fn bin_power(x: &[f64], bin: usize) -> f64 {
        let n = x.len() as f64;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, v) in x.iter().enumerate() {
            let a = std::f64::consts::TAU * bin as f64 * i as f64 / n;
            re += v * a.cos();
            im -= v * a.sin();
        }
        (re * re + im * im) / (n * n)
    }

    #[test]
    fn dither_removes_harmonic_distortion() {
        // 12 bit, sine of 1.5 LSB amplitude on an exact bin.
        let n = 16384usize;
        let fund = 97usize;
        let bits = 12;
        let lsb = 2f64.powi(1 - bits);
        let x: Vec<f32> = (0..n)
            .map(|i| (1.5 * lsb * (std::f64::consts::TAU * fund as f64 * i as f64 / n as f64).sin()) as f32)
            .collect();
        let harmonics = |kind| {
            let mut d = Dither::new(1, FS, bits as u32, 77);
            d.set_kind(kind);
            d.set_blank_after(0);
            let mut y = x.clone();
            d.process(0, &mut y);
            let e: Vec<f64> = y.iter().map(|v| f64::from(*v)).collect();
            let h: f64 = [3usize, 5, 7, 9, 11].iter().map(|k| bin_power(&e, fund * k)).sum();
            let fundamental = bin_power(&e, fund);
            // Average power of nearby non-harmonic bins, the noise floor.
            let floor: f64 = (1..=5).map(|k| bin_power(&e, fund * 2 * k + 13)).sum::<f64>() / 5.0;
            (h / fundamental, floor / fundamental, h / 5.0 / floor)
        };
        let (plain, _, plain_over_floor) = harmonics(DitherKind::None);
        let (tpdf, _, tpdf_over_floor) = harmonics(DitherKind::Tpdf);
        eprintln!("harmonics/fundamental: plain {plain:e} tpdf {tpdf:e}; over floor {plain_over_floor:.1} vs {tpdf_over_floor:.1}");
        // Undithered harmonics tower over the floor (they are the floor's
        // content); dithered ones are just noise-sized.
        assert!(plain > 20.0 * tpdf, "plain {plain:e} tpdf {tpdf:e}");
        assert!(tpdf_over_floor < 5.0);
        assert!(plain_over_floor > 30.0);
    }

    // Power in the band [lo, hi) of a spectrum via rustfft, Hann windowed.
    fn band_powers(e: &[f64], edges: &[(f64, f64)]) -> Vec<f64> {
        use rustfft::num_complex::Complex;
        use rustfft::FftPlanner;
        let n = e.len();
        let mut buf: Vec<Complex<f64>> = e
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
                Complex::new(v * w, 0.0)
            })
            .collect();
        FftPlanner::<f64>::new().plan_fft_forward(n).process(&mut buf);
        edges
            .iter()
            .map(|&(lo, hi)| {
                let (a, b) = ((lo * n as f64) as usize, (hi * n as f64) as usize);
                buf[a..b].iter().map(|c| c.norm_sqr()).sum::<f64>() / (b - a) as f64
            })
            .collect()
    }

    fn error_of(shaping: NoiseShaping, rate: f32) -> Vec<f64> {
        let x = noise_input(1 << 16, 21);
        let mut d = Dither::new(1, rate, 16, 5);
        d.set_shaping(shaping);
        let mut y = x.clone();
        d.process(0, &mut y);
        x.iter().zip(&y).map(|(a, b)| f64::from(*b) - f64::from(*a)).collect()
    }

    fn db(x: f64) -> f64 {
        10.0 * x.log10()
    }

    #[test]
    fn first_order_tilts_as_designed() {
        let e = error_of(NoiseShaping::FirstOrder, FS);
        // Fractions of the sample rate. |1 - z^-1|^2 = 2 - 2 cos(w).
        let p = band_powers(&e, &[(0.01, 0.03), (0.2, 0.25), (0.45, 0.5)]);
        let white = band_powers(&error_of(NoiseShaping::None, FS), &[(0.2, 0.25)])[0];
        let expect = |f: f64| 2.0 - 2.0 * (std::f64::consts::TAU * f).cos();
        let r_mid = db(p[1] / white) - db(expect(0.225));
        let r_top = db(p[2] / white) - db(expect(0.475));
        let r_low = db(p[0] / white) - db(expect(0.02));
        eprintln!("first order vs theory dB: low {r_low:.2} mid {r_mid:.2} top {r_top:.2}; top/low {:.1} dB", db(p[2] / p[0]));
        for r in [r_mid, r_top, r_low] {
            assert!(r.abs() < 0.5, "{r}");
        }
    }

    #[test]
    fn perceptual_shape_matches_its_design() {
        for rate in [44_100.0f32, 48_000.0, 96_000.0] {
            let d = {
                let mut d = Dither::new(1, rate, 16, 1);
                d.set_shaping(NoiseShaping::Perceptual);
                d
            };
            let a = d.noise_transfer().to_vec();
            assert_eq!(a.len(), PERCEPTUAL_ORDER);
            let h = |f: f64| {
                let w = std::f64::consts::TAU * f;
                let (mut re, mut im) = (1.0, 0.0);
                for (k, c) in a.iter().enumerate() {
                    re += c * (w * (k + 1) as f64).cos();
                    im -= c * (w * (k + 1) as f64).sin();
                }
                re * re + im * im
            };
            let fs = f64::from(rate);
            let at = |hz: f64| db(h(hz / fs));
            eprintln!(
                "perceptual @{rate}: |A|^2 dB: 100Hz {:.1} 1k {:.1} 3.5k {:.1} 8k {:.1} 15k {:.1} 20k {:.1}; gain {:.2}",
                at(100.0), at(1000.0), at(3500.0), at(8000.0), at(15000.0), at(20000.0),
                a.iter().map(|c| c * c).sum::<f64>() + 1.0
            );
            // Dip near 3-4 kHz relative to white, lift at the top.
            assert!(at(3500.0) < -3.0);
            assert!(at(3500.0) < at(15000.0).min(at(20000.0)) - 6.0);
            // Audibility-weighted noise is lower than white noise.
            let weighted = |shape: &dyn Fn(f64) -> f64| {
                let (mut s, n) = (0.0, 2000);
                for i in 0..n {
                    let f = (i as f64 + 0.5) / n as f64 * fs * 0.5;
                    let t = threshold_in_quiet_db(f).clamp(-5.0, 25.0);
                    s += 10f64.powf(-t / 10.0) * shape(f);
                }
                s
            };
            let shaped = weighted(&|f| h(f / fs));
            let flat = weighted(&|_| 1.0);
            eprintln!("  weighted noise vs white: {:.2} dB", db(shaped / flat));
            assert!(shaped < flat * 0.5);
            // Gerzon-Craven: the log-average of |A| over frequency is zero.
            let n = 4000;
            let mean: f64 = (0..n).map(|i| h((i as f64 + 0.5) / n as f64 * 0.5).ln()).sum::<f64>() / n as f64;
            assert!(mean.abs() < 1e-3, "log mean {mean}");
        }
    }

    #[test]
    fn perceptual_noise_tilts_in_the_output() {
        let e = error_of(NoiseShaping::Perceptual, FS);
        let w = band_powers(&error_of(NoiseShaping::None, FS), &[(0.07, 0.09)])[0];
        // 3.5 kHz is 0.073 of 48 kHz; the top band is 15-20 kHz.
        let p = band_powers(&e, &[(0.07, 0.09), (0.33, 0.41)]);
        eprintln!("perceptual output: 3.4-4.3k {:.1} dB, 16-19.7k {:.1} dB vs white", db(p[0] / w), db(p[1] / w));
        assert!(p[0] < w * 0.5);
        assert!(p[1] > w * 2.0);
    }

    #[test]
    fn shaping_stays_stable_when_clipping() {
        let mut d = Dither::new(1, 44_100.0, 8, 3);
        d.set_shaping(NoiseShaping::Perceptual);
        let mut buf: Vec<f32> = (0..50_000).map(|i| if (i / 100) % 2 == 0 { 1.5 } else { -1.5 }).collect();
        d.process(0, &mut buf);
        assert!(buf.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        // Back to a small signal: the error has not wound up.
        let mut quiet = vec![0.1f32; 4096];
        d.process(0, &mut quiet);
        let m: f32 = quiet.iter().sum::<f32>() / quiet.len() as f32;
        assert!((m - 0.1).abs() < 0.02, "{m}");
    }
}
