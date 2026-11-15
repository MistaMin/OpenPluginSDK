//! Plate reverberator after Dattorro.
//!
//! Source: Jon Dattorro, "Effect Design, Part 1: Reverberator and Other
//! Filters", J. Audio Eng. Soc., vol. 45, no. 9, pp. 660-684, September 1997:
//! figure 1 ("Simplified plate-class reverberation topology in the style of
//! Griesinger"), table 1 (default parameters) and table 2 (output taps). The
//! code was checked against the paper line by line and is written from it,
//! not from any implementation.
//!
//! ```text
//!  (xL + xR) / 2 -> pre-delay -> bandwidth one-pole
//!     -> input diffusers 142, 107 (input diffusion 1 = 0.75)
//!                        379, 277 (input diffusion 2 = 0.625)
//!                         |
//!       +-----------------+-----------------+
//!       v                                   v
//!   +-> sum <- decay * node 63            sum <- decay * node 39 <-----+
//!   |   diffuser 672 +- excursion         diffuser 908 +- excursion    |
//!   |     (decay diffusion 1 = 0.70, sign reversed, modulated tap)     |
//!   |   delay 4453   (node24_30)          delay 4217   (node48_54)     |
//!   |   damping one-pole, then * decay    damping one-pole, then * decay
//!   |   diffuser 1800 (node31_33)         diffuser 2656 (node55_59)    |
//!   |     (decay diffusion 2 = decay + 0.15, held in 0.25..=0.50)      |
//!   +-- delay 3720   (node33_39) -> 39    delay 3163   (node59_63) -> 63
//! ```
//!
//! What matches the paper exactly:
//!
//! - The order of the stages and the figure-eight cross feed (the end of each
//!   half, times `decay`, enters the other half's sum together with the
//!   diffused input).
//! - Every delay length of figure 1 at the paper's 29 761 Hz, and every tap
//!   position and sign of table 2 with its gain of 0.6 ([`PlateGeometry`]).
//! - The lattice of figure 1 and its signs: an input or "decay diffusion 2"
//!   diffuser with coefficient `g` computes `w = x - g z`, `y = z + g w`
//!   (transfer function `(g + z^-N) / (1 + g z^-N)`); the two "decay diffusion
//!   1" diffusers have the sign reversed ("note sign" in the figure),
//!   `w = x + g z`, `y = z - g w`.
//! - The one-poles: `y = bandwidth x + (1 - bandwidth) y[-1]` on the input and
//!   `y = (1 - damping) x + damping y[-1]` in each half of the tank, the latter
//!   followed by the multiplication by `decay`.
//! - Table 1's coefficients: input diffusion 0.750 and 0.625, decay diffusion 1
//!   0.70, decay diffusion 2 = `decay + 0.15` with floor 0.25 and ceiling 0.50,
//!   EXCURSION = 16 samples as the largest peak excursion.
//! - The modulation of section 1.3.7: the taps at nodes 24 and 48 (the two
//!   "decay diffusion 1" lines) move sinusoidally, the two oscillators in
//!   quadrature, read by allpass interpolation; the default is the footnote's
//!   "on the order of 1 Hz, peak excursion of about 8 samples".
//!
//! Other sample rates: every length and tap position is the paper's number
//! times `fs / 29761`, rounded to the nearest sample, so the delay times in
//! seconds are those of the paper at any rate. The excursion is scaled the same
//! way and is not rounded (the modulated taps are interpolated).
//!
//! Deliberate deviations, and why:
//!
//! - **Parameters in hertz and seconds.** [`PlateReverb::new`] starts from a
//!   2 s decay, a 16 kHz bandwidth and 9 kHz damping instead of table 1's
//!   coefficients, because a coefficient means a different sound at each sample
//!   rate. The mappings are `bandwidth = 1 - exp(-2 pi fc / fs)`,
//!   `damping = exp(-2 pi fc / fs)` (a corner at or above `0.45 fs` turns the
//!   filter off) and `decay = 10^(-3 k T / (4 RT60))`, where `T` is the total
//!   delay of the figure-eight (21 589 samples, 0.7254 s) which the signal
//!   crosses with four multiplications by `decay`, and `k` is a measured
//!   correction (see [`PLATE_DECAY_CALIBRATION`]). The paper's own coefficients
//!   are available as [`PlateReverb::set_decay`], [`PlateReverb::set_bandwidth`],
//!   [`PlateReverb::set_damping`] and [`PlateReverb::set_paper_defaults`].
//! - **Modulation shape.** The paper asks for a sinusoid, in quadrature for the
//!   pair, and that is what runs; the depth setting scales the peak excursion
//!   from 0 to the paper's maximum of 16 samples. The interpolator is the
//!   first-order allpass the paper recommends (its Part 2, section 5) in
//!   Thiran's form with the fraction kept in `[0.5, 1.5)`.
//! - **Floating point.** The paper's magnitude truncation (section 1.3.4)
//!   answers limit cycles of fixed-point lattices; here every value written
//!   into a recursive path is flushed to zero below 1e-20 instead.
//! - **Additions outside the paper:** freeze, stereo width of the wet signal,
//!   a gliding pre-delay of at most 500 ms and non-finite input handling.
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
/// Largest `decay` coefficient outside freeze.
pub const PLATE_MAX_DECAY: f32 = 0.9995;
/// Table 1: "EXCURSION = 16, maximum peak sample excursion of delay
/// modulation", in samples at 29 761 Hz.
pub const PLATE_EXCURSION: f32 = 16.0;
/// Measured correction `k` of the decay rule `T60 = -3 k T / (4 log10 decay)`
/// that [`PlateReverb::set_rt60`] uses.
///
/// With `k = 1` the rule assumes every diffuser delays by its nominal length.
/// A diffuser is allpass only on the unit circle: the resonances that sit where
/// its group delay is long decay more slowly, and they are what is left in the
/// late tail. Measured (Schroeder, -5 to -35 dB, `tests/plate_paper.rs`), the
/// decay is 1.10 times the `k = 1` estimate with the modulation off and 1.00
/// to 1.07 times at the default modulation, which moves the resonances about;
/// 1.07 keeps both within 8 % of the setting.
pub const PLATE_DECAY_CALIBRATION: f32 = 1.07;

/// Figure 1, input diffusers in signal order: delay length at 29 761 Hz and
/// coefficient (input diffusion 1 twice, then input diffusion 2 twice).
const INPUT_AP: [(usize, f32); 4] = [(142, 0.75), (107, 0.75), (379, 0.625), (277, 0.625)];
/// Figure 1, one half of the tank in signal order: modulated diffuser, delay,
/// diffuser, delay.
const LEFT: [usize; 4] = [672, 4453, 1800, 3720];
const RIGHT: [usize; 4] = [908, 4217, 2656, 3163];
/// Table 1, "decay diffusion 1".
const DECAY_DIFFUSION_1: f32 = 0.70;
/// Table 1, "decay diffusion 2 = decay + 0.15, floor = 0.25, ceiling = 0.50".
const DECAY_DIFFUSION_2: (f32, f32, f32) = (0.15, 0.25, 0.50);
/// Decay gain while frozen: loop gain 0.9998^4 per ~0.73 s, bounded and
/// effectively infinite.
const FREEZE_DECAY: f32 = 0.9998;
/// Table 2: every tap is weighted 0.6.
const OUT_GAIN: f32 = 0.6;

/// A delay line of the tank that table 2 taps, with the paper's node name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlateLine {
    /// `node24_30`: the 4453 delay of the left half.
    LeftDelay1,
    /// `node31_33`: the 1800 line inside the left half's second diffuser.
    LeftDiffuser2,
    /// `node33_39`: the 3720 delay of the left half.
    LeftDelay2,
    /// `node48_54`: the 4217 delay of the right half.
    RightDelay1,
    /// `node55_59`: the 2656 line inside the right half's second diffuser.
    RightDiffuser2,
    /// `node59_63`: the 3163 delay of the right half.
    RightDelay2,
}

/// One output tap: `sign * 0.6 * line[offset]`, the offset counted in samples
/// from the line's input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlateTap {
    pub line: PlateLine,
    pub offset: usize,
    pub sign: f32,
}

const fn tap(line: PlateLine, offset: usize, sign: f32) -> PlateTap {
    PlateTap { line, offset, sign }
}

/// Table 2, left output, in the table's order.
const TAPS_L: [PlateTap; 7] = [
    tap(PlateLine::RightDelay1, 266, 1.0),
    tap(PlateLine::RightDelay1, 2974, 1.0),
    tap(PlateLine::RightDiffuser2, 1913, -1.0),
    tap(PlateLine::RightDelay2, 1996, 1.0),
    tap(PlateLine::LeftDelay1, 1990, -1.0),
    tap(PlateLine::LeftDiffuser2, 187, -1.0),
    tap(PlateLine::LeftDelay2, 1066, -1.0),
];
/// Table 2, right output, in the table's order.
const TAPS_R: [PlateTap; 7] = [
    tap(PlateLine::LeftDelay1, 353, 1.0),
    tap(PlateLine::LeftDelay1, 3627, 1.0),
    tap(PlateLine::LeftDiffuser2, 1228, -1.0),
    tap(PlateLine::LeftDelay2, 2673, 1.0),
    tap(PlateLine::RightDelay1, 2111, -1.0),
    tap(PlateLine::RightDiffuser2, 335, -1.0),
    tap(PlateLine::RightDelay2, 121, -1.0),
];

/// The delay lengths and taps a [`PlateReverb`] runs with, in samples at its
/// real sample rate. At 29 761 Hz these are the numbers printed in the paper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlateGeometry {
    /// The four input diffusers in signal order (142, 107, 379, 277).
    pub input_diffusers: [usize; 4],
    /// Left half in signal order: modulated diffuser, delay, diffuser, delay
    /// (672, 4453, 1800, 3720).
    pub left: [usize; 4],
    /// Right half in signal order (908, 4217, 2656, 3163).
    pub right: [usize; 4],
    /// Largest peak excursion of the two modulated taps (16), not rounded.
    pub excursion: f32,
    /// Table 2, left output.
    pub taps_left: [PlateTap; 7],
    /// Table 2, right output.
    pub taps_right: [PlateTap; 7],
}

/// A paper length (samples at 29 761 Hz) at `fs`: `n fs / 29761`, rounded to
/// the nearest sample. Never below 4 samples, which matters only under 2 kHz.
fn scaled(n: usize, fs: f32) -> usize {
    ((n as f64 * f64::from(fs) / f64::from(PLATE_PAPER_RATE)).round() as usize).max(4)
}

/// The lattice of figure 1.
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

    /// `w = x - g z`, `y = z + g w`: the figure's lattice with the minus sign
    /// at the input sum. A negative `g` gives the "note sign" form.
    #[inline]
    fn tick(&mut self, x: f32, g: f32) -> f32 {
        let z = self.ring.at(self.len);
        let w = x - g * z;
        self.ring.push(flush(w));
        z + g * w
    }

    /// As [`Allpass::tick`] with the tap at `len + shift` samples
    /// (allpass-interpolated; exact when `shift` is zero).
    #[inline]
    fn tick_mod(&mut self, x: f32, g: f32, shift: f32) -> f32 {
        let z = self.ring.at_allpass(self.len as f32 + shift, &mut self.interp);
        let w = x - g * z;
        self.ring.push(flush(w));
        z + g * w
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
}

impl Tank {
    fn new(lengths: &[usize; 4], fs: f32) -> Self {
        let exc = (PLATE_EXCURSION * fs / PLATE_PAPER_RATE).ceil() as usize + 2;
        let (d1_len, d2_len) = (scaled(lengths[1], fs), scaled(lengths[3], fs));
        Self {
            ap1: Allpass::new(scaled(lengths[0], fs), exc + 4),
            d1: Ring::new(d1_len),
            d1_len,
            ap2: Allpass::new(scaled(lengths[2], fs), 4),
            d2: Ring::new(d2_len),
            d2_len,
            damp: 0.0,
        }
    }

    fn lengths(&self) -> [usize; 4] {
        [self.ap1.len, self.d1_len, self.ap2.len, self.d2_len]
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
    pre: PreDelay,
    bw_state: f32,
    input_ap: [Allpass; 4],
    left: Tank,
    right: Tank,
    // taps (scaled), left output then right output
    taps_l: [PlateTap; 7],
    taps_r: [PlateTap; 7],
    ring_time_s: f32,
    // The paper's coefficients as set (freeze overrides `decay` and `damping`).
    in_diff: [f32; 4],
    dd1: f32,
    decay_set: f32,
    damp_set: f32,
    bw_pole: f32,
    // In use.
    decay: f32,
    dd2: f32,
    damp_pole: f32,
    rt60: f32,
    excursion: f32,
    mod_depth: f32,
    mod_rate: f32,
    mod_inc: f32,
    lfo: f32,
    width: f32,
    frozen: bool,
    in_gain: f32,
    in_coef: f32,
}

impl PlateReverb {
    /// Builds the plate for `sample_rate` (clamped to the SDK's range). Defaults:
    /// decay 2 s, bandwidth 16 kHz, damping 9 kHz, modulation depth 0.5 (a peak
    /// excursion of 8 paper samples) at 1 Hz, width 1, pre-delay 0. Allocates.
    pub fn new(sample_rate: f32) -> Self {
        let fs = effective_rate(sample_rate);
        let left = Tank::new(&LEFT, fs);
        let right = Tank::new(&RIGHT, fs);
        let ring_time_s =
            (left.lengths().iter().sum::<usize>() + right.lengths().iter().sum::<usize>()) as f32 / fs;
        let mk = |t: &[PlateTap; 7]| {
            let mut out = *t;
            for e in &mut out {
                let cap = match e.line {
                    PlateLine::LeftDelay1 => left.d1_len,
                    PlateLine::LeftDiffuser2 => left.ap2.len,
                    PlateLine::LeftDelay2 => left.d2_len,
                    PlateLine::RightDelay1 => right.d1_len,
                    PlateLine::RightDiffuser2 => right.ap2.len,
                    PlateLine::RightDelay2 => right.d2_len,
                };
                e.offset = scaled(e.offset, fs).min(cap);
            }
            out
        };
        let (taps_l, taps_r) = (mk(&TAPS_L), mk(&TAPS_R));
        let input_ap = INPUT_AP.map(|(n, _)| Allpass::new(scaled(n, fs), 4));
        let mut s = Self {
            sample_rate: fs,
            pre: PreDelay::new(fs, PLATE_MAX_PREDELAY_MS, 0.0),
            bw_state: 0.0,
            input_ap,
            left,
            right,
            taps_l,
            taps_r,
            ring_time_s,
            in_diff: INPUT_AP.map(|(_, g)| g),
            dd1: DECAY_DIFFUSION_1,
            decay_set: 0.5,
            damp_set: 0.0,
            bw_pole: 0.0,
            decay: 0.5,
            dd2: 0.5,
            damp_pole: 0.0,
            rt60: 2.0,
            excursion: PLATE_EXCURSION * fs / PLATE_PAPER_RATE,
            mod_depth: 0.5,
            mod_rate: 1.0,
            mod_inc: 0.0,
            lfo: 0.0,
            width: 1.0,
            frozen: false,
            in_gain: 1.0,
            in_coef: 1.0 - (-1.0 / (0.01 * fs)).exp(),
        };
        s.set_rt60(2.0);
        s.set_bandwidth_hz(16_000.0);
        s.set_damping_hz(9_000.0);
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
        self.damp_pole = if self.frozen { 0.0 } else { self.damp_set };
        self.decay = if self.frozen { FREEZE_DECAY } else { self.decay_set };
        let (add, floor, ceiling) = DECAY_DIFFUSION_2;
        self.dd2 = (self.decay + add).clamp(floor, ceiling);
        self.mod_inc = std::f32::consts::TAU * self.mod_rate / self.sample_rate;
    }

    /// The real sample rate in use.
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Total delay around the figure-eight loop, seconds (21 589 samples at
    /// 29 761 Hz, about 0.725 s at any rate).
    pub fn loop_time_s(&self) -> f32 {
        self.ring_time_s
    }

    /// The delay lengths and output taps in use, in samples at the real rate.
    pub fn geometry(&self) -> PlateGeometry {
        PlateGeometry {
            input_diffusers: [
                self.input_ap[0].len,
                self.input_ap[1].len,
                self.input_ap[2].len,
                self.input_ap[3].len,
            ],
            left: self.left.lengths(),
            right: self.right.lengths(),
            excursion: self.excursion,
            taps_left: self.taps_l,
            taps_right: self.taps_r,
        }
    }

    /// Decay time (-60 dB) of the tank in seconds, `0.1..=120`. NaN is ignored.
    /// Sets the paper's `decay` to `10^(-3 k T / (4 RT60))` (module docs).
    pub fn set_rt60(&mut self, seconds: f32) {
        if seconds.is_finite() {
            self.rt60 = seconds.clamp(PLATE_RT60_RANGE.0, PLATE_RT60_RANGE.1);
            self.decay_set = 10f32
                .powf(-3.0 * PLATE_DECAY_CALIBRATION * self.ring_time_s / (4.0 * self.rt60))
                .min(PLATE_MAX_DECAY);
            self.refresh();
        }
    }

    /// The decay time the current `decay` coefficient stands for, seconds.
    pub fn rt60(&self) -> f32 {
        self.rt60
    }

    /// The paper's `decay` coefficient ("rate of decay", table 1: 0.50),
    /// `0..=0.9995`; "decay diffusion 2" follows it as the paper prescribes.
    /// The alternative to [`PlateReverb::set_rt60`]: the last call wins.
    pub fn set_decay(&mut self, decay: f32) {
        if decay.is_finite() {
            self.decay_set = decay.clamp(0.0, PLATE_MAX_DECAY);
            self.rt60 = if self.decay_set > 0.0 {
                (-3.0 * PLATE_DECAY_CALIBRATION * self.ring_time_s / (4.0 * self.decay_set.log10()))
                    .clamp(0.0, PLATE_RT60_RANGE.1)
            } else {
                0.0
            };
            self.refresh();
        }
    }

    /// The `decay` coefficient in use (0.9998 while frozen).
    pub fn decay(&self) -> f32 {
        self.decay
    }

    /// "Decay diffusion 2" in use: `decay + 0.15` held in `0.25..=0.50`.
    pub fn decay_diffusion_2(&self) -> f32 {
        self.dd2
    }

    /// Corner of the one-pole low-pass in the tank (high-frequency damping), Hz.
    /// `>= 0.45 fs` disables it. Sets the paper's `damping` to
    /// `exp(-2 pi hz / fs)`.
    pub fn set_damping_hz(&mut self, hz: f32) {
        if hz.is_finite() {
            self.damp_set = self.pole_for(hz.clamp(20.0, 1.0e6));
            self.refresh();
        }
    }

    /// The paper's `damping` coefficient (table 1: 0.0005; 0 is no damping),
    /// `0..=0.9999999`. The alternative to [`PlateReverb::set_damping_hz`].
    pub fn set_damping(&mut self, damping: f32) {
        if damping.is_finite() {
            self.damp_set = damping.clamp(0.0, 0.999_999_9);
            self.refresh();
        }
    }

    /// The `damping` coefficient as set (freeze runs with 0).
    pub fn damping(&self) -> f32 {
        self.damp_set
    }

    /// Corner of the input band-limiting one-pole, Hz. `>= 0.45 fs` disables
    /// it. Sets the paper's `bandwidth` to `1 - exp(-2 pi hz / fs)`.
    pub fn set_bandwidth_hz(&mut self, hz: f32) {
        if hz.is_finite() {
            self.bw_pole = self.pole_for(hz.clamp(20.0, 1.0e6));
        }
    }

    /// The paper's `bandwidth` coefficient (table 1: 0.9995; 0.9999999 is full
    /// bandwidth), `0.0000001..=1`. The alternative to
    /// [`PlateReverb::set_bandwidth_hz`].
    pub fn set_bandwidth(&mut self, bandwidth: f32) {
        if bandwidth.is_finite() {
            self.bw_pole = 1.0 - bandwidth.clamp(1.0e-7, 1.0);
        }
    }

    /// The `bandwidth` coefficient in use.
    pub fn bandwidth(&self) -> f32 {
        1.0 - self.bw_pole
    }

    /// Table 1 as printed: decay 0.50, bandwidth 0.9995, damping 0.0005, and
    /// the modulation of the paper's footnote 14 (1 Hz, peak excursion 8 of the
    /// 16 samples). The diffusion coefficients are always the table's.
    pub fn set_paper_defaults(&mut self) {
        self.set_decay(0.5);
        self.set_bandwidth(0.9995);
        self.set_damping(0.0005);
        self.set_modulation(0.5, 1.0);
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
    fn tap(&self, t: &PlateTap) -> f32 {
        let line = match t.line {
            PlateLine::LeftDelay1 => &self.left.d1,
            PlateLine::LeftDiffuser2 => &self.left.ap2.ring,
            PlateLine::LeftDelay2 => &self.left.d2,
            PlateLine::RightDelay1 => &self.right.d1,
            PlateLine::RightDiffuser2 => &self.right.ap2.ring,
            PlateLine::RightDelay2 => &self.right.d2,
        };
        t.sign * line.at(t.offset)
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
        for (ap, g) in self.input_ap.iter_mut().zip(self.in_diff.iter()) {
            x = ap.tick(x, *g);
        }

        // Read everything before any push: tank ends (nodes 39 and 63) and
        // the output taps of table 2.
        let l_end = self.left.d2.at(self.left.d2_len);
        let r_end = self.right.d2.at(self.right.d2_len);
        let mut yl = 0.0;
        let mut yr = 0.0;
        for t in &self.taps_l {
            yl += self.tap(t);
        }
        for t in &self.taps_r {
            yr += self.tap(t);
        }

        let excursion = self.mod_depth * self.excursion;
        let decay = self.decay;
        let dd1 = self.dd1;
        let dd2 = self.dd2;
        let dp = self.damp_pole;
        // Quadrature pair (section 1.3.7).
        let (sin, cos) = self.lfo.sin_cos();

        // Left half, fed by the right half's end (node 63).
        let max_shift = |len: usize| (len as f32 - 3.0).max(0.0);
        let shift_l = sin * excursion.min(max_shift(self.left.ap1.len));
        let s = x + decay * r_end;
        let a = self.left.ap1.tick_mod(s, -dd1, shift_l);
        let d1o = self.left.d1.at(self.left.d1_len);
        self.left.d1.push(flush(a));
        self.left.damp = flush((1.0 - dp) * d1o + dp * self.left.damp);
        let b = self.left.ap2.tick(self.left.damp * decay, dd2);
        self.left.d2.push(flush(b));

        // Right half, fed by the left half's end (node 39).
        let shift_r = cos * excursion.min(max_shift(self.right.ap1.len));
        let s = x + decay * l_end;
        let a = self.right.ap1.tick_mod(s, -dd1, shift_r);
        let d1o = self.right.d1.at(self.right.d1_len);
        self.right.d1.push(flush(a));
        self.right.damp = flush((1.0 - dp) * d1o + dp * self.right.damp);
        let b = self.right.ap2.tick(self.right.damp * decay, dd2);
        self.right.d2.push(flush(b));

        self.lfo += self.mod_inc;
        if self.lfo > std::f32::consts::TAU {
            self.lfo -= std::f32::consts::TAU;
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
        assert_eq!(a.taps_l[1].offset, 2974);
        let b = PlateReverb::new(48_000.0);
        let want = (4453.0f64 * 48_000.0 / 29_761.0).round() as usize;
        assert_eq!(b.left.d1_len, want);
        assert!((a.loop_time_s() - b.loop_time_s()).abs() < 1e-3);
        assert!((a.loop_time_s() - 21_589.0 / 29_761.0).abs() < 1e-6);
    }

    /// With every diffusion coefficient at zero the network is delays only, so
    /// an impulse shows each tap of table 2 alone: at the time its place in
    /// figure 1 implies, with the table's sign, 0.6 and one `decay` per damping
    /// stage passed. This pins the stage order, the lengths, the tap lines,
    /// offsets and signs together.
    #[test]
    fn with_no_diffusion_each_tap_of_table_2_appears_alone_at_its_time() {
        let mut p = PlateReverb::new(PLATE_PAPER_RATE);
        p.set_decay(0.5);
        p.set_bandwidth(1.0);
        p.set_damping(0.0);
        p.set_modulation(0.0, 1.0);
        p.in_diff = [0.0; 4];
        p.dd1 = 0.0;
        p.dd2 = 0.0;
        let input = 142 + 107 + 379 + 277;
        // Time from the input to the entry of each line, and decay stages passed.
        let entry = |line: PlateLine| match line {
            PlateLine::LeftDelay1 => (input + 672, 0),
            PlateLine::LeftDiffuser2 => (input + 672 + 4453, 1),
            PlateLine::LeftDelay2 => (input + 672 + 4453 + 1800, 1),
            PlateLine::RightDelay1 => (input + 908, 0),
            PlateLine::RightDiffuser2 => (input + 908 + 4217, 1),
            PlateLine::RightDelay2 => (input + 908 + 4217 + 2656, 1),
        };
        // The first recirculated sample reaches a tap after this.
        let first_pass = input + 672 + 4453 + 1800 + 3720;
        let expect = |taps: &[(PlateLine, usize, f32); 7]| {
            let mut v = vec![0.0f32; first_pass];
            for &(line, offset, sign) in taps {
                let (t, stages) = entry(line);
                v[t + offset] += sign * 0.6 * 0.5f32.powi(stages);
            }
            v
        };
        use PlateLine::*;
        let want_l = expect(&[
            (RightDelay1, 266, 1.0),
            (RightDelay1, 2974, 1.0),
            (RightDiffuser2, 1913, -1.0),
            (RightDelay2, 1996, 1.0),
            (LeftDelay1, 1990, -1.0),
            (LeftDiffuser2, 187, -1.0),
            (LeftDelay2, 1066, -1.0),
        ]);
        let want_r = expect(&[
            (LeftDelay1, 353, 1.0),
            (LeftDelay1, 3627, 1.0),
            (LeftDiffuser2, 1228, -1.0),
            (LeftDelay2, 2673, 1.0),
            (RightDelay1, 2111, -1.0),
            (RightDiffuser2, 335, -1.0),
            (RightDelay2, 121, -1.0),
        ]);
        assert_eq!(want_l.iter().filter(|v| **v != 0.0).count(), 7);
        assert_eq!(want_r.iter().filter(|v| **v != 0.0).count(), 7);
        for n in 0..first_pass {
            let (l, r) = p.process_frame(f32::from(u8::from(n == 0)), f32::from(u8::from(n == 0)));
            assert!((l - want_l[n]).abs() < 1e-6, "left at {n}: {l} vs {}", want_l[n]);
            assert!((r - want_r[n]).abs() < 1e-6, "right at {n}: {r} vs {}", want_r[n]);
        }
    }

    /// The lattice signs of figure 1: the first sample out of a diffuser is
    /// `+g` times its input for the input and "decay diffusion 2" diffusers and
    /// `-g` for "decay diffusion 1" ("note sign"), and the delayed sample has
    /// the weight `1 - g^2`.
    #[test]
    fn lattice_signs_follow_figure_1() {
        let mut a = Allpass::new(10, 4);
        let first = a.tick(1.0, 0.75);
        let mut later = [0.0f32; 21];
        for v in &mut later {
            *v = a.tick(0.0, 0.75);
        }
        assert_eq!(first, 0.75);
        assert!((later[9] - (1.0 - 0.75 * 0.75)).abs() < 1e-6);
        assert!((later[19] + 0.75 * (1.0 - 0.75 * 0.75)).abs() < 1e-6);

        let mut a = Allpass::new(10, 24);
        let first = a.tick_mod(1.0, -DECAY_DIFFUSION_1, 0.0);
        let mut later = [0.0f32; 21];
        for v in &mut later {
            *v = a.tick_mod(0.0, -DECAY_DIFFUSION_1, 0.0);
        }
        assert_eq!(first, -0.7);
        assert!((later[9] - (1.0 - 0.49)).abs() < 1e-6);
        assert!((later[19] - 0.7 * (1.0 - 0.49)).abs() < 1e-6);
    }

    #[test]
    fn paper_coefficients_and_their_mappings() {
        let mut p = PlateReverb::new(48_000.0);
        p.set_paper_defaults();
        assert_eq!(p.decay(), 0.5);
        assert_eq!(p.decay_diffusion_2(), 0.5);
        assert!((p.bandwidth() - 0.9995).abs() < 1e-6);
        assert!((p.damping() - 0.0005).abs() < 1e-9);
        // Decay diffusion 2 = decay + 0.15, floor 0.25, ceiling 0.50.
        for (decay, want) in [(0.0f32, 0.25f32), (0.05, 0.25), (0.2, 0.35), (0.35, 0.5), (0.9, 0.5)] {
            p.set_decay(decay);
            assert!((p.decay_diffusion_2() - want).abs() < 1e-6, "{decay}");
        }
        // Seconds to coefficient and back.
        p.set_rt60(3.0);
        let d = p.decay();
        p.set_decay(d);
        assert!((p.rt60() - 3.0).abs() < 1e-3);
        // Hertz to coefficient.
        p.set_bandwidth_hz(4_000.0);
        p.set_damping_hz(4_000.0);
        let pole = (-std::f32::consts::TAU * 4_000.0 / 48_000.0).exp();
        assert!((p.bandwidth() - (1.0 - pole)).abs() < 1e-6);
        assert!((p.damping() - pole).abs() < 1e-6);
        // Non-finite settings are ignored.
        p.set_decay(f32::NAN);
        p.set_damping(f32::INFINITY);
        p.set_bandwidth(f32::NAN);
        assert_eq!(p.decay(), d);
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
