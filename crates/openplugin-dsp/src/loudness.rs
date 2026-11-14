//! Loudness after ITU-R BS.1770-4 and EBU R 128, and loudness-matched gain.
//!
//! - [`KWeighting`]: the two-stage K filter (a high shelf, then a high pass).
//!   BS.1770-4 tabulates its coefficients at 48 kHz only; this derives them
//!   at any rate from the analogue prototype behind that table (the shelf at
//!   about 1682 Hz, +4 dB, and the high pass at about 38 Hz), which matches
//!   the 48 kHz table to its printed precision.
//! - [`Loudness`]: momentary (400 ms), short-term (3 s) and gated integrated
//!   loudness in LUFS. Blocks are 400 ms with 75 % overlap; the absolute gate
//!   is -70 LUFS and the relative gate -10 LU, as BS.1770-4 sets them. The
//!   integrated value keeps a fixed histogram of 0.1 LU bins, so it never
//!   allocates while running.
//! - [`LoudnessMatch`]: automatic make-up gain that holds the processed
//!   signal at the loudness of a reference (the plugin's input), measured
//!   over a sliding K-weighted window, gated, clamped, and glided.
//!
//! Channel weights are 1 for every channel (mono, left, right). Nothing here
//! allocates after `new`, and nothing panics on non-finite input (it counts
//! as silence).

/// Below every gate: what a meter reports with no signal.
pub const SILENCE_LUFS: f32 = f32::NEG_INFINITY;
/// BS.1770 absolute gate.
pub const ABSOLUTE_GATE_LUFS: f64 = -70.0;
/// BS.1770 relative gate, below the ungated level.
pub const RELATIVE_GATE_LU: f64 = -10.0;

fn rate_of(sample_rate: f32) -> f64 {
    if sample_rate.is_finite() && sample_rate >= 8_000.0 {
        f64::from(sample_rate)
    } else {
        48_000.0
    }
}

/// Mean square energy to LUFS: `-0.691 + 10 log10(energy)`.
pub fn lufs_of(energy: f64) -> f64 {
    if energy > 0.0 {
        -0.691 + 10.0 * energy.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// LUFS to mean square energy.
pub fn energy_of(lufs: f64) -> f64 {
    10f64.powf((lufs + 0.691) / 10.0)
}

#[derive(Clone, Copy, Debug, Default)]
struct Section {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Section {
    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        // Transposed direct form II.
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The K-weighting filter of BS.1770-4 for one channel.
#[derive(Clone, Debug)]
pub struct KWeighting {
    shelf: Section,
    high_pass: Section,
}

impl KWeighting {
    pub fn new(sample_rate: f32) -> Self {
        let fs = rate_of(sample_rate);
        // Stage 1: the head's acoustic effect, a high shelf.
        let f0 = 1_681.974_450_955_533;
        let gain_db = 3.999_843_853_973_347;
        let q = 0.707_175_236_955_419_6;
        let k = (std::f64::consts::PI * f0 / fs).tan();
        let vh = 10f64.powf(gain_db / 20.0);
        let vb = vh.powf(0.499_666_774_154_541_6);
        let a0 = 1.0 + k / q + k * k;
        let shelf = Section {
            b: [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0],
            a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
            z: [0.0; 2],
        };
        // Stage 2: the RLB high pass.
        let f0 = 38.135_470_876_024_44;
        let q = 0.500_327_037_323_877_3;
        let k = (std::f64::consts::PI * f0 / fs).tan();
        let a0 = 1.0 + k / q + k * k;
        let high_pass = Section {
            b: [1.0, -2.0, 1.0],
            a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
            z: [0.0; 2],
        };
        Self { shelf, high_pass }
    }

    pub fn reset(&mut self) {
        self.shelf.z = [0.0; 2];
        self.high_pass.z = [0.0; 2];
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f64 {
        let x = if x.is_finite() { f64::from(x) } else { 0.0 };
        self.high_pass.run(self.shelf.run(x))
    }

    /// The coefficients `(b, a)` of both stages, `a[0] == 1` left out.
    pub fn coefficients(&self) -> [([f64; 3], [f64; 2]); 2] {
        [(self.shelf.b, self.shelf.a), (self.high_pass.b, self.high_pass.a)]
    }
}

/// K-weighted energy summed over channels, collected in 100 ms steps.
#[derive(Clone, Debug)]
struct Steps {
    filters: [KWeighting; 2],
    step_len: usize,
    in_step: usize,
    acc: f64,
}

impl Steps {
    fn new(sample_rate: f32) -> Self {
        let fs = rate_of(sample_rate);
        Self {
            filters: [KWeighting::new(sample_rate), KWeighting::new(sample_rate)],
            step_len: (fs * 0.1).round().max(1.0) as usize,
            in_step: 0,
            acc: 0.0,
        }
    }

    fn reset(&mut self) {
        for f in &mut self.filters {
            f.reset();
        }
        self.in_step = 0;
        self.acc = 0.0;
    }

    /// Feed one frame. Returns the mean energy of a finished 100 ms step.
    #[inline]
    fn push(&mut self, channels: &[&[f32]], frame: usize) -> Option<f64> {
        for (filter, channel) in self.filters.iter_mut().zip(channels.iter().take(2)) {
            let y = filter.process(channel.get(frame).copied().unwrap_or(0.0));
            self.acc += y * y;
        }
        self.in_step += 1;
        if self.in_step < self.step_len {
            return None;
        }
        let energy = self.acc / self.step_len as f64;
        self.acc = 0.0;
        self.in_step = 0;
        Some(energy)
    }
}

/// The last `N` 100 ms step energies.
#[derive(Clone, Debug)]
struct Window {
    ring: Box<[f64]>,
    next: usize,
    filled: usize,
}

impl Window {
    fn new(steps: usize) -> Self {
        Self { ring: vec![0.0; steps.max(1)].into_boxed_slice(), next: 0, filled: 0 }
    }

    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.next = 0;
        self.filled = 0;
    }

    fn push(&mut self, energy: f64) {
        self.ring[self.next] = energy;
        self.next = (self.next + 1) % self.ring.len();
        self.filled = (self.filled + 1).min(self.ring.len());
    }

    /// Mean of the last `count` steps (fewer while filling); `None` before any.
    fn mean(&self, count: usize) -> Option<f64> {
        let count = count.min(self.filled);
        if count == 0 {
            return None;
        }
        let len = self.ring.len();
        let sum: f64 = (1..=count).map(|back| self.ring[(self.next + len - back) % len]).sum();
        Some(sum / count as f64)
    }
}

const BIN_LU: f64 = 0.1;
const HIST_LOW: f64 = ABSOLUTE_GATE_LUFS;
const HIST_HIGH: f64 = 10.0;
const BINS: usize = ((HIST_HIGH - HIST_LOW) / BIN_LU) as usize;

/// Momentary, short-term and integrated loudness of one stereo (or mono) bus.
#[derive(Clone, Debug)]
pub struct Loudness {
    steps: Steps,
    window: Window,
    /// Gating blocks per 0.1 LU bin above the absolute gate, and their energy.
    counts: Box<[u64]>,
    energies: Box<[f64]>,
}

impl Loudness {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            steps: Steps::new(sample_rate),
            window: Window::new(30),
            counts: vec![0; BINS].into_boxed_slice(),
            energies: vec![0.0; BINS].into_boxed_slice(),
        }
    }

    pub fn reset(&mut self) {
        self.steps.reset();
        self.window.reset();
        self.counts.fill(0);
        self.energies.fill(0.0);
    }

    /// Restart the integrated measurement only.
    pub fn reset_integrated(&mut self) {
        self.counts.fill(0);
        self.energies.fill(0.0);
    }

    /// Feed a block: one slice per channel (one or two are read), all the same length.
    pub fn push(&mut self, channels: &[&[f32]]) {
        let frames = channels.first().map(|c| c.len()).unwrap_or(0);
        for frame in 0..frames {
            if let Some(energy) = self.steps.push(channels, frame) {
                self.window.push(energy);
                // A 400 ms gating block ends every 100 ms (75 % overlap).
                if self.window.filled >= 4 {
                    if let Some(block) = self.window.mean(4) {
                        self.gate_block(block);
                    }
                }
            }
        }
    }

    fn gate_block(&mut self, energy: f64) {
        let lufs = lufs_of(energy);
        if lufs.is_nan() || lufs < HIST_LOW {
            return;
        }
        let bin = (((lufs - HIST_LOW) / BIN_LU) as usize).min(BINS - 1);
        self.counts[bin] += 1;
        self.energies[bin] += energy;
    }

    fn report(energy: Option<f64>) -> f32 {
        match energy {
            Some(e) if e > 0.0 => lufs_of(e) as f32,
            _ => SILENCE_LUFS,
        }
    }

    /// 400 ms loudness, LUFS.
    pub fn momentary_lufs(&self) -> f32 {
        if self.window.filled < 4 {
            return SILENCE_LUFS;
        }
        Self::report(self.window.mean(4))
    }

    /// 3 s loudness, LUFS. Reads over what is there until 3 s have passed.
    pub fn short_term_lufs(&self) -> f32 {
        Self::report(self.window.mean(30))
    }

    /// Gated loudness since the last reset, LUFS.
    pub fn integrated_lufs(&self) -> f32 {
        let (count, energy) = self.counts.iter().zip(self.energies.iter()).fold((0u64, 0.0f64), |(c, e), (n, x)| (c + n, e + x));
        if count == 0 {
            return SILENCE_LUFS;
        }
        let threshold = lufs_of(energy / count as f64) + RELATIVE_GATE_LU;
        let first = if threshold <= HIST_LOW { 0 } else { (((threshold - HIST_LOW) / BIN_LU).ceil() as usize).min(BINS) };
        let (count, energy) = self.counts[first..]
            .iter()
            .zip(self.energies[first..].iter())
            .fold((0u64, 0.0f64), |(c, e), (n, x)| (c + n, e + x));
        if count == 0 {
            return SILENCE_LUFS;
        }
        lufs_of(energy / count as f64) as f32
    }
}

/// Loudness-matched gain: keeps the processed signal as loud as a reference.
///
/// Both signals are K-weighted and averaged over a sliding window (3 s by
/// default, [`Self::with_window`] up to 10 s), updated every 100 ms. When
/// both are above the gate, the target gain is the difference, clamped to
/// ±`limit` dB; below the gate (a pause, a fade) the gain holds, so silence
/// does not pump it. The applied gain glides to the target in dB.
///
/// [`Self::set_enabled`]`(false)` glides back to unity and keeps measuring,
/// so turning it on again starts from a current estimate. [`Self::freeze`]
/// holds the gain and stops learning (a host bypass, an offline preview).
/// Measure the reference where the plugin's input enters it, before any
/// input gain the user means as drive.
#[derive(Clone, Debug)]
pub struct LoudnessMatch {
    reference: Steps,
    processed: Steps,
    reference_window: Window,
    processed_window: Window,
    window_steps: usize,
    gate_lufs: f64,
    limit_db: f64,
    target_db: f64,
    gain_db: f64,
    glide: f64,
    enabled: bool,
    frozen: bool,
}

impl LoudnessMatch {
    /// A 3 s window, a -70 LUFS gate, ±24 dB, a 50 ms glide.
    pub fn new(sample_rate: f32) -> Self {
        let fs = rate_of(sample_rate);
        Self {
            reference: Steps::new(sample_rate),
            processed: Steps::new(sample_rate),
            reference_window: Window::new(100),
            processed_window: Window::new(100),
            window_steps: 30,
            gate_lufs: ABSOLUTE_GATE_LUFS,
            limit_db: 24.0,
            target_db: 0.0,
            gain_db: 0.0,
            glide: 1.0 - (-1.0 / (fs * 0.05)).exp(),
            enabled: true,
            frozen: false,
        }
    }

    /// Measurement window in seconds, 0.4 to 10.
    pub fn with_window(mut self, seconds: f32) -> Self {
        let s = if seconds.is_finite() { seconds.clamp(0.4, 10.0) } else { 3.0 };
        self.window_steps = (s * 10.0).round() as usize;
        self
    }

    /// Correction limit in dB, 0 to 60.
    pub fn with_limit(mut self, db: f32) -> Self {
        self.limit_db = if db.is_finite() { f64::from(db.clamp(0.0, 60.0)) } else { 24.0 };
        self
    }

    /// Gate in LUFS: below it, a signal does not move the gain.
    pub fn with_gate(mut self, lufs: f32) -> Self {
        self.gate_lufs = if lufs.is_finite() { f64::from(lufs) } else { ABSOLUTE_GATE_LUFS };
        self
    }

    /// Glide time constant in milliseconds (at least 1).
    pub fn with_glide_ms(mut self, ms: f32, sample_rate: f32) -> Self {
        let ms = if ms.is_finite() { f64::from(ms.max(1.0)) } else { 50.0 };
        self.glide = 1.0 - (-1.0 / (rate_of(sample_rate) * ms * 0.001)).exp();
        self
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Hold the gain where it is and stop learning.
    pub fn freeze(&mut self, frozen: bool) {
        self.frozen = frozen;
    }

    /// The gain being applied now, dB.
    pub fn gain_db(&self) -> f32 {
        self.gain_db as f32
    }

    /// The gain it is gliding to, dB (zero while disabled).
    pub fn target_db(&self) -> f32 {
        if self.enabled {
            self.target_db as f32
        } else {
            0.0
        }
    }

    /// Forget the measurement and return to unity at once.
    pub fn reset(&mut self) {
        self.reference.reset();
        self.processed.reset();
        self.reference_window.reset();
        self.processed_window.reset();
        self.target_db = 0.0;
        self.gain_db = 0.0;
    }

    /// Re-learn from the last `seconds` only (after the user changes a
    /// setting that moves the level), keeping the current gain meanwhile.
    pub fn relearn(&mut self, seconds: f32) {
        let keep = if seconds.is_finite() { (seconds * 10.0).round().max(1.0) as usize } else { 5 };
        for window in [&mut self.reference_window, &mut self.processed_window] {
            window.filled = window.filled.min(keep);
        }
    }

    fn update_target(&mut self) {
        let (Some(reference), Some(processed)) =
            (self.reference_window.mean(self.window_steps), self.processed_window.mean(self.window_steps))
        else {
            return;
        };
        let (reference, processed) = (lufs_of(reference), lufs_of(processed));
        if reference >= self.gate_lufs && processed >= self.gate_lufs {
            self.target_db = (reference - processed).clamp(-self.limit_db, self.limit_db);
        }
    }

    /// Measure `reference` and `io` (the processed signal, before this
    /// gain), then apply the gain to `io` in place. One slice per channel;
    /// one or two channels are measured, every channel of `io` gets the gain.
    pub fn process(&mut self, reference: &[&[f32]], io: &mut [&mut [f32]]) {
        let frames = io.first().map(|c| c.len()).unwrap_or(0);
        for frame in 0..frames {
            if !self.frozen {
                let r = self.reference.push(reference, frame);
                let measured: [&[f32]; 2] = [
                    io.first().map(|c| &c[frame..=frame]).unwrap_or(&[]),
                    io.get(1).map(|c| &c[frame..=frame]).unwrap_or(&[]),
                ];
                let count = io.len().min(2);
                let p = self.processed.push(&measured[..count], 0);
                if let (Some(r), Some(p)) = (r, p) {
                    self.reference_window.push(r);
                    self.processed_window.push(p);
                    self.update_target();
                }
                let goal = if self.enabled { self.target_db } else { 0.0 };
                self.gain_db += (goal - self.gain_db) * self.glide;
            }
            let gain = 10f64.powf(self.gain_db / 20.0) as f32;
            for channel in io.iter_mut() {
                let s = &mut channel[frame];
                *s = if s.is_finite() { *s * gain } else { 0.0 };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, amp: f32, rate: f32, frames: usize) -> Vec<f32> {
        (0..frames).map(|n| amp * (std::f32::consts::TAU * freq * n as f32 / rate).sin()).collect()
    }

    #[test]
    fn k_filter_matches_the_bs1770_table_at_48k() {
        let [(b1, a1), (b2, a2)] = KWeighting::new(48_000.0).coefficients();
        let close = |x: f64, y: f64| (x - y).abs() < 1e-8;
        assert!(close(b1[0], 1.535_124_859_586_97), "{}", b1[0]);
        assert!(close(b1[1], -2.691_696_189_406_38), "{}", b1[1]);
        assert!(close(b1[2], 1.198_392_810_852_85), "{}", b1[2]);
        assert!(close(a1[0], -1.690_659_293_182_41), "{}", a1[0]);
        assert!(close(a1[1], 0.732_480_774_215_85), "{}", a1[1]);
        assert_eq!(b2, [1.0, -2.0, 1.0]);
        assert!(close(a2[0], -1.990_047_454_833_98), "{}", a2[0]);
        assert!(close(a2[1], 0.990_072_250_366_21), "{}", a2[1]);
    }

    /// The EBU Tech 3341 calibration: a stereo 1 kHz sine of amplitude A dBFS
    /// reads A LUFS. Each channel's RMS is 3 dB below A, two channels add the
    /// 3 dB back, and the K filter's +0.691 dB at 1 kHz is what the -0.691
    /// offset takes out.
    #[test]
    fn stereo_1k_sine_reads_its_level() {
        for rate in [44_100.0f32, 48_000.0, 96_000.0] {
            let peak = 10f32.powf(-20.0 / 20.0); // -20 dBFS peak, -23.01 dBFS RMS
            let tone = sine(1_000.0, peak, rate, (rate * 4.0) as usize);
            let mut meter = Loudness::new(rate);
            meter.push(&[&tone, &tone]);
            for value in [meter.momentary_lufs(), meter.short_term_lufs(), meter.integrated_lufs()] {
                assert!((value - -20.0).abs() < 0.1, "{rate} Hz: {value}");
            }
        }
    }

    #[test]
    fn integrated_gates_silence_and_quiet_passages() {
        let rate = 48_000.0;
        let loud = sine(1_000.0, 10f32.powf(-20.0 / 20.0), rate, 48_000 * 5);
        let quiet = sine(1_000.0, 10f32.powf(-60.0 / 20.0), rate, 48_000 * 5);
        let silence = vec![0.0f32; 48_000 * 5];
        let mut meter = Loudness::new(rate);
        for block in [&loud, &silence, &quiet] {
            meter.push(&[block, block]);
        }
        // Silence is below the absolute gate and -60 is below the relative gate.
        let value = meter.integrated_lufs();
        assert!((value - -20.0).abs() < 0.2, "{value}");
        let mut empty = Loudness::new(rate);
        empty.push(&[&silence]);
        assert_eq!(empty.integrated_lufs(), SILENCE_LUFS);
        assert_eq!(empty.short_term_lufs(), SILENCE_LUFS);
    }

    #[test]
    fn match_brings_the_output_back_to_the_reference_and_holds_in_pauses() {
        let rate = 48_000.0;
        let reference = sine(220.0, 0.5, rate, 48_000 * 6);
        let mut auto = LoudnessMatch::new(rate);
        let mut out_l: Vec<f32> = reference.iter().map(|x| x * 0.25).collect(); // 12 dB down
        let mut out_r = out_l.clone();
        for start in (0..reference.len()).step_by(512) {
            let end = (start + 512).min(reference.len());
            let r = &reference[start..end];
            let (l, rr) = (&mut out_l[start..end], &mut out_r[start..end]);
            auto.process(&[r, r], &mut [l, rr]);
        }
        assert!((auto.gain_db() - 12.04).abs() < 0.1, "{}", auto.gain_db());
        let tail = &out_l[out_l.len() - 4_800..];
        let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.01, "{peak}");
        // Two seconds of silence: the gain holds.
        let silent = vec![0.0f32; 96_000];
        let mut l = silent.clone();
        let mut r = silent.clone();
        auto.process(&[&silent, &silent], &mut [&mut l, &mut r]);
        assert!((auto.gain_db() - 12.04).abs() < 0.1, "{}", auto.gain_db());
        // Disabled: back to unity.
        auto.set_enabled(false);
        let mut l = silent.clone();
        auto.process(&[&silent], &mut [&mut l]);
        assert!(auto.gain_db().abs() < 0.01, "{}", auto.gain_db());
    }

    #[test]
    fn match_is_clamped_and_freeze_holds() {
        let rate = 48_000.0;
        let reference = sine(500.0, 0.9, rate, 48_000 * 4);
        let mut auto = LoudnessMatch::new(rate).with_limit(6.0).with_window(1.0);
        let mut out: Vec<f32> = reference.iter().map(|x| x * 0.01).collect(); // 40 dB down
        auto.process(&[&reference], &mut [&mut out]);
        assert!((auto.gain_db() - 6.0).abs() < 0.05, "{}", auto.gain_db());
        auto.freeze(true);
        let mut loud = reference.clone();
        auto.process(&[&reference], &mut [&mut loud]);
        assert!((auto.gain_db() - 6.0).abs() < 0.05, "frozen: {}", auto.gain_db());
        let mut nan = vec![f32::NAN; 64];
        auto.process(&[&nan.clone()], &mut [&mut nan]);
        assert!(nan.iter().all(|v| *v == 0.0));
    }
}
