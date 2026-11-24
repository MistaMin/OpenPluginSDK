//! Fixed delay line for dry-path alignment.
//!
//! A dry, bypass or wet/dry path has to be delayed by the latency the plugin
//! reports, or the mix comb-filters. The buffer is allocated in [`DelayLine::new`]. `process_sample`
//! does not allocate. A delay of 0 returns the input.

#[derive(Clone, Debug)]
/// A delay of a whole number of samples, for one channel.
///
/// # Example
///
/// ```
/// use openplugin_dsp::DelayLine;
///
/// let mut line = DelayLine::new(8);
/// let out: Vec<f32> = [1.0, 2.0, 3.0, 4.0]
///     .iter()
///     .map(|&x| line.process_sample(x, 2))
///     .collect();
/// assert_eq!(out, [0.0, 0.0, 1.0, 2.0]);
/// ```
pub struct DelayLine {
    buf: Vec<f32>,
    write: usize,
}

impl DelayLine {
    /// `max_delay` is the largest delay, in samples, that `process_sample` will honour.
    pub fn new(max_delay: usize) -> Self {
        Self {
            buf: vec![0.0; max_delay.max(1)],
            write: 0,
        }
    }

    /// The largest delay in samples (at least 1).
    pub fn max_delay(&self) -> usize {
        self.buf.len()
    }

    /// Fills the line with silence.
    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
    }

    #[inline]
    /// Writes `input` and returns the sample written `delay` samples ago.
    /// `delay` is clamped to [`max_delay`](Self::max_delay); 0 returns `input`.
    /// A non-finite `input` is stored as `0.0`. Does not allocate.
    pub fn process_sample(&mut self, input: f32, delay: usize) -> f32 {
        let length = self.buf.len();
        let delay = delay.min(length);
        let sample = if input.is_finite() { input } else { 0.0 };
        let output = if delay == 0 {
            sample
        } else {
            let read = (self.write + length - delay) % length;
            self.buf[read]
        };
        self.buf[self.write] = sample;
        self.write = (self.write + 1) % length;
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_lands_on_the_requested_delay() {
        let mut delay = DelayLine::new(64);
        assert_eq!(delay.process_sample(1.0, 0), 1.0);
        delay.reset();
        let mut response = Vec::new();
        response.push(delay.process_sample(1.0, 4));
        for _ in 0..8 {
            response.push(delay.process_sample(0.0, 4));
        }
        assert_eq!(response.iter().position(|sample| *sample == 1.0), Some(4));
        delay.reset();
        assert_eq!(delay.process_sample(0.5, 4), 0.0);
    }
}
