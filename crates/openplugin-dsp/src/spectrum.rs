//! Spectrum analyser and a lock-free feed to an editor.
//!
//! The audio thread owns a [`SpectrumAnalyzer`]: it windows the input, takes an
//! FFT every hop, converts bin magnitudes to dB (full-scale sine = 0 dBFS at
//! its bin, using the window's coherent gain: `|X| * 2 / sum(w)`), optionally
//! averages them over time, and tracks a peak-hold trace. It then publishes
//! the frame through a [`SpectrumWriter`]. The UI thread owns a
//! [`SpectrumReader`] and copies out the latest frame whenever it draws.
//!
//! The feed is a single-writer seqlock (H.-J. Boehm, "Can seqlocks get along
//! with programming language memory models?", MSPC 2012): the writer bumps a
//! sequence number to odd, stores the frame in relaxed atomics, and bumps it to
//! even; a reader retries when the number moved. In safe Rust, with no lock, no
//! allocation and no wait on the audio side. If the reader is torn too many
//! times in a row it gets `None` and tries again on its next paint.
//!
//! [`log_frequency_bins`] turns the linear-frequency bins into display columns
//! on a logarithmic axis.

use crate::stft::{Complex, StftError};
use crate::windows::WindowKind;
use rustfft::{Fft, FftPlanner};
use std::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

struct Shared {
    seq: AtomicU64,
    words: Box<[AtomicU32]>,
    bins: usize,
}

/// Create a feed carrying `bins` magnitudes and `bins` peaks per frame. Allocates.
pub fn spectrum_feed(bins: usize) -> (SpectrumWriter, SpectrumReader) {
    let bins = bins.max(1);
    let shared = Arc::new(Shared {
        seq: AtomicU64::new(0),
        words: (0..bins * 2).map(|_| AtomicU32::new(0)).collect(),
        bins,
    });
    (
        SpectrumWriter {
            shared: shared.clone(),
        },
        SpectrumReader { shared },
    )
}

/// Audio-side end of the feed. One writer only.
/// A seqlock over atomics: `publish` is wait-free on the audio thread.
///
/// # Example
///
/// ```
/// use openplugin_dsp::spectrum_feed;
///
/// let (mut writer, reader) = spectrum_feed(4);
/// assert_eq!(reader.read(&mut [0.0; 4], &mut [0.0; 4]), None); // nothing yet
/// writer.publish(&[-10.0, -20.0, -30.0, -40.0], &[-6.0; 4]);
///
/// let (mut mags, mut peaks) = ([0.0f32; 4], [0.0f32; 4]);
/// assert_eq!(reader.read(&mut mags, &mut peaks), Some(1));
/// assert_eq!(mags[1], -20.0);
/// ```
pub struct SpectrumWriter {
    shared: Arc<Shared>,
}

impl SpectrumWriter {
    /// Bins per frame.
    pub fn bins(&self) -> usize {
        self.shared.bins
    }

    /// Publish one frame. Never blocks or allocates. Both slices must hold
    /// `bins()` values (extra are ignored, missing leave old values).
    pub fn publish(&mut self, mags: &[f32], peaks: &[f32]) {
        let s = &*self.shared;
        let seq = s.seq.load(Ordering::Relaxed);
        s.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        for (w, v) in s.words[..s.bins].iter().zip(mags) {
            w.store(v.to_bits(), Ordering::Relaxed);
        }
        for (w, v) in s.words[s.bins..].iter().zip(peaks) {
            w.store(v.to_bits(), Ordering::Relaxed);
        }
        s.seq.store(seq.wrapping_add(2), Ordering::Release);
    }
}

/// UI-side end of the feed. Cheap to clone; any number of readers.
/// See the [`SpectrumWriter`] and [`SpectrumAnalyzer`] examples.
#[derive(Clone)]
pub struct SpectrumReader {
    shared: Arc<Shared>,
}

impl SpectrumReader {
    /// Bins per frame.
    pub fn bins(&self) -> usize {
        self.shared.bins
    }

    /// Copy the newest frame into `mags` and `peaks` (each `bins()` long or
    /// shorter) and return its frame number (1 for the first). `None` when
    /// nothing was published yet or the writer kept overtaking the read.
    pub fn read(&self, mags: &mut [f32], peaks: &mut [f32]) -> Option<u64> {
        let s = &*self.shared;
        for _ in 0..64 {
            let a = s.seq.load(Ordering::Acquire);
            if a == 0 {
                return None;
            }
            if a & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            for (o, w) in mags.iter_mut().zip(&s.words[..s.bins]) {
                *o = f32::from_bits(w.load(Ordering::Relaxed));
            }
            for (o, w) in peaks.iter_mut().zip(&s.words[s.bins..]) {
                *o = f32::from_bits(w.load(Ordering::Relaxed));
            }
            fence(Ordering::Acquire);
            if s.seq.load(Ordering::Relaxed) == a {
                return Some(a / 2);
            }
        }
        None
    }
}

/// Analyser settings.
/// # Example
///
/// ```
/// use openplugin_dsp::SpectrumConfig;
///
/// let config = SpectrumConfig { average_ms: 0.0, ..SpectrumConfig::new(4096, 48_000.0) };
/// assert_eq!(config.hop, 2048);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectrumConfig {
    /// FFT size, a power of two of at least 4.
    pub fft_size: usize,
    /// Samples between frames, `1..=fft_size`.
    pub hop: usize,
    /// Analysis window.
    pub window: WindowKind,
    /// Sample rate, Hz.
    pub sample_rate: f32,
    /// Time constant of the average over frames, in ms. 0 shows each frame raw.
    pub average_ms: f32,
    /// How long a peak is held, in ms, before it falls. 0 disables the hold (peak follows the trace).
    pub peak_hold_ms: f32,
    /// Fall rate of a released peak, dB per second.
    pub peak_fall_db_per_s: f32,
    /// Lowest level reported, in dB.
    pub floor_db: f32,
}

impl SpectrumConfig {
    /// Hann, 50 % overlap, 100 ms average, 1 s hold falling at 20 dB/s, -140 dB floor.
    pub fn new(fft_size: usize, sample_rate: f32) -> Self {
        Self {
            fft_size,
            hop: fft_size / 2,
            window: WindowKind::Hann,
            sample_rate,
            average_ms: 100.0,
            peak_hold_ms: 1000.0,
            peak_fall_db_per_s: 20.0,
            floor_db: -140.0,
        }
    }
}

/// Mono spectrum analyser that feeds an editor. Build with [`SpectrumAnalyzer::new`].
/// `new` allocates; `process` is allocation-free and wait-free.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{bin_frequency, SpectrumAnalyzer, SpectrumConfig};
///
/// let rate = 48_000.0;
/// let (mut analyzer, reader) = SpectrumAnalyzer::new(SpectrumConfig::new(1024, rate)).unwrap();
/// let tone: Vec<f32> = (0..4_096)
///     .map(|n| (2.0 * std::f32::consts::PI * 3_000.0 * n as f32 / rate).sin())
///     .collect();
/// analyzer.process(&tone); // audio thread
///
/// // Editor thread: read the newest frame.
/// let mut mags = vec![0.0f32; reader.bins()];
/// let mut peaks = vec![0.0f32; reader.bins()];
/// assert!(reader.read(&mut mags, &mut peaks).is_some());
/// let loudest = (0..mags.len()).max_by(|&a, &b| mags[a].total_cmp(&mags[b])).unwrap();
/// assert!((bin_frequency(loudest, 1024, rate) - 3_000.0).abs() < 50.0);
/// ```
pub struct SpectrumAnalyzer {
    n: usize,
    hop: usize,
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    buf: Vec<Complex<f32>>,
    window: Vec<f32>,
    scale: f32,
    input: Vec<f32>,
    pos: usize,
    since: usize,
    avg_coeff: f32,
    averaged: Vec<f32>,
    mags_db: Vec<f32>,
    peaks_db: Vec<f32>,
    hold_left: Vec<u32>,
    hold_frames: u32,
    fall_db: f32,
    floor_db: f32,
    writer: SpectrumWriter,
}

impl SpectrumAnalyzer {
    /// Prepare: plans the FFT, allocates, and creates the feed. Keep the reader for the editor.
    pub fn new(config: SpectrumConfig) -> Result<(Self, SpectrumReader), StftError> {
        let n = config.fft_size;
        if n < 4 || !n.is_power_of_two() {
            return Err(StftError::FrameSize);
        }
        if config.hop == 0 || config.hop > n {
            return Err(StftError::Hop);
        }
        let sr = config.sample_rate.max(1.0);
        let hop_s = config.hop as f32 / sr;
        let window = config.window.to_vec(n, true);
        let sum: f64 = window.iter().map(|&v| v as f64).sum();
        let bins = n / 2 + 1;
        let (writer, reader) = spectrum_feed(bins);
        let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
        let scratch_len = fft.get_inplace_scratch_len();
        let avg_coeff = if config.average_ms > 0.0 {
            (-hop_s / (config.average_ms * 1e-3)).exp()
        } else {
            0.0
        };
        let floor_db = config.floor_db.min(-1.0);
        Ok((
            Self {
                n,
                hop: config.hop,
                fft,
                scratch: vec![Complex::default(); scratch_len],
                buf: vec![Complex::default(); n],
                window,
                scale: (1.0 / sum) as f32,
                input: vec![0.0; n],
                pos: 0,
                since: 0,
                avg_coeff,
                averaged: vec![0.0; bins],
                mags_db: vec![floor_db; bins],
                peaks_db: vec![floor_db; bins],
                hold_left: vec![0; bins],
                hold_frames: (config.peak_hold_ms.max(0.0) * 1e-3 / hop_s).ceil() as u32,
                fall_db: config.peak_fall_db_per_s.max(0.0) * hop_s,
                floor_db,
                writer,
            },
            reader,
        ))
    }

    /// Bins per spectrum, `fft_size / 2 + 1`.
    pub fn bins(&self) -> usize {
        self.n / 2 + 1
    }

    /// Frequency of bin `k` in Hz for the sample rate this was built with.
    pub fn fft_size(&self) -> usize {
        self.n
    }

    /// Latest magnitudes in dB (what the feed carries).
    pub fn magnitudes_db(&self) -> &[f32] {
        &self.mags_db
    }

    /// Latest peak-hold trace in dB.
    pub fn peaks_db(&self) -> &[f32] {
        &self.peaks_db
    }

    /// Clears the input history, the average and the peaks.
    pub fn reset(&mut self) {
        self.input.fill(0.0);
        self.averaged.fill(0.0);
        self.mags_db.fill(self.floor_db);
        self.peaks_db.fill(self.floor_db);
        self.hold_left.fill(0);
        self.pos = 0;
        self.since = 0;
    }

    /// Feed audio. Publishes a frame every hop. Allocation free and wait free.
    pub fn process(&mut self, input: &[f32]) {
        for &x in input {
            let x = if x.is_finite() { x } else { 0.0 };
            self.input[self.pos] = x;
            self.pos = if self.pos + 1 == self.n { 0 } else { self.pos + 1 };
            self.since += 1;
            if self.since == self.hop {
                self.since = 0;
                self.frame();
            }
        }
    }

    fn frame(&mut self) {
        let n = self.n;
        for k in 0..n {
            let idx = if self.pos + k >= n { self.pos + k - n } else { self.pos + k };
            self.buf[k] = Complex::new(self.input[idx] * self.window[k], 0.0);
        }
        self.fft.process_with_scratch(&mut self.buf, &mut self.scratch);
        let a = self.avg_coeff;
        const DB: f32 = 8.685_889; // 20 / ln 10
        for k in 0..=n / 2 {
            let edge = if k == 0 || k == n / 2 { 1.0 } else { 2.0 };
            let m = self.buf[k].norm() * self.scale * edge;
            self.averaged[k] = a * self.averaged[k] + (1.0 - a) * m;
            let db = (self.averaged[k].max(1e-20).ln() * DB).max(self.floor_db);
            self.mags_db[k] = db;
            let now = (m.max(1e-20).ln() * DB).max(self.floor_db);
            if self.hold_frames == 0 {
                self.peaks_db[k] = db;
            } else if now >= self.peaks_db[k] {
                self.peaks_db[k] = now;
                self.hold_left[k] = self.hold_frames;
            } else if self.hold_left[k] > 0 {
                self.hold_left[k] -= 1;
            } else {
                self.peaks_db[k] = (self.peaks_db[k] - self.fall_db).max(now).max(self.floor_db);
            }
        }
        self.writer.publish(&self.mags_db, &self.peaks_db);
    }
}

/// Frequency of bin `k` for an FFT of `fft_size` at `sample_rate`.
pub fn bin_frequency(k: usize, fft_size: usize, sample_rate: f32) -> f32 {
    k as f32 * sample_rate / fft_size as f32
}

/// Resample linear-frequency dB bins (`0..=fft_size/2`) onto `out.len()`
/// columns spaced logarithmically between `f_lo` and `f_hi` Hz. A column that
/// spans several bins shows their maximum; one narrower than a bin
/// interpolates between neighbours in dB. Allocation free.
pub fn log_frequency_bins(
    db: &[f32],
    sample_rate: f32,
    fft_size: usize,
    f_lo: f32,
    f_hi: f32,
    out: &mut [f32],
) {
    let cols = out.len();
    if cols == 0 || db.is_empty() {
        return;
    }
    let last = db.len() - 1;
    let f_lo = f_lo.max(1e-3);
    let f_hi = f_hi.max(f_lo * 1.0001);
    let per_hz = fft_size as f32 / sample_rate.max(1.0);
    let ratio = (f_hi / f_lo).ln() / cols as f32;
    for (i, o) in out.iter_mut().enumerate() {
        let b0 = (f_lo * (ratio * i as f32).exp() * per_hz).clamp(0.0, last as f32);
        let b1 = (f_lo * (ratio * (i + 1) as f32).exp() * per_hz).clamp(0.0, last as f32);
        let first = b0.ceil() as usize;
        let lastb = b1.floor() as usize;
        if lastb >= first && b1 - b0 >= 1.0 {
            *o = db[first..=lastb].iter().fold(f32::NEG_INFINITY, |m, &v| m.max(v));
        } else {
            let c = 0.5 * (b0 + b1);
            let lo = c.floor() as usize;
            let hi = (lo + 1).min(last);
            let t = c - lo as f32;
            *o = db[lo] * (1.0 - t) + db[hi] * t;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(f: f32, sr: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (std::f64::consts::TAU * f as f64 * i as f64 / sr as f64).sin() as f32)
            .collect()
    }

    fn peak_bin(db: &[f32]) -> usize {
        db.iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0
    }

    #[test]
    fn bin_centred_tone_lands_on_its_bin_at_its_level() {
        let sr = 48_000.0;
        for window in [WindowKind::Hann, WindowKind::BlackmanHarris, WindowKind::FlatTop, WindowKind::Kaiser { beta: 8.0 }] {
            let mut cfg = SpectrumConfig::new(4096, sr);
            cfg.window = window;
            cfg.average_ms = 0.0;
            let (mut a, _r) = SpectrumAnalyzer::new(cfg).unwrap();
            let k = 256; // 3 kHz
            let f = bin_frequency(k, 4096, sr);
            a.process(&tone(f, sr, 0.5, 16384));
            let db = a.magnitudes_db();
            assert_eq!(peak_bin(db), k, "{window:?}");
            let want = 20.0 * 0.5f32.log10();
            assert!((db[k] - want).abs() < 0.01, "{window:?}: {} vs {want}", db[k]);
        }
    }

    #[test]
    fn flat_top_reads_an_off_bin_tone_within_five_hundredths_of_a_db() {
        let sr = 48_000.0;
        let mut cfg = SpectrumConfig::new(4096, sr);
        cfg.window = WindowKind::FlatTop;
        cfg.average_ms = 0.0;
        let (mut a, _r) = SpectrumAnalyzer::new(cfg).unwrap();
        let f = bin_frequency(300, 4096, sr) + 0.5 * sr / 4096.0; // half a bin off
        a.process(&tone(f, sr, 1.0, 16384));
        let db = a.magnitudes_db();
        let peak = db[peak_bin(db)];
        assert!(peak.abs() < 0.05, "{peak}");
        assert!((299..=301).contains(&peak_bin(db)));
    }

    #[test]
    fn dc_and_silence() {
        let (mut a, _r) = SpectrumAnalyzer::new(SpectrumConfig::new(256, 48_000.0)).unwrap();
        a.process(&vec![0.0; 1024]);
        assert!(a.magnitudes_db().iter().all(|&v| v == -140.0));
    }

    #[test]
    fn peak_holds_then_falls() {
        let sr = 48_000.0;
        let mut cfg = SpectrumConfig::new(1024, sr);
        cfg.average_ms = 0.0;
        cfg.peak_hold_ms = 100.0;
        cfg.peak_fall_db_per_s = 100.0;
        let (mut a, _r) = SpectrumAnalyzer::new(cfg).unwrap();
        let f = bin_frequency(64, 1024, sr);
        a.process(&tone(f, sr, 1.0, 4096));
        let k = 64;
        let top = a.peaks_db()[k];
        a.process(&vec![0.0; 1024]); // ~21 ms of silence: still held
        assert!((a.peaks_db()[k] - top).abs() < 1e-3);
        assert!(a.magnitudes_db()[k] < top - 6.0 || a.magnitudes_db()[k] < top);
        a.process(&vec![0.0; 48_000]); // 1 s: held 100 ms then fell for 0.9 s
        assert!(a.peaks_db()[k] < top - 60.0, "{} vs {top}", a.peaks_db()[k]);
    }

    #[test]
    fn averaging_smooths_a_burst() {
        let sr = 48_000.0;
        let mut cfg = SpectrumConfig::new(1024, sr);
        cfg.average_ms = 200.0;
        let (mut a, _r) = SpectrumAnalyzer::new(cfg).unwrap();
        let f = bin_frequency(64, 1024, sr);
        a.process(&tone(f, sr, 1.0, 1024));
        let early = a.magnitudes_db()[64];
        a.process(&tone(f, sr, 1.0, 48_000));
        let late = a.magnitudes_db()[64];
        assert!(early < late - 3.0 && late.abs() < 0.1, "{early} {late}");
    }

    #[test]
    fn log_bins_place_a_tone_at_the_right_column() {
        let sr = 48_000.0;
        let mut cfg = SpectrumConfig::new(8192, sr);
        cfg.average_ms = 0.0;
        let (mut a, _r) = SpectrumAnalyzer::new(cfg).unwrap();
        let f = bin_frequency(853, 8192, sr); // about 4997 Hz
        a.process(&tone(f, sr, 1.0, 32768));
        let mut cols = vec![0.0; 300];
        log_frequency_bins(a.magnitudes_db(), sr, 8192, 20.0, 20_000.0, &mut cols);
        let c = peak_bin(&cols);
        let centre = 20.0 * (1000.0f32).powf((c as f32 + 0.5) / 300.0);
        assert!((centre / f - 1.0).abs() < 0.02, "col {c} centre {centre} vs {f}");
    }

    #[test]
    fn feed_reports_nothing_before_the_first_frame() {
        let (_w, r) = spectrum_feed(4);
        let (mut m, mut p) = ([0.0; 4], [0.0; 4]);
        assert_eq!(r.read(&mut m, &mut p), None);
    }

    #[test]
    fn concurrent_reader_never_sees_a_torn_frame() {
        let (mut w, r) = spectrum_feed(512);
        let writer = std::thread::spawn(move || {
            let mut frame = vec![0.0f32; 512];
            for n in 1..=200_000u32 {
                frame.fill(n as f32);
                w.publish(&frame, &frame);
            }
        });
        let mut m = vec![0.0f32; 512];
        let mut p = vec![0.0f32; 512];
        let (mut last, mut seen) = (0u64, 0u64);
        while last < 200_000 {
            if let Some(n) = r.read(&mut m, &mut p) {
                assert!(n >= last, "frame numbers went back");
                assert!(m.iter().all(|&v| v == m[0]), "torn magnitudes");
                assert!(p.iter().all(|&v| v == m[0]), "torn peaks");
                assert_eq!(m[0] as u64, n, "frame number and content disagree");
                last = n;
                seen += 1;
            }
        }
        writer.join().unwrap();
        assert!(seen > 0);
    }
}
