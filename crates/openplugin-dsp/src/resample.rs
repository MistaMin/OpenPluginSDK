//! Windowed-sinc resampler for sample playback at any pitch ratio.
//!
//! Reading a recorded sample at a fractional position `pos` needs the value of
//! the band-limited signal between its samples. The ideal answer is the sinc
//! sum `sum_n x[n] sinc(pos - n)`; truncating it with a **Kaiser window**
//! (Kaiser, "Nonrecursive digital filter design using the I0-sinh window
//! function", Proc. IEEE ISCAS 1974) gives a finite kernel with a chosen
//! stopband attenuation `A` dB and a transition width of
//! `(A - 7.95) / (14.36 * M)` cycles per sample for `M = 2 * half_width` taps.
//! The window shape parameter is `beta = 0.1102 (A - 8.7)` for `A > 50`
//! (Kaiser; Oppenheim & Schafer, *Discrete-Time Signal Processing*, 3rd ed.,
//! sec. 7.5.3).
//!
//! The kernel is stored once as a table with `phases` points per input sample:
//! that is the **polyphase** decomposition (one sub-filter per fractional
//! position), here with linear interpolation between neighbouring phases. See
//! Schafer & Rabiner, "A digital signal processing approach to interpolation",
//! Proc. IEEE 61(6), 1973; Crochiere & Rabiner, *Multirate Digital Signal
//! Processing*, Prentice-Hall 1983; and Smith, "Digital Audio Resampling Home
//! Page", CCRMA (the band-limited-interpolation scheme with a table and a
//! scaled kernel when the ratio is above one).
//!
//! # Pitch ratio
//!
//! `ratio` is input samples consumed per output sample: `1` plays at the
//! original speed, `2` one octave up, `0.5` one octave down. For `ratio > 1`
//! the kernel is stretched by `ratio` (cutoff scaled to `1 / ratio` of
//! Nyquist), so material that would fold above the new Nyquist is filtered out
//! first; the cost grows in proportion, so the ratio is limited to
//! [`MAX_RATIO`]. For `ratio <= 1` the cutoff stays at the source's Nyquist.
//!
//! The weights at each position are normalised to sum to 1, so a constant input
//! reads as exactly that constant at every position.
//!
//! # Threads
//!
//! [`SincKernel::new`] allocates and computes the table: build it in `prepare`,
//! not in `process`. Reading never allocates. The kernel is immutable and can be
//! shared by every voice.

use crate::windows::bessel_i0;
use std::f64::consts::PI;

/// Largest input-samples-per-output-sample ratio (4 octaves up).
pub const MAX_RATIO: f64 = 16.0;
/// Smallest ratio (6 octaves down).
pub const MIN_RATIO: f64 = 1.0 / 64.0;

/// Kernel quality presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Quality {
    /// 16 taps, 60 dB stopband.
    Low,
    /// 32 taps, 80 dB.
    Medium,
    /// 64 taps, 100 dB.
    High,
    /// 128 taps, 120 dB design target (measured -118.9 dB).
    Best,
}

/// Kernel design parameters. See [`SincKernel::with_config`].
/// # Example
///
/// ```
/// use openplugin_dsp::{Quality, SincConfig, SincKernel};
///
/// let config = SincConfig { stopband_db: 90.0, ..SincConfig::from(Quality::Medium) };
/// let kernel = SincKernel::with_config(config);
/// assert_eq!(kernel.half_width(), 16);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SincConfig {
    /// Zero crossings each side of the centre; `2 * half_width` taps. 4..=256.
    pub half_width: usize,
    /// Kaiser stopband attenuation in dB, 30..=160.
    pub stopband_db: f64,
    /// Table points per input sample, 16..=8192.
    pub phases: usize,
}

impl From<Quality> for SincConfig {
    fn from(q: Quality) -> Self {
        match q {
            Quality::Low => Self { half_width: 8, stopband_db: 60.0, phases: 256 },
            Quality::Medium => Self { half_width: 16, stopband_db: 80.0, phases: 512 },
            Quality::High => Self { half_width: 32, stopband_db: 100.0, phases: 1024 },
            Quality::Best => Self { half_width: 64, stopband_db: 120.0, phases: 2048 },
        }
    }
}

fn kaiser_beta(a: f64) -> f64 {
    if a > 50.0 {
        0.1102 * (a - 8.7)
    } else if a >= 21.0 {
        0.5842 * (a - 21.0).powf(0.4) + 0.078_86 * (a - 21.0)
    } else {
        0.0
    }
}

/// Precomputed windowed-sinc kernel (the polyphase table).
/// `new` and `with_config` allocate the table; reading it does not, so share
/// one kernel between voices on the audio thread.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{Quality, SincKernel};
///
/// let kernel = SincKernel::new(Quality::High);
/// let input: Vec<f32> = (0..1_000).map(|n| (n as f32 * 0.01).sin()).collect();
/// let mut out = vec![0.0f32; 2_000];
/// let written = kernel.resample(&input, 0.5, &mut out); // twice as many samples
/// assert_eq!(written, 2_000);
/// ```
#[derive(Clone, Debug)]
pub struct SincKernel {
    half: usize,
    phases: usize,
    stopband_db: f64,
    cutoff: f64,
    table: Vec<f32>,
}

impl SincKernel {
    /// A preset kernel. Allocates; call off the audio thread.
    pub fn new(quality: Quality) -> Self {
        Self::with_config(quality.into())
    }

    /// A custom kernel; the values are clamped into their ranges.
    pub fn with_config(cfg: SincConfig) -> Self {
        let half = cfg.half_width.clamp(4, 256);
        let phases = cfg.phases.clamp(16, 8192);
        let a = if cfg.stopband_db.is_finite() { cfg.stopband_db.clamp(30.0, 160.0) } else { 80.0 };
        let beta = kaiser_beta(a);
        // Transition width for M = 2 * half taps; the stopband starts at Nyquist
        // (0.5 cycles/sample) so the images of the passband are inside it.
        let delta = ((a - 7.95) / (14.36 * 2.0 * half as f64)).min(0.45);
        let cutoff = 0.5 - 0.5 * delta; // cycles per sample, centre of the transition
        let norm = bessel_i0(beta);
        let len = half * phases + 2;
        let mut table = vec![0.0f32; len];
        for (i, slot) in table.iter_mut().enumerate() {
            let x = i as f64 / phases as f64;
            if x >= half as f64 {
                *slot = 0.0;
                continue;
            }
            let arg = 2.0 * cutoff * x;
            let sinc = if arg.abs() < 1e-12 { 1.0 } else { (PI * arg).sin() / (PI * arg) };
            let t = x / half as f64;
            let win = bessel_i0(beta * (1.0 - t * t).sqrt()) / norm;
            *slot = (2.0 * cutoff * sinc * win) as f32;
        }
        Self { half, phases, stopband_db: a, cutoff, table }
    }

    /// Zero crossings each side of the centre at ratios up to 1.
    pub fn half_width(&self) -> usize {
        self.half
    }

    /// The design stopband attenuation in dB.
    pub fn stopband_db(&self) -> f64 {
        self.stopband_db
    }

    /// Upper edge of the flat passband in cycles per sample (fraction of the
    /// source rate): Nyquist minus the transition width.
    pub fn passband_edge(&self) -> f64 {
        2.0 * self.cutoff - 0.5
    }

    /// Kernel value at distance `x` samples (even function), linear between table points.
    pub fn kernel_at(&self, x: f64) -> f32 {
        let u = x.abs() * self.phases as f64;
        let max = (self.half * self.phases) as f64;
        if u.is_nan() || u >= max {
            return 0.0;
        }
        let i = u as usize;
        let f = (u - i as f64) as f32;
        self.table[i] + f * (self.table[i + 1] - self.table[i])
    }

    fn fetch(data: &[f32], i: i64, looping: Option<(usize, usize)>) -> f32 {
        let mut i = i;
        if let Some((s, e)) = looping {
            let (s, e) = (s as i64, e as i64);
            if i >= e || i < 0 {
                i = s + (i - s).rem_euclid(e - s);
            }
        }
        if i < 0 || i >= data.len() as i64 {
            0.0
        } else {
            data[i as usize]
        }
    }

    /// The band-limited value of `data` at fractional position `pos` (in input
    /// samples), reading at `ratio` input samples per output sample.
    ///
    /// Samples outside `data` read as 0, except that with `looping = Some((start, end))`
    /// taps at or past `end`, and taps before sample 0, wrap into `start..end`
    /// (so a loop is seamless, and a loop that starts at 0 is a periodic signal).
    /// A NaN `pos` or `ratio` returns 0.
    pub fn interpolate(&self, data: &[f32], pos: f64, ratio: f64, looping: Option<(usize, usize)>) -> f32 {
        if !pos.is_finite() || !ratio.is_finite() {
            return 0.0;
        }
        let looping = looping.filter(|&(s, e)| s < e && e <= data.len());
        let ratio = ratio.abs().clamp(MIN_RATIO, MAX_RATIO);
        let scale = if ratio > 1.0 { 1.0 / ratio } else { 1.0 };
        let reach = self.half as f64 / scale;
        let first = (pos - reach).ceil() as i64;
        let last = (pos + reach).floor() as i64;
        let step = scale * self.phases as f64;
        let mut acc = 0.0f64;
        let mut wsum = 0.0f64;
        let inside = first >= 0 && (last as usize) < data.len();
        let mut x = first as f64 - pos; // distance of the tap from the position
        for n in first..=last {
            let u = x.abs() * step;
            let i = u as usize;
            let w = if i + 1 < self.table.len() {
                let f = (u - i as f64) as f32;
                f64::from(self.table[i] + f * (self.table[i + 1] - self.table[i]))
            } else {
                0.0
            };
            let s = if inside { data[n as usize] } else { Self::fetch(data, n, looping) };
            acc += f64::from(s) * w;
            wsum += w;
            x += 1.0;
        }
        if wsum.abs() < 1e-9 {
            return 0.0;
        }
        let out = (acc / wsum) as f32;
        if out.abs() < 1e-30 { 0.0 } else { out }
    }

    /// Resample the whole of `input` at `ratio` into `out`; returns how many
    /// output samples were written (until `out` is full or the input ends).
    pub fn resample(&self, input: &[f32], ratio: f64, out: &mut [f32]) -> usize {
        if !ratio.is_finite() || ratio <= 0.0 {
            return 0;
        }
        let ratio = ratio.clamp(MIN_RATIO, MAX_RATIO);
        let mut written = 0;
        for (i, slot) in out.iter_mut().enumerate() {
            let pos = i as f64 * ratio;
            if pos >= input.len() as f64 {
                break;
            }
            *slot = self.interpolate(input, pos, ratio, None);
            written += 1;
        }
        written
    }
}

/// Input samples per output sample for playing a sample recorded at `source_rate`
/// with root pitch `root_hz` as a note of `note_hz` on a host at `host_rate`.
/// Returns 1.0 for any non-finite or non-positive argument.
pub fn playback_ratio(root_hz: f32, note_hz: f32, source_rate: f32, host_rate: f32) -> f64 {
    let ok = |v: f32| v.is_finite() && v > 0.0;
    if !(ok(root_hz) && ok(note_hz) && ok(source_rate) && ok(host_rate)) {
        return 1.0;
    }
    f64::from(note_hz) / f64::from(root_hz) * f64::from(source_rate) / f64::from(host_rate)
}

/// A playhead over one sample, with an optional loop.
/// Plain state, no allocation; one per voice.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{playback_ratio, Quality, SincKernel, SincPlayer};
///
/// let kernel = SincKernel::new(Quality::Medium);
/// let sample: Vec<f32> = (0..4_800).map(|n| (n as f32 * 0.05).sin()).collect();
/// let mut voice = SincPlayer::new();
/// // A sample recorded at 44.1 kHz with root A4, played as A5 at 48 kHz.
/// voice.start(playback_ratio(440.0, 880.0, 44_100.0, 48_000.0));
/// let mut out = [0.0f32; 256];
/// for y in &mut out {
///     *y = voice.next(&kernel, &sample);
/// }
/// assert!(voice.position() > 400.0);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct SincPlayer {
    pos: f64,
    ratio: f64,
    looping: Option<(usize, usize)>,
    done: bool,
}

impl Default for SincPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl SincPlayer {
    /// A stopped player at position 0 and ratio 1.
    pub const fn new() -> Self {
        Self { pos: 0.0, ratio: 1.0, looping: None, done: false }
    }

    /// Start from the top at `ratio` (see the module documentation).
    pub fn start(&mut self, ratio: f64) {
        self.pos = 0.0;
        self.done = false;
        self.set_ratio(ratio);
    }

    /// Change the ratio while playing (pitch bend, glide). Non-finite is ignored.
    pub fn set_ratio(&mut self, ratio: f64) {
        if ratio.is_finite() && ratio > 0.0 {
            self.ratio = ratio.clamp(MIN_RATIO, MAX_RATIO);
        }
    }

    /// Loop `start..end` (samples) while playing; `None` plays once.
    pub fn set_loop(&mut self, looping: Option<(usize, usize)>) {
        self.looping = looping;
    }

    /// The read position in source samples.
    pub fn position(&self) -> f64 {
        self.pos
    }

    /// True once a one-shot has run past the end.
    pub fn is_finished(&self) -> bool {
        self.done
    }

    /// Next output sample from `data`; 0 once finished.
    pub fn next(&mut self, kernel: &SincKernel, data: &[f32]) -> f32 {
        if self.done {
            return 0.0;
        }
        let looping = self.looping.filter(|&(s, e)| s < e && e <= data.len());
        let out = kernel.interpolate(data, self.pos, self.ratio, looping);
        self.pos += self.ratio;
        match looping {
            Some((s, e)) => {
                let (s, e) = (s as f64, e as f64);
                if self.pos >= e {
                    self.pos = s + (self.pos - s).rem_euclid(e - s);
                }
            }
            None => {
                if self.pos >= data.len() as f64 {
                    self.done = true;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::num_complex::Complex;
    use rustfft::FftPlanner;

    /// |FT| in dB of the interpolation kernel (including table interpolation),
    /// returned as (passband ripple peak-to-peak up to the passband edge, worst
    /// level at or above Nyquist), both in dB.
    fn kernel_response(k: &SincKernel) -> (f64, f64) {
        let over = 16usize; // 16 points per sample, resolves 0..8 cycles/sample
        let n = 1usize << 16;
        let mut buf = vec![Complex::new(0.0f64, 0.0); n];
        let half = k.half_width() as f64;
        for (i, slot) in buf.iter_mut().enumerate() {
            let j = i as i64;
            let x = if j < (n / 2) as i64 { j as f64 } else { j as f64 - n as f64 } / over as f64;
            if x.abs() < half {
                slot.re = f64::from(k.kernel_at(x)) / over as f64;
            }
        }
        FftPlanner::<f64>::new().plan_fft_forward(n).process(&mut buf);
        let hz_per_bin = over as f64 / n as f64; // cycles/sample per bin
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        let mut stop = f64::MIN;
        for (b, c) in buf.iter().enumerate().take(n / 2) {
            let f = b as f64 * hz_per_bin;
            let db = 20.0 * c.norm().max(1e-12).log10();
            if f <= k.passband_edge() {
                lo = lo.min(db);
                hi = hi.max(db);
            } else if f >= 0.5 {
                stop = stop.max(db);
            }
        }
        (hi - lo, stop)
    }

    #[test]
    fn passband_ripple_and_stopband_attenuation_meet_the_design() {
        for (q, ripple_max, stop_max) in [
            (Quality::Low, 0.1, -58.0),
            (Quality::Medium, 0.02, -78.0),
            (Quality::High, 0.005, -98.0),
            (Quality::Best, 0.001, -118.0),
        ] {
            let k = SincKernel::new(q);
            let (ripple, stop) = kernel_response(&k);
            println!("{q:?}: passband edge {:.4}, ripple {ripple:.5} dB, stopband {stop:.1} dB", k.passband_edge());
            assert!(ripple < ripple_max, "{q:?} ripple {ripple}");
            assert!(stop < stop_max, "{q:?} stopband {stop}");
        }
    }

    #[test]
    fn constant_input_is_exact_and_a_sine_keeps_its_amplitude() {
        let k = SincKernel::new(Quality::Medium);
        let dc = vec![0.25f32; 400];
        for pos in [100.0, 100.3, 150.77, 250.5] {
            for ratio in [0.5, 1.0, 1.7, 5.0] {
                assert!((k.interpolate(&dc, pos, ratio, None) - 0.25).abs() < 1e-6);
            }
        }
        // Pitch up a fifth by reading at 1.5; a 0.05 cycle/sample sine -> 0.075.
        let sine: Vec<f32> = (0..4000).map(|i| (2.0 * PI * 0.05 * i as f64).sin() as f32).collect();
        let mut out = vec![0.0f32; 2000];
        let n = k.resample(&sine, 1.5, &mut out);
        assert!(n >= 2000);
        let mid = &out[200..1800];
        let want = |i: usize| (2.0 * PI * 0.075 * (i + 200) as f64).sin() as f32;
        let err = mid.iter().enumerate().map(|(i, v)| (v - want(i)).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-3, "{err}");
    }

    #[test]
    fn looping_is_seamless_and_one_shot_ends() {
        let k = SincKernel::new(Quality::Low);
        // One exact cycle of a sine: looping it must continue the sine.
        let n = 64usize;
        let cycle: Vec<f32> = (0..n).map(|i| (2.0 * PI * i as f64 / n as f64).sin() as f32).collect();
        let mut p = SincPlayer::new();
        p.start(1.25);
        p.set_loop(Some((0, n)));
        let mut worst = 0.0f32;
        for i in 0..2000 {
            let v = p.next(&k, &cycle);
            if i > 20 {
                let want = (2.0 * PI * 1.25 * i as f64 / n as f64).sin() as f32;
                worst = worst.max((v - want).abs());
            }
        }
        assert!(worst < 5e-3, "{worst}");
        let mut once = SincPlayer::new();
        once.start(1.0);
        for _ in 0..n + 4 {
            once.next(&k, &cycle);
        }
        assert!(once.is_finished());
        assert_eq!(once.next(&k, &cycle), 0.0);
    }

    #[test]
    fn hostile_arguments_are_safe() {
        let k = SincKernel::new(Quality::Low);
        let d = vec![1.0f32; 16];
        for pos in [f64::NAN, f64::INFINITY, -1e9, 1e9, -3.5] {
            for ratio in [f64::NAN, 0.0, -2.0, 1e9] {
                assert!(k.interpolate(&d, pos, ratio, Some((3, 9))).is_finite());
                assert!(k.interpolate(&[], pos, ratio, Some((0, 0))).is_finite());
            }
        }
        assert_eq!(playback_ratio(0.0, 440.0, 44_100.0, 48_000.0), 1.0);
        let r = playback_ratio(440.0, 880.0, 44_100.0, 48_000.0);
        assert!((r - 2.0 * 44_100.0 / 48_000.0).abs() < 1e-9);
    }
}
