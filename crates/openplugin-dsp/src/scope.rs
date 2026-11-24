//! Correlation and goniometer (vectorscope) feeds for editors.
//!
//! The audio thread owns a [`ScopeWriter`]; the editor holds a
//! [`ScopeReader`]. They share one [`openplugin_core::Seqlock`] (a
//! single-writer sequence lock). The writer never blocks, never waits for the
//! reader and never allocates: `push` copies samples into a fixed array and,
//! every [`SCOPE_POINTS`] points, publishes a [`ScopeFrame`] of about 2 KB
//! with two atomic increments. The reader copies a consistent frame, retrying
//! while a publish is in progress (which on the editor thread is acceptable
//! and brief). A reader that falls behind simply sees the newest frame.
//!
//! # What is in a frame
//!
//! - `x`, `y`: [`SCOPE_POINTS`] points decimated from the input. The plot is
//!   the usual rotated-45-degree goniometer: `y = (L + R) / sqrt 2` (mid,
//!   vertical) and `x = (R - L) / sqrt 2` (side, horizontal). A mono signal
//!   is a vertical line, an out-of-phase one a horizontal line, and a hard-left
//!   one a diagonal up and to the left. This is the layout of hardware
//!   vectorscopes and of Zolzer, *DAFX* 2nd ed., on stereo analysis.
//! - `correlation`: the phase correlation coefficient
//!   `sum(LR) / sqrt(sum(L^2) sum(R^2))`, from one-pole smoothed power sums
//!   (default 300 ms), `+1` mono, `0` uncorrelated, `-1` out of phase. Silence
//!   reads 1 (nothing to disagree about). The same quantity as
//!   [`crate::meter::correlation`] but running instead of per block.
//! - `balance`: `(rms_R - rms_L) / (rms_R + rms_L)` in `[-1, 1]`.
//! - `sequence`: counts frames, so an editor can tell a new frame from a
//!   repeat.

use openplugin_core::Seqlock;
use std::sync::Arc;

/// Points per published frame.
pub const SCOPE_POINTS: usize = 256;

/// One published snapshot.
#[derive(Clone, Copy, Debug)]
pub struct ScopeFrame {
    /// Side, `(R - L) / sqrt 2`, per point (horizontal).
    pub x: [f32; SCOPE_POINTS],
    /// Mid, `(L + R) / sqrt 2`, per point (vertical).
    pub y: [f32; SCOPE_POINTS],
    /// Phase correlation, -1 to 1; silence reads 1.
    pub correlation: f32,
    /// `(rms_R - rms_L) / (rms_R + rms_L)`, -1 (left) to 1 (right).
    pub balance: f32,
    /// Frame counter; 0 before the first frame.
    pub sequence: u32,
}

impl ScopeFrame {
    const EMPTY: Self = Self { x: [0.0; SCOPE_POINTS], y: [0.0; SCOPE_POINTS], correlation: 1.0, balance: 0.0, sequence: 0 };
}

/// Make a connected writer and reader. Allocates the shared slot once.
pub fn scope_feed(sample_rate: f32) -> (ScopeWriter, ScopeReader) {
    let shared = Arc::new(Seqlock::new(ScopeFrame::EMPTY));
    let rate = crate::rate::effective_rate(sample_rate);
    // About 60 frames a second.
    let stride = ((rate / (60.0 * SCOPE_POINTS as f32)).round() as usize).max(1);
    let writer = ScopeWriter {
        shared: Arc::clone(&shared),
        frame: ScopeFrame::EMPTY,
        filled: 0,
        stride,
        skip: 0,
        alpha: 1.0 - (-1.0 / (f64::from(rate) * 0.3)).exp(),
        ll: 0.0,
        rr: 0.0,
        lr: 0.0,
    };
    (writer, ScopeReader { shared })
}

/// The audio-thread half.
/// # Example
///
/// ```
/// use openplugin_dsp::scope_feed;
///
/// let (mut writer, reader) = scope_feed(48_000.0);
/// let signal: Vec<f32> = (0..4_800).map(|n| (n as f32 * 0.05).sin()).collect();
/// writer.push(&signal, &signal); // audio thread: mono content
///
/// let frame = reader.read(); // editor thread
/// assert!(frame.sequence > 0);
/// assert!(frame.correlation > 0.99);
/// assert!(frame.x.iter().all(|x| x.abs() < 1e-6)); // mono: no side
/// ```
pub struct ScopeWriter {
    shared: Arc<Seqlock<ScopeFrame>>,
    frame: ScopeFrame,
    filled: usize,
    stride: usize,
    skip: usize,
    alpha: f64,
    ll: f64,
    rr: f64,
    lr: f64,
}

impl ScopeWriter {
    /// Smoothing time of the correlation and balance, ms (default 300).
    pub fn set_window_ms(&mut self, sample_rate: f32, ms: f32) {
        let rate = f64::from(crate::rate::effective_rate(sample_rate));
        let ms = if ms.is_finite() { f64::from(ms.max(1.0)) } else { 300.0 };
        self.alpha = 1.0 - (-1.0 / (rate * ms * 0.001)).exp();
    }

    /// Feed a block (the shorter slice sets the length). No allocation, no
    /// blocking.
    pub fn push(&mut self, left: &[f32], right: &[f32]) {
        const R: f32 = std::f32::consts::FRAC_1_SQRT_2;
        for (&l, &r) in left.iter().zip(right.iter()) {
            let l = if l.is_finite() { l } else { 0.0 };
            let r = if r.is_finite() { r } else { 0.0 };
            let (lf, rf) = (f64::from(l), f64::from(r));
            self.ll += (lf * lf - self.ll) * self.alpha;
            self.rr += (rf * rf - self.rr) * self.alpha;
            self.lr += (lf * rf - self.lr) * self.alpha;
            if self.skip > 0 {
                self.skip -= 1;
                continue;
            }
            self.skip = self.stride - 1;
            self.frame.x[self.filled] = (r - l) * R;
            self.frame.y[self.filled] = (l + r) * R;
            self.filled += 1;
            if self.filled == SCOPE_POINTS {
                self.filled = 0;
                self.publish();
            }
        }
    }

    fn publish(&mut self) {
        let denom = (self.ll * self.rr).sqrt();
        self.frame.correlation = if denom > 1e-12 { (self.lr / denom).clamp(-1.0, 1.0) as f32 } else { 1.0 };
        let (a, b) = (self.ll.sqrt(), self.rr.sqrt());
        self.frame.balance = if a + b > 1e-9 { ((b - a) / (a + b)) as f32 } else { 0.0 };
        self.frame.sequence = self.frame.sequence.wrapping_add(1);
        self.shared.publish(self.frame);
    }

    /// Current smoothed correlation without waiting for a frame.
    pub fn correlation(&self) -> f32 {
        let denom = (self.ll * self.rr).sqrt();
        if denom > 1e-12 {
            (self.lr / denom).clamp(-1.0, 1.0) as f32
        } else {
            1.0
        }
    }
}

/// The editor-thread half. Cheap to clone.
/// See the [`ScopeWriter`] example.
#[derive(Clone)]
pub struct ScopeReader {
    shared: Arc<Seqlock<ScopeFrame>>,
}

impl ScopeReader {
    /// The newest consistent frame (an empty one before the first publish).
    pub fn read(&self) -> ScopeFrame {
        self.shared.read()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    fn run(l: &[f32], r: &[f32]) -> ScopeFrame {
        let (mut w, rd) = scope_feed(FS);
        w.push(l, r);
        rd.read()
    }

    fn tone(f: f32, n: usize, phase: f32) -> Vec<f32> {
        (0..n).map(|i| 0.5 * (std::f32::consts::TAU * f * i as f32 / FS + phase).sin()).collect()
    }

    #[test]
    fn mono_is_a_vertical_line_with_correlation_one() {
        let t = tone(440.0, 48_000, 0.0);
        let f = run(&t, &t);
        assert!(f.sequence > 0);
        assert!((f.correlation - 1.0).abs() < 1e-4);
        assert!(f.x.iter().all(|v| v.abs() < 1e-6));
        assert!(f.y.iter().any(|v| v.abs() > 0.3));
        assert!(f.balance.abs() < 1e-4);
    }

    #[test]
    fn inverted_is_a_horizontal_line_with_correlation_minus_one() {
        let t = tone(440.0, 48_000, 0.0);
        let n: Vec<f32> = t.iter().map(|v| -v).collect();
        let f = run(&t, &n);
        assert!((f.correlation + 1.0).abs() < 1e-4);
        assert!(f.y.iter().all(|v| v.abs() < 1e-6));
    }

    #[test]
    fn quadrature_is_uncorrelated_and_balance_follows_level() {
        let a = tone(441.0, 96_000, 0.0);
        let b = tone(441.0, 96_000, std::f32::consts::FRAC_PI_2);
        assert!(run(&a, &b).correlation.abs() < 0.05);
        let quiet: Vec<f32> = a.iter().map(|v| v * 0.25).collect();
        let f = run(&quiet, &a);
        assert!((f.balance - 0.6).abs() < 0.02, "{}", f.balance);
        assert_eq!(run(&[0.0; 48_000], &[0.0; 48_000]).correlation, 1.0);
    }

    #[test]
    fn concurrent_reader_sees_only_whole_frames() {
        let (mut w, rd) = scope_feed(FS);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let rd = rd.clone();
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut seen = 0u32;
                let mut last = 0u32;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let f = rd.read();
                    // The writer fills every frame with one constant value
                    // per frame: x all equal, y all equal to x * 2.
                    let x0 = f.x[0];
                    assert!(f.x.iter().all(|&v| v == x0), "torn x");
                    assert!(f.y.iter().all(|&v| (v - 2.0 * x0).abs() < 1e-6 * x0.abs().max(1.0)), "torn y");
                    assert!(f.sequence >= last, "sequence went back");
                    if f.sequence != last {
                        seen += 1;
                    }
                    last = f.sequence;
                }
                seen
            })
        };
        // Per frame k: l = -k/sqrt2... choose l,r so x = k, y = 2k.
        // x = (r - l)/sqrt2, y = (l + r)/sqrt2  =>  r = (y + x)/sqrt2 * ... solve directly.
        let sqrt2 = std::f32::consts::SQRT_2;
        let stride = ((FS / (60.0 * SCOPE_POINTS as f32)).round() as usize).max(1);
        let per_frame = SCOPE_POINTS * stride;
        for k in 1..=3000u32 {
            let x = k as f32 * 1e-3;
            let y = 2.0 * x;
            let (l, r) = ((y - x) / sqrt2, (y + x) / sqrt2);
            w.push(&vec![l; per_frame], &vec![r; per_frame]);
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let seen = reader.join().unwrap();
        assert!(seen > 0);
        assert_eq!(rd.read().sequence, 3000);
    }
}
