//! Metering primitives: peak, RMS, VU, and the Water Channel meter bank.
//!
//! [`MeterBank`] is the stereo quasi-peak / VU / RMS / gain-reduction bank from
//! Water Channel. 0 VU is a steady -18 dBFS sine (peak-amplitude convention).
//! These are engineering ballistics, not an IEC or ANSI certification.
//! Gain reduction must come from the compressor envelope. It is not estimated
//! from the input/output ratio.

#[derive(Debug, Clone)]
pub struct VuMeter {
    sample_rate: f32,
    envelope: f32,
}

impl VuMeter {
    pub fn new() -> Self {
        Self {
            sample_rate: 44100.0,
            envelope: 0.0,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr.max(1000.0);
    }

    pub fn reset(&mut self) {
        self.envelope = 0.0;
    }

    /// Feeds samples and updates needle position according to ANSI C16.5 VU standard
    /// (300 ms integration time).
    pub fn process_samples(&mut self, samples: &[f32]) -> f32 {
        let coef = (-1.0 / (0.300 * self.sample_rate)).exp();
        for &s in samples {
            let rect = s.abs();
            self.envelope = rect + coef * (self.envelope - rect);
        }
        self.envelope
    }

    /// Returns reading in dB (-inf to +3 dB typical scale).
    pub fn level_db(&self) -> f32 {
        if self.envelope <= 1e-5 {
            -100.0
        } else {
            20.0 * self.envelope.log10()
        }
    }
}

#[derive(Clone, Copy, Default)]
struct ChannelMeter {
    ppm: f64,
    average: f64,
    peak: f64,
    hold: u32,
    power: f64,
}

/// One stereo frame of input, output, and gain-reduction readings, in dB.
#[derive(Clone, Copy, Debug)]
pub struct MeterReadings {
    pub input_peak: [f64; 2],
    pub output_peak: [f64; 2],
    pub input_rms: [f64; 2],
    pub output_rms: [f64; 2],
    pub input_ppm: [f64; 2],
    pub output_ppm: [f64; 2],
    pub input_vu: [f64; 2],
    pub output_vu: [f64; 2],
    pub reduction_ppm: f64,
    pub reduction_vu: f64,
}

/// Stereo peak, RMS, PPM, and VU for input and output, plus envelope GR.
pub struct MeterBank {
    input: [ChannelMeter; 2],
    output: [ChannelMeter; 2],
    rise: f64,
    fall: f64,
    vu: f64,
    gr_fast: f64,
    gr_average: f64,
    hold_samples: u32,
    peak_decay: f64,
    rms_alpha: f64,
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-6).log10()
}

impl MeterBank {
    pub fn new(sample_rate: u32) -> Option<Self> {
        if !(8_000..=768_000).contains(&sample_rate) {
            return None;
        }
        let fs = f64::from(sample_rate);
        Some(Self {
            input: [ChannelMeter::default(); 2],
            output: [ChannelMeter::default(); 2],
            rise: 1.0 - (-1.0 / (fs * 0.005)).exp(),
            fall: 1.0 - (-1.0 / (fs * 0.65)).exp(),
            vu: 1.0 - (-1.0 / (fs * 0.1)).exp(),
            gr_fast: 0.0,
            gr_average: 0.0,
            hold_samples: sample_rate * 3 / 4,
            peak_decay: (-1.0 / (fs * 0.65)).exp(),
            rms_alpha: 1.0 - (-1.0 / (fs * 0.3)).exp(),
        })
    }

    /// One frame. No allocation. `reduction_db` is positive attenuation.
    pub fn push(&mut self, input: [f32; 2], output: [f32; 2], reduction_db: f32) {
        for (channels, frame) in [(&mut self.input, input), (&mut self.output, output)] {
            for (channel, sample) in channels.iter_mut().zip(frame) {
                let v = if sample.is_finite() {
                    f64::from(sample).abs()
                } else {
                    0.0
                };
                channel.ppm += (v - channel.ppm)
                    * if v > channel.ppm {
                        self.rise
                    } else {
                        self.fall
                    };
                channel.average += (v - channel.average) * self.vu;
                channel.power += (v * v - channel.power) * self.rms_alpha;
                if v >= channel.peak {
                    channel.peak = v;
                    channel.hold = self.hold_samples;
                } else if channel.hold > 0 {
                    channel.hold -= 1;
                } else {
                    channel.peak = (channel.peak * self.peak_decay).max(v);
                }
            }
        }
        let gr = if reduction_db.is_finite() {
            f64::from(reduction_db).clamp(0.0, 120.0)
        } else {
            0.0
        };
        self.gr_fast += (gr - self.gr_fast)
            * if gr > self.gr_fast {
                self.rise
            } else {
                self.fall
            };
        self.gr_average += (gr - self.gr_average) * self.vu;
    }

    pub fn readings(&self) -> MeterReadings {
        let ppm = |x: [ChannelMeter; 2]| x.map(|channel| db(channel.ppm));
        let vu = |x: [ChannelMeter; 2]| {
            x.map(|channel| db(channel.average * std::f64::consts::FRAC_PI_2) + 18.0)
        };
        MeterReadings {
            input_peak: self.input.map(|channel| db(channel.peak)),
            output_peak: self.output.map(|channel| db(channel.peak)),
            input_rms: self.input.map(|channel| db(channel.power.max(0.0).sqrt())),
            output_rms: self.output.map(|channel| db(channel.power.max(0.0).sqrt())),
            input_ppm: ppm(self.input),
            output_ppm: ppm(self.output),
            input_vu: vu(self.input),
            output_vu: vu(self.output),
            reduction_ppm: self.gr_fast,
            reduction_vu: self.gr_average,
        }
    }

    pub fn reset(&mut self) {
        self.input = [ChannelMeter::default(); 2];
        self.output = [ChannelMeter::default(); 2];
        self.gr_fast = 0.0;
        self.gr_average = 0.0;
    }
}

/// Pearson correlation of two equal-length blocks. Silence returns 0.
pub fn correlation(left: &[f32], right: &[f32]) -> f32 {
    let n = left.len().min(right.len());
    if n == 0 {
        return 0.0;
    }
    let mut sum_l = 0.0f64;
    let mut sum_r = 0.0f64;
    let mut sum_ll = 0.0f64;
    let mut sum_rr = 0.0f64;
    let mut sum_lr = 0.0f64;
    let mut count = 0.0f64;
    for index in 0..n {
        let l = f64::from(left[index]);
        let r = f64::from(right[index]);
        if !l.is_finite() || !r.is_finite() {
            continue;
        }
        sum_l += l;
        sum_r += r;
        sum_ll += l * l;
        sum_rr += r * r;
        sum_lr += l * r;
        count += 1.0;
    }
    if count < 2.0 {
        return 0.0;
    }
    let mean_l = sum_l / count;
    let mean_r = sum_r / count;
    let var_l = sum_ll - count * mean_l * mean_l;
    let var_r = sum_rr - count * mean_r * mean_r;
    let cov = sum_lr - count * mean_l * mean_r;
    if var_l <= 1e-18 || var_r <= 1e-18 {
        return 0.0;
    }
    (cov / (var_l * var_r).sqrt()).clamp(-1.0, 1.0) as f32
}

fn db_fs(power: f64) -> f32 {
    if power <= 1e-12 {
        -100.0
    } else {
        (10.0 * power.log10()) as f32
    }
}

fn catmull(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

/// Short-term RMS, ungated integrated RMS, and a 4× cubic sample interpolator.
///
/// Short-term is a 400 ms power average in dBFS. Integrated is the mean square
/// since the last reset, also in dBFS. The cubic peak is not an ITU-R BS.1770
/// true-peak meter.
#[derive(Clone, Debug)]
pub struct BlockLevel {
    short: f64,
    integrated: f64,
    samples: u64,
    alpha: f64,
    history: [[f32; 4]; 2],
    true_peak: [f32; 2],
}

impl BlockLevel {
    pub fn new(sample_rate: f32) -> Self {
        let rate = if sample_rate.is_finite() {
            f64::from(sample_rate.max(8_000.0))
        } else {
            48_000.0
        };
        Self {
            short: 0.0,
            integrated: 0.0,
            samples: 0,
            alpha: 1.0 - (-1.0 / (rate * 0.4)).exp(),
            history: [[0.0; 4]; 2],
            true_peak: [0.0; 2],
        }
    }

    pub fn reset(&mut self) {
        self.short = 0.0;
        self.integrated = 0.0;
        self.samples = 0;
        self.history = [[0.0; 4]; 2];
        self.true_peak = [0.0; 2];
    }

    pub fn push_stereo(&mut self, left: &[f32], right: &[f32]) {
        let n = left.len().max(right.len());
        for index in 0..n {
            let l = left.get(index).copied().unwrap_or(0.0);
            let r = right.get(index).copied().unwrap_or(0.0);
            let l = if l.is_finite() { l } else { 0.0 };
            let r = if r.is_finite() { r } else { 0.0 };
            let power = f64::from(l * l + r * r) * 0.5;
            self.short += (power - self.short) * self.alpha;
            self.samples = self.samples.saturating_add(1);
            let count = self.samples as f64;
            self.integrated += (power - self.integrated) / count;
            self.observe(0, l);
            self.observe(1, r);
        }
    }

    fn observe(&mut self, channel: usize, sample: f32) {
        let history = &mut self.history[channel];
        history[0] = history[1];
        history[1] = history[2];
        history[2] = history[3];
        history[3] = sample;
        let mut peak = sample.abs();
        for step in [0.25, 0.5, 0.75] {
            let value = catmull(history[0], history[1], history[2], history[3], step).abs();
            if value > peak {
                peak = value;
            }
        }
        if peak > self.true_peak[channel] {
            self.true_peak[channel] = peak;
        }
    }

    pub fn short_db(&self) -> f32 {
        db_fs(self.short)
    }

    pub fn integrated_db(&self) -> f32 {
        db_fs(self.integrated)
    }

    pub fn true_peak_db(&self) -> [f32; 2] {
        self.true_peak.map(|peak| {
            if peak <= 1e-6 {
                -100.0
            } else {
                20.0 * peak.log10()
            }
        })
    }
}

#[cfg(test)]
mod level_tests {
    use super::*;

    #[test]
    fn identical_channels_correlate_and_opposites_do_not() {
        let left = [0.1, -0.2, 0.3, -0.4, 0.2];
        assert!((correlation(&left, &left) - 1.0).abs() < 1e-5);
        let right = left.map(|sample| -sample);
        assert!((correlation(&left, &right) + 1.0).abs() < 1e-5);
        assert_eq!(correlation(&[0.0; 8], &[0.0; 8]), 0.0);
    }

    #[test]
    fn cubic_peak_is_at_least_the_sample_peak() {
        let mut level = BlockLevel::new(48_000.0);
        let left = [0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
        level.push_stereo(&left, &[0.0; 6]);
        assert!(level.true_peak_db()[0] >= -0.1);
    }
}

#[cfg(test)]
mod bank_tests {
    use super::*;

    #[test]
    fn no_signal_is_finite_and_reset_clears_reduction() {
        let mut meters = MeterBank::new(48_000).unwrap();
        meters.push([f32::NAN, f32::INFINITY], [0.0; 2], f32::NAN);
        assert_eq!(meters.readings().input_ppm, [-120.0; 2]);
        meters.push([1.0; 2], [1.0; 2], 8.0);
        meters.reset();
        assert_eq!(meters.readings().reduction_ppm, 0.0);
        assert!(MeterBank::new(0).is_none());
    }

    #[test]
    fn vu_sine_reference_and_channels_stay_independent() {
        let mut meters = MeterBank::new(48_000).unwrap();
        let amplitude = 10_f64.powf(-18.0 / 20.0);
        for n in 0..96_000 {
            let x = (amplitude * (std::f64::consts::TAU * 1000.0 * f64::from(n) / 48_000.0).sin())
                as f32;
            meters.push([x, 0.0], [x * 0.5, 0.0], 6.0);
        }
        let readings = meters.readings();
        assert!(readings.input_vu[0].abs() < 0.08);
        assert!((readings.output_vu[0] + 6.0206).abs() < 0.08);
        assert_eq!(readings.input_ppm[1], -120.0);
        assert!((readings.reduction_vu - 6.0).abs() < 1e-6);
    }

    #[test]
    fn steady_sine_rms_is_three_db_below_peak() {
        let mut meters = MeterBank::new(48_000).unwrap();
        for n in 0..144_000 {
            let x = (0.5 * (std::f64::consts::TAU * 1000.0 * f64::from(n) / 48_000.0).sin()) as f32;
            meters.push([x, 0.0], [x * 0.5, 0.0], 0.0);
        }
        let readings = meters.readings();
        assert!((readings.input_peak[0] - readings.input_rms[0] - 3.0103).abs() < 0.02);
        assert!((readings.input_rms[0] - readings.output_rms[0] - 6.0206).abs() < 0.02);
    }
}
