//! Gain computer (compressor, expander, gate) and a linked, sidechain-capable
//! dynamics processor built on [`crate::envelope`].
//!
//! Source: D. Giannoulis, M. Massberg, J. D. Reiss, "Digital Dynamic Range
//! Compressor Design - A Tutorial and Analysis", J. Audio Eng. Soc. 60(6), 2012:
//! static characteristics with a quadratic soft knee (eqs. 4-5) and the
//! level-detector / gain-smoothing placements of section 2.3. The expander and
//! gate curves are the same construction mirrored below the threshold. The
//! hysteresis (a Schmitt trigger with a lower closing threshold) and the range
//! (a floor on the attenuation) are standard gate practice and are this
//! crate's own formulation.
//!
//! The older [`crate::Compressor`] stays as it was; this module is the
//! full-featured replacement.

use crate::envelope::{db_to_amp, Ballistics, Detection, Domain, EnvelopeFollower, Topology, FLOOR_DB};
use crate::rate::effective_rate;

/// What the gain computer does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicsMode {
    /// Downward compression above the threshold. A ratio of infinity limits.
    Compressor,
    /// Downward expansion below the threshold (output falls `ratio` dB per dB).
    Expander,
    /// An expander with an (almost) infinite ratio: attenuates down to
    /// `range_db` below the threshold.
    Gate,
}

/// Static gain curve in dB, after Giannoulis et al. (2012).
///
/// All levels are in dB. [`gain_db`](Self::gain_db) returns the gain change
/// (zero or negative, never below `-range_db`) for an input level and carries
/// the gate's hysteresis state; [`static_gain_db`](Self::static_gain_db) is the
/// stateless curve.
#[derive(Debug, Clone)]
pub struct GainComputer {
    pub mode: DynamicsMode,
    pub threshold_db: f32,
    /// 1 (no effect) to infinity (limiter / gate). Values below 1 count as 1.
    pub ratio: f32,
    /// Width of the quadratic soft knee centred on the threshold; 0 is a hard knee.
    pub knee_db: f32,
    /// Largest attenuation, positive dB.
    pub range_db: f32,
    /// Expander and gate: once open, they close only below `threshold - hysteresis`.
    pub hysteresis_db: f32,
    open: bool,
}

impl Default for GainComputer {
    fn default() -> Self {
        Self {
            mode: DynamicsMode::Compressor,
            threshold_db: -18.0,
            ratio: 4.0,
            knee_db: 6.0,
            range_db: 120.0,
            hysteresis_db: 0.0,
            open: false,
        }
    }
}

impl GainComputer {
    pub fn new(mode: DynamicsMode, threshold_db: f32, ratio: f32, knee_db: f32) -> Self {
        Self { mode, threshold_db, ratio, knee_db, ..Self::default() }
    }

    pub fn reset(&mut self) {
        self.open = false;
    }

    /// Whether the gate/expander is open (always `false` for a compressor).
    pub fn is_open(&self) -> bool {
        self.open
    }

    fn curve(&self, level_db: f32, threshold_db: f32) -> f32 {
        let x = if level_db.is_nan() { FLOOR_DB } else { level_db.clamp(FLOOR_DB, 400.0) };
        let w = self.knee_db.max(0.0);
        let r = if self.ratio.is_nan() { 1.0 } else { self.ratio.max(1.0) };
        let d = x - threshold_db;
        let g = match self.mode {
            DynamicsMode::Compressor => {
                let slope = 1.0 / r - 1.0; // -1 for r = infinity
                if 2.0 * d < -w {
                    0.0
                } else if w > 0.0 && 2.0 * d.abs() <= w {
                    slope * (d + w / 2.0) * (d + w / 2.0) / (2.0 * w)
                } else {
                    slope * d
                }
            }
            DynamicsMode::Expander | DynamicsMode::Gate => {
                let r = if self.mode == DynamicsMode::Gate || !r.is_finite() { 1.0e6 } else { r };
                if 2.0 * d > w {
                    0.0
                } else if w > 0.0 && 2.0 * d.abs() <= w {
                    (1.0 - r) * (d - w / 2.0) * (d - w / 2.0) / (2.0 * w)
                } else {
                    (r - 1.0) * d
                }
            }
        };
        g.clamp(-self.range_db.max(0.0), 0.0)
    }

    /// Stateless curve: gain change in dB for `level_db`, using the closed
    /// (no hysteresis) threshold.
    pub fn static_gain_db(&self, level_db: f32) -> f32 {
        self.curve(level_db, self.threshold_db)
    }

    /// Output level in dB for an input level in dB.
    pub fn static_output_db(&self, level_db: f32) -> f32 {
        level_db + self.static_gain_db(level_db)
    }

    /// Gain change in dB (zero or negative), with hysteresis for the expander
    /// and gate. A closed gate opens when the level reaches the threshold; an
    /// open one closes below `threshold - hysteresis`, and while open its
    /// curve uses that lower threshold.
    pub fn gain_db(&mut self, level_db: f32) -> f32 {
        match self.mode {
            DynamicsMode::Compressor => self.curve(level_db, self.threshold_db),
            _ => {
                let hyst = self.hysteresis_db.max(0.0);
                let x = if level_db.is_nan() { FLOOR_DB } else { level_db };
                if x >= self.threshold_db {
                    self.open = true;
                } else if x < self.threshold_db - hyst {
                    self.open = false;
                }
                let t = if self.open { self.threshold_db - hyst } else { self.threshold_db };
                self.curve(level_db, t)
            }
        }
    }
}

/// How the channels' detector signals combine into one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// The largest rectified channel (the usual choice for stereo and surround).
    Max,
    /// The mean of the rectified channels.
    Average,
}

/// Where the attack/release ballistics act.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Smoothing {
    /// On the detected level, before the gain computer (the feed-forward
    /// level-detector form; what [`crate::Compressor`] does).
    Level,
    /// On the gain computer's output, in dB (the paper's "smooth" design).
    /// Attack is the gain moving toward more attenuation for a compressor, and
    /// toward opening for an expander or gate. `domain` does not apply: the
    /// smoothing is always in dB.
    Gain,
}

/// Settings for [`Dynamics`].
#[derive(Debug, Clone, Copy)]
pub struct DynamicsParams {
    pub mode: DynamicsMode,
    pub threshold_db: f32,
    pub ratio: f32,
    pub knee_db: f32,
    pub range_db: f32,
    pub hysteresis_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub hold_ms: f32,
    pub makeup_db: f32,
    pub detection: Detection,
    /// RMS window (time constant) when `detection` is [`Detection::Rms`].
    pub rms_ms: f32,
    pub domain: Domain,
    pub topology: Topology,
    pub smoothing: Smoothing,
    pub link: Link,
}

impl Default for DynamicsParams {
    fn default() -> Self {
        Self {
            mode: DynamicsMode::Compressor,
            threshold_db: -18.0,
            ratio: 4.0,
            knee_db: 6.0,
            range_db: 120.0,
            hysteresis_db: 0.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            hold_ms: 0.0,
            makeup_db: 0.0,
            detection: Detection::Peak,
            rms_ms: 10.0,
            domain: Domain::Log,
            topology: Topology::Branching,
            smoothing: Smoothing::Level,
            link: Link::Max,
        }
    }
}

/// Compressor, expander or gate for any number of channels with one linked
/// detector: every channel receives the same gain.
///
/// No state is kept per channel, so any channel count works, and nothing
/// allocates. A sidechain filter plugs in as a closure, see
/// [`process_filtered`](Self::process_filtered).
#[derive(Debug, Clone)]
pub struct Dynamics {
    params: DynamicsParams,
    follower: EnvelopeFollower,
    gain_smoother: Ballistics,
    computer: GainComputer,
    gain_db: f32,
}

impl Dynamics {
    pub fn new(sample_rate: f32) -> Self {
        let fs = effective_rate(sample_rate);
        let mut d = Self {
            params: DynamicsParams::default(),
            follower: EnvelopeFollower::new(fs),
            gain_smoother: Ballistics::new(fs),
            computer: GainComputer::default(),
            gain_db: 0.0,
        };
        d.set_params(DynamicsParams::default());
        d
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.follower.set_sample_rate(sample_rate);
        self.gain_smoother.set_sample_rate(sample_rate);
    }

    pub fn params(&self) -> &DynamicsParams {
        &self.params
    }


    /// Apply settings. Changing `domain`, `smoothing` or `mode` resets the state.
    pub fn set_params(&mut self, p: DynamicsParams) {
        let old = self.params;
        self.params = p;
        self.computer.mode = p.mode;
        self.computer.threshold_db = p.threshold_db;
        self.computer.ratio = p.ratio;
        self.computer.knee_db = p.knee_db;
        self.computer.range_db = p.range_db;
        self.computer.hysteresis_db = p.hysteresis_db;
        self.follower.set_detection(p.detection);
        self.follower.set_rms_ms(p.rms_ms);
        self.follower.set_topology(p.topology);
        self.gain_smoother.set_topology(p.topology);
        if p.domain != old.domain {
            self.follower.set_domain(p.domain);
        }
        // With level smoothing the ballistics sit in the follower; with gain
        // smoothing the follower only detects and a second smoother acts on
        // the gain. Both get the same times so either placement is valid.
        self.follower.set_attack_ms(p.attack_ms);
        self.follower.set_release_ms(p.release_ms);
        self.follower.set_hold_ms(p.hold_ms);
        self.gain_smoother.set_attack_ms(p.attack_ms);
        self.gain_smoother.set_release_ms(p.release_ms);
        self.gain_smoother.set_hold_ms(p.hold_ms);
        if p.domain != old.domain || p.smoothing != old.smoothing || p.mode != old.mode {
            self.reset();
        }
    }

    pub fn reset(&mut self) {
        self.follower.reset();
        self.gain_smoother.reset(0.0);
        self.computer.reset();
        self.gain_db = 0.0;
    }

    /// Gain of the last frame in dB (without make-up). Zero or negative.
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    /// Gain reduction of the last frame in dB (positive).
    pub fn gain_reduction_db(&self) -> f32 {
        -self.gain_db
    }

    /// The gain computer, for drawing the static curve.
    pub fn computer(&self) -> &GainComputer {
        &self.computer
    }

    /// Advance one frame given the linked detector sample (a rectified,
    /// already-filtered amplitude, the result of [`Link`] over the channels);
    /// returns the linear gain with make-up included. This is the core of the
    /// `process*` methods, for callers that own the channel loop.
    #[inline]
    pub fn next_gain(&mut self, detector: f32) -> f32 {
        let is_comp = self.params.mode == DynamicsMode::Compressor;
        let g = match self.params.smoothing {
            Smoothing::Level => {
                let level = self.follower.process(detector);
                self.computer.gain_db(level)
            }
            Smoothing::Gain => {
                let level = self.follower.detect(detector);
                let g = self.computer.gain_db(level);
                // Feed "more attenuation" (compressor) or "more open" (gate) upward.
                let s = self.gain_smoother.step(if is_comp { -g } else { g });
                if is_comp {
                    -s
                } else {
                    s
                }
            }
        };
        let g = if g.is_finite() { g.min(0.0) } else { 0.0 };
        self.gain_db = g;
        db_to_amp(g + self.params.makeup_db)
    }

    /// Process channels in place; the detector listens to the channels
    /// themselves. Frames beyond the shortest channel are left untouched.
    pub fn process(&mut self, channels: &mut [&mut [f32]]) {
        self.process_filtered(channels, None, |_, x| x);
    }

    /// Process in place with an external sidechain `key`: channel `c` of the
    /// key stands in for channel `c` in the detector (a key with fewer
    /// channels repeats its last one).
    pub fn process_sidechain(&mut self, channels: &mut [&mut [f32]], key: &[&[f32]]) {
        self.process_filtered(channels, Some(key), |_, x| x);
    }

    /// The general form, with the sidechain filter hook. `filter(channel,
    /// sample)` is called once per detector sample, before rectification and
    /// linking, and returns the filtered sample. Keep one filter state per
    /// channel and index it by `channel`:
    ///
    /// ```ignore
    /// dynamics.process_filtered(&mut chans, None, |ch, x| high_pass[ch].process(x));
    /// ```
    ///
    /// Any `f32 -> f32` filter fits (a biquad, state-variable filter or
    /// crossover band of this crate, or your own). Only the detector hears the
    /// filtered signal; the audio is untouched. For a de-esser or dynamic EQ,
    /// band-pass the key here. If the filter delays the signal, delay the audio
    /// to match or accept the offset.
    pub fn process_filtered<F: FnMut(usize, f32) -> f32>(
        &mut self,
        channels: &mut [&mut [f32]],
        key: Option<&[&[f32]]>,
        mut filter: F,
    ) {
        let n_ch = channels.len();
        if n_ch == 0 {
            return;
        }
        let mut frames = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        if let Some(k) = key {
            if k.is_empty() {
                return;
            }
            frames = frames.min(k.iter().map(|c| c.len()).min().unwrap_or(0));
        }
        for i in 0..frames {
            let mut max = 0.0f32;
            let mut sum = 0.0f32;
            for c in 0..n_ch {
                let raw = match key {
                    Some(k) => k[c.min(k.len() - 1)][i],
                    None => channels[c][i],
                };
                let f = filter(c, raw);
                let a = if f.is_finite() { f.abs() } else { 0.0 };
                max = max.max(a);
                sum += a;
            }
            let detector = match self.params.link {
                Link::Max => max,
                Link::Average => sum / n_ch as f32,
            };
            let gain = self.next_gain(detector);
            for ch in channels.iter_mut() {
                ch[i] *= gain;
            }
        }
    }
}
