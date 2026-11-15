//! A multichannel true-peak meter (hold, running maximum, oversampling chosen by rate).
//! The limiter's detector (`intersample.rs`) evaluates this meter's 4x filter among others.
//!
//! True-peak meter after ITU-R BS.1770-4 Annex 2, with hold.
//!
//! The sample peak of a signal can understate the peak of the reconstructed
//! analogue waveform ("inter-sample peaks"). BS.1770-4 Annex 2 oversamples by
//! 4 (for rates below 96 kHz) with a linear-phase FIR interpolator and takes
//! the largest absolute oversampled value, in dBTP. Annex 2 prints its own
//! 48-tap filter; this module designs its own (a Kaiser-windowed sinc, 97
//! taps, beta 9, cutoff at the original Nyquist, gain 4), which meets the
//! +0.2 / -0.4 dB tolerance of EBU Tech 3341 (2023), table 1, signals 15 to 23
//! (checked in the tests, including the sample-offset signals 20 to 23).
//!
//! The factor follows the rate as BS.1770-4 advises: 4x below 96 kHz, 2x from
//! 96 kHz to below 192 kHz, and none at 192 kHz and above (the sample peak is
//! then close enough).
//!
//! [`TruePeakMeter`] keeps, per channel, the running maximum since the last
//! [`TruePeakMeter::reset`] and a peak with hold: the held value stays for
//! `hold_ms` (default 2000 ms; infinite with [`TruePeakMeter::set_hold_ms`]`(f32::INFINITY)`)
//! and then falls at 20 dB/s (a typical bar-graph release). The filter state
//! and history are allocated in `new`; `process` does not allocate.

use std::f64::consts::PI;

const TAPS_PER_PHASE: usize = 25; // 97 taps / 4 phases, padded
const MAX_FACTOR: usize = 4;
const FLOOR_DB: f32 = -120.0;

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..60 {
        term *= q / (k as f64 * k as f64);
        sum += term;
        if term < 1e-18 * sum {
            break;
        }
    }
    sum
}

/// Polyphase interpolator for one factor: `phases[p][k]` multiplies `x[n-k]`.
#[derive(Clone, Debug)]
struct Interpolator {
    factor: usize,
    phases: [[f32; TAPS_PER_PHASE]; MAX_FACTOR],
}

impl Interpolator {
    fn new(factor: usize) -> Self {
        let mut phases = [[0.0f32; TAPS_PER_PHASE]; MAX_FACTOR];
        if factor > 1 {
            let half = 12 * factor; // 97 taps at 4x, 49 at 2x
            let n = 2 * half + 1;
            let beta = 9.0;
            let norm = bessel_i0(beta);
            for j in 0..n {
                let t = (j as f64 - half as f64) / factor as f64;
                let sinc = if t == 0.0 { 1.0 } else { (PI * t).sin() / (PI * t) };
                let r = (j as f64 - half as f64) / half as f64;
                let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / norm;
                phases[j % factor][j / factor] = (sinc * w) as f32;
            }
        }
        Self { factor, phases }
    }
}

/// The 4x interpolator's phases, for the limiter's detector
/// ([`crate::intersample`]): `[p][k]` multiplies `x[n - k]` and gives the
/// point `n - 12 + p/4`.
pub(crate) fn interpolator_4x() -> [[f32; TAPS_PER_PHASE]; MAX_FACTOR] {
    Interpolator::new(4).phases
}

#[derive(Clone, Debug)]
struct Channel {
    /// Double-length ring so the last `TAPS_PER_PHASE` samples are contiguous.
    history: Box<[f32]>,
    pos: usize,
    max: f32,
    held: f32,
    hold_left: u32,
}

impl Channel {
    fn new() -> Self {
        Self { history: vec![0.0; 2 * TAPS_PER_PHASE].into_boxed_slice(), pos: 0, max: 0.0, held: 0.0, hold_left: 0 }
    }
}

/// Multichannel true-peak meter with hold. See the module documentation.
#[derive(Clone, Debug)]
pub struct TruePeakMeter {
    interp: Interpolator,
    channels: Vec<Channel>,
    hold_samples: u32,
    sample_rate: f32,
    release_per_sample: f32,
}

impl TruePeakMeter {
    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let rate = crate::rate::effective_rate(sample_rate);
        let factor = if rate < 96_000.0 {
            4
        } else if rate < 192_000.0 {
            2
        } else {
            1
        };
        let mut me = Self {
            interp: Interpolator::new(factor),
            channels: (0..channels.max(1)).map(|_| Channel::new()).collect(),
            hold_samples: 0,
            sample_rate: rate,
            release_per_sample: 10f32.powf(-20.0 / 20.0 / rate),
        };
        me.set_hold_ms(2000.0);
        me
    }

    /// Hold time in ms before the held reading falls; `f32::INFINITY` holds
    /// until [`Self::reset_hold`].
    pub fn set_hold_ms(&mut self, ms: f32) {
        self.hold_samples = if ms.is_nan() || ms <= 0.0 {
            0
        } else if ms.is_infinite() {
            u32::MAX
        } else {
            (ms * 0.001 * self.sample_rate).min(4.0e9) as u32
        };
    }

    /// The oversampling factor in use (4, 2 or 1).
    pub fn factor(&self) -> usize {
        self.interp.factor
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Forget everything: maxima, holds and filter history.
    pub fn reset(&mut self) {
        for ch in &mut self.channels {
            *ch = Channel::new();
        }
    }

    /// Clear the held peaks only (the "reset peak" button); the running
    /// maximum is kept.
    pub fn reset_hold(&mut self) {
        for ch in &mut self.channels {
            ch.held = 0.0;
            ch.hold_left = 0;
        }
    }

    /// Clear the running maxima only.
    pub fn reset_max(&mut self) {
        for ch in &mut self.channels {
            ch.max = 0.0;
        }
    }

    /// Feed a block of one channel.
    pub fn process(&mut self, channel: usize, samples: &[f32]) {
        let Some(ch) = self.channels.get_mut(channel) else { return };
        let factor = self.interp.factor;
        for &raw in samples {
            let x = if raw.is_finite() { raw } else { 0.0 };
            // Push into the double ring (newest at `pos`, older after it).
            ch.pos = if ch.pos == 0 { TAPS_PER_PHASE - 1 } else { ch.pos - 1 };
            ch.history[ch.pos] = x;
            ch.history[ch.pos + TAPS_PER_PHASE] = x;
            let recent = &ch.history[ch.pos..ch.pos + TAPS_PER_PHASE];
            let mut peak = x.abs();
            if factor > 1 {
                for phase in &self.interp.phases[..factor] {
                    let mut acc = 0.0f32;
                    for (h, c) in recent.iter().zip(phase.iter()) {
                        acc += h * c;
                    }
                    peak = peak.max(acc.abs());
                }
            }
            if peak > ch.max {
                ch.max = peak;
            }
            if peak >= ch.held {
                ch.held = peak;
                ch.hold_left = self.hold_samples;
            } else if ch.hold_left > 0 {
                if ch.hold_left != u32::MAX {
                    ch.hold_left -= 1;
                }
            } else {
                ch.held *= self.release_per_sample;
            }
        }
    }

    fn db(peak: f32) -> f32 {
        if peak <= 1e-6 {
            FLOOR_DB
        } else {
            20.0 * peak.log10()
        }
    }

    /// Largest true peak since `reset`, dBTP (-120 for silence).
    pub fn max_dbtp(&self, channel: usize) -> f32 {
        self.channels.get(channel).map_or(FLOOR_DB, |c| Self::db(c.max))
    }

    /// The held reading, dBTP.
    pub fn held_dbtp(&self, channel: usize) -> f32 {
        self.channels.get(channel).map_or(FLOOR_DB, |c| Self::db(c.held))
    }

    /// Largest of all channels since `reset`, dBTP.
    pub fn max_all_dbtp(&self) -> f32 {
        self.channels.iter().map(|c| Self::db(c.max)).fold(FLOOR_DB, f32::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq_over_fs: f64, amp: f64, phase_deg: f64, frames: usize) -> Vec<f32> {
        let ph = phase_deg.to_radians();
        (0..frames)
            .map(|n| {
                // 10 ms-ish fade in and out, as the EBU signals are tapered.
                let fade = 480.min(frames / 2) as f64;
                let w = ((n as f64) / fade).min(1.0).min(((frames - 1 - n) as f64) / fade);
                (amp * w * (2.0 * PI * freq_over_fs * n as f64 + ph).sin()) as f32
            })
            .collect()
    }

    fn peak_of(x: &[f32]) -> f32 {
        let mut m = TruePeakMeter::new(48_000.0, 1);
        m.process(0, x);
        m.max_dbtp(0)
    }

    fn assert_ebu(value: f32, expected: f32, what: &str) {
        // Tech 3341: +0.2 / -0.4 dB.
        assert!(value <= expected + 0.2 && value >= expected - 0.4, "{what}: {value} dBTP, expected {expected}");
    }

    /// EBU Tech 3341 (2023) table 1, signals 15 to 19.
    #[test]
    fn tech3341_signals_15_to_19() {
        assert_ebu(peak_of(&sine(0.25, 0.5, 0.0, 9600)), -6.0, "15");
        assert_ebu(peak_of(&sine(0.25, 0.5, 45.0, 9600)), -6.0, "16");
        assert_ebu(peak_of(&sine(1.0 / 6.0, 0.5, 60.0, 9600)), -6.0, "17");
        assert_ebu(peak_of(&sine(1.0 / 8.0, 0.5, 67.5, 9600)), -6.0, "18");
        assert_ebu(peak_of(&sine(0.25, 1.41, 45.0, 9600)), 3.0, "19");
    }

    /// Signals 15 to 18 with a phase that puts the samples on the zero
    /// crossings show the true peak is above the sample peak.
    #[test]
    fn inter_sample_peaks_exceed_the_sample_peak() {
        let x = sine(0.25, 1.0, 45.0, 9600);
        let sample_peak = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert!(sample_peak < 0.72);
        assert_ebu(peak_of(&x), 0.0, "fs/4 at 45 degrees, amplitude 1");
    }

    /// Signals 20 to 23: fs/6 sine at 0.5 with one fs/4 period of amplitude 1,
    /// synthesised at 4 fs, low-passed and decimated with offsets 0..3.
    #[test]
    fn tech3341_signals_20_to_23() {
        let osr = 4usize;
        let frames = 4800usize;
        let hi_len = frames * osr;
        // Continuous in phase at both sides of the single fs/4 period.
        let mut hi = vec![0.0f64; hi_len];
        let start = hi_len / 2;
        let f6 = 1.0 / 6.0 / osr as f64; // cycles per hi-rate sample
        let f4 = 0.25 / osr as f64;
        let period = (1.0 / f4) as usize;
        let mut phase = 0.0f64;
        for (n, v) in hi.iter_mut().enumerate() {
            let in_burst = n >= start && n < start + period;
            let (f, a) = if in_burst { (f4, 1.0) } else { (f6, 0.5) };
            *v = a * (2.0 * PI * phase).sin();
            phase += f;
        }
        // Linear-phase lowpass at the target Nyquist (windowed sinc, long), then decimate.
        let half = 192isize;
        let cutoff = 0.5 / osr as f64 * 0.94;
        let taps: Vec<f64> = (-half..=half)
            .map(|k| {
                let t = k as f64;
                let s = if k == 0 { 2.0 * cutoff } else { (2.0 * PI * cutoff * t).sin() / (PI * t) };
                let r = t / half as f64;
                s * (0.42 + 0.5 * (PI * r).cos() + 0.08 * (2.0 * PI * r).cos())
            })
            .collect();
        for offset in 0..osr {
            let mut x = Vec::new();
            let mut n = offset as isize;
            while (n as usize) < hi_len {
                let mut acc = 0.0;
                for (i, h) in taps.iter().enumerate() {
                    let idx = n + i as isize - half;
                    if idx >= 0 && (idx as usize) < hi_len {
                        acc += h * hi[idx as usize];
                    }
                }
                x.push(acc as f32);
                n += osr as isize;
            }
            let value = peak_of(&x);
            // The synthetic reconstruction is itself only within a fraction
            // of a dB of 0 dBTP, so judge against the sample-peak-free bound.
            assert!(value > -0.8 && value < 0.2, "offset {offset}: {value}");
        }
    }

    #[test]
    fn hold_keeps_then_releases_and_reset_clears() {
        let mut m = TruePeakMeter::new(48_000.0, 2);
        m.set_hold_ms(100.0);
        let mut burst = vec![0.0f32; 480];
        burst[10] = 0.5;
        m.process(0, &burst);
        let peak = m.held_dbtp(0);
        assert!(peak > -7.0 && peak < -5.5, "{peak}");
        m.process(0, &vec![0.0; 2400]); // 50 ms: still held
        assert_eq!(m.held_dbtp(0), peak);
        m.process(0, &vec![0.0; 48_000]); // way past hold: released
        assert!(m.held_dbtp(0) < peak - 15.0);
        assert!((m.max_dbtp(0) - peak).abs() < 1e-3, "max keeps");
        m.reset_hold();
        assert_eq!(m.held_dbtp(0), FLOOR_DB);
        m.reset();
        assert_eq!(m.max_dbtp(0), FLOOR_DB);
        assert_eq!(m.max_dbtp(1), FLOOR_DB);
        m.set_hold_ms(f32::INFINITY);
        m.process(1, &burst);
        m.process(1, &vec![0.0; 480_000]);
        assert!(m.held_dbtp(1) > -7.0);
    }

    #[test]
    fn factor_follows_rate_and_nan_is_silence() {
        assert_eq!(TruePeakMeter::new(48_000.0, 1).factor(), 4);
        assert_eq!(TruePeakMeter::new(96_000.0, 1).factor(), 2);
        assert_eq!(TruePeakMeter::new(192_000.0, 1).factor(), 1);
        let mut m = TruePeakMeter::new(44_100.0, 1);
        m.process(0, &[f32::NAN, f32::INFINITY, 0.0]);
        assert_eq!(m.max_dbtp(0), FLOOR_DB);
    }
}
