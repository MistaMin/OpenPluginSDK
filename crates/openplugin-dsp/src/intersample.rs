//! The limiter's inter-sample peak detector: three reconstruction filters,
//! read together.
//!
//! # Why three filters
//!
//! The "true peak" of a sampled signal is the peak of the waveform a
//! reconstruction filter makes of it (Recommendation ITU-R BS.1770-4 (2015),
//! Annex 2). For material with nothing near the Nyquist frequency every
//! reasonable filter gives the same answer. For material with energy close to
//! Nyquist (full-band white noise is the extreme) they do not: an interpolator
//! made of a windowed sinc passes a component at `fs/2 - d` and mirrors a part
//! of it to `fs/2 + d`, and how large that part is depends on the width of the
//! filter's transition band, that is, on its length. Between samples the two
//! parts cancel, so a short filter reads such a component low and a long one
//! reads it nearly in full. Measured on white noise (uniform, 20 000 samples),
//! interval by interval, against a Hann-windowed sinc of 48 samples each side
//! evaluated at 16 points per sample:
//!
//! | Filter | Reading against the 48-sample sinc |
//! | --- | --- |
//! | 16 taps per phase, Kaiser beta 6 (`TruePeak::precise`, the limiter's detector until now) | up to 2.6 dB low |
//! | 24 taps per phase, Kaiser beta 9 (`TruePeakMeter`) | between 2.1 dB low and 1.3 dB high |
//! | 400 taps per phase, Hann | between 0.9 dB low and 0.9 dB high |
//!
//! So no single detector bounds every meter. This one evaluates, for each
//! sample interval, three filters and returns the largest reading:
//!
//! 1. **Sharp.** A Hann-windowed sinc of 48 samples each side (96 taps per
//!    phase), seven phases between the samples, the samples themselves as the
//!    eighth. The crest between two of the 8x points is recovered by fitting a
//!    sinusoid through each local maximum and its two neighbours (three
//!    equally spaced samples `a, b, d` of a sinusoid with step angle `w`
//!    satisfy `a + d = 2 b cos w`, and its amplitude is
//!    `sqrt(b^2 + (a - d)^2 / (4 sin^2 w))`), which is exact for a sine. This
//!    stands for a long reconstruction filter (a sample-rate converter, a
//!    sharp DAC filter). Measured in this module's tests: on white noise it is
//!    at most 0.011 dB below the same kernel evaluated at 16x. (With four
//!    points per sample and the same fit it was 0.08 dB below, which is why
//!    there are eight.)
//! 2. **Meter.** The 4x interpolator of [`crate::TruePeakMeter`] (24 taps per
//!    phase, Kaiser beta 9), at its own grid points with no crest fit, so the
//!    limiter never passes what that meter would show as an over.
//! 3. **Short.** The 12 taps per phase of [`crate::TruePeak::new`], the
//!    structure of the Annex's own filter. Its pass band is not flat, so a
//!    plain sine can read up to 0.11 dB high (measured below).
//!
//! # Cost
//!
//! Written out, that is 7 x 96 + 3 x 24 + 4 x 12 = 792 multiply-adds per
//! sample and channel. All three kernels are even, and every interval's points
//! lie symmetrically about its middle, so the phases come in mirrored pairs
//! (`1/8` and `7/8`, the meter's `1/4` and `3/4`, ...) that use the same taps in
//! opposite order. With `s = x[j] + x[95 - j]` and `d = x[j] - x[95 - j]`, a
//! pair is `P + D` and `P - D`, where `P` is a sum over `s` and `D` a sum
//! over `d`, each of half the length (the folding used for any symmetric FIR
//! filter). That leaves 48 x 7 + 12 x 7 = 420 multiply-adds, done in two
//! passes ([`folded_sum`]).
//!
//! Delay: [`DELAY`] = 48 samples (the sharp filter's half length). `new`
//! allocates; `push` does not.

use crate::true_peak;
use crate::true_peak_meter;

/// Samples each side of the sharp interpolator, and the detector's delay: the
/// value returned for input sample `n` is the peak of the waveform between
/// samples `n - DELAY` and `n - DELAY + 1`.
pub(crate) const DELAY: usize = 48;

const N: usize = 2 * DELAY;
/// Points per sample of the sharp interpolator (the sample itself and seven
/// computed phases).
const SHARP_OVER: usize = 8;
const METER_TAPS: usize = 24;
const SHORT_TAPS: usize = 12;
/// Sums per pass of [`folded_sum`]: four over the pair sums, four over the
/// pair differences.
const LANES: usize = 8;
/// A sinusoid sampled at eight or more points per original sample moves at
/// most pi/8 per point, so its crest is at most 1 / cos(pi/16) = 1.0196 times
/// the largest point. The fit is capped just above that.
const CREST_CAP: f32 = 1.025;

/// The sharp interpolator's continuous kernel: `sinc(t)` under a Hann window
/// that reaches zero at `DELAY + 1`.
fn sharp_kernel(t: f64) -> f64 {
    use std::f64::consts::PI;
    let sinc = if t == 0.0 { 1.0 } else { (PI * t).sin() / (PI * t) };
    sinc * 0.5 * (1.0 + (PI * t / (DELAY as f64 + 1.0)).cos())
}

/// Amplitude of the sinusoid through a local maximum `b` and its neighbours
/// `a` and `d` (signed, equally spaced). Returns `|b|` when the three points
/// are not one lobe.
///
/// Written without an early exit: the answer is computed for every call and
/// chosen at the end, because a branch here would depend on sums that finish
/// late, and guessing it wrong would throw away the next sample's work.
#[inline]
fn crest(a: f32, b: f32, d: f32) -> f32 {
    let m = b.abs();
    let (pa, pd) = (a.abs(), d.abs());
    let cos = (pa + pd) / (2.0 * m).max(f32::MIN_POSITIVE);
    let sin2 = 1.0 - cos * cos;
    let diff = pa - pd;
    let amp = (m * m + diff * diff / (4.0 * sin2.max(1.0e-6))).sqrt().min(m * CREST_CAP);
    let one_lobe = (a * b > 0.0) & (d * b > 0.0) & (m >= pa) & (m >= pd) & (sin2 > 1.0e-6);
    if one_lobe {
        amp
    } else {
        m
    }
}

/// One pass of the folded filters over the window `w` (even length, a multiple
/// of eight). Row `j` of `table` belongs to the pair `w[j]`, `w[len - 1 - j]`:
/// its first four entries multiply the pair's sum and its last four the pair's
/// difference. Returns the eight totals.
///
/// Four partial sums (by `j mod 4`) keep the additions from waiting on each
/// other; their order is fixed, so the result does not depend on how the
/// compiler vectorises a row.
#[inline]
#[allow(clippy::needless_range_loop)]
fn folded_sum(w: &[f32], table: &[[f32; LANES]]) -> [f32; LANES] {
    debug_assert!(w.len() == 2 * table.len() && table.len() % 4 == 0);
    let (front, back) = w.split_at(w.len() / 2);
    let mut acc = [[0.0f32; LANES]; 4];
    for ((xs, ys), rows) in front.chunks_exact(4).zip(back.rchunks_exact(4)).zip(table.chunks_exact(4)) {
        // Fixed-size views, so the inner loops have constant bounds.
        let (Ok(xs), Ok(ys), Ok(rows)) = (<&[f32; 4]>::try_from(xs), <&[f32; 4]>::try_from(ys), <&[[f32; LANES]; 4]>::try_from(rows)) else {
            continue;
        };
        for m in 0..4 {
            let (a, b) = (xs[m], ys[3 - m]);
            let (sum, diff) = (a + b, a - b);
            let v = [sum, sum, sum, sum, diff, diff, diff, diff];
            for l in 0..LANES {
                acc[m][l] += v[l] * rows[m][l];
            }
        }
    }
    let mut out = [0.0f32; LANES];
    for (l, o) in out.iter_mut().enumerate() {
        *o = (acc[0][l] + acc[2][l]) + (acc[1][l] + acc[3][l]);
    }
    out
}

/// Inter-sample peak detector for `channels` channels. See the module docs.
#[derive(Debug, Clone)]
pub(crate) struct InterSample {
    /// `DELAY` rows, one per pair of the window. For the pair sums: half the
    /// even part of the phases 1/8, 2/8, 3/8 and the phase 4/8 itself. For the
    /// pair differences: half the odd part of the first three, and zero.
    sharp: Vec<[f32; LANES]>,
    /// `METER_TAPS / 2` rows for the middle of the window. Sums: the meter's
    /// phases 1/4 (even part) and 2/4, the short filter's two phase pairs
    /// (even parts). Differences: the meter's 1/4 (odd part), zero, the short
    /// filter's two (odd parts).
    others: Vec<[f32; LANES]>,
    /// Per channel a double ring of `2 N`, so the last `N` samples are one slice.
    ring: Vec<f32>,
    /// Per channel, the sharp interpolator's last point of the previous interval.
    last: Vec<f32>,
    pos: usize,
}

impl InterSample {
    pub(crate) fn new(channels: usize) -> Self {
        let channels = channels.max(1);
        // The window holds x[i - 47 ..= i + 48] for the interval that starts at
        // sample i; window index j is sample i + j - 47, and its mirror about
        // the middle of the interval is index 95 - j.
        let tap = |s: usize, j: usize| sharp_kernel(s as f64 / SHARP_OVER as f64 - (j as f64 - (DELAY as f64 - 1.0)));
        let mut sharp = vec![[0.0f32; LANES]; DELAY];
        for (j, row) in sharp.iter_mut().enumerate() {
            for s in 1..=3 {
                // Phase 8 - s uses the taps of phase s in opposite order.
                let (near, far) = (tap(s, j), tap(s, N - 1 - j));
                row[s - 1] = (0.5 * (near + far)) as f32;
                row[4 + s - 1] = (0.5 * (near - far)) as f32;
            }
            row[3] = tap(4, j) as f32;
        }
        // The middle METER_TAPS samples of the window are x[i - 11 ..= i + 12];
        // index j of them mirrors to 23 - j. The meter's phase p, tap k
        // multiplies x[i + 12 - k] and gives the point i + p/4; its phase 3
        // is its phase 1 in opposite order. The short detector's phase p, tap
        // k multiplies x[i + 6 - k]; its phases 3 and 2 are its phases 0 and
        // 1 in opposite order.
        let meter = true_peak_meter::interpolator_4x();
        let short = true_peak::standard_taps();
        let meter_tap = |p: usize, j: usize| f64::from(meter[p][METER_TAPS - 1 - j]);
        let pad = (METER_TAPS - SHORT_TAPS) / 2;
        let short_tap = |p: usize, j: usize| f64::from(short[p][SHORT_TAPS - 1 - (j - pad)]);
        let mut others = vec![[0.0f32; LANES]; METER_TAPS / 2];
        for (j, row) in others.iter_mut().enumerate() {
            let far = METER_TAPS - 1 - j;
            row[0] = (0.5 * (meter_tap(1, j) + meter_tap(1, far))) as f32;
            row[4] = (0.5 * (meter_tap(1, j) - meter_tap(1, far))) as f32;
            row[1] = meter_tap(2, j) as f32;
            if j >= pad {
                for p in 0..2 {
                    row[2 + p] = (0.5 * (short_tap(p, j) + short_tap(p, far))) as f32;
                    row[6 + p] = (0.5 * (short_tap(p, j) - short_tap(p, far))) as f32;
                }
            }
        }
        Self { sharp, others, ring: vec![0.0; channels * 2 * N], last: vec![0.0; channels], pos: 0 }
    }

    pub(crate) fn reset(&mut self) {
        self.ring.fill(0.0);
        self.last.fill(0.0);
        self.pos = 0;
    }

    /// Feed channel `c` its sample of the current frame and return the peak of
    /// its waveform over the interval `DELAY` samples back (linear; it can be
    /// infinite or NaN only if the sums overflow, above about 1e36). Call once
    /// per channel, then [`advance`](Self::advance). `c` must be below the
    /// channel count given to `new`; a sample that is not finite counts as 0.
    #[inline]
    pub(crate) fn push(&mut self, c: usize, x: f32) -> f32 {
        // Not finite, or too small to matter: keep subnormals out of the sums.
        let x = if x.is_finite() && x.abs() > 1.0e-30 { x } else { 0.0 };
        let base = c * 2 * N + self.pos;
        self.ring[base] = x;
        self.ring[base + N] = x;
        let w = &self.ring[base + 1..base + 1 + N];

        // Sharp: the previous interval's last point, sample i, the seven
        // points between, sample i + 1. Every centre has both neighbours.
        let f = folded_sum(w, &self.sharp);
        let seq = [
            self.last[c],
            w[DELAY - 1],
            f[0] + f[4],
            f[1] + f[5],
            f[2] + f[6],
            f[3],
            f[2] - f[6],
            f[1] - f[5],
            f[0] - f[4],
            w[DELAY],
        ];
        self.last[c] = seq[SHARP_OVER];
        // The largest point, and the crest of the lobe it sits on. (Two
        // lobes whose tops both fall inside one sample interval would need a
        // component above the sample rate, so fitting the largest is enough.)
        let mut top = 0.0f32;
        let mut at = 1;
        for (q, v) in seq.iter().enumerate().take(SHARP_OVER + 1).skip(1) {
            let m = v.abs();
            at = if m > top { q } else { at };
            top = top.max(m);
        }
        let mut peak = crest(seq[at - 1], seq[at], seq[at + 1]);

        // The meter's three points between the samples and the short filter's
        // four at the odd eighths, at their own grid points with no fit.
        let g = folded_sum(&w[DELAY - METER_TAPS / 2..DELAY + METER_TAPS / 2], &self.others);
        for v in [g[0] + g[4], g[1], g[0] - g[4], g[2] + g[6], g[3] + g[7], g[3] - g[7], g[2] - g[6]] {
            peak = peak.max(v.abs());
        }
        peak
    }

    /// Step to the next frame, after every channel was pushed.
    #[inline]
    pub(crate) fn advance(&mut self) {
        self.pos += 1;
        if self.pos == N {
            self.pos = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TruePeak, TruePeakMeter};
    use std::f64::consts::PI;

    /// The sharp kernel evaluated directly at `over` points per sample, in
    /// f64: the largest |value| in each interval [i, i + 1).
    fn direct(x: &[f32], over: usize) -> Vec<f64> {
        let r = DELAY as i64;
        let mut out = vec![0.0f64; x.len()];
        for i in r..(x.len() as i64 - r - 1) {
            for s in 0..over {
                let t = s as f64 / over as f64;
                let mut acc = 0.0;
                for k in -r + 1..=r {
                    acc += f64::from(x[(i + k) as usize]) * sharp_kernel(t - k as f64);
                }
                out[i as usize] = out[i as usize].max(acc.abs());
            }
        }
        out
    }

    fn raw_readings(x: &[f32]) -> Vec<f64> {
        let mut det = InterSample::new(1);
        x.iter()
            .map(|&v| {
                let p = det.push(0, v);
                det.advance();
                f64::from(p)
            })
            .collect()
    }

    /// Reading `i` describes the interval that starts at sample `i`.
    fn readings(x: &[f32]) -> Vec<f64> {
        let raw = raw_readings(x);
        (0..x.len()).map(|i| raw.get(i + DELAY).copied().unwrap_or(0.0)).collect()
    }

    fn white(n: usize) -> Vec<f32> {
        let mut s = 0x1234_5678_9abc_def1u64;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((s >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn crest_fit_is_exact_for_a_sine() {
        for step in [0.05f64, 0.2, 0.3, PI / 8.0] {
            for offset in [-0.45f64, -0.2, 0.0, 0.3, 0.49] {
                let v = |k: f64| (2.5 * ((k + offset) * step).cos()) as f32;
                let got = crest(v(-1.0), v(0.0), v(1.0));
                assert!((got - 2.5).abs() < 2.0e-4, "step {step} offset {offset}: {got}");
                let neg = crest(-v(-1.0), -v(0.0), -v(1.0));
                assert!((neg - 2.5).abs() < 2.0e-4);
            }
        }
        // Not one lobe: the centre's magnitude.
        assert_eq!(crest(-0.5, 0.6, -0.5), 0.6);
        assert_eq!(crest(0.0, 0.0, 0.0), 0.0);
        assert_eq!(crest(0.7, 0.6, 0.1), 0.6);
        // A spike is capped.
        assert!(crest(0.01, 1.0, 0.9) <= CREST_CAP);
    }

    /// On full-band white noise the reading is never more than 0.02 dB below
    /// the sharp kernel evaluated at 16x, interval by interval (an interval's
    /// reading and the next one's, as the limiter takes them).
    #[test]
    fn white_noise_reads_like_the_16x_evaluation() {
        let x = white(6000);
        let want = direct(&x, 16);
        let got = readings(&x);
        let (mut under, mut over) = (0.0f64, 0.0f64);
        for i in 200..x.len() - 200 {
            if want[i] > 0.3 {
                // A waveform still rising at the end of the interval peaks at
                // the next sample, which the next reading reports.
                under = under.min(20.0 * (got[i].max(got[i + 1]) / want[i]).log10());
                over = over.max(20.0 * (got[i] / want[i].max(want[i - 1]).max(want[i + 1])).log10());
            }
        }
        eprintln!("detector against its sharp kernel at 16x on white noise: {under:+.4} dB to {over:+.4} dB");
        assert!(under > -0.02, "{under}");
        // Reading high is where the meter or the short filter reads above the
        // sharp one; that is the point of evaluating all three.
        assert!(over > 0.5, "{over}");
    }

    /// The largest reading over a signal is at least what `TruePeakMeter` and
    /// `TruePeak::new` report for it (their filters are evaluated here at the
    /// same points; only the order of the f32 additions differs).
    #[test]
    fn never_reads_below_the_crates_meters() {
        let noise = white(4000);
        let tone: Vec<f32> = (0..4000).map(|n| (0.9 * (2.0 * PI * 0.31 * n as f64 + 0.4).sin()) as f32).collect();
        for x in [noise, tone] {
            let ours = raw_readings(&x).iter().cloned().fold(0.0, f64::max);
            let mut meter = TruePeakMeter::new(48_000.0, 1);
            meter.process(0, &x[..x.len() - DELAY]);
            let meter = 10f64.powf(f64::from(meter.max_dbtp(0)) / 20.0);
            let mut tp = TruePeak::new();
            let short = x[..x.len() - DELAY].iter().fold(0.0f32, |m, v| m.max(tp.push(*v)));
            assert!(ours >= meter * (1.0 - 1.0e-5), "{ours} against the meter's {meter}");
            assert!(ours >= f64::from(short) * (1.0 - 1.0e-5), "{ours} against TruePeak::new's {short}");
        }
    }

    /// The folding relies on the meter's and the short detector's phases
    /// being each other's mirror image.
    #[test]
    fn the_borrowed_filters_are_symmetric() {
        let meter = true_peak_meter::interpolator_4x();
        for k in 0..METER_TAPS {
            assert_eq!(meter[1][k], meter[3][METER_TAPS - 1 - k]);
            assert_eq!(meter[2][k], meter[2][METER_TAPS - 1 - k]);
        }
        let short = true_peak::standard_taps();
        for k in 0..SHORT_TAPS {
            assert!((short[0][k] - short[3][SHORT_TAPS - 1 - k]).abs() < 1.0e-7);
            assert!((short[1][k] - short[2][SHORT_TAPS - 1 - k]).abs() < 1.0e-7);
        }
    }

    /// Sines from 0.01 to 0.49 fs at several phases read between 0.01 dB low
    /// and 0.12 dB high (the high side is the short filter's pass-band ripple).
    #[test]
    fn sines_read_their_amplitude() {
        let (mut under, mut over) = (0.0f64, 0.0f64);
        let mut f = 0.01f64;
        while f < 0.495 {
            for ph in 0..6 {
                let x: Vec<f32> = (0..1500).map(|n| (2.0 * PI * f * n as f64 + ph as f64 * 0.55).sin() as f32).collect();
                let best = readings(&x)[200..1200].iter().cloned().fold(0.0, f64::max);
                let d = 20.0 * best.log10();
                under = under.min(d);
                over = over.max(d);
            }
            f += 0.01;
        }
        eprintln!("sines 0.01..0.49 fs: {under:+.4} dB to {over:+.4} dB");
        assert!(under > -0.01 && over < 0.12, "{under} {over}");
    }

    #[test]
    fn an_impulse_is_reported_at_the_delay_and_hostile_input_is_safe() {
        let mut det = InterSample::new(2);
        let mut at = 0;
        for n in 0..200 {
            let p = det.push(0, if n == 0 { 1.0 } else { 0.0 });
            let q = det.push(1, 0.0);
            det.advance();
            assert_eq!(q, 0.0);
            if p >= 1.0 && at == 0 {
                at = n;
            }
        }
        // The sample itself is the first point of the interval that starts at it.
        assert_eq!(at, DELAY);
        for x in [f32::NAN, f32::INFINITY, -f32::INFINITY, 1.0e30, -1.0e30, 1.0e-42] {
            let p = det.push(0, x);
            det.advance();
            assert!(!p.is_nan());
        }
        det.reset();
        assert_eq!(det.push(0, 0.0), 0.0);
    }
}
