//! Utility blocks: mid/side, stereo width, pan laws, balance, polarity,
//! smoothed dB gain, and a denormal guard.
//!
//! Everything here is allocation-free and treats non-finite input as silence
//! where it matters. The functions are plain and stateless; [`Gain`] and
//! [`Panner`] hold a one-pole smoother ([`openplugin_core::OnePole`]) so a
//! host parameter step does not click.
//!
//! # Mid/side
//!
//! `M = (L + R) / 2`, `S = (L - R) / 2`, and the inverse `L = M + S`,
//! `R = M - S`. This is the matrix of Blumlein (1931, GB 394,325) in the
//! scaling most mixing desks use: a mono signal (L = R) has `M` equal to that
//! signal and `S = 0`, so level is kept. Halving is exact in binary floating
//! point, so a round trip is bit-exact whenever `L + R` and `L - R` are exact
//! (pairs whose exponents are close, which covers nearly all audio). For a
//! pair with very different magnitudes the error is one rounding of the
//! smaller channel.
//!
//! # Width
//!
//! [`stereo_width`] scales `S` by `width` and leaves `M` alone: 0 is mono
//! (L = R = M), 1 is the identity, 2 doubles the side. This is the standard
//! mid/side width control (Blumlein; e.g. Zolzer, DAFX, 2nd ed., chapter on
//! spatial effects).
//!
//! # Pan laws
//!
//! A mono signal at pan `p` in `[-1, 1]` (hard left to hard right) is sent to
//! the left and right outputs with gains `gl(x)`, `gr(x)`, `x = (p + 1) / 2`.
//! The three conventions are the usual DAW ones (-3, -4.5, -6 dB); see
//! Reiss and McPherson, *Audio Effects: Theory, Implementation and
//! Application* (CRC, 2014), on panning.
//!
//! | [`PanLaw`] | gain at centre | law |
//! |---|---|---|
//! | `Linear` / `Minus6` | 0.5 (-6.0206 dB) | `gl = 1 - x`, `gr = x` |
//! | `ConstantPower` (-3 dB) | `1/sqrt 2` (-3.0103 dB) | `gl = cos(x pi/2)`, `gr = sin(x pi/2)` |
//! | `Minus4p5` (-4.5 dB) | `10^(-4.5/20)`, exactly -4.5 dB | `gl = cos(x pi/2)^e`, `gr = sin(x pi/2)^e` |
//!
//! For `Minus4p5` the exponent is chosen so the centre gain is exactly
//! -4.5 dB: `e = -2 log2(10^(-4.5/20))`, about 1.495, between the
//! constant-power (1) and amplitude-linear-like laws. All laws are 0 dB at a
//! hard pan.
//!
//! # Balance
//!
//! [`balance_gains`] is for a stereo input: the near side stays at unity and
//! the far side is turned down by the law's ratio,
//! `g = min(1, law(x) / law(0.5))`. At centre both gains are 1, at a hard pan
//! the far side is zero.
//!
//! # Denormal guard
//!
//! [`DenormalGuard`] sets flush-to-zero (and denormals-are-zero where the CPU
//! has it) for the scope of a process call. It lives in `openplugin-core`
//! (`openplugin_core::DenormalGuard`), where the format entries use it, and
//! is re-exported here. See its documentation.

use openplugin_core::OnePole;

/// Mid/side encode of one pair: `(M, S) = ((L + R) / 2, (L - R) / 2)`.
#[inline]
pub fn ms_encode(left: f32, right: f32) -> (f32, f32) {
    ((left + right) * 0.5, (left - right) * 0.5)
}

/// Mid/side decode of one pair: `(L, R) = (M + S, M - S)`.
#[inline]
pub fn ms_decode(mid: f32, side: f32) -> (f32, f32) {
    (mid + side, mid - side)
}

/// Encode two buffers in place: `left` becomes mid, `right` becomes side.
/// Extra samples in the longer buffer are left alone.
pub fn ms_encode_in_place(left: &mut [f32], right: &mut [f32]) {
    for (l, r) in left.iter_mut().zip(right.iter_mut()) {
        let (m, s) = ms_encode(*l, *r);
        *l = m;
        *r = s;
    }
}

/// Decode two buffers in place: `mid` becomes left, `side` becomes right.
pub fn ms_decode_in_place(mid: &mut [f32], side: &mut [f32]) {
    for (m, s) in mid.iter_mut().zip(side.iter_mut()) {
        let (l, r) = ms_decode(*m, *s);
        *m = l;
        *s = r;
    }
}

/// Change the stereo width of one pair. `width` 0 is mono, 1 is unchanged,
/// above 1 widens. Clamped to `0..=4`; NaN counts as 1.
#[inline]
pub fn stereo_width(left: f32, right: f32, width: f32) -> (f32, f32) {
    let width = if width.is_nan() { 1.0 } else { width.clamp(0.0, 4.0) };
    if width == 1.0 {
        return (left, right);
    }
    let (m, s) = ms_encode(left, right);
    ms_decode(m, s * width)
}

/// [`stereo_width`] over two buffers, in place.
pub fn stereo_width_in_place(left: &mut [f32], right: &mut [f32], width: f32) {
    for (l, r) in left.iter_mut().zip(right.iter_mut()) {
        let (a, b) = stereo_width(*l, *r, width);
        *l = a;
        *r = b;
    }
}

/// How pan position becomes channel gains. See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PanLaw {
    /// Amplitude linear, centre -6.02 dB.
    Linear,
    /// Constant power, centre -3.01 dB.
    ConstantPower,
    /// Power law with centre exactly -4.5 dB.
    Minus4p5,
    /// Same as [`PanLaw::Linear`] (centre -6 dB), under its menu name.
    Minus6,
}

const CENTRE_4P5: f64 = 0.595_662_143_529_010_7; // 10^(-4.5/20)

impl PanLaw {
    /// Gains `(left, right)` for a mono source at `pan` in `[-1, 1]`.
    #[inline]
    pub fn gains(self, pan: f32) -> (f32, f32) {
        let pan = if pan.is_nan() { 0.0 } else { pan.clamp(-1.0, 1.0) };
        let x = f64::from(pan + 1.0) * 0.5;
        let (l, r) = match self {
            Self::Linear | Self::Minus6 => (1.0 - x, x),
            Self::ConstantPower => {
                let a = x * std::f64::consts::FRAC_PI_2;
                (a.cos(), a.sin())
            }
            Self::Minus4p5 => {
                // cos(pi/4)^e = 2^(-e/2) = CENTRE_4P5.
                let e = -2.0 * CENTRE_4P5.log2();
                let a = x * std::f64::consts::FRAC_PI_2;
                (a.cos().max(0.0).powf(e), a.sin().max(0.0).powf(e))
            }
        };
        (l as f32, r as f32)
    }

    /// Gain of either channel at the centre.
    pub fn centre_gain(self) -> f32 {
        self.gains(0.0).0
    }
}

/// Stereo balance gains for `pan` in `[-1, 1]`: unity at centre, the far
/// channel falls by the law's ratio, the near channel stays at unity.
#[inline]
pub fn balance_gains(law: PanLaw, pan: f32) -> (f32, f32) {
    let (l, r) = law.gains(pan);
    let c = f64::from(law.centre_gain());
    (((f64::from(l) / c).min(1.0)) as f32, ((f64::from(r) / c).min(1.0)) as f32)
}

/// Polarity (phase invert) per channel. `true` inverts.
#[inline]
pub fn polarity(left: f32, right: f32, invert_left: bool, invert_right: bool) -> (f32, f32) {
    (if invert_left { -left } else { left }, if invert_right { -right } else { right })
}

/// Linear gain for dB; at or below -120 dB (and -inf, NaN) it is exactly 0.
#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    if db.is_nan() || db <= -120.0 {
        0.0
    } else {
        10f32.powf(db.min(60.0) * 0.05)
    }
}

/// dB for a linear gain, floored at -120 dB.
#[inline]
pub fn gain_to_db(gain: f32) -> f32 {
    if gain.is_nan() || gain <= 1e-6 {
        -120.0
    } else {
        20.0 * gain.log10()
    }
}

/// A gain in dB, smoothed per sample by the core one-pole (default 10 ms), so
/// a parameter step does not click.
/// Plain state, no allocation.
///
/// # Example
///
/// ```
/// use openplugin_dsp::Gain;
///
/// let mut gain = Gain::new(48_000.0, 0.0);
/// gain.set_db(-6.0); // glides over about 10 ms
/// let mut buf = vec![1.0f32; 4_800];
/// gain.process(&mut buf);
/// assert!(buf[0] > 0.9); // no step at the start
/// assert!((buf[4_799] - 0.501).abs() < 1e-3);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Gain {
    smoother: OnePole,
    target: f32,
}

impl Gain {
    /// A gain at `db` with a 10 ms smoothing time at `sample_rate` Hz. Does not
    /// allocate.
    pub fn new(sample_rate: f32, db: f32) -> Self {
        let mut smoother = OnePole::new();
        smoother.set_time(crate::rate::effective_rate(sample_rate), 10.0);
        let target = db_to_gain(db);
        smoother.reset(target);
        Self { smoother, target }
    }

    /// Smoothing time constant in ms (63 %); 0 snaps.
    pub fn set_smoothing_ms(&mut self, sample_rate: f32, ms: f32) {
        self.smoother.set_time(crate::rate::effective_rate(sample_rate), ms);
    }

    /// New target; the gain glides there.
    pub fn set_db(&mut self, db: f32) {
        self.target = db_to_gain(db);
    }

    /// New target applied at once, no glide.
    pub fn jump_db(&mut self, db: f32) {
        self.target = db_to_gain(db);
        self.smoother.reset(self.target);
    }

    /// Next linear gain.
    #[inline]
    pub fn next_gain(&mut self) -> f32 {
        self.smoother.next_value(self.target)
    }

    /// Apply to a buffer in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        for s in buf {
            *s *= self.next_gain();
        }
    }

    /// Apply the same gain curve to two buffers in place.
    pub fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let g = self.next_gain();
            *l *= g;
            *r *= g;
        }
    }
}

/// Smoothed pan or balance for a stereo bus.
/// Plain state, no allocation.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{PanLaw, Panner};
///
/// let mut panner = Panner::new(48_000.0, PanLaw::ConstantPower);
/// panner.set_pan(-1.0); // hard left, glided
/// let input = vec![1.0f32; 4_800];
/// let (mut left, mut right) = (vec![0.0f32; 4_800], vec![0.0f32; 4_800]);
/// panner.pan_mono(&input, &mut left, &mut right);
/// assert!(left[4_799] > 0.99 && right[4_799] < 0.01);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Panner {
    law: PanLaw,
    smoother: OnePole,
    target: f32,
}

impl Panner {
    /// A centred panner with `law` and 10 ms smoothing at `sample_rate` Hz.
    pub fn new(sample_rate: f32, law: PanLaw) -> Self {
        let mut smoother = OnePole::new();
        smoother.set_time(crate::rate::effective_rate(sample_rate), 10.0);
        smoother.reset(0.0);
        Self { law, smoother, target: 0.0 }
    }

    /// Changes the pan law.
    pub fn set_law(&mut self, law: PanLaw) {
        self.law = law;
    }

    /// Pan in `[-1, 1]`; glides.
    pub fn set_pan(&mut self, pan: f32) {
        self.target = if pan.is_nan() { 0.0 } else { pan.clamp(-1.0, 1.0) };
    }

    /// Mono in, stereo out (`out_l`, `out_r` get the panned copies).
    pub fn pan_mono(&mut self, input: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        for ((x, l), r) in input.iter().zip(out_l.iter_mut()).zip(out_r.iter_mut()) {
            let (gl, gr) = self.law.gains(self.smoother.next_value(self.target));
            *l = x * gl;
            *r = x * gr;
        }
    }

    /// Balance a stereo pair in place.
    pub fn balance(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (gl, gr) = balance_gains(self.law, self.smoother.next_value(self.target));
            *l *= gl;
            *r *= gr;
        }
    }
}

/// Flush-to-zero for the lifetime of the value, on the current thread.
///
/// This is [`openplugin_core::DenormalGuard`], re-exported here where it
/// first lived; see its documentation. The format entries already hold one
/// around every audio-thread call into a plugin, unless the plugin opts out
/// with `Plugin::flush_denormals`.
pub use openplugin_core::DenormalGuard;

/// `previous + (next - previous) * position`. `position` outside 0..1 is clamped.
pub fn lerp(previous: f32, next: f32, position: f32) -> f32 {
    let t = if position.is_finite() {
        position.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let a = if previous.is_finite() { previous } else { 0.0 };
    let b = if next.is_finite() { next } else { 0.0 };
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hint::black_box;

    #[test]
    fn mid_side_round_trip_is_exact_for_audio_pairs() {
        for &(l, r) in &[(0.5f32, -0.25), (1.0, 1.0), (0.1875, 0.0), (-0.75, 0.625), (0.0, 0.0)] {
            let (m, s) = ms_encode(l, r);
            assert_eq!(ms_decode(m, s), (l, r));
        }
        let mut x = 0x1234_5678u32;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        let mut exact = 0;
        for _ in 0..100_000 {
            let (l, r) = (next(), next());
            let (m, s) = ms_encode(l, r);
            let (dl, dr) = ms_decode(m, s);
            let bound = f32::EPSILON * l.abs().max(r.abs()) * 2.0;
            assert!((dl - l).abs() <= bound && (dr - r).abs() <= bound);
            if dl == l && dr == r {
                exact += 1;
            }
        }
        assert!(exact > 50_000, "exact in only {exact} of 100000");
    }

    #[test]
    fn mid_side_buffers_round_trip() {
        let mut l = [0.5f32, -0.25, 0.125, 1.0];
        let mut r = [0.25f32, 0.25, -0.125, -1.0];
        let (l0, r0) = (l, r);
        ms_encode_in_place(&mut l, &mut r);
        assert_eq!(l[0], 0.375);
        ms_decode_in_place(&mut l, &mut r);
        assert_eq!((l, r), (l0, r0));
    }

    #[test]
    fn width_zero_is_mono_and_one_is_identity() {
        for &(l, r) in &[(0.5f32, -0.25), (0.9, 0.1), (-0.3, 0.3)] {
            let (a, b) = stereo_width(l, r, 0.0);
            assert_eq!(a, b);
            assert!((a - (l + r) * 0.5).abs() < 1e-7);
            assert_eq!(stereo_width(l, r, 1.0), (l, r));
        }
        assert_eq!(stereo_width(1.0, -1.0, 2.0), (2.0, -2.0));
        assert_eq!(stereo_width(0.4, 0.4, 3.0), (0.4, 0.4));
    }

    #[test]
    fn pan_law_centre_levels_are_exact() {
        let db = |g: f32| 20.0 * f64::from(g).log10();
        let (l, r) = PanLaw::ConstantPower.gains(0.0);
        assert!((f64::from(l) - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-7);
        assert!((db(l) + 3.0103).abs() < 1e-3);
        assert!((l - r).abs() < 1e-7);
        let (l, _) = PanLaw::Minus4p5.gains(0.0);
        assert!((db(l) + 4.5).abs() < 1e-5, "{}", db(l));
        for law in [PanLaw::Linear, PanLaw::Minus6] {
            let (l, r) = law.gains(0.0);
            assert_eq!((l, r), (0.5, 0.5));
            assert!((db(l) + 6.0206).abs() < 1e-3);
        }
    }

    #[test]
    fn pan_laws_hit_unity_and_silence_at_the_ends() {
        for law in [PanLaw::Linear, PanLaw::ConstantPower, PanLaw::Minus4p5, PanLaw::Minus6] {
            let (l, r) = law.gains(-1.0);
            assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6, "{law:?}");
            let (l, r) = law.gains(1.0);
            assert!((r - 1.0).abs() < 1e-6 && l.abs() < 1e-6, "{law:?}");
            let (l, r) = law.gains(f32::NAN);
            assert_eq!(l, r);
        }
        for i in -10..=10 {
            let (l, r) = PanLaw::ConstantPower.gains(i as f32 / 10.0);
            assert!((l * l + r * r - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn balance_is_unity_at_centre_and_mutes_the_far_side() {
        for law in [PanLaw::Linear, PanLaw::ConstantPower, PanLaw::Minus4p5] {
            let (l, r) = balance_gains(law, 0.0);
            assert!((l - 1.0).abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
            let (l, r) = balance_gains(law, 1.0);
            assert!(l.abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
            let (l, r) = balance_gains(law, -0.5);
            assert!(l == 1.0 && r < 1.0 && r > 0.0);
        }
    }

    #[test]
    fn polarity_inverts_per_channel() {
        assert_eq!(polarity(0.5, -0.25, true, false), (-0.5, -0.25));
        assert_eq!(polarity(0.5, -0.25, false, true), (0.5, 0.25));
    }

    #[test]
    fn gain_converts_and_ramps() {
        assert_eq!(db_to_gain(0.0), 1.0);
        assert!((db_to_gain(-6.0206) - 0.5).abs() < 1e-4);
        assert_eq!(db_to_gain(f32::NEG_INFINITY), 0.0);
        let mut g = Gain::new(48_000.0, 0.0);
        g.set_db(-20.0);
        let mut buf = vec![1.0f32; 4800];
        g.process(&mut buf);
        assert!(buf[0] > 0.9, "starts near unity and glides: {}", buf[0]);
        assert!(buf.windows(2).all(|w| w[1] <= w[0]), "monotone ramp");
        assert!((buf[4799] - 0.1).abs() < 0.01);
        g.jump_db(0.0);
        assert_eq!(g.next_gain(), 1.0);
    }

    #[test]
    fn panner_glides_to_the_target() {
        let mut p = Panner::new(48_000.0, PanLaw::ConstantPower);
        p.set_pan(1.0);
        let input = vec![1.0f32; 9600];
        let (mut l, mut r) = (vec![0.0; 9600], vec![0.0; 9600]);
        p.pan_mono(&input, &mut l, &mut r);
        assert!(l[0] > 0.6 && r[0] < 0.8);
        assert!(l[9599] < 1e-3 && (r[9599] - 1.0).abs() < 1e-3);
    }

    /// 2^-126 times one half: a denormal on every IEEE machine.
    fn denormal_product() -> f32 {
        black_box(f32::MIN_POSITIVE) * black_box(0.5f32)
    }

    #[test]
    fn denormal_guard_flushes_and_restores() {
        let before = DenormalGuard::raw_control();
        let plain = denormal_product();
        {
            let guard = DenormalGuard::new();
            let guarded = denormal_product();
            let inside = DenormalGuard::raw_control();
            // A decaying feedback loop, the classic denormal producer.
            let mut y = black_box(1.0f32);
            for _ in 0..4000 {
                y = black_box(y * 0.9);
            }
            if guard.active() {
                assert_eq!(y, 0.0, "loop must flush to zero");
                assert_eq!(guarded, 0.0);
                assert_ne!(inside, before);
            }
        }
        assert_eq!(DenormalGuard::raw_control(), before, "mode restored on drop");
        let again = denormal_product();
        assert!(plain.is_subnormal(), "without the guard the product is denormal");
        assert!(again.is_subnormal(), "denormals are back after the guard");
        // Without the guard the same loop ends in a denormal or zero, never flushed early.
        let outer = DenormalGuard::new();
        let mid = DenormalGuard::raw_control();
        drop(DenormalGuard::new());
        assert_eq!(DenormalGuard::raw_control(), mid);
        drop(outer);
        assert_eq!(DenormalGuard::raw_control(), before);
    }
}
