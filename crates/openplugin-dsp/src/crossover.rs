//! Linkwitz-Riley crossovers for 2, 3 or 4 bands with flat summed magnitude.
//!
//! A Linkwitz-Riley filter of order `2N` is a Butterworth filter of order `N`
//! applied twice (S. Linkwitz, "Active crossover networks for noncoincident
//! drivers", JAES 24(1), 1976; the derivation of the sum is textbook algebra).
//! Its low and high outputs are -6 dB at the crossover and add to an all-pass:
//!
//! - LR4 (`N = 2`) and LR8 (`N = 4`): `low + high` is the all-pass
//!   `B_N(-s) / B_N(s)` (second and fourth order), so the magnitude is flat
//!   and only the phase turns.
//! - LR2 (`N = 1`): `low + high` has a null at the crossover, but
//!   `low - high = (1 - s) / (1 + s)` is an all-pass. This module therefore
//!   **inverts the high output of an LR2** so the bands sum flat; the high band
//!   of an LR2 crossover is polarity-reversed relative to the input.
//!
//! The identities hold in the digital domain too, because every section is a
//! TPT/bilinear image of the analog one (see [`crate::Svf`]) and the identity is
//! algebraic in `s`; the tests measure the sum within 0.01 dB from 20 Hz to
//! 20 kHz.
//!
//! # Multiband
//!
//! [`Crossover`] splits the signal in a tree: band 0 is the low output of
//! split 1, split 2 divides the high output of split 1, and so on. A band that
//! sits below a later split must pass the all-pass the later splits impose on
//! the bands above it, or the sum would no longer be flat. The compensation
//! filter for split `j` is that split's own `low + high` (an all-pass), run on
//! the earlier bands. The band sum is then `AP_n ... AP_1 x`: flat in
//! magnitude. For four bands that costs 3 splits and 3 compensators.
//!
//! No allocation, `f64` state, non-finite input treated as `0.0`, NaN or
//! infinite frequencies ignored; see [`crate::filters`] for the shared rules.
//! Crossover frequencies are clamped to 1 Hz..0.499 fs and forced ascending.

use crate::filters::{Butterworth, ButterworthKind};

/// Linkwitz-Riley order: slope per band edge is 12, 24 or 48 dB/octave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LrOrder {
    /// Second order, 12 dB/octave. The high band is polarity-inverted (see the
    /// module documentation).
    Lr2,
    /// Fourth order, 24 dB/octave.
    Lr4,
    /// Eighth order, 48 dB/octave.
    Lr8,
}

impl LrOrder {
    fn butterworth_order(self) -> usize {
        match self {
            LrOrder::Lr2 => 1,
            LrOrder::Lr4 => 2,
            LrOrder::Lr8 => 4,
        }
    }
}

/// One two-band Linkwitz-Riley split. [`LinkwitzRiley::process_sample`] returns
/// `(low, high)`; for LR2 `high` is polarity-inverted (see the module docs).
#[derive(Debug, Clone)]
///
/// # Example
///
/// ```
/// use openplugin_dsp::{LinkwitzRiley, LrOrder};
///
/// let mut split = LinkwitzRiley::new(LrOrder::Lr4);
/// split.set(48_000.0, 1_000.0);
/// let (low, high) = split.process_sample(1.0);
/// assert!(low.is_finite() && high.is_finite());
/// ```
pub struct LinkwitzRiley {
    order: LrOrder,
    low: [Butterworth; 2],
    high: [Butterworth; 2],
}

impl LinkwitzRiley {
    /// A split of the given order. It passes nothing useful until
    /// [`set`](Self::set) gives it a frequency. Does not allocate.
    pub fn new(order: LrOrder) -> Self {
        Self {
            order,
            low: [Butterworth::new(), Butterworth::new()],
            high: [Butterworth::new(), Butterworth::new()],
        }
    }

    /// The order chosen at construction.
    pub fn order(&self) -> LrOrder {
        self.order
    }

    /// Clears the filter state and keeps the crossover frequency.
    pub fn reset(&mut self) {
        for f in self.low.iter_mut().chain(self.high.iter_mut()) {
            f.reset();
        }
    }

    /// Sets the crossover frequency `freq` in Hz for `sample_rate` in Hz. Safe
    /// to call every sample; non-finite values are ignored (see
    /// [`Butterworth::set`]).
    pub fn set(&mut self, sample_rate: f32, freq: f32) {
        let n = self.order.butterworth_order();
        for f in &mut self.low {
            f.set(ButterworthKind::LowPass, n, sample_rate, freq);
        }
        for f in &mut self.high {
            f.set(ButterworthKind::HighPass, n, sample_rate, freq);
        }
    }

    /// `(low, high)`.
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> (f32, f32) {
        let lo0 = self.low[0].process_sample(x);
        let lo = self.low[1].process_sample(lo0);
        let hi0 = self.high[0].process_sample(x);
        let hi = self.high[1].process_sample(hi0);
        if self.order == LrOrder::Lr2 {
            (lo, -hi)
        } else {
            (lo, hi)
        }
    }

    /// `low + high`: the magnitude-flat all-pass (same sample as the split).
    #[inline]
    pub fn process_allpass(&mut self, x: f32) -> f32 {
        let (lo, hi) = self.process_sample(x);
        lo + hi
    }
}

/// Maximum number of bands.
pub const MAX_BANDS: usize = 4;

/// 2-, 3- or 4-band crossover whose bands sum to an all-pass of the input.
#[derive(Debug, Clone)]
///
/// # Example
///
/// ```
/// use openplugin_dsp::{Crossover, LrOrder};
///
/// let mut crossover = Crossover::new(LrOrder::Lr4, 3, 48_000.0, &[200.0, 2_000.0]);
/// let bands = crossover.process_sample(1.0);
/// // Only the first `bands()` entries are used; they sum to an all-pass of the input.
/// let sum: f32 = bands[..crossover.bands()].iter().sum();
/// assert!(sum.is_finite());
/// ```
pub struct Crossover {
    bands: usize,
    splits: [LinkwitzRiley; 3],
    /// Compensators: `[0]` = split 2 on band 0, `[1]` = split 3 on band 0,
    /// `[2]` = split 3 on band 1 (the unused ones stay idle).
    comp: [LinkwitzRiley; 3],
}

impl Crossover {
    /// `bands` is clamped to 2..=4. `freqs` holds `bands - 1` crossover
    /// frequencies in Hz (missing entries repeat the last given, or 1 kHz).
    pub fn new(order: LrOrder, bands: usize, sample_rate: f32, freqs: &[f32]) -> Self {
        let mut c = Self {
            bands: bands.clamp(2, MAX_BANDS),
            splits: [
                LinkwitzRiley::new(order),
                LinkwitzRiley::new(order),
                LinkwitzRiley::new(order),
            ],
            comp: [
                LinkwitzRiley::new(order),
                LinkwitzRiley::new(order),
                LinkwitzRiley::new(order),
            ],
        };
        c.set_frequencies(sample_rate, freqs);
        c
    }

    /// Number of bands, 2..=4.
    pub fn bands(&self) -> usize {
        self.bands
    }

    /// Clears the filter state and keeps the crossover frequencies.
    pub fn reset(&mut self) {
        for f in self.splits.iter_mut().chain(self.comp.iter_mut()) {
            f.reset();
        }
    }

    /// Sets the crossover frequencies (safe to call every sample). Non-finite
    /// entries keep the previous value; a frequency below its predecessor is
    /// raised to it.
    pub fn set_frequencies(&mut self, sample_rate: f32, freqs: &[f32]) {
        let n = self.bands - 1;
        let mut prev = 1.0_f32;
        for i in 0..n {
            let f = match freqs.get(i).copied() {
                Some(f) if f.is_finite() => f,
                Some(_) => continue,
                None => freqs.last().copied().filter(|v| v.is_finite()).unwrap_or(1000.0),
            };
            let f = f.max(prev);
            prev = f;
            self.splits[i].set(sample_rate, f);
            match (n, i) {
                (3, 1) => self.comp[0].set(sample_rate, f),
                (3, 2) => {
                    self.comp[1].set(sample_rate, f);
                    self.comp[2].set(sample_rate, f);
                }
                (2, 1) => self.comp[0].set(sample_rate, f),
                _ => {}
            }
        }
    }

    /// Splits one sample. Returns the bands low to high; entries at and above
    /// [`Crossover::bands`] are `0.0`.
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> [f32; MAX_BANDS] {
        let mut out = [0.0; MAX_BANDS];
        let (l1, h1) = self.splits[0].process_sample(x);
        match self.bands {
            2 => {
                out[0] = l1;
                out[1] = h1;
            }
            3 => {
                let (l2, h2) = self.splits[1].process_sample(h1);
                out[0] = self.comp[0].process_allpass(l1);
                out[1] = l2;
                out[2] = h2;
            }
            _ => {
                let (l2, h2) = self.splits[1].process_sample(h1);
                let (l3, h3) = self.splits[2].process_sample(h2);
                let a0 = self.comp[0].process_allpass(l1);
                out[0] = self.comp[1].process_allpass(a0);
                out[1] = self.comp[2].process_allpass(l2);
                out[2] = l3;
                out[3] = h3;
            }
        }
        out
    }
}
