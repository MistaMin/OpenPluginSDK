//! Lookahead brickwall limiter with a true-peak ceiling.
//!
//! Design (written for this crate from the general lookahead-limiter
//! construction and the standards below, not derived from any other code):
//!
//! 1. Each frame, every channel is measured by a 4x true-peak detector
//!    ([`crate::true_peak`], after ITU-R BS.1770-4 Annex 2); the largest value
//!    across channels is the frame's peak, so all channels share one gain
//!    (linked, the image does not shift).
//! 2. The gain each frame requires is `u = min(1, ceiling / peak)`.
//! 3. A sliding minimum of `u` over `L + 2` frames makes the requirement
//!    arrive `L` frames before the peak does.
//! 4. A moving average of `L` frames of that minimum turns the corners into
//!    linear ramps, so the gain moves smoothly (no clicks). Because every term
//!    of the average is at most the requirement of the sample being output, the
//!    smoothed gain is also at most that requirement: the smoothing can never
//!    let a peak through.
//! 5. A release stage lets the gain recover toward unity with time constant
//!    `release_ms`, and never above the smoothed value. Recovery is
//!    exponential, so the gain stays continuous.
//! 6. The audio is delayed by `L + 9` frames and multiplied by
//!    the gain. [`Limiter::latency_samples`] reports that delay; return it
//!    from `Plugin::latency`.
//!
//! The ceiling is on the *true* peak as estimated by the 4x detector. A 4x
//! estimate is not the continuous waveform; the tests compare against an
//! independent 16x interpolator and state the measured excess.
//!
//! All buffers are sized in [`Limiter::prepare`]; `process` neither allocates
//! nor panics, treats non-finite input as silence, and never leaves a
//! subnormal number in its state.

use crate::envelope::db_to_amp;
use crate::rate::effective_rate;
use crate::true_peak::TruePeak;

const MAX_LOOKAHEAD_MS: f32 = 50.0;

/// Lookahead brickwall limiter. See the module docs.
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
    detectors: Vec<TruePeak>,
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
        let delay = lookahead + TruePeak::precise().delay();
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
            detectors: vec![TruePeak::precise(); channels],
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

    /// Frames of delay: lookahead plus the true-peak filter delay. Report this
    /// from `Plugin::latency`.
    pub fn latency_samples(&self) -> usize {
        self.delay
    }

    /// Lookahead alone, in frames.
    pub fn lookahead_samples(&self) -> usize {
        self.lookahead
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Ceiling in dBTP (default -1.0). Takes effect with the lookahead.
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

    /// `true` (default): limit the 4x true peak. `false`: limit the sample
    /// peak only (cheaper; intersample peaks may exceed the ceiling).
    pub fn set_true_peak(&mut self, on: bool) {
        self.true_peak = on;
    }

    /// Gain reduction of the last frame in dB (positive).
    pub fn gain_reduction_db(&self) -> f32 {
        -20.0 * self.gain.max(1.0e-6).log10()
    }

    pub fn reset(&mut self) {
        for d in &mut self.detectors {
            d.reset();
        }
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
            for (c, ch) in channels[..nc].iter().enumerate() {
                let x = ch[i];
                let p = if self.true_peak {
                    self.detectors[c].push(x)
                } else if x.is_finite() {
                    x.abs()
                } else {
                    0.0
                };
                peak = peak.max(p);
            }
            let u = if peak > self.ceiling { self.ceiling / peak } else { 1.0 };

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
