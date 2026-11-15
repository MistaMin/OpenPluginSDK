//! Oversampled clipper aimed at a true-peak ceiling.
//!
//! A memoryless clipper at the host rate aliases: its harmonics fold back below
//! Nyquist. The remedy is to clip at a higher rate and low-pass before
//! decimating (the oversampled-nonlinearity argument of Zoelzer, "DAFX",
//! 2nd ed., 2011; Esqueda, Valimaki, Bilbao, "Rounding corners with BLAMP",
//! DAFx-16, on why the corners of a clipper alias). This block clips at 2x
//! or 4x through [`LinearPhaseOversampler`] (linear phase, latency
//! [`TruePeakClipper::latency_samples`], report it from `Plugin::latency`).
//!
//! # Curve
//!
//! With ceiling `C` and knee `k` dB, `x_k = C * 10^(-k/20)`:
//!
//! - `|y| = |x|` for `|x| <= x_k`;
//! - `|y| = x_k + w tanh((|x| - x_k) / w)`, `w = C - x_k`, above it.
//!
//! The curve and its first derivative are continuous at `x_k` and `|y| < C`
//! always (the soft knee approaches `C` asymptotically). `k = 0` is the hard
//! clip `|y| = min(|x|, C)`.
//!
//! # True-peak ceiling and its tolerance
//!
//! The clip holds the *oversampled* waveform under `C`. Decimation filters the
//! clip's out-of-band content and the filter rings, so the base-rate waveform
//! can poke above `C` (measured up to about 1.8 dB for a hard-clipped 11 kHz
//! sine at 4x). A final sample clamp would not help: samples pinned to `C` at
//! 45 degrees of an fs/4 tone have an inter-sample peak 3 dB higher. So the
//! output goes through the crate's lookahead [`Limiter`] (true-peak mode,
//! ceiling `C`, 1 ms lookahead, 20 ms release), which only has the residual
//! ringing to remove and so acts for a few samples at a time. The limiter's
//! delay adds to the clipper's; [`TruePeakClipper::latency_samples`] is the sum.
//! The remaining excess over `C` is the limiter's own (its ceiling check is on
//! a 4x/8x estimate); the tolerance is measured against an independent 16x
//! interpolator in `tests/multiband.rs` and stated in the cookbook.
//! [`TruePeakClipper::set_guard_db`] lowers the clip level (not the limiter's
//! ceiling) to take more of the work off the limiter.
//!
//! `prepare` allocates; `process` does not, treats non-finite input as silence,
//! and never panics.

#![allow(clippy::needless_range_loop)]

use crate::envelope::db_to_amp;
use crate::limiter::Limiter;
use crate::linear_phase::{LinearPhaseOversampler, OversampleError};

/// Oversampled soft/hard clipper with a true-peak ceiling.
pub struct TruePeakClipper {
    os: LinearPhaseOversampler,
    limiter: Limiter,
    channels: usize,
    max_block: usize,
    ceiling: f32,
    guard_db: f32,
    drive: f32,
    knee_db: f32,
    inp: Vec<Vec<f32>>,
    out: Vec<Vec<f32>>,
}

impl TruePeakClipper {
    /// `factor` is 1, 2 or 4; `channels` 1..=8; `max_block` the largest slice
    /// handed to [`process`](Self::process) at once (longer ones are chunked).
    pub fn prepare(sample_rate: f32, factor: usize, channels: usize, max_block: usize) -> Result<Self, OversampleError> {
        let os = LinearPhaseOversampler::prepare(factor, channels, max_block)?;
        let mut limiter = Limiter::prepare(sample_rate, channels, 1.0, 20.0);
        limiter.set_true_peak(true);
        Ok(Self {
            os,
            limiter,
            channels,
            max_block,
            ceiling: 1.0,
            guard_db: 0.0,
            drive: 1.0,
            knee_db: 0.0,
            inp: vec![vec![0.0; max_block]; channels],
            out: vec![vec![0.0; max_block]; channels],
        })
    }

    /// Delay in host samples; return it from `Plugin::latency`.
    pub fn latency_samples(&self) -> usize {
        self.os.latency_samples() + self.limiter.latency_samples()
    }

    pub fn reset(&mut self) {
        self.os.reset();
        self.limiter.reset();
    }

    /// Ceiling in dBFS (-60..=0).
    pub fn set_ceiling_db(&mut self, db: f32) {
        if db.is_finite() {
            self.ceiling = db_to_amp(db.clamp(-60.0, 0.0));
            self.limiter.set_ceiling_db(db.clamp(-60.0, 0.0));
        }
    }

    /// Input gain before the clip, dB (0..=48).
    pub fn set_drive_db(&mut self, db: f32) {
        if db.is_finite() {
            self.drive = db_to_amp(db.clamp(0.0, 48.0));
        }
    }

    /// Knee width in dB (0 = hard clip, up to 24).
    pub fn set_knee_db(&mut self, db: f32) {
        if db.is_finite() {
            self.knee_db = db.clamp(0.0, 24.0);
        }
    }

    /// Extra margin in dB (0..=6) by which the oversampled clip stays under the
    /// ceiling, to absorb decimation ringing.
    pub fn set_guard_db(&mut self, db: f32) {
        if db.is_finite() {
            self.guard_db = db.clamp(0.0, 6.0);
        }
    }

    /// The static transfer curve for ceiling `ceiling` (linear), knee `knee_db`.
    #[inline]
    pub fn curve(x: f32, ceiling: f32, knee_db: f32) -> f32 {
        let a = x.abs();
        let y = if knee_db <= 0.001 {
            a.min(ceiling)
        } else {
            let xk = ceiling * db_to_amp(-knee_db);
            if a <= xk {
                a
            } else {
                let w = ceiling - xk;
                xk + w * ((a - xk) / w).tanh()
            }
        };
        y.copysign(x)
    }

    /// Clips `channels` in place. Only the prepared channels are processed.
    pub fn process(&mut self, channels: &mut [&mut [f32]]) {
        let nc = channels.len().min(self.channels);
        if nc == 0 {
            return;
        }
        let frames = channels[..nc].iter().map(|c| c.len()).min().unwrap_or(0);
        let clip_level = self.ceiling * db_to_amp(-self.guard_db);
        let (drive, knee, ceiling) = (self.drive, self.knee_db, self.ceiling);
        let mut start = 0;
        while start < frames {
            let n = (frames - start).min(self.max_block);
            for c in 0..self.channels {
                if c < nc {
                    for (d, s) in self.inp[c][..n].iter_mut().zip(&channels[c][start..start + n]) {
                        *d = if s.is_finite() { *s * drive } else { 0.0 };
                    }
                } else {
                    self.inp[c][..n].fill(0.0);
                }
            }
            {
                let mut ins: [&[f32]; 8] = [&[]; 8];
                let mut outs: [&mut [f32]; 8] = Default::default();
                for (slot, v) in ins.iter_mut().zip(&self.inp) {
                    *slot = &v[..n];
                }
                for (slot, v) in outs.iter_mut().zip(self.out.iter_mut()) {
                    *slot = &mut v[..n];
                }
                let _ = self.os.process(&ins[..self.channels], &mut outs[..self.channels], |frame| {
                    for s in frame.iter_mut() {
                        *s = Self::curve(*s, clip_level, knee);
                    }
                });
            }
            for c in 0..nc {
                for (d, s) in channels[c][start..start + n].iter_mut().zip(&self.out[c][..n]) {
                    *d = if s.is_finite() { *s } else { 0.0 };
                }
            }
            {
                let mut parts: [&mut [f32]; 8] = Default::default();
                for (slot, ch) in parts.iter_mut().zip(channels.iter_mut()).take(nc) {
                    *slot = &mut ch[start..start + n];
                }
                self.limiter.process(&mut parts[..nc]);
            }
            for c in 0..nc {
                for d in channels[c][start..start + n].iter_mut() {
                    *d = d.clamp(-ceiling, ceiling);
                }
            }
            start += n;
        }
    }
}
