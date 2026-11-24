//! Multiband compressor on the Linkwitz-Riley crossovers.
//!
//! Structure (the textbook multiband arrangement, e.g. Zoelzer, "DAFX: Digital
//! Audio Effects", 2nd ed., Wiley 2011, section 4.2 on dynamic range control
//! with band splitting; the crossover algebra is in [`crate::crossover`]):
//!
//! 1. Each channel is split into 2, 3 or 4 bands by a [`Crossover`] whose bands
//!    sum to an all-pass. With every band at unity gain the output therefore has
//!    a flat magnitude (the tests measure the sum within 0.01 dB from 20 Hz to
//!    20 kHz at several rates; only the phase turns).
//! 2. Each band has its own [`Dynamics`] (any [`DynamicsParams`]: compressor,
//!    expander or gate, knee, ballistics, detection, channel link). The band's
//!    detector listens to that band's signal, linked across channels as the
//!    band's `Link` says, so every channel of a band gets the same gain.
//! 3. **Link across bands.** `set_band_link(l)` with `l` in 0..=1 blends each
//!    band's gain reduction with the deepest reduction among the active bands:
//!    `gr_i' = (1 - l) gr_i + l min_j gr_j` (in dB, reductions negative). `0`
//!    keeps the bands independent, `1` makes every band follow the deepest one.
//! 4. **Solo and bypass** per band. Bypass takes the band's dynamics out of the
//!    path (it still sums in, at unity gain apart from its trim). Solo mutes
//!    the other bands; when anything is soloed only soloed bands are heard.
//!    Solo and trim changes are slewed (5 ms one-pole) so they do not click.
//!
//! No allocation after [`MultibandCompressor::new`], no latency, `f32` audio
//! with `f64` filter state, non-finite input is treated as `0.0`, and the
//! smoothers are flushed so no subnormal number circulates. Frequencies and
//! parameters are sanitised by the blocks underneath.
//!
//! The LR2 crossover reverses the polarity of its high band (see
//! [`crate::crossover`]); that is what makes the sum flat, and it is invisible
//! at the output.

#![allow(clippy::needless_range_loop)]

use crate::crossover::{Crossover, LrOrder, MAX_BANDS};
use crate::envelope::db_to_amp;
use crate::gain_computer::{Dynamics, DynamicsParams, Link};
use crate::rate::effective_rate;

/// Time constant of the solo/trim slew.
const SLEW_MS: f64 = 5.0;

#[derive(Debug, Clone)]
struct Band {
    dynamics: Dynamics,
    bypass: bool,
    solo: bool,
    trim_db: f32,
    /// Smoothed linear static gain (trim and solo mute).
    gain: f32,
}

/// Multiband compressor with 2 to 4 bands. See the [module docs](self).
/// `new` allocates; `process` and the setters do not.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{DynamicsParams, LrOrder, MultibandCompressor};
///
/// let mut mb = MultibandCompressor::new(48_000.0, 2, 3, LrOrder::Lr4, &[200.0, 3_000.0]);
/// mb.set_band_params(0, DynamicsParams { threshold_db: -30.0, ratio: 3.0, ..DynamicsParams::default() });
/// let mut left = vec![0.5f32; 4_800];
/// let mut right = vec![0.5f32; 4_800];
/// mb.process(&mut [&mut left, &mut right]);
/// assert!(mb.gain_reduction_db(0) > 0.0); // DC lands in the low band
/// assert_eq!(mb.bands(), 3);
/// ```
#[derive(Debug, Clone)]
pub struct MultibandCompressor {
    channels: usize,
    bands: usize,
    fs: f32,
    xo: Vec<Crossover>,
    band: [Band; MAX_BANDS],
    link: f32,
    slew: f32,
    /// Split output of the current frame, `channels * MAX_BANDS`.
    split: Vec<f32>,
    reduction: [f32; MAX_BANDS],
}

impl MultibandCompressor {
    /// `bands` is clamped to 2..=4 and `channels` to at least 1. `freqs` holds
    /// `bands - 1` crossover frequencies in Hz (see [`Crossover::new`]).
    pub fn new(sample_rate: f32, channels: usize, bands: usize, order: LrOrder, freqs: &[f32]) -> Self {
        let channels = channels.max(1);
        let bands = bands.clamp(2, MAX_BANDS);
        let fs = effective_rate(sample_rate);
        let mk = || Band {
            dynamics: Dynamics::new(fs),
            bypass: false,
            solo: false,
            trim_db: 0.0,
            gain: 1.0,
        };
        Self {
            channels,
            bands,
            fs,
            xo: (0..channels).map(|_| Crossover::new(order, bands, fs, freqs)).collect(),
            band: [mk(), mk(), mk(), mk()],
            link: 0.0,
            slew: (1.0 - (-1.0 / (SLEW_MS * 0.001 * f64::from(fs))).exp()) as f32,
            split: vec![0.0; channels * MAX_BANDS],
            reduction: [0.0; MAX_BANDS],
        }
    }

    /// The band count.
    pub fn bands(&self) -> usize {
        self.bands
    }

    /// The channel count.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Clears every filter, detector and slew state.
    pub fn reset(&mut self) {
        for x in &mut self.xo {
            x.reset();
        }
        for b in &mut self.band {
            b.dynamics.reset();
        }
        self.reduction = [0.0; MAX_BANDS];
        self.retarget(true);
    }

    /// Sets the crossover frequencies (ascending; see [`Crossover::set_frequencies`]).
    /// Safe to call every block.
    pub fn set_crossovers(&mut self, freqs: &[f32]) {
        for x in &mut self.xo {
            x.set_frequencies(self.fs, freqs);
        }
    }

    /// Settings of one band (the index is clamped to the band count).
    pub fn set_band_params(&mut self, band: usize, params: DynamicsParams) {
        self.band[band.min(self.bands - 1)].dynamics.set_params(params);
    }

    /// The settings of `band` (clamped to the last band).
    pub fn band_params(&self, band: usize) -> &DynamicsParams {
        self.band[band.min(self.bands - 1)].dynamics.params()
    }

    /// Takes the band's dynamics out of the path.
    pub fn set_band_bypass(&mut self, band: usize, bypass: bool) {
        self.band[band.min(self.bands - 1)].bypass = bypass;
    }

    /// Solos a band; with any band soloed the others are muted.
    pub fn set_band_solo(&mut self, band: usize, solo: bool) {
        self.band[band.min(self.bands - 1)].solo = solo;
    }

    /// Output trim of a band in dB (+-60; NaN keeps the old value).
    pub fn set_band_trim_db(&mut self, band: usize, db: f32) {
        if db.is_finite() {
            self.band[band.min(self.bands - 1)].trim_db = db.clamp(-60.0, 60.0);
        }
    }

    /// Link across bands, 0 (independent) to 1 (all follow the deepest).
    pub fn set_band_link(&mut self, link: f32) {
        if link.is_finite() {
            self.link = link.clamp(0.0, 1.0);
        }
    }

    /// Gain reduction of a band at the last frame in dB (positive), after
    /// band linking.
    pub fn gain_reduction_db(&self, band: usize) -> f32 {
        -self.reduction[band.min(self.bands - 1)]
    }

    fn retarget(&mut self, snap: bool) {
        let any_solo = self.band[..self.bands].iter().any(|b| b.solo);
        for b in &mut self.band[..self.bands] {
            let target = if any_solo && !b.solo { 0.0 } else { db_to_amp(b.trim_db) };
            if snap {
                b.gain = target;
            } else {
                b.gain += (target - b.gain) * self.slew;
                if (b.gain - target).abs() < 1.0e-7 {
                    b.gain = target;
                }
            }
        }
    }

    /// Processes the channels in place. Only the first [`channels`](Self::channels)
    /// are processed; frames are limited to the shortest of them.
    pub fn process(&mut self, channels: &mut [&mut [f32]]) {
        let nc = channels.len().min(self.channels);
        if nc == 0 {
            return;
        }
        let frames = channels[..nc].iter().map(|c| c.len()).min().unwrap_or(0);
        let nb = self.bands;
        // A solo or trim change is picked up here and slewed per sample below.
        for i in 0..frames {
            for c in 0..nc {
                let b = self.xo[c].process_sample(channels[c][i]);
                self.split[c * MAX_BANDS..(c + 1) * MAX_BANDS].copy_from_slice(&b);
            }
            self.retarget(false);
            // Per-band detector (linked over channels) and gain.
            let mut gr = [0.0f32; MAX_BANDS];
            let mut deepest = 0.0f32;
            for k in 0..nb {
                let mut max = 0.0f32;
                let mut sum = 0.0f32;
                for c in 0..nc {
                    let v = self.split[c * MAX_BANDS + k].abs();
                    max = max.max(v);
                    sum += v;
                }
                let det = match self.band[k].dynamics.params().link {
                    Link::Max => max,
                    Link::Average => sum / nc as f32,
                };
                // Run even when bypassed so the state is warm on un-bypass.
                self.band[k].dynamics.next_gain(det);
                gr[k] = if self.band[k].bypass { 0.0 } else { self.band[k].dynamics.gain_db() };
                if !self.band[k].bypass {
                    deepest = deepest.min(gr[k]);
                }
            }
            let mut gains = [0.0f32; MAX_BANDS];
            for k in 0..nb {
                let b = &self.band[k];
                let (db, red) = if b.bypass {
                    (0.0, 0.0)
                } else {
                    let linked = (1.0 - self.link) * gr[k] + self.link * deepest;
                    (linked + b.dynamics.params().makeup_db, linked)
                };
                self.reduction[k] = red;
                gains[k] = if db == 0.0 { b.gain } else { db_to_amp(db) * b.gain };
            }
            for c in 0..nc {
                let mut acc = 0.0f32;
                for k in 0..nb {
                    acc += self.split[c * MAX_BANDS + k] * gains[k];
                }
                channels[c][i] = if acc.is_finite() { acc } else { 0.0 };
            }
        }
    }
}
