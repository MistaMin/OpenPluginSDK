//! De-esser: sibilance reduction driven by a band-limited sidechain.
//!
//! Structure (the standard split-detector arrangement, see Zoelzer, "DAFX",
//! 2nd ed., 2011, section 4.2, which describes compression driven by a
//! filtered sidechain; written for this crate, no code consulted):
//!
//! - **Detection.** The detector signal is the input (or an external key)
//!   through a state-variable filter ([`Svf`]): a band-pass at `freq`/`q`
//!   ([`DeEssDetect::BandPass`]) or a high-pass at `freq` ([`DeEssDetect::HighPass`],
//!   the "high-shelf" style that listens to everything above the corner). The
//!   rectified detector sample, linked over channels by the largest value,
//!   drives a [`Dynamics`] compressor (peak detection, decoupled ballistics,
//!   range = maximum reduction).
//! - **Wide-band mode** multiplies the whole signal by the gain.
//! - **Split-band mode** reduces only the detected band: with the filter's
//!   exact split `low + band + high == x` the output is `x + (g - 1) * part`
//!   where `part` is the band-pass (or high-pass) component. At `g = 1` the
//!   output equals the input up to rounding, at the band centre the
//!   reduction is exactly `g`, and everything outside the band is left alone.
//!   (A high-pass component cannot be scaled this way: it is 90 degrees off
//!   the input at its corner, so `x + (g - 1) hp` would boost there. With
//!   [`DeEssDetect::HighPass`] split-band mode instead uses a Linkwitz-Riley 4
//!   split at `freq` and outputs `low + g * high`, in phase at the crossover;
//!   at `g = 1` that is the crossover's all-pass of the input, flat in
//!   magnitude, not the input itself.)
//! - **Listen** outputs the detector signal for tuning `freq`.
//!
//! The threshold is in dBFS of a sine in the detected band. No allocation, no
//! latency, non-finite input counts as `0.0`, NaN or infinite settings are
//! ignored.

#![allow(clippy::needless_range_loop)]

use crate::envelope::{Detection, Domain, Topology};
use crate::gain_computer::{Dynamics, DynamicsMode, DynamicsParams, Link, Smoothing};
use crate::crossover::{LinkwitzRiley, LrOrder};
use crate::svf::{Svf, SvfMode};

/// Which signal the detector listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeEssDetect {
    BandPass,
    HighPass,
}

/// Where the reduction acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeEssMode {
    WideBand,
    SplitBand,
}

/// De-esser for any number of channels (constructed for a fixed count).
#[derive(Debug, Clone)]
pub struct DeEsser {
    fs: f32,
    channels: usize,
    det: Vec<Svf>,
    split: Vec<Svf>,
    lr: Vec<LinkwitzRiley>,
    dynamics: Dynamics,
    params: DynamicsParams,
    freq: f32,
    q: f32,
    detect: DeEssDetect,
    mode: DeEssMode,
    listen: bool,
}

impl DeEsser {
    /// Defaults: band-pass at 6.5 kHz, Q 1.5, threshold -30 dB, ratio 4,
    /// range 12 dB, attack 0.5 ms, release 40 ms, split-band.
    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let channels = channels.max(1);
        let params = DynamicsParams {
            mode: DynamicsMode::Compressor,
            threshold_db: -30.0,
            ratio: 4.0,
            knee_db: 6.0,
            range_db: 12.0,
            attack_ms: 0.5,
            release_ms: 40.0,
            detection: Detection::Peak,
            domain: Domain::Log,
            topology: Topology::Decoupled,
            smoothing: Smoothing::Level,
            link: Link::Max,
            ..DynamicsParams::default()
        };
        let mut d = Dynamics::new(sample_rate);
        d.set_params(params);
        let mut s = Self {
            fs: sample_rate,
            channels,
            det: vec![Svf::new(); channels],
            split: vec![Svf::new(); channels],
            lr: vec![LinkwitzRiley::new(LrOrder::Lr4); channels],
            dynamics: d,
            params,
            freq: 6500.0,
            q: 1.5,
            detect: DeEssDetect::BandPass,
            mode: DeEssMode::SplitBand,
            listen: false,
        };
        s.configure();
        s
    }

    fn configure(&mut self) {
        let (m, q) = match self.detect {
            DeEssDetect::BandPass => (SvfMode::BandPass, self.q),
            DeEssDetect::HighPass => (SvfMode::HighPass, 0.707),
        };
        for f in &mut self.det {
            f.configure(m, self.fs, self.freq, q, 0.0);
        }
        for f in &mut self.lr {
            f.set(self.fs, self.freq);
        }
        for f in &mut self.split {
            f.configure(SvfMode::LowPass, self.fs, self.freq, q, 0.0);
        }
    }

    fn apply(&mut self) {
        self.dynamics.set_params(self.params);
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn reset(&mut self) {
        for f in self.det.iter_mut().chain(self.split.iter_mut()) {
            f.reset();
        }
        for f in &mut self.lr {
            f.reset();
        }
        self.dynamics.reset();
    }

    pub fn set_frequency(&mut self, hz: f32) {
        if hz.is_finite() {
            self.freq = hz;
            self.configure();
        }
    }

    pub fn set_q(&mut self, q: f32) {
        if q.is_finite() {
            self.q = q;
            self.configure();
        }
    }

    pub fn set_threshold_db(&mut self, db: f32) {
        if db.is_finite() {
            self.params.threshold_db = db;
            self.apply();
        }
    }

    pub fn set_ratio(&mut self, ratio: f32) {
        if !ratio.is_nan() {
            self.params.ratio = ratio.max(1.0);
            self.apply();
        }
    }

    /// Maximum reduction in dB (positive).
    pub fn set_range_db(&mut self, db: f32) {
        if db.is_finite() {
            self.params.range_db = db.clamp(0.0, 60.0);
            self.apply();
        }
    }

    pub fn set_times_ms(&mut self, attack_ms: f32, release_ms: f32) {
        if attack_ms.is_finite() && release_ms.is_finite() {
            self.params.attack_ms = attack_ms.max(0.0);
            self.params.release_ms = release_ms.max(0.0);
            self.apply();
        }
    }

    pub fn set_detect(&mut self, d: DeEssDetect) {
        self.detect = d;
        self.configure();
    }

    pub fn set_mode(&mut self, m: DeEssMode) {
        self.mode = m;
    }

    pub fn set_listen(&mut self, on: bool) {
        self.listen = on;
    }

    /// Gain reduction of the last frame in dB (positive).
    pub fn gain_reduction_db(&self) -> f32 {
        self.dynamics.gain_reduction_db()
    }

    /// Processes in place; the detector listens to the input.
    pub fn process(&mut self, channels: &mut [&mut [f32]]) {
        self.run(channels, None);
    }

    /// Processes in place with an external key for the detector.
    pub fn process_sidechain(&mut self, channels: &mut [&mut [f32]], key: &[&[f32]]) {
        if !key.is_empty() {
            self.run(channels, Some(key));
        }
    }

    fn run(&mut self, channels: &mut [&mut [f32]], key: Option<&[&[f32]]>) {
        let nc = channels.len().min(self.channels);
        if nc == 0 {
            return;
        }
        let mut frames = channels[..nc].iter().map(|c| c.len()).min().unwrap_or(0);
        if let Some(k) = key {
            frames = frames.min(k.iter().map(|c| c.len()).min().unwrap_or(0));
        }
        for i in 0..frames {
            let mut det = 0.0f32;
            let mut dsig = [0.0f32; 2];
            for c in 0..nc {
                let raw = match key {
                    Some(k) => k[c.min(k.len() - 1)][i],
                    None => channels[c][i],
                };
                let v = self.det[c].process_sample(raw);
                if c < 2 {
                    dsig[c] = v;
                }
                det = det.max(v.abs());
            }
            let g = self.dynamics.next_gain(det);
            for c in 0..nc {
                let x = if channels[c][i].is_finite() { channels[c][i] } else { 0.0 };
                // Always run the split filter so mode switches are clean.
                let (_, bp, _) = self.split[c].process_split(x);
                let (lo, hi) = self.lr[c].process_sample(x);
                let y = if self.listen {
                    if c < 2 { dsig[c] } else { 0.0 }
                } else {
                    match self.mode {
                        DeEssMode::WideBand => x * g,
                        DeEssMode::SplitBand => {
                            match self.detect {
                                DeEssDetect::BandPass => x + (g - 1.0) * bp,
                                DeEssDetect::HighPass => lo + g * hi,
                            }
                        }
                    }
                };
                channels[c][i] = if y.is_finite() { y } else { 0.0 };
            }
        }
    }
}
