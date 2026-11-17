//! Envelope followers: peak and RMS level detection with attack, release and
//! hold.
//!
//! Source: D. Giannoulis, M. Massberg, J. D. Reiss, "Digital Dynamic Range
//! Compressor Design - A Tutorial and Analysis", J. Audio Eng. Soc. 60(6), 2012.
//! Two ballistics topologies from section 2.2 are implemented:
//!
//! - **Branching** (the classic analogue-style detector):
//!   `y[n] = a_A y[n-1] + (1 - a_A) x[n]` while `x[n] >= y[n-1]`, otherwise
//!   `y[n] = a_R y[n-1] + (1 - a_R) x[n]`.
//! - **Decoupled**: a peak-hold release stage
//!   `y1[n] = max(x[n], a_R y1[n-1] + (1 - a_R) x[n])` followed by an attack
//!   smoother `y[n] = a_A y[n-1] + (1 - a_A) y1[n]`. Attack and release no
//!   longer interact, and the output never overshoots the input.
//!
//! The coefficient `a = exp(-1 / (tau * fs))` makes `tau` a true time constant:
//! after `tau` seconds a step has covered `1 - 1/e` (63.2 %) of its way. The
//! smoothing runs in the linear domain ([`Domain::Linear`]) or on the level in
//! decibels ([`Domain::Log`]), the latter being the paper's recommended
//! log-domain detector. An optional hold keeps the envelope at its last peak
//! for a fixed time before the release starts.
//!
//! Everything is allocation-free. State is `f64` so that long time constants at
//! high sample rates keep their precision; values near zero are flushed so no
//! subnormal numbers circulate; non-finite inputs are ignored.

use crate::rate::effective_rate;

/// Lowest level the detectors report, in dB (also the "silence" value).
pub const FLOOR_DB: f32 = -200.0;
const FLOOR_LIN: f64 = 1.0e-10; // -200 dB

/// Convert a linear amplitude to dB, floored at [`FLOOR_DB`]. NaN reads as the floor.
#[inline]
pub fn amp_to_db(x: f32) -> f32 {
    let x = x as f64;
    if x.is_finite() && x > FLOOR_LIN {
        (20.0 * x.log10()) as f32
    } else {
        FLOOR_DB
    }
}

/// Convert dB to a linear amplitude.
#[inline]
pub fn db_to_amp(db: f32) -> f32 {
    if db.is_nan() {
        return 1.0;
    }
    10.0f64.powf(db.clamp(-400.0, 400.0) as f64 / 20.0) as f32
}

#[inline]
fn flush(x: f64) -> f64 {
    if x.abs() < 1.0e-30 {
        0.0
    } else {
        x
    }
}

/// Smoothing topology, see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Topology {
    Branching,
    Decoupled,
}

/// Domain in which the level is smoothed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Domain {
    Linear,
    Log,
}

/// What is measured before the ballistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Detection {
    /// Rectified sample, `|x|`.
    Peak,
    /// Square root of a one-pole mean square (window time set by `rms_ms`).
    Rms,
}

fn coefficient(ms: f32, fs: f64) -> f64 {
    let n = ms as f64 * 0.001 * fs;
    if n.is_finite() && n > 0.0 {
        (-1.0 / n).exp()
    } else {
        0.0
    }
}

/// Attack / release / hold smoother for any scalar (a level, or a gain
/// reduction in dB). "Attack" is used while the input is at or above the
/// output, "release" while it is below.
#[derive(Debug, Clone)]
pub struct Ballistics {
    fs: f64,
    attack_ms: f32,
    release_ms: f32,
    hold_ms: f32,
    a_att: f64,
    a_rel: f64,
    hold_n: u32,
    hold_left: u32,
    topology: Topology,
    y: f64,
    y1: f64,
}

impl Ballistics {
    /// Create at `sample_rate` (clamped to the SDK range). Defaults: 10 ms attack,
    /// 100 ms release, no hold, branching, state 0.
    pub fn new(sample_rate: f32) -> Self {
        let mut b = Self {
            fs: effective_rate(sample_rate) as f64,
            attack_ms: 10.0,
            release_ms: 100.0,
            hold_ms: 0.0,
            a_att: 0.0,
            a_rel: 0.0,
            hold_n: 0,
            hold_left: 0,
            topology: Topology::Branching,
            y: 0.0,
            y1: 0.0,
        };
        b.update();
        b
    }

    fn update(&mut self) {
        self.a_att = coefficient(self.attack_ms, self.fs);
        self.a_rel = coefficient(self.release_ms, self.fs);
        let h = self.hold_ms as f64 * 0.001 * self.fs;
        self.hold_n = if h.is_finite() && h > 0.0 { h.min(u32::MAX as f64) as u32 } else { 0 };
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.fs = effective_rate(sample_rate) as f64;
        self.update();
    }

    /// Attack time constant in milliseconds (0 or negative: instantaneous).
    pub fn set_attack_ms(&mut self, ms: f32) {
        self.attack_ms = ms;
        self.update();
    }

    /// Release time constant in milliseconds (0 or negative: instantaneous).
    pub fn set_release_ms(&mut self, ms: f32) {
        self.release_ms = ms;
        self.update();
    }

    /// Hold time in milliseconds: after the input last reached the output, the
    /// output stays put for this long before the release begins.
    pub fn set_hold_ms(&mut self, ms: f32) {
        self.hold_ms = ms;
        self.update();
    }

    pub fn set_topology(&mut self, t: Topology) {
        self.topology = t;
    }

    /// Set the state (and the hold counter) to `value`.
    pub fn reset(&mut self, value: f32) {
        let v = if value.is_finite() { value as f64 } else { 0.0 };
        self.y = v;
        self.y1 = v;
        self.hold_left = 0;
    }

    pub fn value(&self) -> f32 {
        self.y as f32
    }

    /// Advance one sample.
    #[inline]
    pub fn step(&mut self, x: f32) -> f32 {
        let x = if x.is_finite() { x as f64 } else { self.y };
        match self.topology {
            Topology::Branching => {
                if x >= self.y {
                    self.hold_left = self.hold_n;
                    self.y = self.a_att * self.y + (1.0 - self.a_att) * x;
                } else if self.hold_left > 0 {
                    self.hold_left -= 1;
                } else {
                    self.y = self.a_rel * self.y + (1.0 - self.a_rel) * x;
                }
            }
            Topology::Decoupled => {
                if x >= self.y1 {
                    self.hold_left = self.hold_n;
                    self.y1 = x;
                } else if self.hold_left > 0 {
                    self.hold_left -= 1;
                } else {
                    self.y1 = self.a_rel * self.y1 + (1.0 - self.a_rel) * x;
                }
                self.y = self.a_att * self.y + (1.0 - self.a_att) * self.y1;
            }
        }
        self.y = flush(self.y);
        self.y1 = flush(self.y1);
        self.y as f32
    }
}

/// Peak or RMS level detector with ballistics. Input is audio (or a
/// pre-filtered sidechain signal); output is the level in dB.
#[derive(Debug, Clone)]
pub struct EnvelopeFollower {
    detection: Detection,
    domain: Domain,
    ballistics: Ballistics,
    fs: f64,
    rms_ms: f32,
    rms_coef: f64,
    mean_square: f64,
}

impl EnvelopeFollower {
    /// Peak detector, log domain, branching, 10 ms attack, 100 ms release.
    pub fn new(sample_rate: f32) -> Self {
        let fs = effective_rate(sample_rate) as f64;
        let mut e = Self {
            detection: Detection::Peak,
            domain: Domain::Log,
            ballistics: Ballistics::new(sample_rate),
            fs,
            rms_ms: 10.0,
            rms_coef: 0.0,
            mean_square: 0.0,
        };
        e.rms_coef = coefficient(e.rms_ms, fs);
        e.reset();
        e
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.fs = effective_rate(sample_rate) as f64;
        self.rms_coef = coefficient(self.rms_ms, self.fs);
        self.ballistics.set_sample_rate(sample_rate);
    }

    pub fn set_detection(&mut self, d: Detection) {
        self.detection = d;
    }

    /// Change the smoothing domain. Resets the state.
    pub fn set_domain(&mut self, d: Domain) {
        self.domain = d;
        self.reset();
    }

    pub fn set_topology(&mut self, t: Topology) {
        self.ballistics.set_topology(t);
    }

    pub fn set_attack_ms(&mut self, ms: f32) {
        self.ballistics.set_attack_ms(ms);
    }

    pub fn set_release_ms(&mut self, ms: f32) {
        self.ballistics.set_release_ms(ms);
    }

    pub fn set_hold_ms(&mut self, ms: f32) {
        self.ballistics.set_hold_ms(ms);
    }

    /// Time constant of the mean-square window used by [`Detection::Rms`].
    pub fn set_rms_ms(&mut self, ms: f32) {
        self.rms_ms = ms;
        self.rms_coef = coefficient(ms, self.fs);
    }

    /// Back to silence.
    pub fn reset(&mut self) {
        self.mean_square = 0.0;
        self.ballistics.reset(match self.domain {
            Domain::Linear => 0.0,
            Domain::Log => FLOOR_DB,
        });
    }

    /// Linear instantaneous level (rectified, or RMS window) of one sample.
    #[inline]
    fn level(&mut self, s: f32) -> f64 {
        let x = if s.is_finite() { (s as f64).abs() } else { 0.0 };
        match self.detection {
            Detection::Peak => x,
            Detection::Rms => {
                self.mean_square = flush(self.rms_coef * self.mean_square + (1.0 - self.rms_coef) * x * x);
                self.mean_square.sqrt()
            }
        }
    }

    /// Level in dB before the ballistics (the RMS window still applies). Use it
    /// when the ballistics run elsewhere, for instance on the gain reduction.
    #[inline]
    pub fn detect(&mut self, sample: f32) -> f32 {
        amp_to_db(self.level(sample) as f32)
    }

    /// Advance one sample; returns the smoothed level in dB.
    #[inline]
    pub fn process(&mut self, sample: f32) -> f32 {
        let x = self.level(sample);
        match self.domain {
            Domain::Log => {
                let db = if x > FLOOR_LIN { 20.0 * x.log10() } else { FLOOR_DB as f64 };
                self.ballistics.step(db as f32).max(FLOOR_DB)
            }
            Domain::Linear => amp_to_db(self.ballistics.step(x as f32)),
        }
    }

    /// The current smoothed level as a linear amplitude.
    pub fn value_linear(&self) -> f32 {
        match self.domain {
            Domain::Log => db_to_amp(self.ballistics.value()),
            Domain::Linear => self.ballistics.value(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    /// Samples until `value` first reaches `target` (rising or falling).
    fn time_to<F: FnMut() -> f32>(mut f: F, target: f32, rising: bool, max: usize) -> usize {
        for n in 1..=max {
            let v = f();
            if (rising && v >= target) || (!rising && v <= target) {
                return n;
            }
        }
        max
    }

    #[test]
    fn linear_branching_time_constants() {
        for &(att, rel) in &[(5.0f32, 50.0f32), (1.0, 200.0), (20.0, 20.0)] {
            let mut e = Ballistics::new(FS);
            e.set_attack_ms(att);
            e.set_release_ms(rel);
            // Attack: 0 -> 1 reaches 1 - 1/e after tau.
            let n = time_to(|| e.step(1.0), 1.0 - (-1.0f32).exp(), true, 100_000);
            let want = att * 0.001 * FS;
            assert!((n as f32 - want).abs() <= want * 0.02 + 1.0, "attack {n} vs {want}");
            // Release: settle at 1, then 1 -> 0 falls to 1/e after tau.
            for _ in 0..200_000 {
                e.step(1.0);
            }
            let n = time_to(|| e.step(0.0), (-1.0f32).exp(), false, 1_000_000);
            let want = rel * 0.001 * FS;
            assert!((n as f32 - want).abs() <= want * 0.02 + 1.0, "release {n} vs {want}");
        }
    }

    #[test]
    fn log_domain_time_constants_in_db() {
        let mut e = EnvelopeFollower::new(FS);
        e.set_attack_ms(10.0);
        e.set_release_ms(80.0);
        // Step from silence (-200 dB) to 0 dB: 63.2 % of the way is -73.6 dB.
        let target = FLOOR_DB * (-1.0f32).exp();
        let n = time_to(|| e.process(1.0), target, true, 100_000);
        let want = 0.010 * FS;
        assert!((n as f32 - want).abs() <= want * 0.02 + 1.0, "attack {n}");
        for _ in 0..100_000 {
            e.process(1.0);
        }
        // Back to silence: from 0 dB, 63.2 % of the way down is -126.4 dB.
        let target = FLOOR_DB * (1.0 - (-1.0f32).exp());
        let n = time_to(|| e.process(0.0), target, false, 1_000_000);
        let want = 0.080 * FS;
        assert!((n as f32 - want).abs() <= want * 0.02 + 1.0, "release {n} vs {want}");
    }

    #[test]
    fn works_at_the_real_rate() {
        for &fs in &[1_000.0f32, 8_000.0, 44_100.0, 192_000.0, 768_000.0] {
            let mut e = Ballistics::new(fs);
            e.set_attack_ms(20.0);
            let n = time_to(|| e.step(1.0), 1.0 - (-1.0f32).exp(), true, 10_000_000);
            let want = 0.020 * fs;
            assert!((n as f32 - want).abs() <= want * 0.03 + 1.0, "{fs}: {n} vs {want}");
        }
    }

    #[test]
    fn decoupled_never_overshoots_and_branching_differs() {
        let mut d = Ballistics::new(FS);
        d.set_topology(Topology::Decoupled);
        d.set_attack_ms(5.0);
        d.set_release_ms(50.0);
        // A short burst: output stays at or below the input peak.
        let mut peak = 0.0f32;
        for n in 0..4800 {
            let x = if n < 100 { 1.0 } else { 0.0 };
            peak = peak.max(d.step(x));
        }
        assert!(peak <= 1.0 + 1e-6);
        // The decoupled attack follows the release-stage output: after the burst
        // ends the output keeps rising (the held peak is still being fed).
        let mut d2 = Ballistics::new(FS);
        d2.set_topology(Topology::Decoupled);
        d2.set_attack_ms(5.0);
        d2.set_release_ms(50.0);
        let mut y = 0.0;
        for _ in 0..50 {
            y = d2.step(1.0);
        }
        let mut after = 0.0;
        for _ in 0..50 {
            after = d2.step(0.0);
        }
        assert!(after > y, "attack stage keeps charging from the held peak");
    }

    #[test]
    fn hold_delays_the_release() {
        let mut e = Ballistics::new(FS);
        e.set_attack_ms(0.0);
        e.set_release_ms(10.0);
        e.set_hold_ms(20.0);
        e.step(1.0);
        let hold = (0.020 * FS) as usize;
        for _ in 0..hold {
            assert_eq!(e.step(0.0), 1.0);
        }
        assert!(e.step(0.0) < 1.0);
        // Decoupled holds the same way.
        let mut d = Ballistics::new(FS);
        d.set_topology(Topology::Decoupled);
        d.set_attack_ms(0.0);
        d.set_release_ms(10.0);
        d.set_hold_ms(20.0);
        d.step(1.0);
        for _ in 0..hold {
            assert_eq!(d.step(0.0), 1.0);
        }
        assert!(d.step(0.0) < 1.0);
    }

    #[test]
    fn rms_of_a_sine_is_amplitude_over_root_two() {
        let mut e = EnvelopeFollower::new(FS);
        e.set_detection(Detection::Rms);
        e.set_domain(Domain::Linear);
        e.set_rms_ms(20.0);
        e.set_attack_ms(0.0);
        e.set_release_ms(0.0);
        let mut v = 0.0;
        for n in 0..48_000 {
            v = e.process(0.5 * (std::f32::consts::TAU * 1000.0 * n as f32 / FS).sin());
        }
        let want = 20.0 * (0.5f32 / 2.0f32.sqrt()).log10();
        assert!((v - want).abs() < 0.3, "{v} vs {want}");
    }

    #[test]
    fn peak_of_a_sine_reaches_amplitude() {
        let mut e = EnvelopeFollower::new(FS);
        e.set_attack_ms(0.0);
        let mut best = FLOOR_DB;
        for n in 0..4800 {
            best = best.max(e.process(0.5 * (std::f32::consts::TAU * 1000.0 * n as f32 / FS).sin()));
        }
        assert!((best - 20.0 * 0.5f32.log10()).abs() < 0.1, "{best}");
    }

    #[test]
    fn robust_to_nan_inf_and_decays_without_subnormals() {
        let mut e = EnvelopeFollower::new(FS);
        for &x in &[f32::NAN, f32::INFINITY, -f32::INFINITY, 1e30, 0.0] {
            let v = e.process(x);
            assert!(v.is_finite());
        }
        let mut l = EnvelopeFollower::new(FS);
        l.set_domain(Domain::Linear);
        l.set_release_ms(1.0);
        l.process(1.0);
        for _ in 0..200_000 {
            l.process(0.0);
        }
        let v = l.value_linear();
        assert!(v == 0.0 || v.is_normal(), "{v}");
    }
}
