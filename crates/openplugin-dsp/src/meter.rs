//! Metering primitives: peak, RMS, VU, and a stereo meter bank.
//!
//! [`MeterBank`] is a stereo quasi-peak / VU / RMS / gain-reduction bank.
//! 0 VU is a steady -18 dBFS sine (peak-amplitude convention).
//! These are engineering ballistics, not an IEC or ANSI certification.
//! [`VuMeter`] is a one-pole 300 ms approximation and does not meet the
//! 300 ms-to-99 % rise of IEC 60268-17; [`VuIec`] does (second-order needle).
//! Gain reduction must come from the compressor envelope. It is not estimated
//! from the input/output ratio.
#![allow(deprecated)] // this module defines the deprecated items

/// A one-pole VU approximation with a 300 ms time constant (see the module
/// documentation for why it is deprecated). Plain state, no allocation.
#[derive(Debug, Clone)]
#[deprecated(since = "1.0.0", note = "a one-pole approximation; use `VuIec`, which meets IEC 60268-17")]
pub struct VuMeter {
    sample_rate: f32,
    envelope: f32,
}

impl Default for VuMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl VuMeter {
    /// A meter at rest, at 44.1 kHz until [`Self::set_sample_rate`] is called.
    pub fn new() -> Self {
        Self {
            sample_rate: 44100.0,
            envelope: 0.0,
        }
    }

    /// Sets the sample rate in Hz; values below 1 kHz are raised to 1 kHz.
    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr.max(1000.0);
    }

    /// Returns the needle to rest.
    pub fn reset(&mut self) {
        self.envelope = 0.0;
    }

    /// Feeds samples and updates needle position according to ANSI C16.5 VU standard
    /// (300 ms integration time).
    pub fn process_samples(&mut self, samples: &[f32]) -> f32 {
        let coef = (-1.0 / (0.300 * self.sample_rate)).exp();
        for &s in samples {
            // A non-finite sample would leave the envelope NaN for good.
            if s.is_finite() {
                let rect = s.abs();
                self.envelope = rect + coef * (self.envelope - rect);
            }
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

#[cfg(test)]
mod vu_meter_tests {
    use super::*;

    #[test]
    fn non_finite_samples_do_not_latch() {
        let mut m = VuMeter::new();
        m.process_samples(&[0.5, f32::NAN, f32::INFINITY, 0.5]);
        assert!(m.level_db().is_finite());
        let mut clean = VuMeter::new();
        clean.process_samples(&[0.5, 0.5]);
        assert_eq!(m.level_db(), clean.level_db());
    }
}

/// A VU meter after IEC 60268-17 (and ANSI C16.5): the needle is a
/// second-order mechanical system driven by the rectified signal.
///
/// The standard asks that a steady tone starting from rest bring the needle
/// to 99 % of its final reading in 300 ms, with an overshoot of 1 % to
/// 1.5 %. [`VuMeter`] above is a one-pole of 300 ms time constant, which
/// takes about 1.4 s to reach 99 % and has no overshoot, so it does not meet
/// that; it is kept as it was. `VuIec` models the needle as
/// `H(s) = wn^2 / (s^2 + 2 z wn s + wn^2)` with damping `z = 0.8107`
/// (overshoot `exp(-pi z / sqrt(1 - z^2))` = 1.3 %) and `wn` set at
/// construction so the step response first crosses 99 % at 300 ms. It is
/// discretised with the bilinear transform. The input is the full-wave
/// rectified (average-responding) signal scaled by `pi / (2 sqrt 2)`, so a
/// steady sine reads its RMS, the VU convention.
/// Plain state, no allocation; one per channel, owned by the audio thread.
///
/// # Example
///
/// ```
/// use openplugin_dsp::VuIec;
///
/// let rate = 48_000.0;
/// let mut vu = VuIec::new(rate);
/// // One second of a full-scale sine: the needle settles at its RMS, -3 dB.
/// let sine: Vec<f32> = (0..48_000)
///     .map(|n| (2.0 * std::f32::consts::PI * 1_000.0 * n as f32 / rate).sin())
///     .collect();
/// vu.process(&sine);
/// assert!((vu.level_db() + 3.0).abs() < 0.3);
/// ```
#[derive(Clone, Debug)]
pub struct VuIec {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
    y: f64,
}

/// Time (in units of 1 / wn) at which a second-order step response with
/// damping `zeta` first reaches `fraction` of its final value.
fn second_order_rise(zeta: f64, fraction: f64) -> f64 {
    let wd = (1.0 - zeta * zeta).sqrt();
    let step = |t: f64| 1.0 - (-zeta * t).exp() * ((wd * t).cos() + zeta / wd * (wd * t).sin());
    let (mut lo, mut hi) = (0.0f64, std::f64::consts::PI / wd); // first peak is at pi/wd
    for _ in 0..80 {
        let mid = 0.5 * (lo + hi);
        if step(mid) < fraction {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

impl VuIec {
    /// Damping giving a 1.3 % overshoot.
    pub const DAMPING: f64 = 0.8107;
    /// Seconds to 99 % of a steady tone's reading.
    pub const RISE_SECONDS: f64 = 0.3;

    /// A needle at rest for `sample_rate` in Hz (an out-of-range rate is clamped
    /// to the SDK's range). Does not allocate.
    pub fn new(sample_rate: f32) -> Self {
        let fs = f64::from(crate::rate::effective_rate(sample_rate));
        let zeta = Self::DAMPING;
        let wn = second_order_rise(zeta, 0.99) / Self::RISE_SECONDS;
        // Bilinear: s = 2 fs (1 - z^-1) / (1 + z^-1).
        let k = 2.0 * fs;
        let a0 = k * k + 2.0 * zeta * wn * k + wn * wn;
        let b0 = wn * wn / a0;
        Self {
            b: [b0, 2.0 * b0, b0],
            a: [(2.0 * wn * wn - 2.0 * k * k) / a0, (k * k - 2.0 * zeta * wn * k + wn * wn) / a0],
            z: [0.0; 2],
            y: 0.0,
        }
    }

    /// Returns the needle to rest.
    pub fn reset(&mut self) {
        self.z = [0.0; 2];
        self.y = 0.0;
    }

    /// One sample; returns the needle position (linear, RMS-calibrated).
    #[inline]
    pub fn next(&mut self, x: f32) -> f32 {
        let rect = if x.is_finite() { f64::from(x.abs()) } else { 0.0 } * (std::f64::consts::PI / (2.0 * std::f64::consts::SQRT_2));
        let y = self.b[0] * rect + self.z[0];
        self.z[0] = self.b[1] * rect - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * rect - self.a[1] * y;
        self.y = y;
        y as f32
    }

    /// Feeds a block and returns the needle position after its last sample
    /// (linear, RMS-calibrated). Non-finite samples count as 0. Realtime-safe.
    pub fn process(&mut self, block: &[f32]) -> f32 {
        for &x in block {
            self.next(x);
        }
        self.y as f32
    }

    /// Needle position in dB re full scale, floor -100.
    pub fn level_db(&self) -> f32 {
        if self.y <= 1e-5 {
            -100.0
        } else {
            (20.0 * self.y.log10()) as f32
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
#[non_exhaustive]
pub struct MeterReadings {
    /// Input sample peak per channel (held 750 ms, then a 650 ms decay), dBFS.
    pub input_peak: [f64; 2],
    /// Output sample peak per channel, as `input_peak`, dBFS.
    pub output_peak: [f64; 2],
    /// Input RMS per channel (300 ms one-pole on the power), dBFS.
    pub input_rms: [f64; 2],
    /// Output RMS per channel, dBFS.
    pub output_rms: [f64; 2],
    /// Input quasi-peak per channel (5 ms rise, 650 ms fall), dBFS.
    pub input_ppm: [f64; 2],
    /// Output quasi-peak per channel, dBFS.
    pub output_ppm: [f64; 2],
    /// Input VU per channel, dB with 0 VU at a -18 dBFS sine.
    pub input_vu: [f64; 2],
    /// Output VU per channel, dB with 0 VU at a -18 dBFS sine.
    pub output_vu: [f64; 2],
    /// Gain reduction with PPM ballistics, positive dB of attenuation.
    pub reduction_ppm: f64,
    /// Gain reduction with VU ballistics, positive dB of attenuation.
    pub reduction_vu: f64,
}

/// Stereo peak, RMS, PPM, and VU for input and output, plus envelope GR.
/// Plain state, no allocation; `push` runs per frame on the audio thread.
/// Levels floor at -120 dB.
///
/// # Example
///
/// ```
/// use openplugin_dsp::MeterBank;
///
/// let mut bank = MeterBank::new(48_000).expect("supported rate");
/// for _ in 0..4_800 {
///     bank.push([0.5, 0.5], [0.25, 0.25], 6.0);
/// }
/// let r = bank.readings();
/// assert!((r.input_peak[0] - (-6.02)).abs() < 0.1);
/// assert!(r.reduction_ppm > 5.0);
/// assert!(MeterBank::new(500).is_none()); // below the SDK's rate range
/// ```
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
    /// `None` outside the SDK's rate range, 1 kHz to 768 kHz.
    pub fn new(sample_rate: u32) -> Option<Self> {
        if !openplugin_core::sample_rate_supported(f64::from(sample_rate)) {
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

    /// The current readings of both channels, in dB. Does not allocate.
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

    /// Clears every meter to silence.
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
/// Plain state, no allocation.
///
/// # Example
///
/// ```
/// use openplugin_dsp::BlockLevel;
///
/// let mut level = BlockLevel::new(48_000.0);
/// let block = [0.5f32; 480];
/// level.push_stereo(&block, &block);
/// assert!(level.integrated_db() > -10.0);
/// assert!(level.true_peak_db()[0] <= 0.0);
/// ```
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
    /// A cleared meter for `sample_rate` in Hz (clamped to the SDK's range). Does
    /// not allocate.
    pub fn new(sample_rate: f32) -> Self {
        let rate = f64::from(crate::rate::effective_rate(sample_rate));
        Self {
            short: 0.0,
            integrated: 0.0,
            samples: 0,
            alpha: 1.0 - (-1.0 / (rate * 0.4)).exp(),
            history: [[0.0; 4]; 2],
            true_peak: [0.0; 2],
        }
    }

    /// Clears the short-term, integrated and peak values.
    pub fn reset(&mut self) {
        self.short = 0.0;
        self.integrated = 0.0;
        self.samples = 0;
        self.history = [[0.0; 4]; 2];
        self.true_peak = [0.0; 2];
    }

    /// Feeds a stereo block. Slices of different lengths are padded with zeros
    /// to the longer one; non-finite samples count as 0. Realtime-safe.
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

    /// Short-term (400 ms) level, dBFS.
    pub fn short_db(&self) -> f32 {
        db_fs(self.short)
    }

    /// Ungated mean level since the last reset, dBFS.
    pub fn integrated_db(&self) -> f32 {
        db_fs(self.integrated)
    }

    /// Interpolated (4x cubic) peak per channel since the last reset, dBFS;
    /// -100 when below -120 dBFS.
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

#[cfg(test)]
mod vu_iec_tests {
    use super::*;

    /// IEC 60268-17: 99 % of the final reading in 300 ms, overshoot 1 % to 1.5 %.
    #[test]
    fn needle_rises_in_300_ms_with_about_one_percent_overshoot() {
        for rate in [44_100.0f32, 48_000.0, 96_000.0] {
            let mut vu = VuIec::new(rate);
            let mut peak = 0.0f32;
            let mut t99 = None;
            let total = (rate * 3.0) as usize;
            // A rectified constant is the step the standard describes; the
            // calibration scale pi / (2 sqrt 2) is divided out.
            let drive = (2.0 * std::f64::consts::SQRT_2 / std::f64::consts::PI) as f32;
            let mut last = 0.0;
            for n in 0..total {
                last = vu.next(drive);
                peak = peak.max(last);
                if t99.is_none() && last >= 0.99 {
                    t99 = Some(n as f32 / rate);
                }
            }
            let t99 = t99.expect("never reached 99 %");
            assert!((t99 - 0.3).abs() < 0.01, "{rate} Hz: 99 % at {t99} s");
            assert!((last - 1.0).abs() < 1e-3, "settles at 1.0, got {last}");
            let overshoot = (peak - 1.0) * 100.0;
            assert!((1.0..=1.5).contains(&overshoot), "{rate} Hz: overshoot {overshoot} %");
        }
    }

    #[test]
    fn a_steady_sine_reads_its_rms() {
        let rate = 48_000.0f32;
        let mut vu = VuIec::new(rate);
        let mut last = 0.0;
        for n in 0..(rate as usize * 2) {
            last = vu.next((std::f32::consts::TAU * 1000.0 * n as f32 / rate).sin());
        }
        let rms = 1.0 / std::f32::consts::SQRT_2;
        assert!((last - rms).abs() < 0.005, "{last}");
    }

    /// The legacy meter does not meet the standard; recorded so the reason
    /// for `VuIec` stays on the page.
    #[test]
    fn the_one_pole_vu_meter_is_too_slow_for_iec() {
        let rate = 48_000.0f32;
        let mut vu = VuMeter::new();
        vu.set_sample_rate(rate);
        vu.process_samples(&vec![1.0; (rate * 0.3) as usize]);
        assert!(vu.level_db() < -0.2, "one-pole is at 63 % at 300 ms, not 99 %");
    }
}
