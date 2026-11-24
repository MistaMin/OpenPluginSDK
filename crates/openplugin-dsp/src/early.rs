//! Early reflections: a tapped delay line with panned taps.
//!
//! The first echoes of a room reach the listener as discrete, sparse arrivals
//! before the tail turns diffuse; they carry most of the cue for room size
//! and distance (see Moorer, "About This Reverberation Business", Computer
//! Music Journal 3(2), 1979, who builds early reflections as a tapped delay
//! line ahead of the recirculating part). This module is that tapped line.
//!
//! A mono sum of the input feeds a ring; each [`EarlyTap`] reads it at its
//! own delay, scales it, and places it with a constant-power pan law. The
//! built-in [`EarlyPattern`] tables are hand-written tap patterns (times grow
//! roughly irregularly, gains fall with time, pans alternate sides); they are
//! not measurements of any real room. [`EarlyReflections::set_taps`] takes
//! any pattern of your own.
//!
//! Tap delays are rounded to whole samples. Nothing here allocates after
//! [`EarlyReflections::new`].

use crate::rate::effective_rate;
use crate::reverb_common::{Ring, clean, flush};

/// One early reflection.
/// # Example
///
/// ```
/// use openplugin_dsp::{EarlyReflections, EarlyTap};
///
/// let mut er = EarlyReflections::new(48_000.0);
/// er.set_taps(&[
///     EarlyTap { time_ms: 7.0, gain: 0.8, pan: -0.5 },
///     EarlyTap { time_ms: 11.0, gain: 0.6, pan: 0.5 },
/// ]);
/// assert!(er.longest_tap_samples() >= 528);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EarlyTap {
    /// Delay in milliseconds at size 1.
    pub time_ms: f32,
    /// Linear gain.
    pub gain: f32,
    /// Position, `-1.0` hard left to `1.0` hard right.
    pub pan: f32,
}

/// Built-in tap patterns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EarlyPattern {
    /// Eight taps within 40 ms.
    SmallRoom,
    /// Ten taps within 75 ms.
    LargeRoom,
    /// Ten taps spread over 150 ms.
    Hall,
    /// Twelve closely packed taps within 45 ms.
    Dense,
    /// Three well separated echoes out to 180 ms.
    Slap,
}

const fn t(time_ms: f32, gain: f32, pan: f32) -> EarlyTap {
    EarlyTap { time_ms, gain, pan }
}

const SMALL_ROOM: [EarlyTap; 8] = [
    t(5.3, 0.82, -0.6),
    t(8.9, 0.74, 0.5),
    t(12.4, 0.66, -0.2),
    t(16.8, 0.58, 0.8),
    t(21.1, 0.50, -0.9),
    t(26.3, 0.42, 0.3),
    t(31.7, 0.35, -0.4),
    t(37.9, 0.28, 0.7),
];
const LARGE_ROOM: [EarlyTap; 10] = [
    t(9.7, 0.80, 0.4),
    t(14.3, 0.72, -0.7),
    t(19.9, 0.66, 0.1),
    t(25.1, 0.60, -0.3),
    t(32.6, 0.52, 0.9),
    t(40.2, 0.46, -0.8),
    t(48.7, 0.40, 0.5),
    t(57.4, 0.33, -0.1),
    t(65.8, 0.27, 0.6),
    t(73.9, 0.22, -0.5),
];
const HALL: [EarlyTap; 10] = [
    t(14.1, 0.78, -0.5),
    t(23.7, 0.70, 0.6),
    t(34.2, 0.64, -0.8),
    t(46.5, 0.58, 0.2),
    t(61.3, 0.52, -0.2),
    t(77.8, 0.46, 0.9),
    t(94.6, 0.40, -0.6),
    t(112.0, 0.34, 0.4),
    t(130.7, 0.28, -0.9),
    t(148.9, 0.22, 0.7),
];
const DENSE: [EarlyTap; 12] = [
    t(3.1, 0.70, 0.3),
    t(5.7, 0.68, -0.4),
    t(8.3, 0.64, 0.7),
    t(11.0, 0.62, -0.8),
    t(14.2, 0.58, 0.1),
    t(17.9, 0.54, -0.2),
    t(21.6, 0.50, 0.9),
    t(25.8, 0.46, -0.6),
    t(30.3, 0.41, 0.5),
    t(34.9, 0.36, -0.9),
    t(39.6, 0.31, 0.2),
    t(44.4, 0.26, -0.3),
];
const SLAP: [EarlyTap; 3] = [t(61.0, 0.70, -0.6), t(121.0, 0.42, 0.6), t(179.0, 0.22, 0.0)];

impl EarlyPattern {
    /// The tap table of this pattern.
    pub fn taps(self) -> &'static [EarlyTap] {
        match self {
            Self::SmallRoom => &SMALL_ROOM,
            Self::LargeRoom => &LARGE_ROOM,
            Self::Hall => &HALL,
            Self::Dense => &DENSE,
            Self::Slap => &SLAP,
        }
    }
}

/// Most taps a custom pattern may hold.
pub const MAX_EARLY_TAPS: usize = 32;
/// Longest tap delay, in milliseconds, after size scaling (taps beyond are clamped).
pub const MAX_EARLY_MS: f32 = 500.0;
/// Size (time scale) range.
pub const EARLY_SIZE_RANGE: (f32, f32) = (0.25, 4.0);

#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    delay: usize,
    gl: f32,
    gr: f32,
}

/// Tapped-delay early reflections; stereo out from a mono sum of the input.
/// `new` allocates the delay ring; processing and the setters do not.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{EarlyPattern, EarlyReflections};
///
/// let mut er = EarlyReflections::new(48_000.0);
/// er.set_pattern(EarlyPattern::SmallRoom);
/// er.set_size(1.5);
/// let mut left = vec![0.0f32; 4_800];
/// let mut right = vec![0.0f32; 4_800];
/// left[0] = 1.0;
/// right[0] = 1.0;
/// er.process(&mut left, &mut right);
/// assert!(left[1..].iter().any(|s| s.abs() > 0.0)); // the reflections arrive later
/// ```
#[derive(Clone, Debug)]
pub struct EarlyReflections {
    sample_rate: f32,
    ring: Ring,
    max_delay: usize,
    taps: [EarlyTap; MAX_EARLY_TAPS],
    count: usize,
    slots: [Slot; MAX_EARLY_TAPS],
    size: f32,
    level: f32,
}

impl EarlyReflections {
    /// Allocates the delay ring for `sample_rate`. Level starts at 1.0, size at 1.0,
    /// pattern [`EarlyPattern::SmallRoom`].
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = effective_rate(sample_rate);
        let max_delay = (MAX_EARLY_MS * 0.001 * sample_rate) as usize + 1;
        let mut e = Self {
            sample_rate,
            ring: Ring::new(max_delay + 4),
            max_delay,
            taps: [t(0.0, 0.0, 0.0); MAX_EARLY_TAPS],
            count: 0,
            slots: [Slot::default(); MAX_EARLY_TAPS],
            size: 1.0,
            level: 1.0,
        };
        e.set_pattern(EarlyPattern::SmallRoom);
        e
    }

    /// Loads a built-in pattern.
    pub fn set_pattern(&mut self, pattern: EarlyPattern) {
        self.set_taps(pattern.taps());
    }

    /// Loads up to [`MAX_EARLY_TAPS`] custom taps (extra taps are ignored;
    /// non-finite fields count as zero gain).
    pub fn set_taps(&mut self, taps: &[EarlyTap]) {
        self.count = taps.len().min(MAX_EARLY_TAPS);
        self.taps[..self.count].copy_from_slice(&taps[..self.count]);
        self.rebuild();
    }

    /// Time scale of all taps, `0.25..=4.0`.
    pub fn set_size(&mut self, size: f32) {
        if size.is_finite() {
            self.size = size.clamp(EARLY_SIZE_RANGE.0, EARLY_SIZE_RANGE.1);
            self.rebuild();
        }
    }

    /// Output level (linear), `0.0..=4.0`.
    pub fn set_level(&mut self, level: f32) {
        if level.is_finite() {
            self.level = level.clamp(0.0, 4.0);
            self.rebuild();
        }
    }

    /// The output level (linear).
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Longest tap delay in samples with the current size.
    pub fn longest_tap_samples(&self) -> usize {
        self.slots[..self.count].iter().map(|s| s.delay).max().unwrap_or(0)
    }

    fn rebuild(&mut self) {
        for i in 0..self.count {
            let tap = self.taps[i];
            let time = if tap.time_ms.is_finite() { tap.time_ms.max(0.0) } else { 0.0 };
            let gain = if tap.gain.is_finite() { tap.gain } else { 0.0 } * self.level;
            let pan = if tap.pan.is_finite() { tap.pan.clamp(-1.0, 1.0) } else { 0.0 };
            let theta = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
            let delay = ((time * self.size * 0.001 * self.sample_rate).round() as usize)
                .min(self.max_delay);
            self.slots[i] = Slot {
                delay,
                gl: gain * theta.cos(),
                gr: gain * theta.sin(),
            };
        }
    }

    /// Clears the delay line. Does not allocate.
    pub fn reset(&mut self) {
        self.ring.clear();
    }

    /// One stereo frame in, the reflections out (no dry signal).
    #[inline]
    pub fn process_frame(&mut self, l: f32, r: f32) -> (f32, f32) {
        let m = 0.5 * (clean(l) + clean(r));
        self.ring.push(flush(m));
        let mut ol = 0.0;
        let mut or = 0.0;
        for s in &self.slots[..self.count] {
            let v = self.ring.at(s.delay + 1);
            ol += v * s.gl;
            or += v * s.gr;
        }
        (ol, or)
    }

    /// In place: replaces the buffers with the reflections only.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (a, b) = self.process_frame(*l, *r);
            *l = a;
            *r = b;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taps_land_at_their_times() {
        let mut e = EarlyReflections::new(48_000.0);
        e.set_taps(&[t(1.0, 0.5, -1.0), t(2.0, 0.25, 1.0)]);
        let mut l = vec![0.0f32; 200];
        let mut r = vec![0.0f32; 200];
        l[0] = 1.0;
        r[0] = 1.0;
        e.process(&mut l, &mut r);
        assert!((l[48] - 0.5).abs() < 1e-6 && r[48].abs() < 1e-6);
        assert!(l[96].abs() < 1e-6 && (r[96] - 0.25).abs() < 1e-6);
        let others = l.iter().chain(r.iter()).filter(|v| v.abs() > 1e-6).count();
        assert_eq!(others, 2);
    }

    #[test]
    fn size_scales_times_and_nan_is_silent() {
        let mut e = EarlyReflections::new(48_000.0);
        e.set_taps(&[t(1.0, 1.0, 0.0)]);
        e.set_size(2.0);
        assert_eq!(e.longest_tap_samples(), 96);
        e.set_size(f32::NAN);
        assert_eq!(e.longest_tap_samples(), 96);
        let (a, b) = e.process_frame(f32::NAN, f32::INFINITY);
        assert!(a == 0.0 && b == 0.0);
    }

    #[test]
    fn every_pattern_is_well_formed() {
        for p in [
            EarlyPattern::SmallRoom,
            EarlyPattern::LargeRoom,
            EarlyPattern::Hall,
            EarlyPattern::Dense,
            EarlyPattern::Slap,
        ] {
            let taps = p.taps();
            assert!(!taps.is_empty() && taps.len() <= MAX_EARLY_TAPS);
            assert!(taps.windows(2).all(|w| w[0].time_ms < w[1].time_ms));
            assert!(taps.iter().all(|x| x.gain > 0.0 && x.gain < 1.0 && x.pan.abs() <= 1.0));
        }
    }
}
