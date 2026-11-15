//! ADSR and AHDSR envelope generator.
//!
//! Attack, optional hold, decay to a sustain level, and release, each linear
//! or exponential. The stage lengths are whole numbers of samples
//! (`round(seconds * sample_rate)`), so a stage takes exactly that many samples
//! and the level reaches its target on the sample the stage ends, never
//! asymptotically. This is the usual "analog" envelope with a hard guarantee on
//! timing; the shape is a normalised exponential:
//!
//! ```text
//! rise(p)  = (1 - e^(-c p)) / (1 - e^(-c))      attack, fast start, slow finish
//! fall(p)  = (e^(-c p) - e^(-c)) / (1 - e^(-c)) decay and release, fast start
//! level    = from + (to - from) * rise(p)   (attack)
//! level    = to + (from - to) * fall(p)     (decay, release)
//! ```
//!
//! with `p = k / length` and curvature `c` (default 5, about 43 dB over the
//! stage). `rise(0) = 0`, `rise(1) = 1`, `fall(0) = 1`, `fall(1) = 0` for any
//! `c`, which is what makes the timing exact. The curvature is the usual RC
//! charge/discharge curve scaled to end on target (see Reiss and McPherson,
//! *Audio Effects: Theory, Implementation and Application*, CRC 2014, ch. 6 on
//! envelopes; and Bristow-Johnson, "Envelope generators" notes on analog ADSR
//! behaviour).
//!
//! # Conventions
//!
//! * [`Adsr::next`] returns the level for the current sample, then steps. The
//!   first sample after [`Adsr::gate_on`] is the starting level; the sample
//!   `attack_samples` later is exactly 1.0. Hold holds 1.0, decay starts after
//!   `attack + hold` samples and lands on the sustain level `decay` samples
//!   after that. After [`Adsr::gate_off`] the level falls to exactly 0.0 in
//!   `release` samples, from wherever it was.
//! * A stage of length zero is skipped.
//! * Re-triggering starts the attack from the current level, so there is no
//!   click. The attack time is scaled by the distance still to climb
//!   (`attack * (1 - level)`), so the rate stays constant. From silence it is
//!   the full attack time.
//! * [`RetriggerMode::Legato`]: a `gate_on` while the gate is already held
//!   (attack, hold, decay or sustain) is ignored, so overlapping notes on a
//!   mono voice do not restart the envelope. During release or idle it starts
//!   a normal attack.
//! * The sustain level is read each sample while sustaining, so a change moves
//!   the level in one step. Smooth it in the plugin (`openplugin_core::smooth`)
//!   if it is automated.
//! * Sample accuracy inside a block: [`Adsr::process`] takes gate events with
//!   block offsets and applies each at its sample.
//!
//! No allocation, and every setter sanitises NaN and infinity. The level is
//! `f64` inside and flushed to exact zero at the end of release, so it cannot
//! become denormal.

/// Shape of a stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Curve {
    Linear,
    Exponential,
}

/// What `gate_on` does while the gate is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetriggerMode {
    /// Every `gate_on` restarts the attack from the current level.
    Retrigger,
    /// A `gate_on` while held is ignored.
    Legato,
}

/// Current stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Idle,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
}

/// A gate event for [`Adsr::process`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    On,
    Off,
}

const MAX_SECONDS: f32 = 3600.0;

/// ADSR (hold of zero) or AHDSR envelope. See the module documentation.
#[derive(Clone, Copy, Debug)]
pub struct Adsr {
    sample_rate: f64,
    attack: f64,
    hold: f64,
    decay: f64,
    sustain: f64,
    release: f64,
    attack_curve: Curve,
    decay_curve: Curve,
    release_curve: Curve,
    curvature: f64,
    mode: RetriggerMode,
    stage: Stage,
    level: f64,
    from: f64,
    to: f64,
    pos: u64,
    len: u64,
}

/// An AHDSR is an [`Adsr`] with a non-zero hold.
pub type Ahdsr = Adsr;

fn secs(v: f32) -> f64 {
    if v.is_finite() { f64::from(v.clamp(0.0, MAX_SECONDS)) } else { 0.0 }
}

impl Adsr {
    /// An idle envelope: attack 10 ms, no hold, decay 100 ms, sustain 0.7, release 200 ms,
    /// exponential decay and release, linear attack.
    pub fn new(sample_rate: f32) -> Self {
        let mut e = Self {
            sample_rate: 48_000.0,
            attack: 0.01,
            hold: 0.0,
            decay: 0.1,
            sustain: 0.7,
            release: 0.2,
            attack_curve: Curve::Linear,
            decay_curve: Curve::Exponential,
            release_curve: Curve::Exponential,
            curvature: 5.0,
            mode: RetriggerMode::Retrigger,
            stage: Stage::Idle,
            level: 0.0,
            from: 0.0,
            to: 0.0,
            pos: 0,
            len: 0,
        };
        e.set_sample_rate(sample_rate);
        e
    }

    /// The real sample rate. A non-finite or non-positive value is ignored.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if sample_rate.is_finite() && sample_rate > 0.0 {
            self.sample_rate = f64::from(sample_rate);
        }
    }

    /// Attack, hold, decay and release in seconds; sustain is a level 0..1.
    pub fn set_ahdsr(&mut self, attack: f32, hold: f32, decay: f32, sustain: f32, release: f32) {
        self.attack = secs(attack);
        self.hold = secs(hold);
        self.decay = secs(decay);
        self.release = secs(release);
        self.set_sustain(sustain);
    }

    /// Attack, decay and release in seconds and the sustain level; hold is zero.
    pub fn set_adsr(&mut self, attack: f32, decay: f32, sustain: f32, release: f32) {
        self.set_ahdsr(attack, 0.0, decay, sustain, release);
    }

    /// Sustain level, clamped to 0..=1 (NaN ignored).
    pub fn set_sustain(&mut self, sustain: f32) {
        if sustain.is_finite() {
            self.sustain = f64::from(sustain.clamp(0.0, 1.0));
        }
    }

    pub fn set_curves(&mut self, attack: Curve, decay: Curve, release: Curve) {
        self.attack_curve = attack;
        self.decay_curve = decay;
        self.release_curve = release;
    }

    /// Exponential curvature `c`, clamped to 0.1..=20 (NaN ignored).
    pub fn set_curvature(&mut self, c: f32) {
        if c.is_finite() {
            self.curvature = f64::from(c.clamp(0.1, 20.0));
        }
    }

    pub fn set_mode(&mut self, mode: RetriggerMode) {
        self.mode = mode;
    }

    pub fn mode(&self) -> RetriggerMode {
        self.mode
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// Current level (the value the next call to [`Self::next`] returns).
    pub fn level(&self) -> f32 {
        self.level as f32
    }

    /// Not idle (still producing a non-zero or rising level).
    pub fn is_active(&self) -> bool {
        self.stage != Stage::Idle
    }

    /// Whether the gate is held (attack through sustain).
    pub fn is_gated(&self) -> bool {
        matches!(self.stage, Stage::Attack | Stage::Hold | Stage::Decay | Stage::Sustain)
    }

    /// Stage length in samples for `seconds`.
    pub fn samples_for(&self, seconds: f32) -> u64 {
        (secs(seconds) * self.sample_rate).round() as u64
    }

    fn to_samples(&self, seconds: f64) -> u64 {
        (seconds * self.sample_rate).round() as u64
    }

    /// Key down.
    pub fn gate_on(&mut self) {
        if self.mode == RetriggerMode::Legato && self.is_gated() {
            return;
        }
        let len = self.to_samples(self.attack * (1.0 - self.level));
        self.begin(Stage::Attack, self.level, 1.0, len);
        self.settle();
    }

    /// Key up. Ignored when idle or already releasing.
    pub fn gate_off(&mut self) {
        if !self.is_gated() {
            return;
        }
        let len = self.to_samples(self.release);
        self.begin(Stage::Release, self.level, 0.0, len);
        self.settle();
    }

    /// Cut to silence now, without a release.
    pub fn kill(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
        self.pos = 0;
        self.len = 0;
    }

    fn begin(&mut self, stage: Stage, from: f64, to: f64, len: u64) {
        self.stage = stage;
        self.from = from;
        self.to = to;
        self.pos = 0;
        self.len = len;
        self.level = from;
    }

    /// Skip zero-length stages and finished ones until the stage has work left.
    fn settle(&mut self) {
        for _ in 0..8 {
            if matches!(self.stage, Stage::Idle | Stage::Sustain) {
                if self.stage == Stage::Sustain {
                    self.level = self.sustain;
                }
                return;
            }
            if self.pos < self.len {
                return;
            }
            // The stage is complete: land exactly on its target and move on.
            self.level = self.to;
            match self.stage {
                Stage::Attack => {
                    let len = self.to_samples(self.hold);
                    self.begin(Stage::Hold, 1.0, 1.0, len);
                }
                Stage::Hold => {
                    let len = self.to_samples(self.decay);
                    self.begin(Stage::Decay, 1.0, self.sustain, len);
                }
                Stage::Decay => {
                    self.begin(Stage::Sustain, self.sustain, self.sustain, 0);
                }
                Stage::Release => {
                    self.kill();
                }
                Stage::Idle | Stage::Sustain => return,
            }
        }
    }

    fn shape(&self, curve: Curve, rising: bool, p: f64) -> f64 {
        match curve {
            Curve::Linear => {
                if rising { p } else { 1.0 - p }
            }
            Curve::Exponential => {
                let c = self.curvature;
                let denom = 1.0 - (-c).exp();
                if rising {
                    (1.0 - (-c * p).exp()) / denom
                } else {
                    ((-c * p).exp() - (-c).exp()) / denom
                }
            }
        }
    }

    fn advance(&mut self) {
        match self.stage {
            Stage::Idle => {}
            Stage::Sustain => self.level = self.sustain,
            Stage::Hold => {
                self.pos += 1;
                self.settle();
            }
            Stage::Attack | Stage::Decay | Stage::Release => {
                self.pos += 1;
                if self.pos >= self.len {
                    self.settle();
                } else {
                    let p = self.pos as f64 / self.len as f64;
                    self.level = match self.stage {
                        Stage::Attack => {
                            self.from + (self.to - self.from) * self.shape(self.attack_curve, true, p)
                        }
                        Stage::Decay => {
                            self.to + (self.from - self.to) * self.shape(self.decay_curve, false, p)
                        }
                        _ => self.to + (self.from - self.to) * self.shape(self.release_curve, false, p),
                    };
                }
            }
        }
    }

    /// The level for this sample, then one step.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        let out = self.level as f32;
        self.advance();
        if out.abs() < 1e-30 { 0.0 } else { out }
    }

    /// Fill `out`, applying `events` (block offset, gate) at their sample, before
    /// that sample is produced. Events must be sorted by offset; those at or past
    /// `out.len()` are ignored (the caller carries them to the next block).
    pub fn process(&mut self, out: &mut [f32], events: &[(usize, Gate)]) {
        let mut ev = 0;
        for (i, v) in out.iter_mut().enumerate() {
            while ev < events.len() && events[ev].0 <= i {
                match events[ev].1 {
                    Gate::On => self.gate_on(),
                    Gate::Off => self.gate_off(),
                }
                ev += 1;
            }
            *v = self.next();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(e: &mut Adsr, n: usize) -> Vec<f32> {
        (0..n).map(|_| e.next()).collect()
    }

    #[test]
    fn stage_timings_are_exact_to_the_sample() {
        for sr in [22_050.0f32, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for curves in [
                (Curve::Linear, Curve::Linear, Curve::Linear),
                (Curve::Exponential, Curve::Exponential, Curve::Exponential),
            ] {
                let mut e = Adsr::new(sr);
                e.set_ahdsr(0.013, 0.007, 0.021, 0.4, 0.034);
                e.set_curves(curves.0, curves.1, curves.2);
                let a = e.samples_for(0.013) as usize;
                let h = e.samples_for(0.007) as usize;
                let d = e.samples_for(0.021) as usize;
                let r = e.samples_for(0.034) as usize;
                e.gate_on();
                let x = run(&mut e, a + h + d + 50);
                assert_eq!(x[0], 0.0);
                // Peak is reached on exactly sample `a`, not before.
                assert!(x[a - 1] < 1.0 && x[a] == 1.0, "attack end at {sr}");
                // Hold: still 1.0 through a + h, decay has begun one sample later.
                assert_eq!(x[a + h], 1.0);
                assert!(x[a + h + 1] < 1.0);
                // Sustain level is reached on exactly sample a + h + d.
                assert!(x[a + h + d - 1] > 0.4 + 1e-6, "{}", x[a + h + d - 1]);
                assert!((x[a + h + d] - 0.4).abs() < 1e-7);
                assert!(x[a + h + d + 49] == x[a + h + d]);
                assert_eq!(e.stage(), Stage::Sustain);
                e.gate_off();
                let y = run(&mut e, r + 5);
                assert!((y[0] - 0.4).abs() < 1e-7);
                assert!(y[r - 1] > 0.0 && y[r] == 0.0, "release end at {sr}: {} {}", y[r - 1], y[r]);
                assert!(y[r + 4] == 0.0);
                assert!(!e.is_active());
            }
        }
    }

    #[test]
    fn exponential_attack_and_release_are_monotone_and_bounded() {
        let mut e = Adsr::new(48_000.0);
        e.set_adsr(0.05, 0.1, 0.5, 0.3);
        e.set_curves(Curve::Exponential, Curve::Exponential, Curve::Exponential);
        e.gate_on();
        let x = run(&mut e, 2400);
        assert!(x.windows(2).all(|w| w[1] >= w[0]));
        assert!(x.iter().all(|v| (0.0..=1.0).contains(v)));
        // Exponential attack is above the straight line early on.
        assert!(x[600] > 0.25 + 0.1);
        e.gate_off();
        let y = run(&mut e, 14_400);
        assert!(y.windows(2).all(|w| w[1] <= w[0]));
    }

    #[test]
    fn retrigger_restarts_from_the_current_level_and_legato_does_not() {
        let mut e = Adsr::new(48_000.0);
        e.set_adsr(0.01, 0.5, 0.5, 0.5);
        e.gate_on();
        run(&mut e, 2000);
        let before = e.level();
        e.gate_on(); // retrigger
        assert_eq!(e.stage(), Stage::Attack);
        assert!((e.level() - before).abs() < 1e-6, "no jump");
        run(&mut e, 480);
        assert!(e.level() >= before);

        let mut l = Adsr::new(48_000.0);
        l.set_adsr(0.01, 0.5, 0.5, 0.5);
        l.set_mode(RetriggerMode::Legato);
        l.gate_on();
        run(&mut l, 2000);
        let stage = l.stage();
        let lvl = l.level();
        l.gate_on();
        assert_eq!(l.stage(), stage);
        assert_eq!(l.level(), lvl);
        l.gate_off();
        run(&mut l, 100);
        l.gate_on(); // during release: attacks again
        assert_eq!(l.stage(), Stage::Attack);
    }

    #[test]
    fn events_are_sample_accurate_and_split_blocks_match() {
        let mut one = Adsr::new(48_000.0);
        let mut split = Adsr::new(48_000.0);
        for e in [&mut one, &mut split] {
            e.set_ahdsr(0.002, 0.001, 0.003, 0.3, 0.004);
        }
        let mut a = vec![0.0f32; 1000];
        one.process(&mut a, &[(10, Gate::On), (500, Gate::Off)]);
        let mut b = vec![0.0f32; 1000];
        let (b1, b2) = b.split_at_mut(333);
        split.process(b1, &[(10, Gate::On)]);
        split.process(b2, &[(500 - 333, Gate::Off)]);
        assert_eq!(a, b);
        assert_eq!(a[9], 0.0);
        assert_eq!(a[10], 0.0);
        assert!(a[11] > 0.0);
        assert!(a[999] == 0.0 && a[499] > 0.0);
    }

    #[test]
    fn zero_lengths_and_hostile_values() {
        let mut e = Adsr::new(f32::NAN);
        e.set_ahdsr(0.0, f32::NAN, f32::INFINITY, 2.0, -1.0);
        e.set_curvature(f32::NAN);
        e.gate_on();
        for _ in 0..10 {
            assert!(e.next().is_finite());
        }
        e.gate_off();
        assert_eq!(e.next(), 0.0);
        let mut z = Adsr::new(48_000.0);
        z.set_adsr(0.0, 0.0, 0.6, 0.0);
        z.gate_on();
        assert_eq!(z.next(), 0.6);
        z.gate_off();
        assert_eq!(z.next(), 0.0);
        assert!(!z.is_active());
    }
}
