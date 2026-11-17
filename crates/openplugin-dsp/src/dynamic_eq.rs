//! Dynamic EQ band: an [`Svf`] bell or shelf whose gain follows a level
//! detector.
//!
//! A dynamic EQ band is a parametric band whose gain is not fixed but pushed
//! away from its resting value when a detector crosses a threshold, i.e. a
//! compressor or expander gain law (Giannoulis, Massberg and Reiss, "Digital
//! Dynamic Range Compressor Design", JAES 60(6), 2012, eqs. 4-5, the same
//! quadratic knee as [`crate::GainComputer`]) steering an EQ section instead of
//! a broadband gain. Written from the papers named here, with no code consulted.
//!
//! # Signal flow
//!
//! 1. **Detector.** The detector signal is the band's own input or an external
//!    sidechain key, filtered to the band: band-pass at `freq`/`q` for a bell,
//!    low-pass for a low shelf, high-pass for a high shelf (so a shelf listens
//!    to what it will act on). Channels are linked by the largest rectified
//!    sample. The level is a one-pole RMS ([`EnvelopeFollower`]) in dB,
//!    **calibrated so a sine reads its peak amplitude** (+3.01 dB), so the
//!    threshold is in dBFS of a sine.
//! 2. **Gain law.** `excess` is `level - threshold` in [`DynEqMode::Above`] and
//!    `threshold - level` in [`DynEqMode::Below`]; with the soft knee `w`
//!    (quadratic, zero below `-w/2`) the target engagement in dB is
//!    `min(|range|, (1 - 1/ratio) * excess)`, so a ratio of infinity reaches the
//!    full `range_db` at once. The sign of `range_db` is the direction: negative
//!    cuts when triggered, positive boosts.
//! 3. **Smoothing.** The engagement (dB) goes through [`Ballistics`], attack
//!    while it grows and release while it falls, each a true time constant: a
//!    step covers 63.2 % in `attack_ms`.
//! 4. **Filter.** The applied gain is `gain_db + sign(range) * engagement`.
//!
//! # Coefficient updates
//!
//! The SVF is a topology-preserving-transform filter, stable under any
//! coefficient change and free of the transient a direct-form biquad injects
//! (see [`crate::svf`]), so its coefficients may move at any rate. They are
//! recomputed every **control period of 125 us** (`fs / 8000` samples, 6 at
//! 48 kHz, minimum 1) instead of every sample, because a bell or shelf needs a
//! `tan` and a `powf`; the engagement itself is smoothed at audio rate and the
//! coefficients only sample it. A change smaller than 0.001 dB is not applied.
//!
//! # Real-time and error behaviour
//!
//! No allocation after [`DynamicEqBand::new`], no latency, `f64` filter state,
//! non-finite input is treated as `0.0` (and the key likewise), subnormal state
//! is flushed, NaN or infinite settings are ignored.

#![allow(clippy::needless_range_loop)]

use crate::envelope::{Ballistics, Detection, EnvelopeFollower};
use crate::rate::effective_rate;
use crate::svf::{Svf, SvfMode};

/// A sine's peak is 3.01 dB above its RMS.
const SINE_PEAK_DB: f32 = 3.0103;

/// Shape of the band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DynEqKind {
    Bell,
    LowShelf,
    HighShelf,
}

/// When the band reacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DynEqMode {
    /// Engages as the detected level rises above the threshold.
    Above,
    /// Engages as the detected level falls below the threshold (upward
    /// processing; silence engages it fully).
    Below,
}

/// Settings of a [`DynamicEqBand`].
#[derive(Debug, Clone, Copy)]
pub struct DynamicEqParams {
    pub kind: DynEqKind,
    pub mode: DynEqMode,
    /// Centre (bell) or corner (shelf) frequency in Hz.
    pub freq_hz: f32,
    /// Quality factor of the bell and shelf (and of the bell's detector).
    pub q: f32,
    /// Resting gain in dB when the detector is idle.
    pub gain_db: f32,
    pub threshold_db: f32,
    /// 1 (no reaction) to infinity.
    pub ratio: f32,
    pub knee_db: f32,
    /// Largest dynamic gain change in dB; negative cuts, positive boosts.
    pub range_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    /// RMS window of the detector.
    pub rms_ms: f32,
}

impl Default for DynamicEqParams {
    fn default() -> Self {
        Self {
            kind: DynEqKind::Bell,
            mode: DynEqMode::Above,
            freq_hz: 1000.0,
            q: 1.0,
            gain_db: 0.0,
            threshold_db: -24.0,
            ratio: 4.0,
            knee_db: 6.0,
            range_db: -12.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            rms_ms: 5.0,
        }
    }
}

impl DynamicEqParams {
    fn sanitized(mut self, old: &Self) -> Self {
        let keep = |v: f32, o: f32| if v.is_finite() { v } else { o };
        self.freq_hz = keep(self.freq_hz, old.freq_hz);
        self.q = keep(self.q, old.q);
        self.gain_db = keep(self.gain_db, old.gain_db);
        self.threshold_db = keep(self.threshold_db, old.threshold_db);
        self.knee_db = keep(self.knee_db, old.knee_db).max(0.0);
        self.range_db = keep(self.range_db, old.range_db);
        self.attack_ms = keep(self.attack_ms, old.attack_ms).max(0.0);
        self.release_ms = keep(self.release_ms, old.release_ms).max(0.0);
        self.rms_ms = keep(self.rms_ms, old.rms_ms).max(0.01);
        // Infinity is a valid ratio; NaN keeps the old one.
        self.ratio = if self.ratio.is_nan() { old.ratio } else { self.ratio.max(1.0) };
        self
    }

    /// Dynamic part of the gain in dB (signed by `range_db`) for a steady
    /// detected level: the stateless static curve.
    pub fn static_delta_db(&self, level_db: f32) -> f32 {
        let excess = match self.mode {
            DynEqMode::Above => level_db - self.threshold_db,
            DynEqMode::Below => self.threshold_db - level_db,
        };
        let w = self.knee_db;
        let over = if 2.0 * excess < -w {
            0.0
        } else if w > 0.0 && 2.0 * excess.abs() <= w {
            (excess + w / 2.0) * (excess + w / 2.0) / (2.0 * w)
        } else {
            excess.max(0.0)
        };
        let slope = if self.ratio.is_infinite() { 1.0 } else { 1.0 - 1.0 / self.ratio };
        // A NaN range is no dynamic gain (`clamp` panics on a NaN bound).
        let range = if self.range_db.is_nan() { 0.0 } else { self.range_db };
        let mag = (slope * over).clamp(0.0, range.abs());
        mag.copysign(range)
    }

    /// Total gain in dB of the band for a steady detected level (the static
    /// band-gain-versus-level curve).
    pub fn static_gain_db(&self, level_db: f32) -> f32 {
        self.gain_db + self.static_delta_db(level_db)
    }
}

/// One dynamic EQ band for any number of channels. See the [module docs](self).
#[derive(Debug, Clone)]
pub struct DynamicEqBand {
    fs: f32,
    channels: usize,
    params: DynamicEqParams,
    audio: Vec<Svf>,
    det: Vec<Svf>,
    follower: EnvelopeFollower,
    smoother: Ballistics,
    engagement: f32,
    applied_db: f32,
    interval: usize,
    countdown: usize,
    dirty: bool,
}

impl DynamicEqBand {
    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let fs = effective_rate(sample_rate);
        let channels = channels.max(1);
        let mut follower = EnvelopeFollower::new(fs);
        follower.set_detection(Detection::Rms);
        let mut b = Self {
            fs,
            channels,
            params: DynamicEqParams::default(),
            audio: vec![Svf::new(); channels],
            det: vec![Svf::new(); channels],
            follower,
            smoother: Ballistics::new(fs),
            engagement: 0.0,
            applied_db: f32::NAN,
            interval: ((fs / 8000.0).round() as usize).clamp(1, 64),
            countdown: 0,
            dirty: true,
        };
        b.set_params(DynamicEqParams::default());
        b
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn params(&self) -> &DynamicEqParams {
        &self.params
    }

    /// Applies new settings. Non-finite fields keep their previous value.
    pub fn set_params(&mut self, p: DynamicEqParams) {
        let p = p.sanitized(&self.params);
        self.follower.set_rms_ms(p.rms_ms);
        self.smoother.set_attack_ms(p.attack_ms);
        self.smoother.set_release_ms(p.release_ms);
        self.params = p;
        self.dirty = true;
    }

    pub fn reset(&mut self) {
        for f in self.audio.iter_mut().chain(self.det.iter_mut()) {
            f.reset();
        }
        self.follower.reset();
        self.smoother.reset(0.0);
        self.engagement = 0.0;
        self.countdown = 0;
        self.dirty = true;
    }

    /// Current dynamic gain change in dB (signed), without the resting gain.
    pub fn dynamic_gain_db(&self) -> f32 {
        self.engagement.copysign(self.params.range_db)
    }

    /// Current total gain of the filter in dB.
    pub fn gain_db(&self) -> f32 {
        self.params.gain_db + self.dynamic_gain_db()
    }

    fn configure(&mut self) {
        let p = self.params;
        let total = (p.gain_db + self.engagement.copysign(p.range_db)).clamp(-60.0, 60.0);
        let (audio_mode, det_mode, det_q) = match p.kind {
            DynEqKind::Bell => (SvfMode::Bell, SvfMode::BandPass, p.q),
            DynEqKind::LowShelf => (SvfMode::LowShelf, SvfMode::LowPass, 0.707),
            DynEqKind::HighShelf => (SvfMode::HighShelf, SvfMode::HighPass, 0.707),
        };
        if self.dirty {
            for d in &mut self.det {
                d.configure(det_mode, self.fs, p.freq_hz, det_q, 0.0);
            }
        }
        if self.dirty || (total - self.applied_db).abs() >= 0.001 || self.applied_db.is_nan() {
            for a in &mut self.audio {
                a.configure(audio_mode, self.fs, p.freq_hz, p.q, total);
            }
            self.applied_db = total;
        }
        self.dirty = false;
    }

    /// Processes the channels in place; the detector listens to the band's own
    /// input. Only the first [`channels`](Self::channels) are processed.
    pub fn process(&mut self, channels: &mut [&mut [f32]]) {
        self.process_with_key(channels, None);
    }

    /// Processes in place with an external sidechain `key`. Channel `c` of the
    /// key stands in for channel `c` (a key with fewer channels repeats its
    /// last one); it is filtered to the band like the band's own signal.
    pub fn process_sidechain(&mut self, channels: &mut [&mut [f32]], key: &[&[f32]]) {
        if key.is_empty() {
            return;
        }
        self.process_with_key(channels, Some(key));
    }

    fn process_with_key(&mut self, channels: &mut [&mut [f32]], key: Option<&[&[f32]]>) {
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
            for c in 0..nc {
                let raw = match key {
                    Some(k) => k[c.min(k.len() - 1)][i],
                    None => channels[c][i],
                };
                det = det.max(self.det[c].process_sample(raw).abs());
            }
            let level = self.follower.detect(det) + SINE_PEAK_DB;
            let target = self.params.static_delta_db(level).abs();
            self.engagement = self.smoother.step(target);
            if self.countdown == 0 {
                self.configure();
                self.countdown = self.interval;
            }
            self.countdown -= 1;
            for c in 0..nc {
                channels[c][i] = self.audio[c].process_sample(channels[c][i]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nan_range_is_no_dynamic_gain() {
        let p = DynamicEqParams { range_db: f32::NAN, ..Default::default() };
        assert_eq!(p.static_delta_db(0.0), 0.0);
        assert_eq!(p.static_gain_db(0.0), p.gain_db);
    }
}
