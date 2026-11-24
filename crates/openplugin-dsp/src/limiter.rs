//! Lookahead brickwall limiter with a true-peak ceiling.
//!
//! Design (written for this crate from the general lookahead-limiter
//! construction and the standards below, not derived from any other code):
//!
//! 1. Each frame, every channel is measured by the inter-sample peak detector
//!    of `intersample.rs` (after ITU-R BS.1770-4 Annex 2): three
//!    reconstruction filters, the largest reading of the three. The largest
//!    value across channels is the frame's peak, so all channels share one
//!    gain (linked, the image does not shift).
//! 2. The gain each frame requires is `u = min(1, target / peak)`, where the
//!    target is the ceiling less a small margin (see "Margin").
//! 3. A sliding minimum of `u` over `L + 2` frames makes the requirement
//!    arrive `L` frames before the peak does.
//! 4. A moving average of `L` frames of that minimum turns the corners into
//!    linear ramps, so the gain moves smoothly (no clicks). Because every term
//!    of the average is at most the requirement of the sample being output, the
//!    smoothed gain is also at most that requirement.
//! 5. A release stage lets the gain recover toward unity with time constant
//!    `release_ms`, and never above the smoothed value. Recovery is
//!    exponential, so the gain stays continuous.
//! 6. The audio is delayed by `L + 48` frames (the lookahead plus the
//!    detector's delay, so the gain is in place when the peak comes out) and
//!    multiplied by the gain. [`Limiter::latency_samples`] reports that delay;
//!    return it from `Plugin::latency`.
//!
//! # What the ceiling means
//!
//! The true peak of a signal with energy near the Nyquist frequency depends on
//! the filter that reconstructs it: on full-band white noise two meters that
//! differ only in filter length disagree by more than 2 dB (measured, see
//! `intersample.rs`). Until now the limiter read one short filter (16 taps
//! per phase), which read such material up to 2.6 dB lower than a long one,
//! and that was the whole of its known excess on white noise. It now holds the
//! ceiling for three filters at once: a Hann-windowed sinc of 48 samples each
//! side, the filter of [`crate::TruePeakMeter`], and the BS.1770 structure of
//! [`crate::TruePeak::new`]. `tests/true_peak_ceiling.rs` checks it with an
//! independent 16x evaluation and with `TruePeakMeter`, for white and pink
//! noise, square waves, a sweep, impulses, clipped drum hits and the fs/4
//! tone, at 44.1 to 192 kHz: the largest reading is at or below the ceiling
//! (the worst measured is 0.01 dB under it). A reconstruction filter that is
//! none of the three can still read full-band noise higher (measured there:
//! 0.14 dB over for a sinc of 24 samples each side, 0.42 dB over for one of
//! 200); material that is band-limited below about 0.45 fs reads the same on
//! all of them.
//!
//! # Margin
//!
//! A gain applied sample by sample is not the same as a gain applied to the
//! reconstructed waveform. The output samples are `g[n] x[n]`, and their
//! reconstruction between two samples weighs neighbours that carry slightly
//! different gains, so it differs from `g(t) x(t)` by an amount proportional
//! to how fast the gain moves and to how much of the signal lies near Nyquist.
//! With the detector reading the input exactly, the output of this limiter
//! measured up to 4.3 dB / L over the ceiling on full-band white noise (`L` the
//! lookahead in frames: 0.06 dB at 72 frames, 0.10 dB at 44, 0.5 dB at 8), and
//! less on everything else. So in true-peak mode the limiter aims
//! `4.5 dB / L` below the ceiling (at most 1 dB): 0.06 dB at 1.5 ms and
//! 48 kHz, 0.10 dB at 1 ms and 44.1 kHz, 0.016 dB at 1.5 ms and 192 kHz.
//! [`Limiter::margin_db`] returns the figure. A signal whose true peak lies
//! within the margin of the ceiling is turned down by up to that much.
//!
//! All buffers are sized in [`Limiter::prepare`]; `process` neither allocates
//! nor panics, treats non-finite input as silence, and never leaves a
//! subnormal number in its state.

use crate::envelope::db_to_amp;
use crate::rate::effective_rate;
use crate::intersample::{self, InterSample};

const MAX_LOOKAHEAD_MS: f32 = 50.0;
/// Margin under the ceiling, in dB times lookahead frames (see "Margin").
const MARGIN_DB_FRAMES: f32 = 4.5;
const MAX_MARGIN_DB: f32 = 1.0;

/// Lookahead brickwall limiter. See the module docs.
/// `prepare` allocates; `process` and the setters do not.
///
/// # Example
///
/// ```
/// use openplugin_dsp::Limiter;
///
/// let mut limiter = Limiter::prepare(48_000.0, 2, 1.5, 100.0);
/// limiter.set_ceiling_db(-1.0);
/// let mut left = vec![2.0f32; 4_800]; // +6 dBFS
/// let mut right = vec![2.0f32; 4_800];
/// limiter.process(&mut [&mut left, &mut right]);
/// let ceiling = 10f32.powf(-1.0 / 20.0);
/// assert!(left.iter().all(|s| s.abs() <= ceiling + 1e-4));
/// assert!(limiter.gain_reduction_db() > 6.0);
/// // Report limiter.latency_samples() to the host and delay the dry path by it.
/// ```
#[derive(Debug, Clone)]
pub struct Limiter {
    fs: f64,
    channels: usize,
    lookahead: usize,
    delay: usize,
    ceiling: f32,
    release_coef: f64,
    release_ms: f32,
    true_peak: bool,
    detector: InterSample,
    margin: f32,
    // Audio delay lines, channel-major, each `delay` long.
    audio: Vec<f32>,
    audio_pos: usize,
    // Sliding minimum (monotonic deque in a ring of `window + 1`).
    min_idx: Vec<u64>,
    min_val: Vec<f32>,
    min_head: usize,
    min_len: usize,
    window: usize,
    // Moving average of the minimum.
    avg_ring: Vec<f32>,
    avg_pos: usize,
    avg_sum: f64,
    avg_count: usize,
    release_state: f64,
    n: u64,
    gain: f32,
}

impl Limiter {
    /// Build a limiter for `channels` channels (at least 1) at `sample_rate`,
    /// with `lookahead_ms` of lookahead (clamped to 0.1..=50 ms, at least one
    /// frame) and `release_ms` recovery time. Allocates; call from `prepare`.
    pub fn prepare(sample_rate: f32, channels: usize, lookahead_ms: f32, release_ms: f32) -> Self {
        let fs = effective_rate(sample_rate) as f64;
        let ms = if lookahead_ms.is_finite() { lookahead_ms.clamp(0.1, MAX_LOOKAHEAD_MS) } else { 1.0 };
        let lookahead = ((ms as f64 * 0.001 * fs).round() as usize).max(1);
        let delay = lookahead + intersample::DELAY;
        let channels = channels.max(1);
        let window = lookahead + 2;
        let mut l = Self {
            fs,
            channels,
            lookahead,
            delay,
            ceiling: db_to_amp(-1.0),
            release_coef: 0.0,
            release_ms,
            true_peak: true,
            detector: InterSample::new(channels),
            margin: db_to_amp(-Self::margin_db_for(lookahead)),
            audio: vec![0.0; channels * delay],
            audio_pos: 0,
            min_idx: vec![0; window + 1],
            min_val: vec![1.0; window + 1],
            min_head: 0,
            min_len: 0,
            window,
            avg_ring: vec![1.0; lookahead],
            avg_pos: 0,
            avg_sum: lookahead as f64,
            avg_count: 0,
            release_state: 1.0,
            n: 0,
            gain: 1.0,
        };
        l.set_release_ms(release_ms);
        l
    }

    /// Decibels by which the true-peak target sits under the ceiling for a
    /// lookahead of `lookahead` frames. See the module docs ("Margin").
    fn margin_db_for(lookahead: usize) -> f32 {
        (MARGIN_DB_FRAMES / lookahead.max(1) as f32).min(MAX_MARGIN_DB)
    }

    /// The margin in dB that true-peak mode keeps under the ceiling: 4.5 dB
    /// divided by the lookahead in frames, at most 1 dB (0.06 dB at 1.5 ms and
    /// 48 kHz). See the module docs, "Margin".
    pub fn margin_db(&self) -> f32 {
        Self::margin_db_for(self.lookahead)
    }

    /// Frames of delay: the lookahead plus the 48 frames of the true-peak
    /// detector. Report this from `Plugin::latency`.
    pub fn latency_samples(&self) -> usize {
        self.delay
    }

    /// Lookahead alone, in frames.
    pub fn lookahead_samples(&self) -> usize {
        self.lookahead
    }

    /// The channel count given to `new`.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Ceiling in dBTP (default -1.0). Takes effect with the lookahead.
    /// True-peak mode aims [`margin_db`](Self::margin_db) below it.
    pub fn set_ceiling_db(&mut self, db: f32) {
        let db = if db.is_nan() { 0.0 } else { db.clamp(-60.0, 12.0) };
        self.ceiling = db_to_amp(db);
    }

    /// Recovery time constant in milliseconds (clamped to 1 ms..=5 s).
    pub fn set_release_ms(&mut self, ms: f32) {
        let ms = if ms.is_finite() { ms.clamp(1.0, 5000.0) } else { 100.0 };
        self.release_ms = ms;
        self.release_coef = (-1.0 / (ms as f64 * 0.001 * self.fs)).exp();
    }

    /// `true` (default): limit the true peak. `false`: limit the sample peak
    /// only (about a tenth of the CPU; intersample peaks may exceed the
    /// ceiling, and there is no margin). The delay is the same in both modes.
    /// The detector does not run in sample-peak mode, so after switching it
    /// on its history is empty: the true-peak ceiling holds again once 96
    /// frames have passed. Choose the mode before audio runs.
    pub fn set_true_peak(&mut self, on: bool) {
        if on && !self.true_peak {
            self.detector.reset();
        }
        self.true_peak = on;
    }

    /// Gain reduction of the last frame in dB (positive).
    pub fn gain_reduction_db(&self) -> f32 {
        -20.0 * self.gain.max(1.0e-6).log10()
    }

    /// Clears the lookahead, the detector and the gain.
    pub fn reset(&mut self) {
        self.detector.reset();
        self.audio.fill(0.0);
        self.audio_pos = 0;
        self.min_head = 0;
        self.min_len = 0;
        self.avg_ring.fill(1.0);
        self.avg_pos = 0;
        self.avg_sum = self.lookahead as f64;
        self.avg_count = 0;
        self.release_state = 1.0;
        self.n = 0;
        self.gain = 1.0;
    }

    /// Limit `channels` in place. Only the first `self.channels()` are
    /// processed (further ones are left untouched); a missing channel counts
    /// as silence. Frames are limited to the shortest processed channel.
    pub fn process(&mut self, channels: &mut [&mut [f32]]) {
        let nc = channels.len().min(self.channels);
        if nc == 0 {
            return;
        }
        let frames = channels[..nc].iter().map(|c| c.len()).min().unwrap_or(0);
        let cap = self.window + 1;
        for i in 0..frames {
            // 1-2. Measure and derive the required gain.
            let mut peak = 0.0f32;
            let target = if self.true_peak {
                for (c, ch) in channels[..nc].iter().enumerate() {
                    peak = peak.max(self.detector.push(c, ch[i]));
                }
                self.detector.advance();
                self.ceiling * self.margin
            } else {
                // The sample the true-peak detector would be reporting on:
                // `intersample::DELAY` frames back, read from the delay line.
                let mut at = self.audio_pos + self.lookahead;
                if at >= self.delay {
                    at -= self.delay;
                }
                for c in 0..nc {
                    peak = peak.max(self.audio[c * self.delay + at].abs());
                }
                self.ceiling
            };
            // A reading that overflowed to infinity gives gain 0.
            let u = if peak <= target {
                1.0
            } else if peak.is_finite() {
                target / peak
            } else {
                0.0
            };

            // 3. Sliding minimum over `window` frames.
            while self.min_len > 0 {
                let back = (self.min_head + self.min_len - 1) % cap;
                if self.min_val[back] >= u {
                    self.min_len -= 1;
                } else {
                    break;
                }
            }
            let tail = (self.min_head + self.min_len) % cap;
            self.min_idx[tail] = self.n;
            self.min_val[tail] = u;
            self.min_len += 1;
            while self.min_len > 0 && self.min_idx[self.min_head] + self.window as u64 <= self.n {
                self.min_head = (self.min_head + 1) % cap;
                self.min_len -= 1;
            }
            let m = self.min_val[self.min_head];

            // 4. Moving average of the minimum, exact running sum with a
            //    periodic full recomputation so rounding cannot accumulate.
            let old = self.avg_ring[self.avg_pos];
            self.avg_ring[self.avg_pos] = m;
            self.avg_pos = (self.avg_pos + 1) % self.lookahead;
            self.avg_sum += m as f64 - old as f64;
            self.avg_count += 1;
            if self.avg_count >= self.lookahead {
                self.avg_count = 0;
                self.avg_sum = self.avg_ring.iter().map(|&v| v as f64).sum();
            }
            let smooth = (self.avg_sum / self.lookahead as f64).min(1.0);

            // 5. Release: recover toward unity, never above the smoothed gain.
            let recovered = self.release_state + (1.0 - self.release_state) * (1.0 - self.release_coef);
            self.release_state = recovered.min(smooth);
            if self.release_state > 1.0 - 1.0e-12 {
                self.release_state = 1.0;
            }
            let g = self.release_state as f32;
            self.gain = g;

            // 6. Delay the audio and apply the gain.
            for (c, ch) in channels[..nc].iter_mut().enumerate() {
                let x = if ch[i].is_finite() { ch[i] } else { 0.0 };
                let slot = c * self.delay + self.audio_pos;
                let delayed = self.audio[slot];
                self.audio[slot] = x;
                ch[i] = delayed * g;
            }
            self.audio_pos += 1;
            if self.audio_pos == self.delay {
                self.audio_pos = 0;
            }
            self.n += 1;
        }
    }
}
