//! Hann weight and a rectangular RMS window.
//!
//! `dasp_window` and `dasp_rms` (MIT OR Apache-2.0) are these two shapes.
//! The RMS window is allocated in [`WindowRms::new`]. `push` does not allocate.
//! This is a sliding average of squares, not the ballistic meter in `MeterBank`.
#![allow(deprecated)] // this module defines the deprecated items

/// Raised cosine. `phase` is the 0..1 position in the window.
#[deprecated(since = "1.0.0", note = "use `WindowKind::Hann`")]
pub fn hann(phase: f32) -> f32 {
    if !phase.is_finite() {
        return 0.0;
    }
    let phase = phase.clamp(0.0, 1.0);
    0.5 * (1.0 - (phase * std::f32::consts::TAU).cos())
}

#[derive(Clone, Debug)]
pub struct WindowRms {
    squares: Vec<f32>,
    index: usize,
    // f64: an f32 running sum keeps rounding residue after loud material.
    sum: f64,
}

impl WindowRms {
    pub fn new(window: usize) -> Self {
        Self {
            squares: vec![0.0; window.max(1)],
            index: 0,
            sum: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.squares.fill(0.0);
        self.index = 0;
        self.sum = 0.0;
    }

    /// Push one sample and return the window RMS.
    pub fn push(&mut self, sample: f32) -> f32 {
        let sample = if sample.is_finite() { sample } else { 0.0 };
        let square = sample * sample;
        let evicted = self.squares[self.index];
        self.sum = (self.sum - f64::from(evicted) + f64::from(square)).max(0.0);
        self.squares[self.index] = square;
        self.index += 1;
        if self.index == self.squares.len() {
            self.index = 0;
        }
        (self.sum / self.squares.len() as f64).sqrt() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hann_is_zero_at_the_ends_and_one_in_the_middle() {
        assert!(hann(0.0).abs() < 1e-5);
        assert!(hann(1.0).abs() < 1e-5);
        assert!((hann(0.5) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn constant_signal_rms_settles_on_its_magnitude() {
        let mut rms = WindowRms::new(4);
        let mut last = 0.0;
        for _ in 0..4 {
            last = rms.push(0.5);
        }
        assert!((last - 0.5).abs() < 1e-5);
    }

    #[test]
    fn silence_after_ten_minutes_of_loud_noise_reads_near_silence() {
        // With an f32 sum this read -66.7 dBFS instead of silence.
        let rate = 48_000;
        let mut rms = WindowRms::new(rate);
        let mut seed = 1u32;
        for _ in 0..rate * 600 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            rms.push((seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0);
        }
        let mut last = 1.0;
        for _ in 0..rate * 2 {
            last = rms.push(0.0);
        }
        assert!(last < 1e-6, "{} dBFS", 20.0 * last.log10());
    }
}
