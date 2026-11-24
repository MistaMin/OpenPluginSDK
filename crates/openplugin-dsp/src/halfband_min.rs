//! Minimum-phase half-band filters for 2x oversampling.
//!
//! [`MinPhaseHalfband`] has the same shape as [`crate::Oversampler2x`]
//! (`upsample_2x`, `downsample_2x`, `reset`) but uses one designed
//! minimum-phase FIR as both the interpolation and the anti-alias filter, in
//! polyphase form (only the outputs that are kept are computed).
//!
//! Design: a Kaiser linear-phase low-pass prototype with cutoff at the base
//! Nyquist (a quarter of the 2x rate) is spectrally factored
//! ([`crate::fir_phase::minimum_phase_from_linear`]) into a minimum-phase filter
//! of half the order. Passband edge is a user figure, the stopband starts at
//! the mirror image `fs - passband` so that content folding about the base
//! Nyquist lands in the stopband. The prototype is asked for `2 A + 10` dB
//! because taking the square root halves dB figures (the lift adds 3 dB, the rest is margin for the Kaiser estimate).
//! Delay is low and frequency dependent (group delay is largest near the
//! cutoff); a linear-phase half-band of equal rejection has `(N-1)/2` samples at
//! the 2x rate at all frequencies and pre-rings.
//!
//! Not a strict half-band (the odd taps are not zero), so no polyphase tap is
//! saved over a general FIR; this is the price of minimum phase.

use crate::fir_design::{design_kaiser_f64, FirError, FirKind};
use crate::fir_phase::minphase_from_linear_f64;

/// History of the newest `k` samples, newest first, contiguous.
#[derive(Clone, Debug)]
struct History {
    buf: Vec<f32>,
    pos: usize,
    k: usize,
}

impl History {
    fn new(k: usize) -> Self {
        Self {
            buf: vec![0.0; 2 * k],
            pos: 0,
            k,
        }
    }
    fn push(&mut self, x: f32) {
        self.pos = if self.pos == 0 { self.k - 1 } else { self.pos - 1 };
        self.buf[self.pos] = x;
        self.buf[self.pos + self.k] = x;
    }
    fn window(&self) -> &[f32] {
        &self.buf[self.pos..self.pos + self.k]
    }
    fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// 2x oversampling with a designed minimum-phase half-band filter.
/// `design` allocates; `upsample_2x` and `downsample_2x` do not.
///
/// # Example
///
/// ```
/// use openplugin_dsp::MinPhaseHalfband;
///
/// let mut up = MinPhaseHalfband::design(48_000.0, 20_000.0, 90.0).unwrap();
/// let mut down = up.clone();
/// let input = vec![0.5f32; 256];
/// let mut oversampled = vec![0.0f32; 512];
/// up.upsample_2x(&input, &mut oversampled);
/// // ... the non-linear stage runs at 96 kHz here ...
/// let mut output = vec![0.0f32; 256];
/// down.downsample_2x(&oversampled, &mut output);
/// assert!((output[255] - 0.5).abs() < 1e-3); // DC passes at unity
/// assert!(MinPhaseHalfband::design(48_000.0, 30_000.0, 90.0).is_err()); // passband above Nyquist
/// ```
#[derive(Clone, Debug)]
pub struct MinPhaseHalfband {
    taps: Vec<f32>,
    even: Vec<f32>,
    odd: Vec<f32>,
    up: History,
    down_even: History,
    down_odd: History,
}

impl MinPhaseHalfband {
    /// Design for a base rate `sample_rate`, passband edge `passband_hz`
    /// (below the base Nyquist, e.g. 20 kHz at 44.1 kHz) and stopband
    /// attenuation `stopband_db` (10 to 130). Allocates.
    pub fn design(
        sample_rate: f64,
        passband_hz: f64,
        stopband_db: f64,
    ) -> Result<Self, FirError> {
        let nyq = sample_rate / 2.0;
        if !(sample_rate.is_finite()
            && passband_hz.is_finite()
            && passband_hz > 0.0
            && passband_hz < nyq
            && (10.0..=130.0).contains(&stopband_db))
        {
            return Err(FirError::BadSpec);
        }
        let over = 2.0 * sample_rate;
        let transition = sample_rate - 2.0 * passband_hz;
        let proto = design_kaiser_f64(
            FirKind::LowPass { cutoff_hz: nyq },
            over,
            2.0 * stopband_db + 10.0,
            transition,
        )?;
        let h = minphase_from_linear_f64(&proto)?;
        let taps: Vec<f32> = h.iter().map(|&v| v as f32).collect();
        Ok(Self::from_taps(taps))
    }

    fn from_taps(taps: Vec<f32>) -> Self {
        let k = taps.len().div_ceil(2);
        let mut even = vec![0.0; k];
        let mut odd = vec![0.0; k];
        for (i, &t) in taps.iter().enumerate() {
            if i % 2 == 0 {
                even[i / 2] = t;
            } else {
                odd[i / 2] = t;
            }
        }
        Self {
            taps,
            even,
            odd,
            up: History::new(k),
            down_even: History::new(k),
            down_odd: History::new(k),
        }
    }

    /// The filter at the 2x rate (unity passband gain).
    pub fn taps(&self) -> &[f32] {
        &self.taps
    }

    /// Clears the filter history. Does not allocate.
    pub fn reset(&mut self) {
        self.up.clear();
        self.down_even.clear();
        self.down_odd.clear();
    }

    /// Group delay in base-rate samples at `freq_hz` (a round trip is twice this).
    pub fn group_delay_base_samples(&self, freq_hz: f64, sample_rate: f64) -> f64 {
        crate::fir_design::fir_group_delay(&self.taps, freq_hz / (2.0 * sample_rate)) / 2.0
    }

    /// Zero-stuff and filter: `output.len() >= 2 * input.len()`. Gain 2 makes
    /// the passband unity after the zero stuffing.
    pub fn upsample_2x(&mut self, input: &[f32], output: &mut [f32]) {
        assert!(output.len() >= input.len() * 2);
        for (i, &x) in input.iter().enumerate() {
            self.up.push(x);
            let w = self.up.window();
            output[2 * i] = 2.0 * dot(&self.even, w);
            output[2 * i + 1] = 2.0 * dot(&self.odd, w);
        }
    }

    /// Filter and decimate: `oversampled.len() >= 2 * output.len()`.
    pub fn downsample_2x(&mut self, oversampled: &[f32], output: &mut [f32]) {
        assert!(oversampled.len() >= output.len() * 2);
        for (i, y) in output.iter_mut().enumerate() {
            self.down_even.push(oversampled[2 * i]);
            *y = dot(&self.even, self.down_even.window())
                + dot(&self.odd, self.down_odd.window());
            self.down_odd.push(oversampled[2 * i + 1]);
        }
    }
}
