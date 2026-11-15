//! Peak programme meter (PPM) ballistics after IEC 60268-10.
//!
//! A PPM is a quasi-peak detector: a short integration time on the way up, so
//! that a brief burst reads a little low, and a slow, steady fall in dB on
//! the way down. [`Ppm`] models it as a rectifier feeding a capacitor that
//! charges through a resistor (attack time constant) and discharges at a
//! fixed rate in dB per second (the return time).
//!
//! | [`PpmType`] | Standard | Integration time | Return time |
//! |---|---|---|---|
//! | `Din` | DIN 45406 = IEC 60268-10 Type I | 5 ms | 20 dB in 1.7 s (+/- 0.3 s) |
//! | `Nordic` | IEC 60268-10 Type I, Nordic scale (NRK, SR, DR, YLE) | 5 ms | 20 dB in 1.7 s |
//! | `Ebu` | IEC 60268-10 Type IIb (EBU Tech 3205-E) | 10 ms | 20 dB in 1.7 s (+/- 0.3 s) |
//! | `Bbc` | IEC 60268-10 Type IIa (BBC) | 10 ms | 24 dB in 2.8 s (+/- 0.3 s) |
//!
//! "Integration time" is the standard's: the time for which a burst of a
//! steady tone must last to be read 2 dB (80 %) below the steady reading.
//! The charging model is a diode and capacitor: it charges only while the
//! rectified input is above the reading, so a plain one-pole of time constant
//! `T / ln 5` would read too low (the charging window is short, near each
//! peak). The time constant used is `0.5 T / ln 5`, found by measuring tone
//! bursts at 44.1, 48 and 96 kHz and 1 and 5 kHz; the result does not depend
//! on rate or tone frequency to 0.05 dB. Measured against the tone-burst table
//! (reading below the steady value, in dB; the table is Type I: 10 ms -1,
//! 5 ms -2, 3 ms -4, 1.5 ms -9; Type II: 10 ms -2, 5 ms -4) the model reads,
//! at 48 kHz, Type I: -0.9, -2.4, -4.1, -7.6; Type II: -2.2 (10 ms),
//! -4.8 (5 ms). The nearest points are within 0.4 dB, the farthest (Type I
//! at 1.5 ms) 1.5 dB. Real meters are a few-pole network; this is the
//! simplest model that honours the defining integration time. It is an
//! engineering model after the standard, not a type approval. The tolerances
//! of IEC 60268-10 were not available to the author; the tests use +/- 1 dB
//! at the defining points and +/- 1.6 dB elsewhere, stated as the model's own.
//!
//! The dB scales differ per type; [`PpmType::display`] maps a level to the
//! number on the meter, given the alignment level in dBFS (EBU R 68 and
//! EBU Tech 3205 put alignment at -18 dBFS, 0 dBu):
//!
//! - `Ebu` and `Nordic`: dB re alignment ("TEST" is 0, the permitted maximum
//!   level is +9);
//! - `Din`: dB re the permitted maximum level, which is 9 dB above alignment
//!   (alignment reads -9, permitted maximum 0);
//! - `Bbc`: scale numbers 1 to 7 at 4 dB per step, alignment at 4, the
//!   permitted maximum (+8 dB) at 6.
//!
//! The alignment and permitted-maximum levels and the scale layouts are from
//! the tables in IEC 60268-10 as summarised by EBU Tech 3205-E and the
//! "Peak programme meter" survey in the AES/BBC literature.

/// Which PPM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
            Self::Din | Self::Nordic | Self::Ebu => (20.0, 1.7),
            Self::Bbc => (24.0, 2.8),
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
}

/// One channel of PPM detector.
#[derive(Clone, Copy, Debug)]
pub struct Ppm {
    kind: PpmType,
    attack: f32,
    release: f32,
    level: f32,
}

impl Ppm {
    pub fn new(sample_rate: f32, kind: PpmType) -> Self {
        let rate = crate::rate::effective_rate(sample_rate);
        let (db, seconds) = kind.return_time();
        let tau = f64::from(kind.integration_ms()) * 0.001 / 5f64.ln();
        let attack = 1.0 - (-1.0 / (tau * f64::from(rate) * ATTACK_SCALE)).exp();
        let release = 10f64.powf(-f64::from(db) / 20.0 / (f64::from(seconds) * f64::from(rate)));
        Self { kind, attack: attack as f32, release: release as f32, level: 0.0 }
    }

    pub fn kind(&self) -> PpmType {
        self.kind
    }

    pub fn reset(&mut self) {
        self.level = 0.0;
    }

    /// One sample; returns the linear reading.
    #[inline]
    pub fn next(&mut self, x: f32) -> f32 {
        let v = if x.is_finite() { x.abs() } else { 0.0 };
        if v > self.level {
            self.level += (v - self.level) * self.attack;
        } else {
            self.level *= self.release;
        }
        self.level
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

const ATTACK_SCALE: f64 = 0.50;

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    fn tone(f: f32, n: usize) -> impl Iterator<Item = f32> {
        (0..n).map(move |i| (std::f32::consts::TAU * f * i as f32 / FS).sin())
    }

    fn steady(kind: PpmType) -> f32 {
        let mut p = Ppm::new(FS, kind);
        p.process(&tone(1000.0, 24_000).collect::<Vec<_>>());
        p.level()
    }

    /// Reading peak of a burst relative to the steady reading, dB.
    fn burst_db(kind: PpmType, f: f32, ms: f32) -> f32 {
        let mut p = Ppm::new(FS, kind);
        let n = (ms * 0.001 * FS) as usize;
        let mut mx = 0.0f32;
        for x in tone(f, n) {
            mx = mx.max(p.next(x));
        }
        20.0 * (mx / steady(kind)).log10()
    }

    #[test]
    fn tone_bursts_read_as_the_standard_table() {
        // (type, burst ms, expected dB, tolerance)
        let cases = [
            (PpmType::Din, 100.0, 0.0, 0.5),
            (PpmType::Din, 10.0, -1.0, 1.0),
            (PpmType::Din, 5.0, -2.0, 1.0),
            (PpmType::Din, 3.0, -4.0, 1.6),
            (PpmType::Din, 1.5, -9.0, 1.6),
            (PpmType::Nordic, 5.0, -2.0, 1.0),
            (PpmType::Ebu, 100.0, 0.0, 0.5),
            (PpmType::Ebu, 10.0, -2.0, 1.0),
            (PpmType::Ebu, 5.0, -4.0, 1.0),
            (PpmType::Bbc, 10.0, -2.0, 1.0),
            (PpmType::Bbc, 5.0, -4.0, 1.0),
        ];
        for (kind, ms, expected, tol) in cases {
            for f in [1000.0, 5000.0] {
                let got = burst_db(kind, f, ms);
                assert!((got - expected).abs() <= tol, "{kind:?} {ms} ms {f} Hz: {got} dB, wanted {expected} +/- {tol}");
            }
        }
    }

    /// Return time: from a steady reading, silence; time to fall 20 dB (24 dB BBC).
    #[test]
    fn return_times_match_the_standard() {
        for kind in [PpmType::Din, PpmType::Nordic, PpmType::Ebu, PpmType::Bbc] {
            let (db, seconds) = kind.return_time();
            let mut p = Ppm::new(FS, kind);
            p.process(&tone(1000.0, 48_000).collect::<Vec<_>>());
            let start = p.level();
            let mut n = 0usize;
            while 20.0 * (p.next(0.0) / start).log10() > -db {
                n += 1;
                assert!(n < 480_000);
            }
            let took = n as f32 / FS;
            assert!((took - seconds).abs() < 0.02, "{kind:?}: {took} s for {db} dB, wanted {seconds}");
        }
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
        let mut p = Ppm::new(FS, PpmType::Ebu);
        assert_eq!(p.next(f32::NAN), 0.0);
        p.process(&[1.0; 4800]);
        assert!(p.level_dbfs() > -1.0);
        p.reset();
        assert_eq!(p.level_dbfs(), -120.0);
    }
}
