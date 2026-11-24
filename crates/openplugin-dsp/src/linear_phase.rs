//! Linear-phase 1x / 2x / 4x oversampling.
//!
//! The up/down path is a Blackman-windowed sinc at the original
//! Nyquist, prepared outside the callback. At 2x and 4x the combined latency
//! is 64 host samples. At 1x it is 0. A dry or bypass path has to delay by
//! [`LinearPhaseOversampler::latency_samples`] or the mix will comb-filter.
//!
//! `prepare` allocates. `process` does not, and it checks the buffer shape
//! before it touches filter state.

use std::f64::consts::PI;

/// Why [`LinearPhaseOversampler::prepare`] or `process` refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OversampleError {
    /// The factor is not 1, 2 or 4.
    Factor,
    /// The channel count is not 1 to 8.
    Channels,
    /// The block size is 0 or above 1 048 576 at prepare time, or a block is
    /// longer than the prepared one.
    BlockSize,
    /// The buffers do not match the prepared channel count, or their lengths
    /// differ.
    BufferShape,
}

/// Linear-phase 1x, 2x or 4x oversampling around a per-sample closure; see the
/// module documentation. `prepare` allocates; `process` does not.
///
/// # Example
///
/// ```
/// use openplugin_dsp::LinearPhaseOversampler;
///
/// let mut os = LinearPhaseOversampler::prepare(2, 2, 512).unwrap();
/// assert_eq!(os.latency_samples(), 64);
///
/// let input = [vec![0.5f32; 512], vec![0.5f32; 512]];
/// let mut left = vec![0.0f32; 512];
/// let mut right = vec![0.0f32; 512];
/// let inputs: [&[f32]; 2] = [&input[0], &input[1]];
/// os.process(&inputs, &mut [&mut left, &mut right], |frame| {
///     for s in frame.iter_mut() {
///         *s = s.tanh(); // the non-linearity runs at 96 kHz
///     }
/// })
/// .unwrap();
/// assert!((left[511] - 0.5f32.tanh()).abs() < 1e-3);
/// ```
pub struct LinearPhaseOversampler {
    factor: usize,
    channels: usize,
    max_block: usize,
    taps: Vec<f64>,
    /// `phases[p]` = `taps[p], taps[p + factor], ...` times `factor`: the up
    /// side only meets the non-zero (every `factor`-th) samples of the stuffed signal.
    phases: Vec<Vec<f64>>,
    /// Base-rate input per channel, twice the polyphase length so the newest
    /// `phases[0].len()` samples are one slice.
    up: Vec<Vec<f64>>,
    /// Oversampled output per channel, twice the tap count, same layout.
    down: Vec<Vec<f64>>,
    up_position: usize,
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
        let phases: Vec<Vec<f64>> = (0..factor)
            .map(|phase| {
                taps.iter()
                    .skip(phase)
                    .step_by(factor)
                    .map(|tap| tap * factor as f64)
                    .collect()
            })
            .collect();
        Ok(Self {
            factor,
            channels,
            max_block,
            up: vec![vec![0.0; 2 * phases[0].len()]; channels],
            down: vec![vec![0.0; 2 * taps.len()]; channels],
            taps,
            phases,
            up_position: 0,
            position: 0,
            frame: vec![0.0; channels],
            frame_f32: vec![0.0; channels],
        })
    }

    /// The oversampling factor, 1, 2 or 4.
    pub fn factor(&self) -> usize {
        self.factor
    }

    /// Latency at the host rate in samples: 64 at 2x and 4x, 0 at 1x. Report it
    /// and delay any dry path by it.
    pub fn latency_samples(&self) -> usize {
        if self.factor == 1 {
            0
        } else {
            (self.taps.len() - 1) / self.factor
        }
    }

    /// Clears the filter history. Does not allocate.
    pub fn reset(&mut self) {
        for ring in self.up.iter_mut().chain(self.down.iter_mut()) {
            ring.fill(0.0);
        }
        self.frame.fill(0.0);
        self.frame_f32.fill(0.0);
        self.up_position = 0;
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
        if self.factor == 1 {
            for index in 0..frames {
                for (slot, source) in self.frame_f32.iter_mut().zip(input.iter()) {
                    *slot = source[index];
                }
                processor(&mut self.frame_f32);
                for (slot, dest) in self.frame_f32.iter().zip(output.iter_mut()) {
                    dest[index] = *slot;
                }
            }
            return Ok(());
        }
        let span = self.phases[0].len();
        let taps = self.taps.len();
        for index in 0..frames {
            for (channel, source) in input.iter().enumerate() {
                let ring = &mut self.up[channel];
                let sample = f64::from(source[index]);
                ring[self.up_position] = sample;
                ring[self.up_position + span] = sample;
            }
            self.up_position = if self.up_position + 1 == span { 0 } else { self.up_position + 1 };
            for phase in 0..self.factor {
                for (channel, frame) in self.frame.iter_mut().enumerate() {
                    let history = &self.up[channel][self.up_position..self.up_position + span];
                    *frame = convolve(&self.phases[phase], history);
                }
                for (slot, sample) in self.frame_f32.iter_mut().zip(self.frame.iter()) {
                    *slot = *sample as f32;
                }
                processor(&mut self.frame_f32);
                for (channel, dest) in output.iter_mut().enumerate() {
                    let ring = &mut self.down[channel];
                    let sample = f64::from(self.frame_f32[channel]);
                    ring[self.position] = sample;
                    ring[self.position + taps] = sample;
                    if phase == 0 {
                        let history = &ring[self.position + 1..self.position + 1 + taps];
                        dest[index] = convolve(&self.taps, history) as f32;
                    }
                }
                self.position = if self.position + 1 == taps { 0 } else { self.position + 1 };
            }
        }
        Ok(())
    }
}

/// `sum taps[k] * x[n - k]`, with `history` oldest first and newest last; a
/// shorter `taps` meets only the newest samples.
fn convolve(taps: &[f64], history: &[f64]) -> f64 {
    let mut sum = 0.0;
    for (tap, sample) in taps.iter().zip(history.iter().rev()) {
        sum += tap * sample;
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

    /// The direct form this module ran before the polyphase rewrite: every
    /// tap against the zero-stuffed ring, with a modulo per tap.
    fn reference(factor: usize, input: &[f32], drive: f32) -> Vec<f32> {
        let taps = coefficients(factor).unwrap();
        let n = taps.len();
        let ring_sum = |ring: &[f64], position: usize| -> f64 {
            taps.iter()
                .enumerate()
                .map(|(k, tap)| tap * ring[(position + n - k) % n])
                .fold(0.0, |sum, term| sum + term)
        };
        let (mut up, mut down, mut position) = (vec![0.0; n], vec![0.0; n], 0);
        let mut output = Vec::new();
        for &x in input {
            for phase in 0..factor {
                up[position] = if phase == 0 { f64::from(x) * factor as f64 } else { 0.0 };
                let y = (ring_sum(&up, position) as f32 * drive).tanh();
                down[position] = f64::from(y);
                if phase == 0 {
                    output.push(ring_sum(&down, position) as f32);
                }
                position = (position + 1) % n;
            }
        }
        output
    }

    #[test]
    fn polyphase_matches_the_direct_form() {
        let input: Vec<f32> = (0..3000).map(|i| (i as f32 * 0.0731).sin() * 0.9).collect();
        for factor in [2, 4] {
            let want = reference(factor, &input, 3.0);
            let mut oversampler = LinearPhaseOversampler::prepare(factor, 2, 700).unwrap();
            let mut got = [vec![0.0f32; input.len()], vec![0.0f32; input.len()]];
            for (start, end) in [(0, 1), (1, 700), (700, 1337), (1337, 2000), (2000, 2700), (2700, 3000)] {
                let [left, right] = &mut got;
                let chunk = &input[start..end];
                oversampler
                    .process(&[chunk, chunk], &mut [&mut left[start..end], &mut right[start..end]], |frame| {
                        for sample in frame.iter_mut() {
                            *sample = (*sample * 3.0).tanh();
                        }
                    })
                    .unwrap();
            }
            // Same terms in the same order (the skipped ones are exact zeros), so the bound is 0.
            for channel in &got {
                let worst = channel.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
                assert!(worst <= 0.0, "factor {factor}: {worst}");
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
