//! Mip-mapped wavetable oscillator.
//!
//! A single-cycle table played at a high pitch skips samples, and every
//! harmonic that lands above Nyquist folds back as an alias. The classic remedy
//! is a **mip-map of tables, one per octave**: level `k` holds the same cycle
//! with its harmonics cut off above `H_k = (N/8) / 2^k` (for a table of `N`
//! points). The oscillator picks the level with the most harmonics for which
//! `H_k * f0 < Nyquist`, so no partial in the table can fold, and the cut is
//! never more than one octave early. The idea is that of the mip-map in
//! texture filtering (Williams, "Pyramidal parametrics", SIGGRAPH 1983) applied
//! to a periodic signal, as in band-limited wavetable synthesis (see Roads,
//! *The Computer Music Tutorial*, MIT Press 1996, ch. 5, and Massie, "Wavetable
//! synthesis", in Greenebaum & Barzel (eds.), *Audio Anecdotes*, 2004).
//!
//! Tables are built by an FFT of the source cycle, zeroing bins above `H_k`,
//! and an inverse FFT. The FFT is `rustfft`. Reading uses Catmull-Rom cubic
//! interpolation on `N` points where the highest kept harmonic is at most
//! `N/8` cycles per table, which keeps interpolation images far below the
//! signal (measured in `tests/synth_proof.rs`).
//!
//! # Build off the audio thread
//!
//! [`WavetableSet`] construction allocates and runs FFTs. Never do it in
//! `process`. Build it in `prepare`, or on the worker thread from
//! `openplugin_core::worker`, which swaps the finished set in without a lock:
//!
//! ```
//! use openplugin_core::worker::loader;
//! use openplugin_dsp::{WavetableOsc, WavetableSet};
//!
//! // Audio thread starts with a plain sine and polls for the real table.
//! let (mut jobs, mut slot) = loader(WavetableSet::sine(), 2);
//! jobs.submit(|| WavetableSet::saw(2048).unwrap()).unwrap();
//! jobs.wait_idle(std::time::Duration::from_secs(10));
//!
//! let mut osc = WavetableOsc::new();
//! // Top of `process`:
//! slot.poll();
//! let sample = osc.next(slot.get(), 440.0, 48_000.0);
//! assert!(sample.is_finite());
//! ```
//!
//! The oscillator itself ([`WavetableOsc`]) allocates nothing; it only borrows a
//! set. Non-finite or negative frequencies hold the phase.

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

/// Why a [`WavetableSet`] could not be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WavetableError {
    /// The table size is not a power of two in 64..=65536.
    BadSize,
    /// The source cycle has fewer than 4 points or is longer than 1,048,576.
    BadCycle,
    /// The source holds NaN or an infinity.
    NonFinite,
}

impl std::fmt::Display for WavetableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::BadSize => "table size must be a power of two from 64 to 65536",
            Self::BadCycle => "cycle must have 4 to 1048576 points",
            Self::NonFinite => "cycle contains NaN or infinity",
        })
    }
}

impl std::error::Error for WavetableError {}

/// One cycle at several band limits, ready to read.
#[derive(Clone, Debug)]
pub struct WavetableSet {
    size: usize,
    /// Per level: `size + 3` points, `data[i + 1] = x[i]`, with a wrapped guard each side.
    levels: Vec<Vec<f32>>,
    harmonics: Vec<usize>,
}

impl WavetableSet {
    /// Build from one cycle of any length (4 or more points) at `size` points per table.
    ///
    /// Allocates and runs FFTs: call off the audio thread.
    pub fn from_cycle(cycle: &[f32], size: usize) -> Result<Self, WavetableError> {
        if !size.is_power_of_two() || !(64..=65_536).contains(&size) {
            return Err(WavetableError::BadSize);
        }
        if cycle.len() < 4 || cycle.len() > 1_048_576 {
            return Err(WavetableError::BadCycle);
        }
        if cycle.iter().any(|v| !v.is_finite()) {
            return Err(WavetableError::NonFinite);
        }
        let m = cycle.len();
        let mut planner = FftPlanner::<f64>::new();
        let fwd = planner.plan_fft_forward(m);
        let mut spec: Vec<Complex<f64>> = cycle.iter().map(|v| Complex::new(f64::from(*v), 0.0)).collect();
        fwd.process(&mut spec);
        let norm = 1.0 / m as f64;
        let source_harmonics = (m - 1) / 2;
        Ok(Self::from_spectrum(
            |h| spec[h] * norm,
            source_harmonics,
            size,
            &mut planner,
        ))
    }

    /// Build from sine amplitudes: `amps[0]` is the fundamental, `amps[1]` the 2nd harmonic, and so on.
    pub fn from_harmonics(amps: &[f32], size: usize) -> Result<Self, WavetableError> {
        if !size.is_power_of_two() || !(64..=65_536).contains(&size) {
            return Err(WavetableError::BadSize);
        }
        if amps.iter().any(|v| !v.is_finite()) {
            return Err(WavetableError::NonFinite);
        }
        let mut planner = FftPlanner::<f64>::new();
        // sin(2 pi h n / N) = (e^{j..} - e^{-j..}) / 2j  ->  X[h] = -j a / 2.
        let n = amps.len();
        Ok(Self::from_spectrum(
            |h| {
                if h == 0 || h > n {
                    Complex::new(0.0, 0.0)
                } else {
                    Complex::new(0.0, -0.5 * f64::from(amps[h - 1]))
                }
            },
            n,
            size,
            &mut planner,
        ))
    }

    fn from_spectrum<F: Fn(usize) -> Complex<f64>>(
        coeff: F,
        source_harmonics: usize,
        size: usize,
        planner: &mut FftPlanner<f64>,
    ) -> Self {
        let inv = planner.plan_fft_inverse(size);
        let top = size / 8;
        let mut levels = Vec::new();
        let mut harmonics = Vec::new();
        let mut h_k = top;
        loop {
            let keep = h_k.min(source_harmonics);
            let mut buf = vec![Complex::new(0.0, 0.0); size];
            buf[0] = Complex::new(coeff(0).re, 0.0);
            for h in 1..=keep {
                let c = coeff(h);
                buf[h] = c;
                buf[size - h] = c.conj();
            }
            inv.process(&mut buf);
            let mut data = vec![0.0f32; size + 3];
            for i in 0..size {
                data[i + 1] = buf[i].re as f32;
            }
            data[0] = data[size];
            data[size + 1] = data[1];
            data[size + 2] = data[2];
            levels.push(data);
            harmonics.push(keep);
            if h_k <= 1 {
                break;
            }
            h_k /= 2;
        }
        Self { size, levels, harmonics }
    }

    /// A plain sine at 256 points: cheap, for the audio thread's initial value.
    pub fn sine() -> Self {
        Self::from_harmonics(&[1.0], 256).expect("valid size")
    }

    /// Rising sawtooth `2 phase - 1` from its Fourier series `-(2/pi) sum sin(2 pi h phase)/h`.
    pub fn saw(size: usize) -> Result<Self, WavetableError> {
        let amps: Vec<f32> = (1..=size / 8).map(|h| -2.0 / (std::f32::consts::PI * h as f32)).collect();
        Self::from_harmonics(&amps, size)
    }

    /// Square wave from odd harmonics `4/(pi h)`.
    pub fn square(size: usize) -> Result<Self, WavetableError> {
        let amps: Vec<f32> = (1..=size / 8)
            .map(|h| if h % 2 == 1 { 4.0 / (std::f32::consts::PI * h as f32) } else { 0.0 })
            .collect();
        Self::from_harmonics(&amps, size)
    }

    /// Points per table.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Number of octave levels.
    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// Highest harmonic kept in `level`.
    pub fn harmonics_in(&self, level: usize) -> usize {
        self.harmonics[level.min(self.harmonics.len() - 1)]
    }

    /// The level used at `cycles_per_sample` (`hz / sample_rate`).
    pub fn level_for(&self, cycles_per_sample: f64) -> usize {
        let top = (self.size / 8) as f64;
        if cycles_per_sample.is_nan() || cycles_per_sample <= 0.0 {
            return 0;
        }
        let ds = cycles_per_sample.min(0.5);
        let allowed = 0.5 / ds; // harmonics that fit below Nyquist
        if allowed >= top {
            return 0;
        }
        let k = (top / allowed).log2().ceil();
        (k.max(0.0) as usize).min(self.levels.len() - 1)
    }

    /// Read `level` at `phase` (0..1) with cubic interpolation.
    #[inline]
    pub fn read(&self, level: usize, phase: f64) -> f32 {
        let data = &self.levels[level.min(self.levels.len() - 1)];
        let pos = phase.rem_euclid(1.0) * self.size as f64;
        let i = (pos as usize).min(self.size - 1);
        let f = (pos - i as f64) as f32;
        let (p0, p1, p2, p3) = (data[i], data[i + 1], data[i + 2], data[i + 3]);
        let a = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
        let b = 2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3;
        let c = p2 - p0;
        p1 + 0.5 * f * (c + f * (b + f * a))
    }
}

/// Phase accumulator reading a [`WavetableSet`].
#[derive(Clone, Copy, Debug, Default)]
pub struct WavetableOsc {
    phase: f64,
}

impl WavetableOsc {
    pub const fn new() -> Self {
        Self { phase: 0.0 }
    }

    pub fn phase(&self) -> f64 {
        self.phase
    }

    /// Set the phase (wrapped; NaN becomes 0).
    pub fn set_phase(&mut self, phase: f64) {
        self.phase = if phase.is_finite() { phase.rem_euclid(1.0) } else { 0.0 };
    }

    /// One sample at `hz`. Non-finite or negative `hz` or `sample_rate` holds the phase.
    pub fn next(&mut self, set: &WavetableSet, hz: f32, sample_rate: f32) -> f32 {
        let ds = if hz.is_finite() && hz >= 0.0 && sample_rate.is_finite() && sample_rate > 0.0 {
            f64::from(hz) / f64::from(sample_rate)
        } else {
            0.0
        };
        self.next_normalized(set, ds)
    }

    /// The same with the frequency as `hz / sample_rate`.
    pub fn next_normalized(&mut self, set: &WavetableSet, cycles_per_sample: f64) -> f32 {
        let ds = if cycles_per_sample.is_finite() { cycles_per_sample.clamp(0.0, 0.5) } else { 0.0 };
        let level = set.level_for(ds);
        let out = set.read(level, self.phase);
        self.phase += ds;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        if out.abs() < 1e-30 { 0.0 } else { out }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_halve_and_selection_keeps_partials_below_nyquist() {
        let set = WavetableSet::saw(2048).unwrap();
        assert_eq!(set.harmonics_in(0), 256);
        assert_eq!(set.harmonics_in(1), 128);
        assert_eq!(set.harmonics_in(set.level_count() - 1), 1);
        for sr in [22_050.0f64, 44_100.0, 96_000.0, 384_000.0] {
            let mut hz = 5.0;
            while hz < 0.49 * sr {
                let ds = hz / sr;
                let h = set.harmonics_in(set.level_for(ds)) as f64;
                assert!(h * ds <= 0.5 + 1e-12 || h <= 1.0, "{hz} Hz at {sr}: {h} harmonics");
                hz *= 1.07;
            }
        }
    }

    #[test]
    fn saw_table_is_a_saw() {
        let set = WavetableSet::saw(2048).unwrap();
        // Mid-cycle of the rising ramp is 0 and the ramp rises.
        assert!(set.read(0, 0.5).abs() < 0.02);
        assert!(set.read(0, 0.75) > 0.4 && set.read(0, 0.25) < -0.4);
    }

    #[test]
    fn cycle_round_trip() {
        let cycle: Vec<f32> = (0..100).map(|i| (i as f32 * std::f32::consts::TAU / 100.0).sin() * 0.5).collect();
        let set = WavetableSet::from_cycle(&cycle, 512).unwrap();
        for k in 0..50 {
            let ph = f64::from(k) / 50.0;
            let want = (ph as f32 * std::f32::consts::TAU).sin() * 0.5;
            assert!((set.read(0, ph) - want).abs() < 1e-3);
        }
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(WavetableSet::from_cycle(&[0.0; 8], 100).unwrap_err(), WavetableError::BadSize);
        assert_eq!(WavetableSet::from_cycle(&[0.0; 2], 256).unwrap_err(), WavetableError::BadCycle);
        assert_eq!(
            WavetableSet::from_cycle(&[0.0, f32::NAN, 0.0, 0.0], 256).unwrap_err(),
            WavetableError::NonFinite
        );
        let set = WavetableSet::sine();
        let mut o = WavetableOsc::new();
        for hz in [f32::NAN, -1.0, f32::INFINITY, 1e9] {
            assert!(o.next(&set, hz, 48_000.0).is_finite());
        }
    }
}
