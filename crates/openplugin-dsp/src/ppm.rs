//! Peak programme meter (PPM) ballistics after IEC 60268-10 and EBU Tech 3205-E.
//!
//! A PPM is a quasi-peak detector: it rises fast but not instantly, so a
//! brief burst reads low by a specified amount, and it falls slowly at a
//! steady rate in dB. [`Ppm`] models it as a full-wave rectifier feeding
//! diode-and-capacitor stages that discharge at a fixed rate in dB per second.
//!
//! | [`PpmType`] | Standard | Integration time | Return time |
//! |---|---|---|---|
//! | `Din` | DIN 45406 = IEC 60268-10 Type I | 5 ms | 20 dB in 1.7 s (+/- 0.3 s) |
//! | `Nordic` | IEC 60268-10 Type I, Nordic scale (NRK, SR, DR, YLE) | 5 ms | 20 dB in 1.7 s (+/- 0.3 s) |
//! | `Ebu` | IEC 60268-10 Type IIb = EBU Tech 3205-E | 10 ms | 24 dB in 2.8 s (+/- 0.3 s) |
//! | `Bbc` | IEC 60268-10 Type IIa (BBC) | 10 ms | 24 dB in 2.8 s (+/- 0.3 s) |
//!
//! # Requirements and where they come from
//!
//! IEC 60268-10 is not free. The numbers below are the publicly documented
//! ones; each row is a test in this module at 44.1, 48, 96 and 192 kHz.
//!
//! Type II (`Ebu`, `Bbc`), from EBU Tech 3205-E (2nd edition, 1979), section 3,
//! which states that types IIa and IIb "differ only in respect of the scale
//! divisions and marking". An isolated burst of 5 kHz tone, read against the
//! steady reading of the same tone:
//!
//! | Burst | Reading | Tolerance | Source |
//! |---|---|---|---|
//! | 100 ms | 0 dB | +/- 0.5 | Tech 3205-E table 2 |
//! | 10 ms | -2 dB | +/- 0.5 | table 2; 3.3: integration time 10 +/- 2 ms |
//! | 5 ms | -4 dB | +/- 0.75 | table 2 |
//! | 1.5 ms | -9 dB | +/- 1.0 | table 2 |
//! | 0.5 ms (10 kHz) | -17 dB | +/- 2.0 | table 2 |
//! | delay time | within 1 dB of steady in 150 ms (1 kHz) | | 3.8 |
//! | overswing | at most 0.5 dB (1 kHz) | | 3.9 |
//! | return | 24 dB in 2.8 s, at about constant speed | +/- 0.3 s | 3.10 |
//! | reversibility | at most 0.5 dB between polarities | | 2.4 |
//! | frequency response | flat from 31.5 Hz to 16 kHz | +/- 0.3 | 2.3 |
//!
//! Type I (`Din`, `Nordic`): the burst readings are from the table in
//! Wikipedia's "Peak programme meter" (citing IEC 60268-10) and from
//! ITU-T J.15 table 1 (10 ms -1 dB, 5 ms -2 dB, 0.4 ms -15 dB); the return
//! time from ITU-R BS.645-2 table 1 and the same Wikipedia table. No public
//! source gives Type I tolerances, so the tolerances here are this module's
//! own, taken from Tech 3205-E's tolerance at the same depth of reading:
//!
//! | Burst | Reading | Tolerance (ours) |
//! |---|---|---|
//! | 100 ms | 0 dB | +/- 0.5 |
//! | 10 ms | -1 dB | +/- 0.5 |
//! | 5 ms | -2 dB | +/- 0.5 |
//! | 3 ms | -4 dB | +/- 0.75 |
//! | 0.4 ms (10 kHz) | -15 dB | +/- 2.0 |
//! | return | 20 dB in 1.7 s | +/- 0.3 s (IEC, as quoted by Wikipedia) |
//!
//! Where the sources disagree: DIN 45406 itself gives the return as 1.5 s for
//! 20 dB and 2.5 s for 40 dB (ITU-T J.15; maker's data for DIN instruments),
//! IEC 60268-10 Type I as 1.7 +/- 0.3 s for 20 dB. This module uses the IEC
//! figure at constant speed (40 dB in 3.4 s); 1.5 s is inside its tolerance,
//! the faster second 20 dB of DIN 45406 is not modelled. The BBC meter's
//! return is quoted as 26 dB in 3 s by ITU-R BS.645-2 and as 24 dB in 2.8 s
//! by IEC Type IIa: the same speed to 1 %. Neither standard's public text
//! specifies repeated bursts in the normal mode (Tech 3205-E does only for its
//! "slow" mode, which is not implemented here).
//!
//! # The model
//!
//! Each stage is a capacitor charged through a resistor and an ideal diode
//! from the rectified signal `v = |x|`, and discharged at a constant rate in
//! dB: `y += (v - y) (1 - exp(-1 / (tau fs)))` while `v > y`, otherwise
//! `y *= release`. For a tone whose period is short against `tau`, averaging
//! over a cycle gives the charging law
//!
//! ```text
//! dy/dt = g(y) / tau,   g(y) = (2 / pi) (sqrt(1 - y^2) - y acos y)
//! ```
//!
//! with `y` relative to the tone's peak, so a burst of length `T` reads
//! `y(T)` with `T / tau = u(y) = integral of dy / g(y)` from 0. `u` is 6.654
//! at -1 dB, 3.918 at -2 dB, 2.082 at -4 dB, 0.780 at -9 dB, 0.325 at -15 dB
//! and 0.250 at -17 dB. The standard defines "integration time" as the burst
//! that reads 2 dB low, so the time constant follows from the burst table,
//! not from the nominal figure taken as an RC constant:
//!
//! - Type I: the durations 10 : 5 : 3 : 0.4 ms are close to
//!   `u` = 6.65 : 3.92 : 2.08 : 0.33 times one constant (1.50, 1.28, 1.44 and
//!   1.23 ms), so one stage fits. `tau` = 1.42 ms is the least-worst fit.
//! - Type II: 10 : 5 : 1.5 : 0.5 ms against `u` = 3.92 : 2.08 : 0.78 : 0.25
//!   asks for 2.55, 2.40, 1.92 and 2.00 ms: the table rises faster at first
//!   and slower later than one stage can. Two stages in parallel fit it: the
//!   reading is 0.75 of a stage with `tau` = 1.5 ms plus 0.25 of a stage with
//!   `tau` = 10 ms (a search over the weight and both constants for the
//!   smallest worst error against the table's tolerances).
//!
//! In the steady state each stage settles a little below the peak, where the
//! charge taken near each crest equals the discharge over the rest of the
//! cycle: `g(y) / tau = lambda y (1 - (2 / pi) acos y)`, `lambda` the
//! discharge rate in nepers per second. [`Ppm::new`] solves this and scales
//! the reading so that a steady sine reads its peak (the meter is calibrated
//! on tone, like the instrument). A square wave or DC of the same peak then
//! reads up to 0.23 dB (Type II) or 0.17 dB (Type I) higher.
//!
//! The rectifier works on an oversampled signal (8x below 88.2 kHz, 4x below
//! 176.4 kHz, 2x below 352.8 kHz): the sample peaks of a high tone understate
//! it (a sine at a quarter of the rate sampled at 45 degrees reads 3 dB low),
//! and the charging constants are then the same at every rate. The
//! interpolator is a Kaiser-windowed sinc designed here, 16 taps per phase.
//! The reading lags the input by 8 samples.
//!
//! Measured (bursts of 5 kHz, or 10 kHz for 0.5 ms and less; the range over
//! 44.1, 48, 96 and 192 kHz, bursts starting at a zero crossing and at a
//! crest, and four sub-sample offsets), in dB against the steady reading:
//! Type II 100 ms -0.06, 10 ms -2.01 to -2.02, 5 ms -3.76 to -3.80, 1.5 ms
//! -9.07 to -9.24, 0.5 ms -15.93 to -16.61; Type I 100 ms 0.00, 10 ms -0.77
//! to -0.78, 5 ms -2.09 to -2.13, 3 ms -3.72 to -3.80, 0.4 ms -15.21 to
//! -16.38. The spread of the shortest bursts is the test signal's: a gated
//! 10 kHz tone of 18 samples at 44.1 kHz is not the same signal at every
//! offset. A steady sine from 31.5 Hz to 16 kHz reads within 0.1 dB of its
//! peak. It is an engineering model after the public requirements, not a
//! type approval.
//!
//! The dB scales differ per type; [`PpmType::display`] maps a level to the
//! number on the meter, given the alignment level in dBFS (EBU R 68 puts
//! alignment at -18 dBFS, 0 dBu):
//!
//! - `Ebu` and `Nordic`: dB re alignment ("TEST" is 0, the permitted maximum
//!   level is +9);
//! - `Din`: dB re the permitted maximum level, which is 9 dB above alignment
//!   (alignment reads -9, permitted maximum 0);
//! - `Bbc`: scale numbers 1 to 7 at 4 dB per step, alignment at 4, the
//!   permitted maximum (+8 dB) at 6.
//!
//! The alignment and permitted-maximum levels and the scale layouts are from
//! EBU Tech 3205-E (its table 8 compares the types) and the table in
//! Wikipedia's "Peak programme meter".

use crate::windows::bessel_i0;
use std::f64::consts::PI;

/// Which PPM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PpmType {
    /// EBU, IEC 60268-10 Type IIb.
    Ebu,
    /// DIN 45406, IEC 60268-10 Type I.
    Din,
    /// Nordic (Type I ballistics, Nordic scale).
    Nordic,
    /// BBC, IEC 60268-10 Type IIa.
    Bbc,
}

impl PpmType {
    /// Integration time in ms (reads -2 dB for a burst this long).
    pub fn integration_ms(self) -> f32 {
        match self {
            Self::Din | Self::Nordic => 5.0,
            Self::Ebu | Self::Bbc => 10.0,
        }
    }

    /// `(dB, seconds)`: the fall of `dB` takes `seconds`.
    pub fn return_time(self) -> (f32, f32) {
        match self {
            Self::Din | Self::Nordic => (20.0, 1.7),
            Self::Ebu | Self::Bbc => (24.0, 2.8),
        }
    }

    /// The number on this meter's scale for `level_dbfs`, with the alignment
    /// level at `alignment_dbfs`. See the module documentation.
    pub fn display(self, level_dbfs: f32, alignment_dbfs: f32) -> f32 {
        let re_align = level_dbfs - alignment_dbfs;
        match self {
            Self::Ebu | Self::Nordic => re_align,
            Self::Din => re_align - 9.0,
            Self::Bbc => 4.0 + re_align / 4.0,
        }
    }

    /// The two charging stages: `(weight, time constant in seconds)` each.
    /// Fitted to the tone-burst tables; see the module documentation.
    fn stages(self) -> [(f64, f64); 2] {
        match self {
            Self::Din | Self::Nordic => [(1.0, 1.42e-3), (0.0, 1.42e-3)],
            Self::Ebu | Self::Bbc => [(0.75, 1.5e-3), (0.25, 10.0e-3)],
        }
    }
}

const TAPS: usize = 16;
const MAX_FACTOR: usize = 8;
const KAISER_BETA: f64 = 7.0;
/// The rectifier runs at this rate or above (while a factor up to 8 allows).
const DETECTOR_RATE: f32 = 352_800.0;
/// A stage below this (-240 dB) is set to zero, so silence leaves no denormals.
const STAGE_FLOOR: f64 = 1.0e-12;

/// Kaiser-windowed sinc interpolator, cut-off at the input Nyquist rate, each
/// phase normalised to unity gain at DC. `[p][k]` multiplies the `k`-th newest
/// input sample for output phase `p`.
fn interpolator(factor: usize) -> [[f32; TAPS]; MAX_FACTOR] {
    let mut out = [[0.0f32; TAPS]; MAX_FACTOR];
    let n = TAPS * factor;
    let centre = (n as f64 - 1.0) / 2.0;
    let norm = bessel_i0(KAISER_BETA);
    for (p, phase) in out.iter_mut().enumerate().take(factor) {
        let mut h = [0.0f64; TAPS];
        for (k, tap) in h.iter_mut().enumerate() {
            let m = (k * factor + p) as f64;
            let t = (m - centre) / factor as f64;
            let sinc = if t == 0.0 { 1.0 } else { (PI * t).sin() / (PI * t) };
            let r = (m - centre) / centre;
            *tap = sinc * bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / norm;
        }
        let sum: f64 = h.iter().sum();
        for (o, tap) in phase.iter_mut().zip(h) {
            *o = (tap / sum) as f32;
        }
    }
    out
}

/// Where one stage settles on a steady sine of unit peak: the charge taken
/// near each crest equals the discharge over the rest of the cycle.
/// `lambda` is the discharge rate in nepers per second.
fn settled(tau: f64, lambda: f64) -> f64 {
    let balance = |y: f64| {
        let angle = y.acos();
        let charge = (2.0 / PI) * ((1.0 - y * y).sqrt() - y * angle) / tau;
        charge - lambda * y * (1.0 - 2.0 * angle / PI)
    };
    let (mut low, mut high) = (0.0f64, 1.0f64);
    for _ in 0..60 {
        let mid = 0.5 * (low + high);
        if balance(mid) > 0.0 {
            low = mid;
        } else {
            high = mid;
        }
    }
    0.5 * (low + high)
}

/// One channel of PPM detector. No allocation; `Copy`.
/// # Example
///
/// ```
/// use openplugin_dsp::{Ppm, PpmType};
///
/// let rate = 48_000.0;
/// let mut ppm = Ppm::new(rate, PpmType::Ebu);
/// let tone: Vec<f32> = (0..48_000)
///     .map(|n| 0.5 * (2.0 * std::f32::consts::PI * 1_000.0 * n as f32 / rate).sin())
///     .collect();
/// ppm.process(&tone);
/// assert!((ppm.level_dbfs() - -6.02).abs() < 0.5); // a steady tone reads its peak
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Ppm {
    kind: PpmType,
    factor: usize,
    taps: [[f32; TAPS]; MAX_FACTOR],
    /// `history[0]` is the newest input sample.
    history: [f32; TAPS],
    /// Per detector step: charge coefficient of each stage.
    attack: [f64; 2],
    /// Stage weight times the calibration that makes a steady sine read its peak.
    gain: [f64; 2],
    /// Per detector step: discharge factor.
    release: f64,
    stage: [f64; 2],
    level: f32,
}

impl Ppm {
    /// A meter of `kind` for `sample_rate` (Hz, clamped to the SDK range). Plain
    /// state, no allocation.
    pub fn new(sample_rate: f32, kind: PpmType) -> Self {
        let rate = crate::rate::effective_rate(sample_rate);
        let mut factor = 1;
        while factor < MAX_FACTOR && rate * (factor as f32) < DETECTOR_RATE {
            factor *= 2;
        }
        let detector_rate = f64::from(rate) * factor as f64;
        let (db, seconds) = kind.return_time();
        let lambda = f64::from(db) / f64::from(seconds) * std::f64::consts::LN_10 / 20.0;
        let stages = kind.stages();
        let steady: f64 = stages.iter().map(|&(weight, tau)| weight * settled(tau, lambda)).sum();
        Self {
            kind,
            factor,
            taps: if factor > 1 { interpolator(factor) } else { [[0.0; TAPS]; MAX_FACTOR] },
            history: [0.0; TAPS],
            attack: stages.map(|(_, tau)| 1.0 - (-1.0 / (tau * detector_rate)).exp()),
            gain: stages.map(|(weight, _)| weight / steady),
            release: (-lambda / detector_rate).exp(),
            stage: [0.0; 2],
            level: 0.0,
        }
    }

    /// The meter type.
    pub fn kind(&self) -> PpmType {
        self.kind
    }

    /// The oversampling factor of the rectifier (8, 4, 2 or 1 by rate).
    pub fn oversampling(&self) -> usize {
        self.factor
    }

    /// Returns the meter to rest.
    pub fn reset(&mut self) {
        self.history = [0.0; TAPS];
        self.stage = [0.0; 2];
        self.level = 0.0;
    }

    /// One sample; returns the linear reading.
    #[inline]
    pub fn next(&mut self, x: f32) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        let mut stage = self.stage;
        if self.factor == 1 {
            self.charge(&mut stage, f64::from(x.abs()));
        } else {
            self.history.copy_within(0..TAPS - 1, 1);
            self.history[0] = x;
            for phase in &self.taps[..self.factor] {
                let v: f32 = phase.iter().zip(&self.history).map(|(a, b)| a * b).sum();
                self.charge(&mut stage, f64::from(v.abs()));
            }
        }
        self.stage = stage;
        self.level = (self.gain[0] * stage[0] + self.gain[1] * stage[1]) as f32;
        self.level
    }

    /// One detector step: each stage charges towards `v` while it is above
    /// the stage, and otherwise falls at the return rate.
    #[inline]
    fn charge(&self, stage: &mut [f64; 2], v: f64) {
        for (y, attack) in stage.iter_mut().zip(self.attack) {
            if v > *y {
                *y += (v - *y) * attack;
            } else {
                *y *= self.release;
                if *y < STAGE_FLOOR {
                    *y = 0.0;
                }
            }
        }
    }

    /// A block; returns the reading at its end.
    pub fn process(&mut self, block: &[f32]) -> f32 {
        for &x in block {
            self.next(x);
        }
        self.level
    }

    /// Linear reading now.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Reading in dBFS (-120 floor).
    pub fn level_dbfs(&self) -> f32 {
        if self.level <= 1e-6 {
            -120.0
        } else {
            20.0 * self.level.log10()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    /// The rates every requirement is checked at.
    const RATES: [f64; 4] = [44_100.0, 48_000.0, 96_000.0, 192_000.0];
    const TYPE_I: [PpmType; 2] = [PpmType::Din, PpmType::Nordic];
    const TYPE_II: [PpmType; 2] = [PpmType::Ebu, PpmType::Bbc];

    fn db(ratio: f64) -> f64 {
        20.0 * ratio.max(1e-12).log10()
    }

    /// Largest reading over the last quarter of 0.6 s of a unit sine.
    fn steady(kind: PpmType, fs: f64, f: f64, phase: f64) -> f64 {
        let mut p = Ppm::new(fs as f32, kind);
        let n = (0.6 * fs) as usize;
        let mut max = 0.0f64;
        for i in 0..n {
            let y = p.next((TAU * f * i as f64 / fs + phase).sin() as f32);
            if i >= n * 3 / 4 {
                max = max.max(f64::from(y));
            }
        }
        max
    }

    /// Largest reading for one burst of `ms` of a unit sine that starts at
    /// `phase`, `offset` of a sample after a sample instant; 20 ms of silence follow.
    fn burst_peak(kind: PpmType, fs: f64, f: f64, ms: f64, phase: f64, offset: f64) -> f64 {
        let mut p = Ppm::new(fs as f32, kind);
        let n = ((ms + 20.0) * 0.001 * fs) as usize;
        let mut max = 0.0f64;
        for i in 0..n {
            let t = (i as f64 - offset) / fs;
            let x = if t >= 0.0 && t < ms * 0.001 { (TAU * f * t + phase).sin() } else { 0.0 };
            max = max.max(f64::from(p.next(x as f32)));
        }
        max
    }

    /// One row of a tone-burst table: at every rate, for a burst that starts
    /// at a zero crossing and one that starts at a crest, on and between
    /// sample instants, the reading against the steady reading of the same
    /// tone is `want` +/- `tolerance` dB. The tone is 5 kHz, or 10 kHz for
    /// bursts of 0.5 ms and less (Tech 3205-E table 2, footnote).
    fn burst_row(kinds: [PpmType; 2], ms: f64, want: f64, tolerance: f64) {
        let f = if ms <= 0.5 { 10_000.0 } else { 5_000.0 };
        for kind in kinds {
            for fs in RATES {
                let reference = steady(kind, fs, f, 0.3);
                for phase in [0.0, TAU / 4.0] {
                    for offset in [0.0, 0.5] {
                        let got = db(burst_peak(kind, fs, f, ms, phase, offset) / reference);
                        assert!(
                            (got - want).abs() <= tolerance,
                            "{kind:?} at {fs} Hz, {ms} ms burst (phase {phase:.2}, offset {offset}): {got:.2} dB, wanted {want} +/- {tolerance}"
                        );
                    }
                }
            }
        }
    }

    // Type II, EBU Tech 3205-E table 2, with the table's tolerances.

    #[test]
    fn type_ii_100_ms_burst_reads_0_db() {
        burst_row(TYPE_II, 100.0, 0.0, 0.5);
    }

    #[test]
    fn type_ii_10_ms_burst_reads_minus_2_db() {
        burst_row(TYPE_II, 10.0, -2.0, 0.5);
    }

    #[test]
    fn type_ii_5_ms_burst_reads_minus_4_db() {
        burst_row(TYPE_II, 5.0, -4.0, 0.75);
    }

    #[test]
    fn type_ii_1_5_ms_burst_reads_minus_9_db() {
        burst_row(TYPE_II, 1.5, -9.0, 1.0);
    }

    #[test]
    fn type_ii_0_5_ms_burst_reads_minus_17_db() {
        burst_row(TYPE_II, 0.5, -17.0, 2.0);
    }

    // Type I, the public table (see the module documentation); tolerances are ours.

    #[test]
    fn type_i_100_ms_burst_reads_0_db() {
        burst_row(TYPE_I, 100.0, 0.0, 0.5);
    }

    #[test]
    fn type_i_10_ms_burst_reads_minus_1_db() {
        burst_row(TYPE_I, 10.0, -1.0, 0.5);
    }

    #[test]
    fn type_i_5_ms_burst_reads_minus_2_db() {
        burst_row(TYPE_I, 5.0, -2.0, 0.5);
    }

    #[test]
    fn type_i_3_ms_burst_reads_minus_4_db() {
        burst_row(TYPE_I, 3.0, -4.0, 0.75);
    }

    #[test]
    fn type_i_0_4_ms_burst_reads_minus_15_db() {
        burst_row(TYPE_I, 0.4, -15.0, 2.0);
    }

    /// Integration time: the burst that reads 2 dB low. Tech 3205-E 3.3 allows
    /// 10 +/- 2 ms; held here to +/- 1 ms, and Type I to 5 +/- 1 ms.
    #[test]
    fn integration_time_is_the_burst_that_reads_2_db_low() {
        for kind in [PpmType::Din, PpmType::Nordic, PpmType::Ebu, PpmType::Bbc] {
            let nominal = f64::from(kind.integration_ms());
            for fs in RATES {
                let reference = steady(kind, fs, 5000.0, 0.3);
                let short = db(burst_peak(kind, fs, 5000.0, nominal - 1.0, 0.0, 0.0) / reference);
                let long = db(burst_peak(kind, fs, 5000.0, nominal + 1.0, 0.0, 0.0) / reference);
                assert!(short < -2.0 && long > -2.0, "{kind:?} at {fs} Hz: {short:.2} dB at -1 ms, {long:.2} dB at +1 ms");
            }
        }
    }

    /// The readings are those of the cycle-averaged charging law the constants
    /// were derived from (module documentation), integrated here independently.
    #[test]
    fn bursts_follow_the_derived_charging_law() {
        fn charged(time_in_tau: f64) -> f64 {
            let g = |y: f64| (2.0 / PI) * ((1.0 - y * y).sqrt() - y * y.acos());
            let steps = 20_000;
            let h = time_in_tau / steps as f64;
            let mut y = 0.0;
            for _ in 0..steps {
                y += h * g(y + 0.5 * h * g(y));
            }
            y
        }
        for (kind, bursts) in [(PpmType::Din, [10.0, 5.0, 3.0]), (PpmType::Ebu, [10.0, 5.0, 1.5])] {
            let (decibels, seconds) = kind.return_time();
            let lambda = f64::from(decibels) / f64::from(seconds) * std::f64::consts::LN_10 / 20.0;
            let stages = kind.stages();
            let reference: f64 = stages.iter().map(|&(w, tau)| w * settled(tau, lambda)).sum();
            for ms in bursts {
                let law: f64 = stages.iter().map(|&(w, tau)| w * charged(ms * 0.001 / tau)).sum();
                let predicted = db(law / reference);
                for fs in RATES {
                    let got = db(burst_peak(kind, fs, 5000.0, ms, 0.0, 0.0) / steady(kind, fs, 5000.0, 0.3));
                    assert!((got - predicted).abs() < 0.1, "{kind:?} {ms} ms at {fs} Hz: {got:.3} dB, the law gives {predicted:.3}");
                }
            }
        }
    }

    /// Return time (Tech 3205-E 3.10; IEC Type I as quoted): from a steady
    /// 1 kHz reading, the fall of 24 dB takes 2.8 s (Type II) and of 20 dB
    /// 1.7 s (Type I), tolerance +/- 0.3 s; held here to +/- 0.02 s. The speed
    /// is constant: half the fall takes half the time.
    #[test]
    fn return_times_match_the_standard() {
        for kind in [PpmType::Din, PpmType::Nordic, PpmType::Ebu, PpmType::Bbc] {
            let (fall, seconds) = kind.return_time();
            let (fall, seconds) = (f64::from(fall), f64::from(seconds));
            for fs in RATES {
                let mut p = Ppm::new(fs as f32, kind);
                for i in 0..fs as usize {
                    p.next((TAU * 1000.0 * i as f64 / fs).sin() as f32);
                }
                let start = f64::from(p.level());
                let (mut half, mut full) = (0.0, 0.0);
                let mut n = 0usize;
                while full == 0.0 {
                    let down = -db(f64::from(p.next(0.0)) / start);
                    n += 1;
                    if half == 0.0 && down >= fall / 2.0 {
                        half = n as f64 / fs;
                    }
                    if down >= fall {
                        full = n as f64 / fs;
                    }
                    assert!(n < 10 * fs as usize);
                }
                assert!((full - seconds).abs() < 0.02, "{kind:?} at {fs} Hz: {full:.3} s for {fall} dB, wanted {seconds}");
                assert!((half - seconds / 2.0).abs() < 0.02, "{kind:?} at {fs} Hz: {half:.3} s for half the fall");
            }
        }
    }

    /// Tech 3205-E 3.8 and 3.9: a 1 kHz tone switched on reads within 1 dB of
    /// its steady reading inside 150 ms and never more than 0.5 dB above it.
    /// Held here to 40 ms and 0.05 dB.
    #[test]
    fn delay_time_and_overswing() {
        for kind in [PpmType::Din, PpmType::Nordic, PpmType::Ebu, PpmType::Bbc] {
            for fs in RATES {
                let reference = steady(kind, fs, 1000.0, 0.0);
                let mut p = Ppm::new(fs as f32, kind);
                let (mut max, mut delay) = (0.0f64, None);
                for i in 0..fs as usize {
                    let y = f64::from(p.next((TAU * 1000.0 * i as f64 / fs).sin() as f32));
                    max = max.max(y);
                    if delay.is_none() && db(y / reference) >= -1.0 {
                        delay = Some(i as f64 / fs);
                    }
                }
                let delay = delay.expect("reaches -1 dB");
                assert!(delay < 0.040, "{kind:?} at {fs} Hz: {delay} s to within 1 dB");
                assert!(db(max / reference) < 0.05, "{kind:?} at {fs} Hz: overswing {} dB", db(max / reference));
            }
        }
    }

    /// A steady 1 kHz sine reads its own level: with alignment at -18 dBFS
    /// (EBU R 68) a -18 dBFS tone is TEST on the EBU and Nordic scales, 4 on
    /// the BBC scale and -9 on the DIN scale, within 0.05 dB.
    #[test]
    fn alignment_tone_reads_the_reference_mark() {
        let amplitude = 10f64.powf(-18.0 / 20.0);
        for (kind, mark, per_db) in [(PpmType::Ebu, 0.0, 1.0), (PpmType::Nordic, 0.0, 1.0), (PpmType::Din, -9.0, 1.0), (PpmType::Bbc, 4.0, 0.25)] {
            for fs in RATES {
                let mut p = Ppm::new(fs as f32, kind);
                for i in 0..fs as usize {
                    p.next((amplitude * (TAU * 1000.0 * i as f64 / fs).sin()) as f32);
                }
                let dbfs = p.level_dbfs();
                assert!((dbfs + 18.0).abs() < 0.05, "{kind:?} at {fs} Hz: {dbfs} dBFS for a -18 dBFS tone");
                let shown = kind.display(dbfs, -18.0);
                assert!((shown - mark).abs() < 0.05 * per_db, "{kind:?} at {fs} Hz: shows {shown}, wanted {mark}");
            }
        }
    }

    /// Tech 3205-E 2.3: within +/- 0.3 dB of the 1 kHz reading from 31.5 Hz to
    /// 16 kHz, here also for a tone at a quarter of the sample rate sampled at
    /// 45 degrees, whose sample peaks are 3 dB under its true peak.
    #[test]
    fn steady_reading_is_flat_from_31_5_hz_to_16_khz() {
        for kind in [PpmType::Din, PpmType::Ebu] {
            for fs in RATES {
                let reference = steady(kind, fs, 1000.0, 0.3);
                assert!(db(reference).abs() < 0.05, "{kind:?} at {fs} Hz: 1 kHz reads {} dBFS", db(reference));
                for f in [31.5, 125.0, 4000.0, 12_000.0, 16_000.0] {
                    for phase in [0.0, TAU / 8.0] {
                        let got = db(steady(kind, fs, f, phase) / reference);
                        assert!(got.abs() <= 0.3, "{kind:?} at {fs} Hz, {f} Hz tone: {got:.3} dB re 1 kHz");
                    }
                }
                if fs / 4.0 <= 16_000.0 {
                    let got = db(steady(kind, fs, fs / 4.0, TAU / 8.0) / reference);
                    assert!(got.abs() <= 0.3, "{kind:?} at {fs} Hz, fs/4 at 45 degrees: {got:.3} dB re 1 kHz");
                }
            }
        }
    }

    /// Tech 3205-E 2.4: a tone with the crests of one polarity cut to 25 %
    /// (DC removed) reads the same either way up, within 0.5 dB; here 0.01 dB.
    #[test]
    fn reading_does_not_depend_on_polarity() {
        let per = 4096;
        let mean = (0..per).map(|j| (TAU * j as f64 / per as f64).sin().max(-0.25)).sum::<f64>() / per as f64;
        for kind in [PpmType::Din, PpmType::Ebu] {
            for fs in RATES {
                let mut readings = [0.0f64; 2];
                for (reading, sign) in readings.iter_mut().zip([1.0, -1.0]) {
                    let mut p = Ppm::new(fs as f32, kind);
                    for i in 0..(fs / 2.0) as usize {
                        let s = (TAU * 1000.0 * i as f64 / fs).sin().max(-0.25) - mean;
                        *reading = reading.max(f64::from(p.next((sign * s) as f32)));
                    }
                }
                let difference = db(readings[0] / readings[1]).abs();
                assert!(difference < 0.01, "{kind:?} at {fs} Hz: {difference} dB between polarities");
            }
        }
    }

    /// Tech 3205-E 3.5: 10 ms bursts at falling levels read lower by the same
    /// number of dB (within 1 dB; the model is linear, so 0.01 dB).
    #[test]
    fn burst_readings_scale_with_level() {
        for fs in RATES {
            let full = burst_peak(PpmType::Ebu, fs, 5000.0, 10.0, 0.0, 0.0);
            for down in [10.0, 20.0, 40.0] {
                let gain = 10f64.powf(-down / 20.0);
                let mut p = Ppm::new(fs as f32, PpmType::Ebu);
                let mut max = 0.0f64;
                for i in 0..(0.03 * fs) as usize {
                    let t = i as f64 / fs;
                    let x = if t < 0.01 { gain * (TAU * 5000.0 * t).sin() } else { 0.0 };
                    max = max.max(f64::from(p.next(x as f32)));
                }
                assert!((db(max / full) + down).abs() < 0.01, "at {fs} Hz, {down} dB down: {}", db(max / full));
            }
        }
    }

    /// Not a requirement of the public texts (normal mode): 10 ms bursts every
    /// 100 ms read between the single burst and the steady tone, the same at
    /// every rate.
    #[test]
    fn repeated_bursts_read_between_one_burst_and_steady() {
        for kind in [PpmType::Din, PpmType::Ebu] {
            let mut readings = [0.0f64; 4];
            for (reading, fs) in readings.iter_mut().zip(RATES) {
                let mut p = Ppm::new(fs as f32, kind);
                let mut max = 0.0f64;
                for i in 0..fs as usize {
                    let t = (i as f64 / fs) % 0.1;
                    let x = if t < 0.01 { (TAU * 5000.0 * t).sin() } else { 0.0 };
                    max = max.max(f64::from(p.next(x as f32)));
                }
                let reference = steady(kind, fs, 5000.0, 0.3);
                *reading = db(max / reference);
                let single = db(burst_peak(kind, fs, 5000.0, 10.0, 0.0, 0.0) / reference);
                assert!(*reading > single && *reading < 0.0, "{kind:?} at {fs} Hz: {reading} dB repeated, {single} dB single");
            }
            let spread = readings.iter().fold(f64::MIN, |a, &b| a.max(b)) - readings.iter().fold(f64::MAX, |a, &b| a.min(b));
            assert!(spread < 0.1, "{kind:?}: {readings:?}");
        }
    }

    #[test]
    fn oversampling_follows_the_rate() {
        for (rate, factor) in [(8_000.0, 8), (44_100.0, 8), (48_000.0, 8), (88_200.0, 4), (96_000.0, 4), (176_400.0, 2), (192_000.0, 2), (352_800.0, 1), (384_000.0, 1)] {
            assert_eq!(Ppm::new(rate, PpmType::Ebu).oversampling(), factor, "{rate}");
        }
        // The plain path (no interpolation) has the same ballistics.
        let reference = steady(PpmType::Ebu, 384_000.0, 5000.0, 0.3);
        let got = db(burst_peak(PpmType::Ebu, 384_000.0, 5000.0, 10.0, 0.0, 0.0) / reference);
        assert!((got + 2.0).abs() <= 0.5, "{got}");
        assert!(db(reference).abs() < 0.05);
    }

    #[test]
    fn scales_map_alignment_and_maximum() {
        let align = -18.0;
        let pml = align + 9.0;
        assert_eq!(PpmType::Ebu.display(align, align), 0.0);
        assert_eq!(PpmType::Ebu.display(pml, align), 9.0);
        assert_eq!(PpmType::Nordic.display(pml, align), 9.0);
        assert_eq!(PpmType::Din.display(align, align), -9.0);
        assert_eq!(PpmType::Din.display(pml, align), 0.0);
        assert_eq!(PpmType::Bbc.display(align, align), 4.0);
        assert_eq!(PpmType::Bbc.display(align + 8.0, align), 6.0);
    }

    #[test]
    fn ignores_non_finite_and_resets() {
        let mut p = Ppm::new(48_000.0, PpmType::Ebu);
        assert_eq!(p.next(f32::NAN), 0.0);
        assert_eq!(p.next(f32::INFINITY), 0.0);
        p.process(&[1.0; 4800]);
        assert!(p.level_dbfs() > -1.0);
        p.reset();
        assert_eq!(p.level_dbfs(), -120.0);
        assert_eq!(p.next(0.0), 0.0);
    }

    /// Silence takes the reading to exactly zero: no denormals are left to decay.
    #[test]
    fn silence_ends_at_exactly_zero() {
        for kind in [PpmType::Din, PpmType::Ebu] {
            let mut p = Ppm::new(48_000.0, kind);
            p.process(&[0.5; 480]);
            let mut n = 0usize;
            while p.next(0.0) != 0.0 {
                n += 1;
                assert!(n < 48_000 * 40, "{kind:?} never reaches zero");
            }
            assert!(p.stage.iter().all(|&y| y == 0.0));
        }
    }
}
