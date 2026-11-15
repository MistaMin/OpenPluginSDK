//! Plate reverberator after Dattorro.
//!
//! Source: Jon Dattorro, "Effect Design, Part 1: Reverberator and Other
//! Filters", Journal of the Audio Engineering Society 45(9), 660-684, 1997.
//! The topology below is the "plate" of section 3 / figure 1 of that paper,
//! written from the paper's description, not from any implementation.
//!
//! ```text
//!  in L+R -> pre-delay -> bandwidth one-pole -> 4 input allpasses
//!      (0.75: 142, 107 | 0.625: 379, 277)
//!                         |
//!       +-----------------+-----------------+
//!       v                                   v
//!   +-> sum <- decay * right tank end   sum <- decay * left tank end <-+
//!   |   mod-allpass 672 (g = -0.7)        mod-allpass 908 (g = -0.7)   |
//!   |   delay 4453                        delay 4217                   |
//!   |   damping one-pole * decay          damping one-pole * decay     |
//!   |   allpass 1800 (g = decay + 0.15)   allpass 2656                 |
//!   +-- delay 3720 (left tank end)        delay 3163 (right tank end) -+
//! ```
//!
//! The two halves form a figure-eight loop. Each output channel is the sum of
//! seven taps into the *other* half and the same half (the paper's table of
//! output taps, with the signs given there). All lengths are the paper's
//! integer delays at 29 761 Hz; they are rescaled to the real sample rate by
//! `fs / 29761` and rounded, so the sound (delay times in seconds) is the same
//! at 44.1, 48, 96 kHz.
//!
//! Differences from the paper, all deliberate: the input one-pole and the
//! damping one-pole are set by a corner frequency in hertz (so they do not
//! drift with the sample rate; the paper's coefficient `c` corresponds to
//! `fc = -ln(c) fs / 2 pi`), the decay is set as a reverberation time (the
//! loop passes through four `decay` multiplications and `T` seconds of delay,
//! so `decay = 10^(-3 T / (4 RT60))`), the allpass uses the standard Schroeder
//! form `w = x + g z; y = z - g w`, and the two modulated allpasses are read
//! with first-order allpass interpolation.
//!
//! Allocation happens in [`PlateReverb::new`] only.

use crate::rate::effective_rate;
use crate::reverb_common::{PreDelay, Ring, apply_width, clean, flush};

/// Sample rate at which the paper gives its delay lengths.
pub const PLATE_PAPER_RATE: f32 = 29_761.0;
/// Decay time range, seconds.
pub const PLATE_RT60_RANGE: (f32, f32) = (0.1, 120.0);
/// Longest pre-delay, milliseconds.
pub const PLATE_MAX_PREDELAY_MS: f32 = 500.0;

/// Paper delay lengths (samples at 29 761 Hz).
const INPUT_AP: [(usize, f32); 4] = [(142, 0.75), (107, 0.75), (379, 0.625), (277, 0.625)];
const L_AP1: usize = 672;
const L_D1: usize = 4453;
const L_AP2: usize = 1800;
const L_D2: usize = 3720;
const R_AP1: usize = 908;
const R_D1: usize = 4217;
const R_AP2: usize = 2656;
const R_D2: usize = 3163;
/// Peak LFO excursion of the two modulated allpasses, paper samples.
const MOD_EXCURSION: f32 = 16.0;
/// Decay gain while frozen: loop gain 0.9998^4 per ~0.73 s, bounded and
/// effectively infinite.
const FREEZE_DECAY: f32 = 0.9998;
const OUT_GAIN: f32 = 0.6;
/// Empirical correction of the decay rule `decay = 10^(-3 T / (4 RT60))`: the
/// measured (Schroeder, -5 to -35 dB) decay of this topology runs 4 to 9 %
/// slower than the ideal loop-time estimate, so the loop time is taken 7 %
/// longer. Measured result in `tests/reverb_proof.rs`.
const LOOP_CAL: f32 = 1.07;

fn scaled(n: usize, sc: f32) -> usize {
    ((n as f32 * sc).round() as usize).max(4)
}

#[derive(Clone, Debug)]
struct Allpass {
    ring: Ring,
    len: usize,
    interp: f32,
}

impl Allpass {
    fn new(len: usize, extra: usize) -> Self {
        Self { ring: Ring::new(len + extra), len, interp: 0.0 }
    }

    #[inline]
    fn tick(&mut self, x: f32, g: f32) -> f32 {
        let z = self.ring.at(self.len);
        let w = x + g * z;
        self.ring.push(flush(w));
        z - g * w
    }

    /// Delay `len + shift` samples (allpass-interpolated).
    #[inline]
    fn tick_mod(&mut self, x: f32, g: f32, shift: f32) -> f32 {
        let z = self.ring.at_allpass(self.len as f32 + shift, &mut self.interp);
        let w = x + g * z;
        self.ring.push(flush(w));
        z - g * w
    }
}

#[derive(Clone, Debug)]
struct Tank {
    ap1: Allpass,
    d1: Ring,
    d1_len: usize,
    ap2: Allpass,
    d2: Ring,
    d2_len: usize,
    damp: f32,
    lfo: f32,
}

impl Tank {
    fn new(ap1: usize, d1: usize, ap2: usize, d2: usize, sc: f32, phase: f32) -> Self {
        let exc = (MOD_EXCURSION * sc).ceil() as usize + 2;
        let (d1_len, d2_len) = (scaled(d1, sc), scaled(d2, sc));
        Self {
            ap1: Allpass::new(scaled(ap1, sc), exc + 4),
            d1: Ring::new(d1_len),
            d1_len,
            ap2: Allpass::new(scaled(ap2, sc), 4),
            d2: Ring::new(d2_len),
            d2_len,
            damp: 0.0,
            lfo: phase,
        }
    }

    fn clear(&mut self) {
        self.ap1.ring.clear();
        self.ap1.interp = 0.0;
        self.ap2.ring.clear();
        self.d1.clear();
        self.d2.clear();
        self.damp = 0.0;
    }
}

/// Dattorro plate reverberator, stereo in and out, wet signal only.
#[derive(Clone, Debug)]
pub struct PlateReverb {
    sample_rate: f32,
    scale: f32,
    pre: PreDelay,
    bw_state: f32,
    bw_pole: f32,
    input_ap: [Allpass; 4],
    left: Tank,
    right: Tank,
    // taps (scaled), left output then right output
    taps_l: [(usize, u8, f32); 7],
    taps_r: [(usize, u8, f32); 7],
    ring_time_s: f32,
    // settings
    rt60: f32,
    decay: f32,
    dd2: f32,
    damp_hz: f32,
    damp_pole: f32,
    bw_hz: f32,
    mod_depth: f32,
    mod_rate: f32,
    mod_inc: f32,
    width: f32,
    frozen: bool,
    in_gain: f32,
    in_coef: f32,
}

// Tap source ids: 0 left d1, 1 left ap2, 2 left d2, 3 right d1, 4 right ap2, 5 right d2.
const TAPS_L: [(usize, u8, f32); 7] = [
    (266, 3, 1.0),
    (2974, 3, 1.0),
    (1913, 4, -1.0),
    (1996, 5, 1.0),
    (1990, 0, -1.0),
    (187, 1, -1.0),
    (1066, 2, -1.0),
];
const TAPS_R: [(usize, u8, f32); 7] = [
    (353, 0, 1.0),
    (3627, 0, 1.0),
    (1228, 1, -1.0),
    (2673, 2, 1.0),
    (2111, 3, -1.0),
    (335, 4, -1.0),
    (121, 5, -1.0),
];

impl PlateReverb {
    /// Builds the plate for `sample_rate` (clamped to the SDK's range). Defaults:
    /// decay 2 s, bandwidth 16 kHz, damping 9 kHz, modulation depth 0.5 at 1 Hz,
    /// width 1, pre-delay 0. Allocates.
    pub fn new(sample_rate: f32) -> Self {
        let fs = effective_rate(sample_rate);
        let sc = fs / PLATE_PAPER_RATE;
        let left = Tank::new(L_AP1, L_D1, L_AP2, L_D2, sc, 0.0);
        let right = Tank::new(R_AP1, R_D1, R_AP2, R_D2, sc, 1.5);
        let ring_time_s = (left.ap1.len + left.d1_len + left.ap2.len + left.d2_len
            + right.ap1.len + right.d1_len + right.ap2.len + right.d2_len) as f32
            / fs;
        let mk = |t: &[(usize, u8, f32); 7]| {
            let mut out = *t;
            for e in &mut out {
                let cap = match e.1 {
                    0 => left.d1_len,
                    1 => left.ap2.len,
                    2 => left.d2_len,
                    3 => right.d1_len,
                    4 => right.ap2.len,
                    _ => right.d2_len,
                };
                e.0 = scaled(e.0, sc).min(cap);
            }
            out
        };
        let (taps_l, taps_r) = (mk(&TAPS_L), mk(&TAPS_R));
        let input_ap = INPUT_AP.map(|(n, _)| Allpass::new(scaled(n, sc), 4));
        let mut s = Self {
            sample_rate: fs,
            scale: sc,
            pre: PreDelay::new(fs, PLATE_MAX_PREDELAY_MS, 0.0),
            bw_state: 0.0,
            bw_pole: 0.0,
            input_ap,
            left,
            right,
            taps_l,
            taps_r,
            ring_time_s,
            rt60: 2.0,
            decay: 0.5,
            dd2: 0.5,
            damp_hz: 9_000.0,
            damp_pole: 0.0,
            bw_hz: 16_000.0,
            mod_depth: 0.5,
            mod_rate: 1.0,
            mod_inc: 0.0,
            width: 1.0,
            frozen: false,
            in_gain: 1.0,
            in_coef: 1.0 - (-1.0 / (0.01 * fs)).exp(),
        };
        s.refresh();
        s
    }

    fn pole_for(&self, hz: f32) -> f32 {
        if hz >= 0.45 * self.sample_rate {
            0.0
        } else {
            (-std::f32::consts::TAU * hz / self.sample_rate).exp()
        }
    }

    fn refresh(&mut self) {
        self.bw_pole = self.pole_for(self.bw_hz);
        self.damp_pole = if self.frozen { 0.0 } else { self.pole_for(self.damp_hz) };
        self.decay = if self.frozen {
            FREEZE_DECAY
        } else {
            10f32.powf(-3.0 * LOOP_CAL * self.ring_time_s / (4.0 * self.rt60)).min(0.9995)
        };
        self.dd2 = (self.decay + 0.15).clamp(0.25, 0.5);
        self.mod_inc = std::f32::consts::TAU * self.mod_rate / self.sample_rate;
    }

    /// The real sample rate in use.
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Total delay around the figure-eight loop, seconds (about 0.72 s at any rate).
    pub fn loop_time_s(&self) -> f32 {
        self.ring_time_s
    }

    /// Decay time (-60 dB) of the tank in seconds, `0.1..=120`. NaN is ignored.
    pub fn set_rt60(&mut self, seconds: f32) {
        if seconds.is_finite() {
            self.rt60 = seconds.clamp(PLATE_RT60_RANGE.0, PLATE_RT60_RANGE.1);
            self.refresh();
        }
    }

    /// Corner of the one-pole low-pass in the tank (high-frequency damping), Hz.
    /// `>= 0.45 fs` disables it.
    pub fn set_damping_hz(&mut self, hz: f32) {
        if hz.is_finite() {
            self.damp_hz = hz.clamp(20.0, 1.0e6);
            self.refresh();
        }
    }

    /// Corner of the input band-limiting one-pole, Hz.
    pub fn set_bandwidth_hz(&mut self, hz: f32) {
        if hz.is_finite() {
            self.bw_hz = hz.clamp(20.0, 1.0e6);
            self.refresh();
        }
    }

    /// Modulation depth `0..=1` (peak excursion 16 paper samples, i.e. 0.54 ms,
    /// scaled to the real rate) and LFO rate in hertz (`0.05..=5`).
    pub fn set_modulation(&mut self, depth: f32, rate_hz: f32) {
        if depth.is_finite() {
            self.mod_depth = depth.clamp(0.0, 1.0);
        }
        if rate_hz.is_finite() {
            self.mod_rate = rate_hz.clamp(0.05, 5.0);
        }
        self.refresh();
    }

    /// Pre-delay, milliseconds `0..=500`.
    pub fn set_predelay_ms(&mut self, ms: f32) {
        self.pre.set_ms(self.sample_rate, ms);
    }

    /// Wet stereo width `0..=2`.
    pub fn set_width(&mut self, width: f32) {
        if width.is_finite() {
            self.width = width.clamp(0.0, 2.0);
        }
    }

    /// Freeze: input fades out, the tank loop gain is set just under one and
    /// damping is removed, so the tail holds with bounded output.
    pub fn set_freeze(&mut self, frozen: bool) {
        self.frozen = frozen;
        self.refresh();
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Samples until the wet output is 60 dB down after the input stops; infinite
    /// while frozen (report `Tail::Infinite`).
    pub fn tail_samples(&self) -> usize {
        ((self.rt60 + self.ring_time_s) * self.sample_rate).ceil() as usize + self.pre.target_samples()
    }

    /// Clears all delays and filters.
    pub fn reset(&mut self) {
        self.pre.reset();
        self.bw_state = 0.0;
        for a in &mut self.input_ap {
            a.ring.clear();
        }
        self.left.clear();
        self.right.clear();
        self.in_gain = if self.frozen { 0.0 } else { 1.0 };
    }

    #[inline]
    fn tap(&self, src: u8, offset: usize) -> f32 {
        match src {
            0 => self.left.d1.at(offset),
            1 => self.left.ap2.ring.at(offset),
            2 => self.left.d2.at(offset),
            3 => self.right.d1.at(offset),
            4 => self.right.ap2.ring.at(offset),
            _ => self.right.d2.at(offset),
        }
    }

    /// One stereo frame in, the wet frame out.
    pub fn process_frame(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let target = if self.frozen { 0.0 } else { 1.0 };
        self.in_gain += (target - self.in_gain) * self.in_coef;
        let (pl, pr) = self.pre.process(clean(in_l), clean(in_r));
        let mono = 0.5 * (pl + pr) * self.in_gain;

        // Input bandwidth and diffusion.
        self.bw_state = flush((1.0 - self.bw_pole) * mono + self.bw_pole * self.bw_state);
        let mut x = self.bw_state;
        for (ap, (_, g)) in self.input_ap.iter_mut().zip(INPUT_AP.iter()) {
            x = ap.tick(x, *g);
        }

        // Read everything before any push: tank ends and output taps.
        let l_end = self.left.d2.at(self.left.d2_len);
        let r_end = self.right.d2.at(self.right.d2_len);
        let mut yl = 0.0;
        let mut yr = 0.0;
        for &(off, src, sign) in &self.taps_l {
            yl += sign * self.tap(src, off);
        }
        for &(off, src, sign) in &self.taps_r {
            yr += sign * self.tap(src, off);
        }

        let excursion = self.mod_depth * MOD_EXCURSION * self.scale;
        let decay = self.decay;
        let dd2 = self.dd2;
        let dp = self.damp_pole;

        // Left half, fed by the right half's end.
        let max_shift = |len: usize| (len as f32 - 3.0).max(0.0);
        let shift_l = self.left.lfo.sin() * excursion.min(max_shift(self.left.ap1.len));
        let s = x + decay * r_end;
        let a = self.left.ap1.tick_mod(s, -0.7, shift_l);
        let d1o = self.left.d1.at(self.left.d1_len);
        self.left.d1.push(flush(a));
        self.left.damp = flush((1.0 - dp) * d1o + dp * self.left.damp);
        let b = self.left.ap2.tick(self.left.damp * decay, dd2);
        self.left.d2.push(flush(b));

        // Right half, fed by the left half's end.
        let shift_r = self.right.lfo.sin() * excursion.min(max_shift(self.right.ap1.len));
        let s = x + decay * l_end;
        let a = self.right.ap1.tick_mod(s, -0.7, shift_r);
        let d1o = self.right.d1.at(self.right.d1_len);
        self.right.d1.push(flush(a));
        self.right.damp = flush((1.0 - dp) * d1o + dp * self.right.damp);
        let b = self.right.ap2.tick(self.right.damp * decay, dd2);
        self.right.d2.push(flush(b));

        for lfo in [&mut self.left.lfo, &mut self.right.lfo] {
            *lfo += self.mod_inc;
            if *lfo > std::f32::consts::TAU {
                *lfo -= std::f32::consts::TAU;
            }
        }

        let (ol, or) = (yl * OUT_GAIN, yr * OUT_GAIN);
        if !(ol.is_finite() && or.is_finite()) {
            self.reset();
            return (0.0, 0.0);
        }
        apply_width(ol, or, self.width)
    }

    /// In place: the buffers are replaced by the wet signal.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (a, b) = self.process_frame(*l, *r);
            *l = a;
            *r = b;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_scale_to_the_real_rate() {
        let a = PlateReverb::new(PLATE_PAPER_RATE);
        assert_eq!(a.left.d1_len, 4453);
        assert_eq!(a.right.d2_len, 3163);
        assert_eq!(a.taps_l[1].0, 2974);
        let b = PlateReverb::new(48_000.0);
        let want = (4453.0f32 * 48_000.0 / PLATE_PAPER_RATE).round() as usize;
        assert_eq!(b.left.d1_len, want);
        assert!((a.loop_time_s() - b.loop_time_s()).abs() < 1e-3);
    }

    #[test]
    fn taps_fit_their_lines() {
        for fs in [1_000.0f32, 8_000.0, 44_100.0, 192_000.0] {
            let mut p = PlateReverb::new(fs);
            for _ in 0..3000 {
                let (a, b) = p.process_frame(0.1, -0.1);
                assert!(a.is_finite() && b.is_finite());
            }
        }
    }

    #[test]
    fn silent_until_the_input_arrives_then_nan_safe() {
        let mut p = PlateReverb::new(48_000.0);
        for _ in 0..500 {
            let (a, b) = p.process_frame(f32::NAN, f32::NEG_INFINITY);
            assert!(a == 0.0 && b == 0.0);
        }
    }
}
