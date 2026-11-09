//! Linear-phase 1x / 2x / 4x oversampling.
//!
//! Water Channel's up/down path: a Blackman-windowed sinc at the original
//! Nyquist, prepared outside the callback. At 2x and 4x the combined latency
//! is 64 host samples. At 1x it is 0. A dry or bypass path has to delay by
//! [`LinearPhaseOversampler::latency_samples`] or the mix will comb-filter.
//!
//! `prepare` allocates. `process` does not, and it checks the buffer shape
//! before it touches filter state.

use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OversampleError {
    Factor,
    Channels,
    BlockSize,
    BufferShape,
}

pub struct LinearPhaseOversampler {
    factor: usize,
    channels: usize,
    max_block: usize,
    taps: Vec<f64>,
    up: Vec<Vec<f64>>,
    down: Vec<Vec<f64>>,
    position: usize,
    frame: Vec<f64>,
    frame_f32: Vec<f32>,
}

impl LinearPhaseOversampler {
    /// `factor` is 1, 2, or 4. `channels` is 1 to 8. `max_block` is the largest
    /// host block `process` will accept.
    pub fn prepare(
        factor: usize,
        channels: usize,
        max_block: usize,
    ) -> Result<Self, OversampleError> {
        if ![1, 2, 4].contains(&factor) {
            return Err(OversampleError::Factor);
        }
        if !(1..=8).contains(&channels) {
            return Err(OversampleError::Channels);
        }
        if max_block == 0 || max_block > 1_048_576 {
            return Err(OversampleError::BlockSize);
        }
        let taps = coefficients(factor)?;
        let rings = vec![vec![0.0; taps.len()]; channels];
        Ok(Self {
            factor,
            channels,
            max_block,
            taps,
            up: rings.clone(),
            down: rings,
            position: 0,
            frame: vec![0.0; channels],
            frame_f32: vec![0.0; channels],
        })
    }

    pub fn factor(&self) -> usize {
        self.factor
    }

    pub fn latency_samples(&self) -> usize {
        if self.factor == 1 {
            0
        } else {
            (self.taps.len() - 1) / self.factor
        }
    }

    pub fn reset(&mut self) {
        for ring in self.up.iter_mut().chain(self.down.iter_mut()) {
            ring.fill(0.0);
        }
        self.frame.fill(0.0);
        self.frame_f32.fill(0.0);
        self.position = 0;
    }

    /// Planar input and output. Every channel must have the same length, and
    /// that length must be at most the prepared block. `processor` sees one
    /// sample of every channel at the internal rate.
    pub fn process<F: FnMut(&mut [f32])>(
        &mut self,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        mut processor: F,
    ) -> Result<(), OversampleError> {
        if input.len() != self.channels || output.len() != self.channels {
            return Err(OversampleError::BufferShape);
        }
        let frames = input[0].len();
        if frames > self.max_block {
            return Err(OversampleError::BlockSize);
        }
        if input.iter().any(|channel| channel.len() != frames)
            || output.iter().any(|channel| channel.len() != frames)
        {
            return Err(OversampleError::BufferShape);
        }
        let taps = self.taps.len();
        for index in 0..frames {
            for phase in 0..self.factor {
                for (channel, source) in input.iter().enumerate() {
                    if self.factor == 1 {
                        self.frame[channel] = f64::from(source[index]);
                    } else {
                        self.up[channel][self.position] = if phase == 0 {
                            f64::from(source[index]) * self.factor as f64
                        } else {
                            0.0
                        };
                        self.frame[channel] =
                            convolve(&self.taps, &self.up[channel], self.position);
                    }
                }
                for (slot, sample) in self.frame_f32.iter_mut().zip(self.frame.iter()) {
                    *slot = *sample as f32;
                }
                processor(&mut self.frame_f32);
                for (channel, sample) in self.frame.iter_mut().zip(self.frame_f32.iter()) {
                    *channel = f64::from(*sample);
                }
                for (channel, dest) in output.iter_mut().enumerate() {
                    if self.factor == 1 {
                        dest[index] = self.frame[channel] as f32;
                    } else {
                        self.down[channel][self.position] = self.frame[channel];
                        if phase == 0 {
                            dest[index] =
                                convolve(&self.taps, &self.down[channel], self.position) as f32;
                        }
                    }
                }
                self.position = (self.position + 1) % taps;
            }
        }
        Ok(())
    }
}

fn convolve(taps: &[f64], ring: &[f64], position: usize) -> f64 {
    let mut sum = 0.0;
    for (k, tap) in taps.iter().enumerate() {
        let index = (position + ring.len() - k) % ring.len();
        sum += tap * ring[index];
    }
    sum
}

fn coefficients(factor: usize) -> Result<Vec<f64>, OversampleError> {
    if ![1, 2, 4].contains(&factor) {
        return Err(OversampleError::Factor);
    }
    if factor == 1 {
        return Ok(vec![1.0]);
    }
    let order = 64 * factor;
    let cutoff = 0.5 / factor as f64;
    let mut coeffs: Vec<f64> = (0..=order)
        .map(|i| {
            let x = i as f64 - order as f64 / 2.0;
            let sinc = if x == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * x).sin() / (PI * x)
            };
            let phase = 2.0 * PI * i as f64 / order as f64;
            sinc * (0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos())
        })
        .collect();
    let sum: f64 = coeffs.iter().sum();
    for coeff in &mut coeffs {
        *coeff /= sum;
    }
    Ok(coeffs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(factor: usize, input: &[f32], blocks: &[usize]) -> Vec<f32> {
        let mut oversampler = LinearPhaseOversampler::prepare(factor, 1, input.len()).unwrap();
        let mut output = vec![0.0; input.len()];
        let mut start = 0;
        for &count in blocks {
            oversampler
                .process(
                    &[&input[start..start + count]],
                    &mut [&mut output[start..start + count]],
                    |_| {},
                )
                .unwrap();
            start += count;
        }
        assert_eq!(start, input.len());
        output
    }

    #[test]
    fn block_splits_match_and_the_impulse_lands_on_the_latency() {
        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.137).sin()).collect();
        for factor in [1, 2, 4] {
            let whole = run(factor, &input, &[1000]);
            let chunks = run(factor, &input, &[1, 0, 63, 128, 7, 256, 545]);
            for (left, right) in whole.iter().zip(chunks.iter()) {
                assert!((left - right).abs() < 1e-5, "{left} vs {right}");
            }
            assert!(whole.iter().all(|sample| sample.is_finite()));
            let mut impulse = vec![0.0; 512];
            impulse[0] = 1.0;
            let response = run(factor, &impulse, &[512]);
            let peak = response
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .unwrap()
                .0;
            let oversampler = LinearPhaseOversampler::prepare(factor, 1, 512).unwrap();
            assert_eq!(peak, oversampler.latency_samples());
            if factor != 1 {
                assert_eq!(oversampler.latency_samples(), 64);
            }
        }
    }

    #[test]
    fn bad_shapes_do_not_change_state() {
        let mut oversampler = LinearPhaseOversampler::prepare(2, 1, 8).unwrap();
        let input = [0.5f32; 4];
        let mut output = [0.0f32; 4];
        assert_eq!(
            oversampler.process(&[&input], &mut [&mut output], |_| {}),
            Ok(())
        );
        let position = oversampler.position;
        assert_eq!(
            oversampler.process(&[&input, &input], &mut [&mut output], |_| {}),
            Err(OversampleError::BufferShape)
        );
        assert_eq!(oversampler.position, position);
        assert!(LinearPhaseOversampler::prepare(3, 1, 8).is_err());
    }
}
