//! K-system metering after Bob Katz.
//!
//! Source: B. Katz, "Integrating the Art and Science of Mastering" /
//! "An Integrated Approach to Metering, Monitoring, and Leveling Practices",
//! Journal of the Audio Engineering Society, vol. 48, no. 9, 2000; also
//! *Mastering Audio* (Focal Press, 2002, 2nd ed. 2007), chapter 16.
//!
//! The K-system is one RMS meter with a movable zero. The scale is named by
//! the headroom above zero: on **K-20**, 0 on the meter is -20 dBFS RMS and
//! the top of the scale (+20) is full scale; **K-14** puts 0 at -14 dBFS and
//! **K-12** at -12 dBFS. Katz's meter reads RMS so that a full-scale sine
//! reads 0 dBFS (the AES17 convention, "RMS + 3 dB"): the mean square is
//! doubled, so a sine of peak amplitude `A` reads `20 log10 A`. A sine at the
//! scale's reference peak therefore reads 0 on the K scale, and a sine at full
//! scale reads +20, +14 or +12. Katz recommends K-20 for wide-range material
//! (film, classical), K-14 for most popular music and K-12 for broadcast.
//!
//! The averaging window is 600 ms, the integration time Katz gives for the
//! K-meter (an approximation of how long the ear takes to judge a level; a
//! rectangular sliding window here). The peak reading is the sample peak with
//! hold, shown on the same scale. Katz's colour zones are green up to 0,
//! yellow from 0 to +4 and red above +4 ([`KZone`]).
//!
//! The 600 ms and zone figures are from the author's reading of the book and
//! the JAES paper as widely summarised; the AES17 reading convention is the
//! one Katz uses in the paper. Allocation happens in `new`.

/// A K-system scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum KScale {
    /// 0 at -12 dBFS (broadcast).
    K12,
    /// 0 at -14 dBFS (most popular music).
    K14,
    /// 0 at -20 dBFS (wide-range material: film, classical).
    K20,
}

impl KScale {
    /// The dBFS (AES17 RMS) that reads 0 on this scale.
    pub fn reference_dbfs(self) -> f32 {
        match self {
            Self::K12 => -12.0,
            Self::K14 => -14.0,
            Self::K20 => -20.0,
        }
    }
}

/// The colour band of a K reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum KZone {
    /// Up to 0 on the K scale.
    Green,
    /// Above 0 up to +4.
    Yellow,
    /// Above +4.
    Red,
}

impl KZone {
    /// The zone of a reading on the K scale (dB re the scale's 0).
    pub fn of(k_reading: f32) -> Self {
        if k_reading > 4.0 {
            Self::Red
        } else if k_reading > 0.0 {
            Self::Yellow
        } else {
            Self::Green
        }
    }
}

const FLOOR_DB: f32 = -120.0;

#[derive(Clone, Debug)]
struct Channel {
    ring: Box<[f32]>,
    pos: usize,
    sum: f64,
    /// The squares written since the ring last wrapped, summed in ring order.
    fresh: f64,
    peak: f32,
    held: f32,
    hold_left: u32,
}

/// Per-channel K-system RMS and peak meter.
/// `new` allocates the 600 ms windows; `process` and the readouts do not.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{KMeter, KScale, KZone};
///
/// let rate = 48_000.0;
/// let mut meter = KMeter::new(rate, KScale::K14, 1);
/// // A sine peaking at -14 dBFS reads 0 on K-14.
/// let amp = 10f32.powf(-14.0 / 20.0);
/// let sine: Vec<f32> = (0..48_000)
///     .map(|n| amp * (2.0 * std::f32::consts::PI * 1_000.0 * n as f32 / rate).sin())
///     .collect();
/// meter.process(0, &sine);
/// assert!(meter.rms_k(0).abs() < 0.1);
/// assert_eq!(KZone::of(-3.0), KZone::Green);
/// assert_eq!(KZone::of(6.0), KZone::Red);
/// ```
#[derive(Clone, Debug)]
pub struct KMeter {
    scale: KScale,
    channels: Vec<Channel>,
    hold_samples: u32,
    fall: f32,
}

impl KMeter {
    /// A meter for `sample_rate` (Hz, clamped to the SDK range) and `channels`
    /// (at least 1). Allocates.
    pub fn new(sample_rate: f32, scale: KScale, channels: usize) -> Self {
        let rate = crate::rate::effective_rate(sample_rate);
        let len = ((rate * 0.6).round() as usize).max(1);
        let channel = Channel {
            ring: vec![0.0; len].into_boxed_slice(),
            pos: 0,
            sum: 0.0,
            fresh: 0.0,
            peak: 0.0,
            held: 0.0,
            hold_left: 0,
        };
        Self {
            scale,
            channels: vec![channel; channels.max(1)],
            hold_samples: (rate * 1.5) as u32,
            fall: 10f32.powf(-20.0 / 20.0 / rate),
        }
    }

    /// The scale in use.
    pub fn scale(&self) -> KScale {
        self.scale
    }

    /// Changes the scale; the readings in dBFS are unaffected.
    pub fn set_scale(&mut self, scale: KScale) {
        self.scale = scale;
    }

    /// Clears every channel's window and peaks.
    pub fn reset(&mut self) {
        for ch in &mut self.channels {
            ch.ring.fill(0.0);
            ch.pos = 0;
            ch.sum = 0.0;
            ch.fresh = 0.0;
            ch.peak = 0.0;
            ch.held = 0.0;
            ch.hold_left = 0;
        }
    }

    /// Feed a block of one channel.
    pub fn process(&mut self, channel: usize, samples: &[f32]) {
        let hold = self.hold_samples;
        let fall = self.fall;
        let Some(ch) = self.channels.get_mut(channel) else { return };
        for &raw in samples {
            let x = if raw.is_finite() { raw } else { 0.0 };
            let sq = x * x;
            ch.sum += f64::from(sq) - f64::from(ch.ring[ch.pos]);
            ch.ring[ch.pos] = sq;
            ch.fresh += f64::from(sq);
            ch.pos += 1;
            if ch.pos == ch.ring.len() {
                ch.pos = 0;
                // `fresh` now holds the whole window summed afresh, the same as
                // re-adding the ring, so rounding cannot accumulate; spread over samples.
                ch.sum = ch.fresh;
                ch.fresh = 0.0;
            }
            let a = x.abs();
            ch.peak = ch.peak.max(a);
            if a >= ch.held {
                ch.held = a;
                ch.hold_left = hold;
            } else if ch.hold_left > 0 {
                ch.hold_left -= 1;
            } else {
                ch.held *= fall;
            }
        }
    }

    /// RMS over the last 600 ms in dBFS, AES17 convention (a full-scale sine
    /// reads 0 dBFS). -120 for silence or a missing channel.
    pub fn rms_dbfs(&self, channel: usize) -> f32 {
        let Some(ch) = self.channels.get(channel) else { return FLOOR_DB };
        let mean_square = (ch.sum / ch.ring.len() as f64).max(0.0);
        if mean_square < 1e-12 {
            FLOOR_DB
        } else {
            (10.0 * (2.0 * mean_square).log10()) as f32
        }
    }

    /// The RMS reading on the K scale (0 at the scale's reference).
    pub fn rms_k(&self, channel: usize) -> f32 {
        let db = self.rms_dbfs(channel);
        if db <= FLOOR_DB {
            FLOOR_DB
        } else {
            db - self.scale.reference_dbfs()
        }
    }

    /// Held sample peak in dBFS.
    pub fn peak_hold_dbfs(&self, channel: usize) -> f32 {
        self.channels.get(channel).map_or(FLOOR_DB, |c| if c.held <= 1e-6 { FLOOR_DB } else { 20.0 * c.held.log10() })
    }

    /// Largest sample peak since reset, dBFS.
    pub fn peak_max_dbfs(&self, channel: usize) -> f32 {
        self.channels.get(channel).map_or(FLOOR_DB, |c| if c.peak <= 1e-6 { FLOOR_DB } else { 20.0 * c.peak.log10() })
    }

    /// Held peak on the K scale.
    pub fn peak_k(&self, channel: usize) -> f32 {
        let db = self.peak_hold_dbfs(channel);
        if db <= FLOOR_DB {
            FLOOR_DB
        } else {
            db - self.scale.reference_dbfs()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    fn sine(amp: f32, frames: usize) -> Vec<f32> {
        (0..frames).map(|n| amp * (std::f32::consts::TAU * 997.0 * n as f32 / FS).sin()).collect()
    }

    #[test]
    fn full_scale_sine_reads_plus_headroom_on_each_scale() {
        for (scale, plus) in [(KScale::K12, 12.0), (KScale::K14, 14.0), (KScale::K20, 20.0)] {
            let mut m = KMeter::new(FS, scale, 2);
            m.process(0, &sine(1.0, 48_000));
            assert!((m.rms_k(0) - plus).abs() < 0.02, "{scale:?}: {}", m.rms_k(0));
            assert!((m.rms_dbfs(0)).abs() < 0.02);
            assert!((m.peak_k(0) - plus).abs() < 0.01);
            assert_eq!(m.rms_k(1), FLOOR_DB);
        }
    }

    #[test]
    fn reference_sine_reads_zero() {
        for scale in [KScale::K12, KScale::K14, KScale::K20] {
            let amp = 10f32.powf(scale.reference_dbfs() / 20.0);
            let mut m = KMeter::new(FS, scale, 1);
            m.process(0, &sine(amp, 48_000));
            assert!(m.rms_k(0).abs() < 0.02, "{scale:?}: {}", m.rms_k(0));
        }
    }

    #[test]
    fn window_is_600_ms() {
        let mut m = KMeter::new(FS, KScale::K20, 1);
        m.process(0, &sine(1.0, 48_000));
        // 0.3 s of silence: half the window gone, -3 dB.
        m.process(0, &vec![0.0; 14_400]);
        assert!((m.rms_dbfs(0) - -3.01).abs() < 0.1, "{}", m.rms_dbfs(0));
        m.process(0, &vec![0.0; 14_400 + 10]);
        assert_eq!(m.rms_dbfs(0), FLOOR_DB);
    }

    #[test]
    fn running_sum_equals_a_re_sum_at_every_wrap() {
        let mut m = KMeter::new(1000.0, KScale::K20, 1);
        let x: Vec<f32> = (0..2400).map(|n| ((n * 7919 % 1000) as f32 / 500.0 - 1.0) * 0.8).collect();
        for _ in 0..3 {
            m.process(0, &x);
            let ch = &m.channels[0];
            assert_eq!(ch.pos, 0);
            let want: f64 = ch.ring.iter().map(|&v| f64::from(v)).sum();
            assert_eq!(ch.sum, want);
        }
    }

    #[test]
    fn peak_hold_then_fall_and_zones() {
        let mut m = KMeter::new(FS, KScale::K14, 1);
        let mut burst = vec![0.0f32; 480];
        burst[0] = 0.5;
        m.process(0, &burst);
        let held = m.peak_hold_dbfs(0);
        assert!((held - -6.02).abs() < 0.01);
        m.process(0, &vec![0.0; 24_000]);
        assert_eq!(m.peak_hold_dbfs(0), held);
        m.process(0, &vec![0.0; 96_000]);
        assert!(m.peak_hold_dbfs(0) < held - 10.0);
        assert!((m.peak_max_dbfs(0) - held).abs() < 1e-3);
        assert_eq!(KZone::of(-3.0), KZone::Green);
        assert_eq!(KZone::of(2.0), KZone::Yellow);
        assert_eq!(KZone::of(4.5), KZone::Red);
    }
}
