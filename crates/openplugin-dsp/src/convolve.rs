//! Overlap-save convolution for a real impulse of at most 1024 taps.
//!
//! Playback is one 2048-point transform per chunk, planned by `rustfft` so the
//! butterflies use Neon on Apple Silicon. The inverse transform is divided by
//! 2048 here. `rustfft` leaves that scale to the caller.
//!
//! A call shorter than 48 samples (`DIRECT_BELOW`) convolves directly against a
//! time-domain copy of the impulse instead, because the two transforms cost the
//! same whatever the call length: per-sample or split-block calls stay cheap.
//!
//! `new` and `set_kernel` allocate. `process` does not.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use crate::simd::Kernels;
use std::sync::Arc;

const FFT: usize = 2048;
const IMPULSE: usize = 1024;
const SCALE: f32 = 1.0 / FFT as f32;
/// Calls shorter than this run the direct form (measured crossover, Apple Silicon, 1024 taps).
const DIRECT_BELOW: usize = 48;

impl std::fmt::Debug for BlockConvolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BlockConvolver")
            .field("channels", &self.channels.len())
            .finish()
    }
}

#[derive(Clone)]
struct Channel {
    overlap: [f32; IMPULSE],
    buffer: Vec<Complex<f32>>,
}

/// One shared impulse, one history per channel.
#[derive(Clone)]
///
/// # Example
///
/// ```
/// use openplugin_dsp::BlockConvolver;
///
/// let mut convolver = BlockConvolver::new(1);
/// convolver.set_kernel(&[0.5]); // off the audio thread: it allocates
///
/// let mut block = [1.0f32; 64];
/// convolver.process(0, &mut block);
/// assert!(block.iter().all(|s| (s - 0.5).abs() < 1e-4));
/// ```
pub struct BlockConvolver {
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    kernel: Vec<Complex<f32>>,
    /// The impulse reversed, for the direct form.
    taps_rev: Vec<f32>,
    /// Impulse-length history then the call's samples, for the direct form.
    line: Vec<f32>,
    kernels: Kernels,
    channels: Vec<Channel>,
    scratch: Vec<Complex<f32>>,
}

impl BlockConvolver {
    /// A convolver for `channels` channels (at least 1) with an all-zero impulse,
    /// so it outputs silence until [`set_kernel`](Self::set_kernel). Allocates and
    /// plans the transforms: call it off the audio thread.
    pub fn new(channels: usize) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(FFT);
        let inverse = planner.plan_fft_inverse(FFT);
        let scratch_len = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());
        let channels = (0..channels.max(1))
            .map(|_| Channel {
                overlap: [0.0; IMPULSE],
                buffer: vec![Complex::default(); FFT],
            })
            .collect();
        Self {
            forward,
            inverse,
            kernel: vec![Complex::default(); FFT],
            taps_rev: Vec::new(),
            line: vec![0.0; IMPULSE + DIRECT_BELOW],
            kernels: Kernels::detect(),
            channels,
            scratch: vec![Complex::default(); scratch_len],
        }
    }

    /// Number of channels.
    pub fn channels(&self) -> usize {
        self.channels.len()
    }

    /// Clears every channel's history. The impulse is kept.
    pub fn reset(&mut self) {
        for channel in &mut self.channels {
            channel.overlap = [0.0; IMPULSE];
        }
    }

    /// Replace the impulse. `fir[0]` multiplies the current sample. Longer than 1024 taps is truncated.
    pub fn set_kernel(&mut self, fir: &[f32]) {
        for bin in &mut self.kernel {
            *bin = Complex::default();
        }
        for (index, coeff) in fir.iter().take(IMPULSE).enumerate() {
            self.kernel[index].re = *coeff;
        }
        self.taps_rev = fir.iter().take(IMPULSE).rev().copied().collect();
        self.forward
            .process_with_scratch(&mut self.kernel, &mut self.scratch);
    }

    /// Filter `samples` in place. Chunks longer than 1024 are split. No extra latency.
    pub fn process(&mut self, channel: usize, samples: &mut [f32]) {
        if channel >= self.channels.len() || samples.is_empty() {
            return;
        }
        let mut offset = 0;
        while offset < samples.len() {
            let count = (samples.len() - offset).min(IMPULSE);
            if count < DIRECT_BELOW {
                self.direct(channel, &mut samples[offset..offset + count]);
            } else {
                self.chunk(channel, &mut samples[offset..offset + count]);
            }
            offset += count;
        }
    }

    fn direct(&mut self, channel: usize, samples: &mut [f32]) {
        let count = samples.len();
        let state = &mut self.channels[channel];
        let history = self.taps_rev.len().saturating_sub(1);
        self.line[..IMPULSE].copy_from_slice(&state.overlap);
        self.line[IMPULSE..IMPULSE + count].copy_from_slice(samples);
        state.overlap.copy_within(count..IMPULSE, 0);
        state.overlap[IMPULSE - count..].copy_from_slice(samples);
        let x = &self.line[IMPULSE - history..IMPULSE + count];
        self.kernels.fir(samples, x, &self.taps_rev);
    }

    fn chunk(&mut self, channel: usize, samples: &mut [f32]) {
        let count = samples.len();
        let forward = &self.forward;
        let inverse = &self.inverse;
        let kernel = &self.kernel;
        let scratch = &mut self.scratch;
        let state = &mut self.channels[channel];

        for bin in &mut state.buffer {
            *bin = Complex::default();
        }
        for index in 0..IMPULSE {
            state.buffer[index].re = state.overlap[index];
        }
        for (index, sample) in samples.iter().enumerate() {
            state.buffer[IMPULSE + index].re = *sample;
        }
        if count == IMPULSE {
            state.overlap.copy_from_slice(samples);
        } else {
            state.overlap.copy_within(count..IMPULSE, 0);
            state.overlap[IMPULSE - count..].copy_from_slice(samples);
        }

        forward.process_with_scratch(&mut state.buffer, scratch);
        for (bin, coeff) in state.buffer.iter_mut().zip(kernel.iter()) {
            *bin *= *coeff;
        }
        inverse.process_with_scratch(&mut state.buffer, scratch);
        for (sample, bin) in samples.iter_mut().zip(state.buffer[IMPULSE..IMPULSE + count].iter()) {
            *sample = bin.re * SCALE;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn direct(fir: &[f32], input: &[f32]) -> Vec<f32> {
        let mut delay = vec![0.0f32; fir.len()];
        let mut pos = 0usize;
        let mut output = Vec::with_capacity(input.len());
        for &sample in input {
            delay[pos] = sample;
            let mut acc = 0.0f32;
            for (tap, coeff) in fir.iter().enumerate() {
                let index = (pos + fir.len() - tap) % fir.len();
                acc += *coeff * delay[index];
            }
            pos = (pos + 1) % fir.len();
            output.push(acc);
        }
        output
    }

    #[test]
    fn overlap_save_matches_a_direct_1024_walk() {
        let mut fir = vec![0.0f32; 1024];
        fir[0] = 0.4;
        fir[3] = -0.2;
        fir[511] = 0.15;
        fir[512] = 0.7;
        fir[1000] = -0.05;
        let mut input = vec![0.0f32; 3000];
        input[0] = 1.0;
        input[40] = -0.5;
        input[512] = 0.25;
        input[2000] = 0.8;
        for (index, sample) in input.iter_mut().enumerate().skip(10) {
            if index % 17 == 0 {
                *sample += 0.1;
            }
        }
        let expected = direct(&fir, &input);
        let mut convolver = BlockConvolver::new(1);
        convolver.set_kernel(&fir);
        let mut blocks = input.clone();
        // Uneven block sizes, the way a host actually calls process.
        let mut offset = 0;
        for size in [512, 64, 1024, 137, 700, 563] {
            if offset >= blocks.len() {
                break;
            }
            let end = (offset + size).min(blocks.len());
            convolver.process(0, &mut blocks[offset..end]);
            offset = end;
        }
        let mut worst = 0.0f32;
        for (got, want) in blocks.iter().zip(expected.iter()) {
            worst = worst.max((got - want).abs());
        }
        assert!(worst < 2.0e-4, "overlap-save drifted by {worst}");
    }

    #[test]
    fn short_calls_take_the_direct_form_and_agree() {
        let fir: Vec<f32> = (0..1024).map(|i| ((i * 37 % 101) as f32 - 50.0) * 1e-3).collect();
        let input: Vec<f32> = (0..6000).map(|i| (i as f32 * 0.31).sin() * 0.7).collect();
        let expected = direct(&fir, &input);
        let mut convolver = BlockConvolver::new(1);
        convolver.set_kernel(&fir);
        let mut blocks = input.clone();
        // Mixed short (direct) and long (FFT) calls share one history.
        let sizes = [1, 16, 47, 48, 1, 1024, 3, 600, 2000];
        let (mut offset, mut i) = (0, 0);
        while offset < blocks.len() {
            let end = (offset + sizes[i % sizes.len()]).min(blocks.len());
            convolver.process(0, &mut blocks[offset..end]);
            offset = end;
            i += 1;
        }
        let worst = blocks.iter().zip(&expected).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(worst < 2.0e-4, "direct and overlap-save disagree by {worst}");
        // A short kernel: the direct form only reads the history it needs.
        let mut short = BlockConvolver::new(1);
        short.set_kernel(&[0.5, -0.25]);
        let mut x = [1.0, 0.0, 0.0];
        short.process(0, &mut x);
        assert_eq!(x, [0.5, -0.25, 0.0]);
    }
}
