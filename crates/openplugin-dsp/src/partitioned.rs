//! Long convolution: uniformly and non-uniformly partitioned, zero latency.
//!
//! This module plays impulse responses of seconds (reverbs, cabinets, rooms,
//! head-related responses) at 44.1 to 192 kHz. [`BlockConvolver`](crate::BlockConvolver)
//! stays the right tool for kernels up to 1024 taps; this is the one for the
//! rest.
//!
//! # Method
//!
//! A long linear convolution `y = x * h` is split in the impulse. Cut `h` into
//! partitions of `N` taps. Each partition is transformed once (zero padded to
//! `2N`, forward FFT) when the impulse is loaded. For every `N` input samples
//! the newest input block is transformed too and pushed into a *frequency-
//! domain delay line* (FDL) that keeps the spectra of the last blocks. The
//! output block is the inverse transform of `sum_p X[j-p] * H[p]` (one complex
//! multiply-add per bin and partition), with the second half of each result
//! added into the next block (overlap-add). This is uniformly partitioned
//! convolution, described by Stockham 1966 ("High-speed convolution and
//! correlation", AFIPS Spring Joint Computer Conf.) and, with the FDL, by
//! Soo and Pang 1990 ("Multidelay block frequency domain adaptive filter",
//! IEEE Trans. ASSP 38(2)).
//!
//! # Zero latency: direct head, then growing partitions
//!
//! A block transform needs a whole block of input before it can answer, so
//! taps `0..N` cannot go through it without adding `N` samples of latency.
//! Following Gardner 1995 ("Efficient convolution without input-output delay",
//! J. Audio Eng. Soc. 43(3)), the first taps are computed directly, one
//! multiply-add per tap and sample. The stage with block size `N` then starts
//! at tap `N`: its result for output block `m` only needs input blocks up to
//! `m-1`, which are complete when block `m` begins. The same argument chains:
//!
//! | part | taps | how |
//! |---|---|---|
//! | head | `0 .. N1` | direct form, every sample |
//! | stage 1 | `N1 .. N2` | partitions of `N1`, FFT size `2*N1` |
//! | stage 2 | `N2 .. N3` | partitions of `N2`, FFT size `2*N2` |
//! | last stage | `N_last ..` | partitions of `N_last` to the end |
//!
//! With the default sizes `[64, 512, 4096]` the head is 64 taps, stage 1 has 7
//! partitions of 64, stage 2 has 7 partitions of 512, and the rest is
//! partitions of 4096. Small partitions keep the cost per sample flat at small
//! host blocks, large partitions keep the number of multiply-adds low for the
//! long tail (Wefers 2015, "Partitioned convolution algorithms for real-time
//! auralization", ch. 4, gives the cost model). A single size (see
//! [`ConvolverConfig::uniform`]) is the uniformly partitioned case.
//!
//! The sizes are powers of two, so every block boundary of a small stage is
//! also a boundary of the larger ones and the whole thing runs on one sample
//! counter. Input and output are handled in spans that never cross a
//! boundary of the smallest stage, so any host block size, 1 included, works.
//!
//! # Honest trade-off: work comes in bursts
//!
//! Everything runs on the audio thread, no helper threads. The multiply-adds
//! of each stage that need only older input are spread evenly over the stage's
//! block (see `Stage::advance`), so they cost the same in every host block. What
//! cannot be spread is the transform of the newest block, the `p = 0` term and
//! the inverse transform: they happen once every `N` samples, so the block
//! that crosses a multiple of 4096 samples runs the transforms of all three
//! stages at once. Averaged over time the cost is small, but the worst block
//! is what a deadline sees. Gardner and Wefers both note this; the remedy in the
//! literature is to compute the large stages in a background thread with a
//! deadline of one block of the next-larger stage, which needs a priority
//! thread and an overrun policy. This module does not do that: it keeps the
//! simple, deterministic, single-thread version. Pick a smaller last size
//! (`[64, 512, 2048]`) if the burst matters more than the average.
//! `tests/long_convolution_bench.rs` prints the mean and worst block time.
//!
//! The transforms are complex FFTs of real data (`rustfft`), so a real-FFT
//! half-size packing would roughly halve the transform cost. That is not done
//! here.
//!
//! # Channels
//!
//! [`Layout::Mono`] is one input, one impulse. [`Layout::Stereo`] is two
//! independent channels, one impulse each (`[left, right]`).
//! [`Layout::TrueStereo`] is a 2x2 matrix of four impulses
//! `[LL, LR, RL, RR]` where `XY` runs from input `X` to output `Y`:
//! `out_L = in_L * LL + in_R * RL`, `out_R = in_L * LR + in_R * RR`.
//! The input spectra are shared between all impulses that read that input, so
//! true stereo costs two forward and two inverse transforms per stage, not
//! four of each.
//!
//! # Loading an impulse off the audio thread
//!
//! The expensive part (one FFT per partition) lives in [`ConvolutionIr`], an
//! immutable, `Send` value built by [`ConvolverConfig::build_ir`]. The running
//! state lives in [`LongConvolver`] and is separate, so a new impulse of the
//! same configuration replaces the old one with no allocation and no break in
//! the input history: the audio thread keeps the spectra of past input and
//! simply multiplies them with the new impulse from the next block on. Hand
//! the impulse over with `openplugin_core::worker`:
//!
//! ```
//! use openplugin_core::worker::loader;
//! use openplugin_dsp::partitioned::{ConvolverConfig, Layout};
//!
//! let config = ConvolverConfig::new(Layout::Mono, 48_000);
//! let mut convolver = config.build().unwrap();
//! // Until the first impulse arrives the audio side uses a unit impulse.
//! let unit = config.build_ir(&[&[1.0]]).unwrap();
//! let (mut loader, mut slot) = loader(unit, 2);
//!
//! // Main thread: the job runs the FFTs on the worker thread.
//! let cfg = config.clone();
//! let taps = vec![0.5f32; 1000];
//! loader.submit(move || cfg.build_ir(&[&taps]).unwrap()).unwrap();
//! loader.wait_idle(std::time::Duration::from_secs(5));
//!
//! // Audio thread, in `process`:
//! slot.poll();
//! let mut block = [1.0f32; 64];
//! convolver.process_mono(slot.get(), &mut block);
//! ```
//!
//! # Real-time properties
//!
//! `build`, `build_ir` and everything else that allocates run at prepare or
//! load time. [`LongConvolver::process`] allocates nothing, does not lock,
//! and cannot panic: wrong channel counts or an impulse built for a different
//! configuration leave the buffers untouched. Non-finite input samples are
//! replaced by zero in the convolver's own copy (a NaN would otherwise stay in
//! the delay line for as long as the impulse is), and values below `1e-30` are
//! flushed to zero in the stage results and the output, so decaying tails never
//! run through the slow denormal path. [`LongConvolver::latency`] is zero.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

type C = Complex<f32>;

/// Most stage sizes a configuration may list (head and partition sizes).
/// Most stages a [`ConvolverConfig`] may have.
pub const MAX_STAGES: usize = 4;
/// Smallest stage size.
/// Smallest stage (partition) size, samples.
pub const MIN_STAGE: usize = 16;
/// Largest stage size.
/// Largest stage (partition) size, samples.
pub const MAX_STAGE: usize = 1 << 16;
/// Stage sizes used by [`ConvolverConfig::new`].
/// The stage sizes of [`ConvolverConfig::new`].
pub const DEFAULT_STAGES: [usize; 3] = [64, 512, 4096];

const TINY: f32 = 1.0e-30;

/// Channel arrangement and how impulses map onto it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Layout {
    /// One input, one output, one impulse.
    Mono,
    /// Two independent channels, impulses `[left, right]`.
    Stereo,
    /// Two inputs, two outputs, impulses `[LL, LR, RL, RR]` (`XY` is from
    /// input `X` to output `Y`).
    TrueStereo,
}

impl Layout {
    /// Number of audio channels in (and out).
    pub const fn channels(self) -> usize {
        match self {
            Layout::Mono => 1,
            _ => 2,
        }
    }

    /// Number of impulses expected by [`ConvolverConfig::build_ir`].
    pub const fn filters(self) -> usize {
        match self {
            Layout::Mono => 1,
            Layout::Stereo => 2,
            Layout::TrueStereo => 4,
        }
    }

    /// `(input, output)` of each impulse, in impulse order.
    fn routes(self) -> &'static [(usize, usize)] {
        match self {
            Layout::Mono => &[(0, 0)],
            Layout::Stereo => &[(0, 0), (1, 1)],
            Layout::TrueStereo => &[(0, 0), (0, 1), (1, 0), (1, 1)],
        }
    }
}

/// Why a configuration or an impulse was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConvolveError {
    /// Stage sizes must be powers of two in `MIN_STAGE..=MAX_STAGE`, strictly
    /// ascending, at most `MAX_STAGES` of them.
    BadStages,
    /// The longest impulse (`max_len`) must be at least 1.
    ZeroLength,
    /// The number of impulses does not match the layout.
    WrongFilterCount {
        /// Impulses the layout takes.
        expected: usize,
        /// Impulses given.
        got: usize,
    },
    /// An impulse is longer than the `max_len` the configuration was built for.
    TooLong {
        /// Length of the impulse, taps.
        len: usize,
        /// The configuration's `max_len`, taps.
        max: usize,
    },
    /// An impulse contains NaN or infinity.
    NotFinite,
}

impl std::fmt::Display for ConvolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConvolveError::BadStages => f.write_str("stage sizes must be ascending powers of two"),
            ConvolveError::ZeroLength => f.write_str("maximum impulse length must be at least 1"),
            ConvolveError::WrongFilterCount { expected, got } => {
                write!(f, "expected {expected} impulses, got {got}")
            }
            ConvolveError::TooLong { len, max } => {
                write!(f, "impulse of {len} taps exceeds the maximum of {max}")
            }
            ConvolveError::NotFinite => f.write_str("impulse contains NaN or infinity"),
        }
    }
}

impl std::error::Error for ConvolveError {}

/// Everything that fixes the memory layout of a convolver and its impulses.
///
/// A [`LongConvolver`] and the [`ConvolutionIr`]s it plays must come from equal
/// configurations.
/// # Example
///
/// ```
/// use openplugin_dsp::{ConvolverConfig, Layout};
///
/// // Prepare time: a convolver and an impulse for it.
/// let config = ConvolverConfig::new(Layout::Mono, 48_000);
/// let mut conv = config.build().unwrap();
/// let mut impulse = vec![0.0f32; 48_000];
/// impulse[0] = 0.5;
/// let ir = config.build_ir(&[&impulse]).unwrap(); // FFT work: a worker thread
///
/// // Audio thread: zero latency, allocation-free.
/// let mut block = [1.0f32, 0.0, 0.0, 0.0];
/// conv.process_mono(&ir, &mut block);
/// assert!((block[0] - 0.5).abs() < 1e-6);
/// assert_eq!(conv.latency(), 0);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvolverConfig {
    layout: Layout,
    stages: Vec<usize>,
    max_len: usize,
}

impl ConvolverConfig {
    /// Non-uniform partitioning with [`DEFAULT_STAGES`] for impulses of up to
    /// `max_len` taps (per impulse).
    pub fn new(layout: Layout, max_len: usize) -> Self {
        Self {
            layout,
            stages: DEFAULT_STAGES.to_vec(),
            max_len,
        }
    }

    /// Uniform partitioning: a direct head of `block` taps, then partitions of
    /// `block` to the end. The head costs `block` multiply-adds per sample, so
    /// keep `block` small (32 to 128) unless latency is acceptable.
    pub fn uniform(layout: Layout, block: usize, max_len: usize) -> Result<Self, ConvolveError> {
        Self::new(layout, max_len).with_stages(&[block])
    }

    /// Replace the stage sizes. The first is the direct head and the first
    /// partition size, the following are the partition sizes of later stages.
    /// Powers of two in `MIN_STAGE..=MAX_STAGE`, strictly ascending, at most
    /// `MAX_STAGES`.
    pub fn with_stages(mut self, stages: &[usize]) -> Result<Self, ConvolveError> {
        let ok = !stages.is_empty()
            && stages.len() <= MAX_STAGES
            && stages
                .iter()
                .all(|s| s.is_power_of_two() && (MIN_STAGE..=MAX_STAGE).contains(s))
            && stages.windows(2).all(|w| w[0] < w[1]);
        if !ok {
            return Err(ConvolveError::BadStages);
        }
        self.stages = stages.to_vec();
        Ok(self)
    }

    /// The channel layout.
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// The stage sizes, ascending.
    pub fn stages(&self) -> &[usize] {
        &self.stages
    }

    /// The longest impulse this configuration accepts, taps.
    pub fn max_len(&self) -> usize {
        self.max_len
    }

    /// Partitions stage `index` holds for an impulse of `len` taps.
    fn parts(&self, index: usize, len: usize) -> usize {
        let n = self.stages[index];
        if len <= n {
            return 0;
        }
        let next = self.stages.get(index + 1).copied().unwrap_or(usize::MAX);
        (len.min(next) - n).div_ceil(n)
    }

    /// Stage indices that carry at least one partition at `max_len`.
    fn active(&self) -> Vec<usize> {
        (0..self.stages.len())
            .filter(|&s| self.parts(s, self.max_len) > 0)
            .collect()
    }

    /// Allocate a convolver for this configuration. Prepare-time only.
    pub fn build(&self) -> Result<LongConvolver, ConvolveError> {
        if self.max_len == 0 {
            return Err(ConvolveError::ZeroLength);
        }
        Ok(LongConvolver::new(self.clone()))
    }

    /// Transform impulses (one per `Layout::filters`) into a [`ConvolutionIr`].
    /// This does all the FFT work and allocates: run it on a worker thread.
    pub fn build_ir(&self, filters: &[&[f32]]) -> Result<ConvolutionIr, ConvolveError> {
        if self.max_len == 0 {
            return Err(ConvolveError::ZeroLength);
        }
        let expected = self.layout.filters();
        if filters.len() != expected {
            return Err(ConvolveError::WrongFilterCount {
                expected,
                got: filters.len(),
            });
        }
        for f in filters {
            if f.len() > self.max_len {
                return Err(ConvolveError::TooLong {
                    len: f.len(),
                    max: self.max_len,
                });
            }
            if f.iter().any(|v| !v.is_finite()) {
                return Err(ConvolveError::NotFinite);
            }
        }
        let head_len = self.stages[0];
        let head = filters
            .iter()
            .map(|f| {
                let mut rev = vec![0.0f32; head_len];
                for (k, tap) in f.iter().take(head_len).enumerate() {
                    rev[head_len - 1 - k] = *tap;
                }
                rev
            })
            .collect();

        let mut planner = FftPlanner::<f32>::new();
        let mut stages = Vec::new();
        for index in self.active() {
            let n = self.stages[index];
            let fft = planner.plan_fft_forward(2 * n);
            let mut scratch = vec![C::default(); fft.get_inplace_scratch_len()];
            let mut work = vec![C::default(); 2 * n];
            let bins = n + 1;
            let scale = 1.0 / (2 * n) as f32;
            let mut routes = Vec::with_capacity(filters.len());
            for f in filters {
                let parts = self.parts(index, f.len());
                let mut data = vec![C::default(); parts * bins];
                for p in 0..parts {
                    let from = n + p * n;
                    let to = (from + n).min(f.len());
                    work.fill(C::default());
                    for (w, tap) in work.iter_mut().zip(&f[from..to]) {
                        w.re = *tap;
                    }
                    fft.process_with_scratch(&mut work, &mut scratch);
                    for (d, w) in data[p * bins..(p + 1) * bins].iter_mut().zip(&work) {
                        *d = *w * scale;
                    }
                }
                routes.push(Route { parts, data });
            }
            stages.push(IrStage { routes });
        }
        Ok(ConvolutionIr {
            config: self.clone(),
            head,
            stages,
        })
    }
}

struct Route {
    parts: usize,
    /// `parts * (n + 1)` bins, scaled by `1 / 2n` so the inverse needs none.
    data: Vec<C>,
}

struct IrStage {
    routes: Vec<Route>,
}

/// Prepared impulses: the head taps and the spectra of every partition.
///
/// Immutable and `Send + Sync`; build it on a worker thread and hand it to
/// the audio thread with `openplugin_core::worker`. Memory is about 16 bytes
/// per tap per impulse.
/// See the [`ConvolverConfig`] example.
pub struct ConvolutionIr {
    config: ConvolverConfig,
    /// Per impulse, the first `N1` taps reversed (oldest sample first).
    head: Vec<Vec<f32>>,
    stages: Vec<IrStage>,
}

impl ConvolutionIr {
    /// The configuration these impulses were built for.
    pub fn config(&self) -> &ConvolverConfig {
        &self.config
    }
}

impl std::fmt::Debug for ConvolutionIr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConvolutionIr")
            .field("config", &self.config)
            .finish()
    }
}

struct Stage {
    n: usize,
    p_max: usize,
    fwd: Arc<dyn Fft<f32>>,
    inv: Arc<dyn Fft<f32>>,
    /// Newest spectrum sits at `head`; per input channel `p_max * (n + 1)` bins.
    fdl: Vec<Vec<C>>,
    head: usize,
    out: Vec<Vec<f32>>,
    tail: Vec<Vec<f32>>,
    /// Per output: the running sum for the next block, `n + 1` bins. During a
    /// block it collects the terms that only need older input (`p >= 1`).
    acc: Vec<Vec<C>>,
    /// Progress of the spread-out accumulation: next `(route, partition)` and
    /// how many units are done in this block.
    cursor: (usize, usize),
    done: usize,
}

/// The running state: input history, delay lines and overlap buffers.
///
/// Create with [`ConvolverConfig::build`]; play impulses from
/// [`ConvolverConfig::build_ir`]. Switching to another impulse of the same
/// configuration is just passing a different `&ConvolutionIr`.
/// Processing is allocation-free and lock-free. See the [`ConvolverConfig`]
/// example.
pub struct LongConvolver {
    config: ConvolverConfig,
    channels: usize,
    head_len: usize,
    stages: Vec<Stage>,
    /// Sanitised input, `n_last` samples per channel, written at `phase`.
    rec: Vec<Vec<f32>>,
    hist: Vec<Vec<f32>>,
    hist_pos: usize,
    phase: usize,
    n_last: usize,
    work: Vec<C>,
    scratch: Vec<C>,
}

impl std::fmt::Debug for LongConvolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LongConvolver")
            .field("config", &self.config)
            .finish()
    }
}

#[inline]
fn flush(x: f32) -> f32 {
    if x.abs() < TINY { 0.0 } else { x }
}

#[inline]
fn sanitize(x: f32) -> f32 {
    // NaN fails the comparison; infinity and absurd values fail it too.
    if x.abs() < 1.0e30 { flush(x) } else { 0.0 }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0.0f32; 8];
    let mut ca = a.chunks_exact(8);
    let mut cb = b.chunks_exact(8);
    for (x, y) in (&mut ca).zip(&mut cb) {
        for k in 0..8 {
            lanes[k] += x[k] * y[k];
        }
    }
    let mut sum: f32 = lanes.iter().sum();
    for (x, y) in ca.remainder().iter().zip(cb.remainder()) {
        sum += x * y;
    }
    sum
}

fn mac(acc: &mut [C], x: &[C], h: &[C]) {
    for ((a, x), h) in acc.iter_mut().zip(x).zip(h) {
        a.re += x.re * h.re - x.im * h.im;
        a.im += x.re * h.im + x.im * h.re;
    }
}

impl Stage {
    /// Spread the accumulation over the block (Wefers 2015, ch. 4: the terms
    /// that need only older input can be done at any time before the block
    /// ends). `t` is how many samples of this stage's block have passed; at
    /// `t == n` everything left is finished. The unit of work is one
    /// partition of one impulse, a multiply-add over `n + 1` bins. Without
    /// this the whole sum would land in the block that completes a
    /// partition, which for seconds of impulse is a large burst.
    fn advance(&mut self, ir: &IrStage, routes: &[(usize, usize)], t: usize) {
        let bins = self.n + 1;
        let total: usize = ir
            .routes
            .iter()
            .map(|r| r.parts.min(self.p_max).saturating_sub(1))
            .sum();
        let target = if t >= self.n {
            usize::MAX
        } else {
            total * t / self.n
        };
        while self.done < target {
            let (r, p) = self.cursor;
            let Some(&(input, output)) = routes.get(r) else {
                break;
            };
            let route = &ir.routes[r];
            if p >= route.parts.min(self.p_max) {
                self.cursor = (r + 1, 1);
                continue;
            }
            // The newest spectrum is X[j-1] at `head`, so X[j-p] is p-1 older.
            let at = self.head + p - 1;
            let slot = if at >= self.p_max {
                at - self.p_max
            } else {
                at
            };
            mac(
                &mut self.acc[output],
                &self.fdl[input][slot * bins..(slot + 1) * bins],
                &route.data[p * bins..(p + 1) * bins],
            );
            self.cursor = (r, p + 1);
            self.done += 1;
        }
    }

    /// One block of the stage is complete: transform it, push it into the
    /// delay line, add the term for the newest block (`p = 0`) and compute the
    /// next output block (Gardner 1995).
    fn update(
        &mut self,
        ir: &IrStage,
        routes: &[(usize, usize)],
        rec: &[Vec<f32>],
        start: usize,
        work: &mut [C],
        scratch: &mut [C],
    ) {
        let n = self.n;
        let bins = n + 1;
        self.advance(ir, routes, n);
        self.head = if self.head == 0 {
            self.p_max - 1
        } else {
            self.head - 1
        };
        let work = &mut work[..2 * n];
        for (input, r) in rec.iter().enumerate() {
            for (w, s) in work[..n].iter_mut().zip(&r[start..start + n]) {
                *w = C::new(*s, 0.0);
            }
            work[n..].fill(C::default());
            self.fwd.process_with_scratch(work, scratch);
            let at = self.head * bins;
            self.fdl[input][at..at + bins].copy_from_slice(&work[..bins]);
        }
        for o in 0..self.out.len() {
            let mut any = false;
            for (r, &(input, output)) in routes.iter().enumerate() {
                let route = &ir.routes[r];
                if output == o && route.parts > 0 {
                    mac(
                        &mut self.acc[o],
                        &self.fdl[input][self.head * bins..(self.head + 1) * bins],
                        &route.data[..bins],
                    );
                    any = true;
                }
            }
            let acc = &mut self.acc[o];
            let (out, tail) = (&mut self.out[o], &mut self.tail[o]);
            if any {
                work[..bins].copy_from_slice(acc);
                work[0].im = 0.0;
                work[n].im = 0.0;
                for k in n + 1..2 * n {
                    work[k] = acc[2 * n - k].conj();
                }
                self.inv.process_with_scratch(work, scratch);
                for k in 0..n {
                    out[k] = flush(work[k].re + tail[k]);
                    tail[k] = flush(work[n + k].re);
                }
            } else {
                for k in 0..n {
                    out[k] = tail[k];
                    tail[k] = 0.0;
                }
            }
            acc.fill(C::default());
        }
        self.cursor = (0, 1);
        self.done = 0;
    }
}

impl LongConvolver {
    fn new(config: ConvolverConfig) -> Self {
        let channels = config.layout.channels();
        let head_len = config.stages[0];
        let mut planner = FftPlanner::<f32>::new();
        let mut scratch_len = 0;
        let mut n_last = head_len;
        let mut stages = Vec::new();
        for index in config.active() {
            let n = config.stages[index];
            let p_max = config.parts(index, config.max_len);
            let fwd = planner.plan_fft_forward(2 * n);
            let inv = planner.plan_fft_inverse(2 * n);
            scratch_len = scratch_len
                .max(fwd.get_inplace_scratch_len())
                .max(inv.get_inplace_scratch_len());
            n_last = n;
            stages.push(Stage {
                n,
                p_max,
                fwd,
                inv,
                fdl: (0..channels)
                    .map(|_| vec![C::default(); p_max * (n + 1)])
                    .collect(),
                head: 0,
                out: (0..channels).map(|_| vec![0.0; n]).collect(),
                tail: (0..channels).map(|_| vec![0.0; n]).collect(),
                acc: (0..channels).map(|_| vec![C::default(); n + 1]).collect(),
                cursor: (0, 1),
                done: 0,
            });
        }
        Self {
            channels,
            head_len,
            stages,
            rec: (0..channels).map(|_| vec![0.0; n_last]).collect(),
            hist: (0..channels).map(|_| vec![0.0; 2 * head_len]).collect(),
            hist_pos: 0,
            phase: 0,
            n_last,
            work: vec![C::default(); 2 * n_last],
            scratch: vec![C::default(); scratch_len],
            config,
        }
    }

    /// The configuration this convolver was built for.
    pub fn config(&self) -> &ConvolverConfig {
        &self.config
    }

    /// Latency added to the signal, in samples. Always 0: report it from
    /// `Plugin::latency`.
    pub const fn latency(&self) -> u32 {
        0
    }

    /// Clear the input history and all overlap state. Does not allocate.
    pub fn reset(&mut self) {
        for r in &mut self.rec {
            r.fill(0.0);
        }
        for h in &mut self.hist {
            h.fill(0.0);
        }
        for s in &mut self.stages {
            s.head = 0;
            for f in &mut s.fdl {
                f.fill(C::default());
            }
            for o in &mut s.out {
                o.fill(0.0);
            }
            for t in &mut s.tail {
                t.fill(0.0);
            }
            for a in &mut s.acc {
                a.fill(C::default());
            }
            s.cursor = (0, 1);
            s.done = 0;
        }
        self.hist_pos = 0;
        self.phase = 0;
    }

    /// Mono in place. Does nothing if the layout is not [`Layout::Mono`].
    pub fn process_mono(&mut self, ir: &ConvolutionIr, samples: &mut [f32]) {
        self.process(ir, &mut [samples]);
    }

    /// Stereo in place (`Stereo` or `TrueStereo`); the shorter channel sets
    /// the frame count.
    pub fn process_stereo(&mut self, ir: &ConvolutionIr, left: &mut [f32], right: &mut [f32]) {
        self.process(ir, &mut [left, right]);
    }

    /// Convolve `buffers` (one slice per channel) in place, any length
    /// including 0 and 1. Allocation-free and panic-free. If the number of
    /// buffers is not the layout's channel count, or `ir` was built for another
    /// configuration, the buffers are left unchanged. Buffers of different
    /// lengths are processed up to the shortest.
    pub fn process(&mut self, ir: &ConvolutionIr, buffers: &mut [&mut [f32]]) {
        if buffers.len() != self.channels || ir.config != self.config {
            return;
        }
        let frames = buffers.iter().map(|b| b.len()).min().unwrap_or(0);
        let h = self.head_len;
        let routes = self.config.layout.routes();
        let mut done = 0;
        while done < frames {
            let ph = self.phase;
            let span = (frames - done).min(h - ph % h);

            for (rec, b) in self.rec.iter_mut().zip(buffers.iter()) {
                for (d, s) in rec[ph..ph + span].iter_mut().zip(&b[done..done + span]) {
                    *d = sanitize(*s);
                }
            }

            // Direct head: taps 0..N1 for every sample.
            for i in 0..span {
                let pos = self.hist_pos;
                let mut y = [0.0f32; 2];
                for (c, hist) in self.hist.iter_mut().enumerate() {
                    let x = self.rec[c][ph + i];
                    hist[pos] = x;
                    hist[pos + h] = x;
                }
                for (r, &(input, output)) in routes.iter().enumerate() {
                    y[output] += dot(&self.hist[input][pos + 1..pos + 1 + h], &ir.head[r]);
                }
                for (b, v) in buffers.iter_mut().zip(y) {
                    b[done + i] = v;
                }
                self.hist_pos = if pos + 1 == h { 0 } else { pos + 1 };
            }

            // Results of the partitioned stages for this span.
            for stage in &self.stages {
                let off = ph % stage.n;
                for (b, out) in buffers.iter_mut().zip(&stage.out) {
                    for (d, s) in b[done..done + span].iter_mut().zip(&out[off..off + span]) {
                        *d += *s;
                    }
                }
            }
            for b in buffers.iter_mut() {
                for d in &mut b[done..done + span] {
                    *d = flush(*d);
                }
            }

            self.phase = (ph + span) % self.n_last;
            done += span;

            // Spread accumulation, then the stages whose block just completed.
            for (stage, ir_stage) in self.stages.iter_mut().zip(&ir.stages) {
                let t = self.phase % stage.n;
                if t != 0 {
                    stage.advance(ir_stage, routes, t);
                } else {
                    let start = (self.phase + self.n_last - stage.n) % self.n_last;
                    stage.update(
                        ir_stage,
                        routes,
                        &self.rec,
                        start,
                        &mut self.work,
                        &mut self.scratch,
                    );
                }
            }
        }
    }
}
