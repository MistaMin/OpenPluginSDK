//! Transient shaper: attack and sustain control from a differential envelope.
//!
//! Method. Two envelope followers with different ballistics watch the same
//! rectified signal (peak envelope detection with separate attack and release
//! times, Zoelzer, "DAFX: Digital Audio Effects", 2nd ed., Wiley 2011,
//! section 4.2, "Dynamic range control"). A *fast* follower tracks the signal
//! closely and a *slow* one lags it. Their difference in dB,
//! `d = fast_dB - slow_dB`, is positive while the level is rising (an onset:
//! the fast follower is ahead), near zero in steady state, and negative while
//! the level falls (the decay or sustain tail: the fast follower has already
//! dropped). That level-independent difference signal drives a gain, the
//! differential-envelope technique used by hardware and software transient
//! designers. The mapping below is this crate's own formulation; no
//! implementation was consulted.
//!
//! The gain, with `attack` and `sustain` in -1..=1:
//!
//! - onsets (`d > 0`): `g = attack * d`
//! - tails  (`d < 0`): `g = -sustain * d`   (so `+1` lifts the tail by `|d|`)
//!
//! with `attack` and `sustain` in -1..=1. `attack = -1` removes the fast
//! follower's lead over the slow one, `+1` doubles it (in dB). The gain is
//! clamped to `+-max_gain_db`, weighted to zero below `floor_db` of the slow
//! envelope so noise floors are not "shaped", and smoothed by a short one-pole
//! (`smooth_ms`) to avoid zipper noise.
//!
//! Detection is linked over channels (largest rectified sample) and one gain
//! applies to all channels, so the stereo image does not move.
//!
//! Choose `fast_release_ms` above a few periods of the lowest frequency you
//! shape: the fast follower must not ripple at the waveform rate, or the gain
//! modulates at audio rate and distorts.
//!
//! No allocation, no latency, non-finite input counts as `0.0`, denormal-safe
//! (the followers flush), NaN or infinite settings are ignored.

#![allow(clippy::needless_range_loop)]

use crate::envelope::{db_to_amp, Ballistics, Detection, Domain, EnvelopeFollower, Topology};
use crate::rate::effective_rate;

/// Transient shaper. See the [module docs](self).
#[derive(Debug, Clone)]
pub struct TransientShaper {
    fast: EnvelopeFollower,
    slow: EnvelopeFollower,
    smoother: Ballistics,
    attack: f32,
    sustain: f32,
    max_gain_db: f32,
    floor_db: f32,
    gain_db: f32,
}

impl TransientShaper {
    /// Defaults: fast 1 ms / 20 ms, slow 25 ms / 150 ms, limit 12 dB, floor
    /// -70 dB, smoothing 0.3 ms, attack and sustain at 0 (transparent).
    pub fn new(sample_rate: f32) -> Self {
        let fs = effective_rate(sample_rate);
        let mk = |a: f32, r: f32| {
            let mut e = EnvelopeFollower::new(fs);
            e.set_detection(Detection::Peak);
            e.set_topology(Topology::Decoupled);
            e.set_domain(Domain::Linear);
            e.set_attack_ms(a);
            e.set_release_ms(r);
            e
        };
        let mut smoother = Ballistics::new(fs);
        smoother.set_attack_ms(0.3);
        smoother.set_release_ms(0.3);
        Self {
            fast: mk(1.0, 20.0),
            slow: mk(25.0, 150.0),
            smoother,
            attack: 0.0,
            sustain: 0.0,
            max_gain_db: 12.0,
            floor_db: -70.0,
            gain_db: 0.0,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.fast.set_sample_rate(sample_rate);
        self.slow.set_sample_rate(sample_rate);
        self.smoother.set_sample_rate(sample_rate);
    }

    pub fn reset(&mut self) {
        self.fast.reset();
        self.slow.reset();
        self.smoother.reset(0.0);
        self.gain_db = 0.0;
    }

    /// Attack amount, -1 (soften onsets) to +1 (emphasise them).
    pub fn set_attack(&mut self, amount: f32) {
        if amount.is_finite() {
            self.attack = amount.clamp(-1.0, 1.0);
        }
    }

    /// Sustain amount, -1 (shorten the tail) to +1 (lift it).
    pub fn set_sustain(&mut self, amount: f32) {
        if amount.is_finite() {
            self.sustain = amount.clamp(-1.0, 1.0);
        }
    }

    /// Largest gain change in either direction, dB (0..=24).
    pub fn set_max_gain_db(&mut self, db: f32) {
        if db.is_finite() {
            self.max_gain_db = db.clamp(0.0, 24.0);
        }
    }

    /// Level of the slow envelope below which shaping fades out, dB.
    pub fn set_floor_db(&mut self, db: f32) {
        if db.is_finite() {
            self.floor_db = db.clamp(-120.0, 0.0);
        }
    }

    pub fn set_fast_ms(&mut self, attack_ms: f32, release_ms: f32) {
        if attack_ms.is_finite() && release_ms.is_finite() {
            self.fast.set_attack_ms(attack_ms.max(0.0));
            self.fast.set_release_ms(release_ms.max(0.0));
        }
    }

    pub fn set_slow_ms(&mut self, attack_ms: f32, release_ms: f32) {
        if attack_ms.is_finite() && release_ms.is_finite() {
            self.slow.set_attack_ms(attack_ms.max(0.0));
            self.slow.set_release_ms(release_ms.max(0.0));
        }
    }

    /// Time constant of the gain smoother in ms.
    pub fn set_smooth_ms(&mut self, ms: f32) {
        if ms.is_finite() {
            self.smoother.set_attack_ms(ms.max(0.0));
            self.smoother.set_release_ms(ms.max(0.0));
        }
    }

    /// Gain applied to the last frame, dB.
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    /// Advances one frame given the linked rectified detector sample and
    /// returns the linear gain.
    #[inline]
    pub fn next_gain(&mut self, detector: f32) -> f32 {
        let f = self.fast.process(detector);
        let s = self.slow.process(detector);
        let d = f - s;
        let w = ((s - self.floor_db) / 6.0).clamp(0.0, 1.0);
        let raw = if d > 0.0 { self.attack * d } else { -self.sustain * d };
        let target = (raw * w).clamp(-self.max_gain_db, self.max_gain_db);
        let g = self.smoother.step(target);
        self.gain_db = if g.is_finite() { g } else { 0.0 };
        db_to_amp(self.gain_db)
    }

    /// Processes any number of channels in place with one linked gain.
    pub fn process(&mut self, channels: &mut [&mut [f32]]) {
        let nc = channels.len();
        if nc == 0 {
            return;
        }
        let frames = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        for i in 0..frames {
            let mut det = 0.0f32;
            for ch in channels.iter() {
                let v = ch[i];
                if v.is_finite() {
                    det = det.max(v.abs());
                }
            }
            let g = self.next_gain(det);
            for ch in channels.iter_mut() {
                let v = ch[i];
                ch[i] = if v.is_finite() { v * g } else { 0.0 };
            }
        }
    }
}
