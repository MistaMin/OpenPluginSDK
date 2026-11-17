//! Vectorised kernels with run-time selected paths.
//!
//! Hot loops of audio code (gain and mix, frequency-domain multiply-accumulate,
//! FIR, biquad cascades, matrix-vector products) in three flavours: a scalar
//! reference, aarch64 NEON, and x86_64 SSE2 or AVX with FMA. [`Kernels::detect`]
//! picks the best path once (`is_x86_feature_detected!` on x86_64; NEON is part
//! of aarch64) and stores function pointers; calling a kernel afterwards is one
//! indirect call. Nothing allocates, no kernel touches global state, and every
//! kernel takes unaligned slices of any length.
//!
//! # Equality with the scalar reference
//!
//! Every kernel has a scalar twin in [`scalar`]. The scalar twin defines the
//! operation order, and the vector paths follow it:
//!
//! - Exactly equal on every path, bit for bit (no fused multiply-add is used):
//!   [`Kernels::gain`], [`Kernels::scale`], [`Kernels::mix_add`],
//!   [`Kernels::crossfade`] and [`BiquadBank4`].
//! - Exactly equal on SSE2 and on the scalar path, and equal up to the last
//!   bit on [`Isa::Neon`] and [`Isa::AvxFma`], which use fused multiply-add
//!   (one rounding instead of two): [`Kernels::cmac_interleaved`],
//!   [`Kernels::cmac_split`], [`Kernels::fir`], [`Kernels::dot`],
//!   [`Kernels::matvec`]. The bound is `n * f32::EPSILON * sum(|terms|)` for a
//!   sum of `n` terms (the tests use it); in practice the difference is a few
//!   units in the last place.
//! - `dot` and `matvec` sum in a fixed order that does not depend on the path:
//!   sixteen logical lanes (`element i` goes to lane `i % 16`), then lane `i`
//!   plus lane `i + 8`, then `+ 4`, then `(s0 + s2) + (s1 + s3)`, then the
//!   leftover `len % 16` elements (after one extra step of eight if at least
//!   eight are left) one at a time. This is why vector and scalar agree.
//! - NaN and infinity propagate as in the scalar code (the vector paths do the
//!   same multiplications and additions; there is no `min`, `max` or compare).
//!   With FMA an overflow in `a * b` that the scalar code would round to
//!   infinity and then cancel can differ; that needs values above 1e19.
//! - Denormals: the kernels do nothing special (no flush); [`BiquadBank4`] does
//!   not flush its state like [`crate::Biquad`] does.
//!
//! Lengths: kernels that take several slices assert they match (or, for
//! [`Kernels::fir`] and [`Kernels::matvec`], are large enough) and panic
//! otherwise. [`Kernels::crossfade`] counts its ramp in `f32` so its block may
//! not exceed 2^24 samples.
//!
//! # Layouts
//!
//! - Complex data is interleaved `[re0, im0, re1, im1, ..]` (the memory layout
//!   of `rustfft`'s `Complex<f32>`, which `partitioned.rs` uses) or split
//!   (separate `re` and `im` slices). [`Kernels::cmac_complex`] takes
//!   `Complex<f32>` slices directly.
//! - FIR: the taps are stored **reversed** (`taps_rev[k] = h[T - 1 - k]`) so that
//!   output `n` is the dot product of `taps_rev` with `x[n .. n + T]`. `x` is
//!   `T - 1` samples of history followed by the block. See [`Kernels::fir`].
//! - Matrices are row-major: `w[row * cols + col]`.
//! - [`BiquadBank4`]: four channels of one cascade in the four lanes of a
//!   register ("SIMD across channels"). The audio is interleaved by frame
//!   (`[c0 c1 c2 c3 | c0 c1 c2 c3 | ..]`), coefficients are shared by the four
//!   channels, and the state of section `s` is stored as eight floats
//!   `[z1 of c0..c3, z2 of c0..c3]`. Transposed direct form II.
//!
//! # Use
//!
//! ```
//! use openplugin_dsp::simd::Kernels;
//! let k = Kernels::detect();
//! let x = [1.0_f32, 2.0, 3.0];
//! let mut y = [0.0_f32; 3];
//! k.gain(&mut y, &x, 0.5);
//! k.mix_add(&mut y, &x, 1.0);
//! assert_eq!(y, [1.5, 3.0, 4.5]);
//! ```

use rustfft::num_complex::Complex;
use std::sync::OnceLock;

/// The instruction set a [`Kernels`] table runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Isa {
    /// Plain Rust, the reference.
    Scalar,
    /// aarch64 NEON (4 lanes, fused multiply-add).
    Neon,
    /// x86_64 SSE2 (4 lanes, no FMA; the x86_64 baseline).
    Sse2,
    /// x86_64 AVX with FMA (8 lanes).
    AvxFma,
}

impl Isa {
    /// Whether this path uses fused multiply-add (so results may differ from
    /// the scalar reference in the last bit; see the module docs).
    pub fn fused(self) -> bool {
        matches!(self, Isa::Neon | Isa::AvxFma)
    }
    /// A short name for reports.
    pub fn name(self) -> &'static str {
        match self {
            Isa::Scalar => "scalar",
            Isa::Neon => "neon",
            Isa::Sse2 => "sse2",
            Isa::AvxFma => "avx+fma",
        }
    }
}

type GainFn = fn(&mut [f32], &[f32], f32);
type CrossFn = fn(&mut [f32], &[f32], &[f32], f32, f32);
type CmacIFn = fn(&mut [f32], &[f32], &[f32]);
type CmacSFn = fn(&mut [f32], &mut [f32], &[f32], &[f32], &[f32], &[f32]);
type FirFn = fn(&mut [f32], &[f32], &[f32]);
type DotFn = fn(&[f32], &[f32]) -> f32;
type MatFn = fn(&mut [f32], &[f32], &[f32], Option<&[f32]>);
type BiqFn = fn(&[[f32; 5]], &mut [f32], &mut [f32]);

/// A table of kernels for one instruction set. `Copy`; pass it by value.
#[derive(Clone, Copy)]
pub struct Kernels {
    isa: Isa,
    gain: GainFn,
    mix_add: GainFn,
    crossfade: CrossFn,
    cmac_i: CmacIFn,
    cmac_s: CmacSFn,
    fir: FirFn,
    dot: DotFn,
    matvec: MatFn,
    biquad4: BiqFn,
}

impl std::fmt::Debug for Kernels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Kernels({})", self.isa.name())
    }
}

static BEST: OnceLock<Kernels> = OnceLock::new();

impl Kernels {
    /// The best path this CPU supports, chosen on first use and kept. Cheap
    /// afterwards (one atomic load); call it in `prepare` and keep the value.
    pub fn detect() -> Kernels {
        *BEST.get_or_init(|| {
            for isa in [Isa::AvxFma, Isa::Neon, Isa::Sse2] {
                if let Some(k) = Kernels::for_isa(isa) {
                    return k;
                }
            }
            Kernels::scalar()
        })
    }

    /// The scalar reference table.
    pub fn scalar() -> Kernels {
        Kernels {
            isa: Isa::Scalar,
            gain: scalar::gain,
            mix_add: scalar::mix_add,
            crossfade: |d, a, b, g0, s| scalar::crossfade(d, a, b, g0, s),
            cmac_i: scalar::cmac_interleaved,
            cmac_s: scalar::cmac_split,
            fir: scalar::fir,
            dot: scalar::dot,
            matvec: scalar::matvec,
            biquad4: scalar::biquad4,
        }
    }

    /// The table for `isa`, or `None` if this build or CPU cannot run it. For
    /// tests and benchmarks; use [`Kernels::detect`] in plugins.
    pub fn for_isa(isa: Isa) -> Option<Kernels> {
        match isa {
            Isa::Scalar => Some(Kernels::scalar()),
            #[cfg(target_arch = "aarch64")]
            Isa::Neon => Some(neon::table()),
            #[cfg(target_arch = "x86_64")]
            Isa::Sse2 => Some(sse2::table()),
            #[cfg(target_arch = "x86_64")]
            Isa::AvxFma => {
                if std::arch::is_x86_feature_detected!("avx")
                    && std::arch::is_x86_feature_detected!("fma")
                {
                    Some(avx::table())
                } else {
                    None
                }
            }
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    /// Every path this build and CPU can run, scalar first.
    pub fn available() -> Vec<Kernels> {
        [Isa::Scalar, Isa::Neon, Isa::Sse2, Isa::AvxFma]
            .into_iter()
            .filter_map(Kernels::for_isa)
            .collect()
    }

    /// Which instruction set this table uses.
    pub fn isa(&self) -> Isa {
        self.isa
    }

    /// `dst[i] = a * src[i]`. Panics if the lengths differ.
    pub fn gain(&self, dst: &mut [f32], src: &[f32], a: f32) {
        assert_eq!(dst.len(), src.len());
        (self.gain)(dst, src, a)
    }

    /// `buf[i] *= a` in place.
    pub fn scale(&self, buf: &mut [f32], a: f32) {
        // Two Rust references may not alias, so go through a stack copy.
        let mut tmp = [0.0_f32; 256];
        for chunk in buf.chunks_mut(256) {
            let n = chunk.len();
            tmp[..n].copy_from_slice(chunk);
            (self.gain)(chunk, &tmp[..n], a);
        }
    }

    /// `dst[i] += a * src[i]`. Panics if the lengths differ.
    pub fn mix_add(&self, dst: &mut [f32], src: &[f32], a: f32) {
        assert_eq!(dst.len(), src.len());
        (self.mix_add)(dst, src, a)
    }

    /// Crossfade two buffers with a linear ramp:
    /// `g = g0 + step * i; dst[i] = a[i] * (1 - g) + b[i] * g`. `g` is computed
    /// from the index (not accumulated) so every path gives the same value.
    /// Panics if the lengths differ or exceed 2^24.
    pub fn crossfade(&self, dst: &mut [f32], a: &[f32], b: &[f32], g0: f32, step: f32) {
        assert_eq!(dst.len(), a.len());
        assert_eq!(dst.len(), b.len());
        assert!(dst.len() <= 1 << 24);
        (self.crossfade)(dst, a, b, g0, step)
    }

    /// Complex multiply-accumulate on interleaved data:
    /// `acc[k] += x[k] * h[k]` for `k < len / 2`. All three slices have the
    /// same even length. Panics otherwise.
    pub fn cmac_interleaved(&self, acc: &mut [f32], x: &[f32], h: &[f32]) {
        assert_eq!(acc.len(), x.len());
        assert_eq!(acc.len(), h.len());
        assert!(acc.len() % 2 == 0);
        (self.cmac_i)(acc, x, h)
    }

    /// The same on split data. All six slices have the same length.
    pub fn cmac_split(
        &self,
        acc_re: &mut [f32],
        acc_im: &mut [f32],
        x_re: &[f32],
        x_im: &[f32],
        h_re: &[f32],
        h_im: &[f32],
    ) {
        let n = acc_re.len();
        assert!(acc_im.len() == n && x_re.len() == n && x_im.len() == n);
        assert!(h_re.len() == n && h_im.len() == n);
        (self.cmac_s)(acc_re, acc_im, x_re, x_im, h_re, h_im)
    }

    /// [`Kernels::cmac_interleaved`] on `Complex<f32>` slices, the type
    /// `partitioned.rs` uses.
    pub fn cmac_complex(&self, acc: &mut [Complex<f32>], x: &[Complex<f32>], h: &[Complex<f32>]) {
        assert_eq!(acc.len(), x.len());
        assert_eq!(acc.len(), h.len());
        // SAFETY: `Complex<f32>` is `#[repr(C)]` with two `f32` fields, so a
        // slice of n values is 2n contiguous, equally aligned `f32`s.
        let (a, x, h) = unsafe {
            (
                std::slice::from_raw_parts_mut(acc.as_mut_ptr().cast::<f32>(), acc.len() * 2),
                std::slice::from_raw_parts(x.as_ptr().cast::<f32>(), x.len() * 2),
                std::slice::from_raw_parts(h.as_ptr().cast::<f32>(), h.len() * 2),
            )
        };
        (self.cmac_i)(a, x, h)
    }

    /// FIR over a history buffer: `out[n] = sum_k taps_rev[k] * x[n + k]` for
    /// `n < out.len()`, summed in increasing `k` (so it equals the scalar
    /// reference exactly on SSE2). `taps_rev` is the impulse response
    /// reversed; `x` holds `taps_rev.len() - 1` samples of history followed by
    /// the new block, so `x.len() >= out.len() + taps_rev.len() - 1`. Any tap
    /// count. Panics if `x` is too short; with no taps the output is zero.
    pub fn fir(&self, out: &mut [f32], x: &[f32], taps_rev: &[f32]) {
        if taps_rev.is_empty() {
            out.fill(0.0);
            return;
        }
        assert!(x.len() >= out.len() + taps_rev.len() - 1);
        (self.fir)(out, x, taps_rev)
    }

    /// Dot product in the fixed order described in the module docs. Panics if
    /// the lengths differ.
    pub fn dot(&self, a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len());
        (self.dot)(a, b)
    }

    /// `y = W x (+ bias)`, row-major `W` of `y.len()` rows and `x.len()`
    /// columns. Each element is [`Kernels::dot`] of a row with `x`, plus the
    /// bias. Panics if `w` or `bias` is too short.
    pub fn matvec(&self, y: &mut [f32], w: &[f32], x: &[f32], bias: Option<&[f32]>) {
        assert!(w.len() >= y.len() * x.len());
        if let Some(b) = bias {
            assert!(b.len() >= y.len());
        }
        (self.matvec)(y, w, x, bias)
    }
}

/// A cascade of biquads over four channels at once (see the module docs for
/// the layout). Coefficients are shared by the four channels; each section is
/// `[b0, b1, b2, a1, a2]` with `a0 = 1`. Built off the audio thread (it
/// allocates); processing does not allocate.
#[derive(Clone, Debug)]
pub struct BiquadBank4 {
    kernels: Kernels,
    coeffs: Vec<[f32; 5]>,
    state: Vec<f32>,
}

impl BiquadBank4 {
    /// A cascade with these sections, using [`Kernels::detect`].
    pub fn new(sections: &[[f32; 5]]) -> Self {
        Self::with_kernels(sections, Kernels::detect())
    }

    /// The same on a chosen path (for tests and benchmarks).
    pub fn with_kernels(sections: &[[f32; 5]], kernels: Kernels) -> Self {
        Self {
            kernels,
            coeffs: sections.to_vec(),
            state: vec![0.0; sections.len() * 8],
        }
    }

    /// Number of sections.
    pub fn sections(&self) -> usize {
        self.coeffs.len()
    }

    /// Replace the coefficients of one section (no allocation, state kept).
    /// An `index` past the last section does nothing.
    pub fn set_section(&mut self, index: usize, c: [f32; 5]) {
        if let Some(section) = self.coeffs.get_mut(index) {
            *section = c;
        }
    }

    /// Clear the state.
    pub fn reset(&mut self) {
        self.state.fill(0.0);
    }

    /// Process frames of four interleaved channels in place. `io.len()` must
    /// be a multiple of 4.
    pub fn process_interleaved(&mut self, io: &mut [f32]) {
        assert!(io.len() % 4 == 0);
        (self.kernels.biquad4)(&self.coeffs, &mut self.state, io);
    }

    /// Process four planar channels in place (all the same length). Transposes
    /// through a small stack buffer, so it costs a little more than
    /// [`BiquadBank4::process_interleaved`].
    pub fn process(&mut self, io: [&mut [f32]; 4]) {
        let [c0, c1, c2, c3] = io;
        let n = c0.len();
        assert!(c1.len() == n && c2.len() == n && c3.len() == n);
        let mut buf = [0.0_f32; 256];
        let mut start = 0;
        while start < n {
            let m = (n - start).min(64);
            for i in 0..m {
                buf[4 * i] = c0[start + i];
                buf[4 * i + 1] = c1[start + i];
                buf[4 * i + 2] = c2[start + i];
                buf[4 * i + 3] = c3[start + i];
            }
            (self.kernels.biquad4)(&self.coeffs, &mut self.state, &mut buf[..4 * m]);
            for i in 0..m {
                c0[start + i] = buf[4 * i];
                c1[start + i] = buf[4 * i + 1];
                c2[start + i] = buf[4 * i + 2];
                c3[start + i] = buf[4 * i + 3];
            }
            start += m;
        }
    }
}

/// Scalar reference kernels. They define the operation order the vector paths
/// follow, and are what [`Isa::Scalar`] runs.
pub mod scalar {
    /// `dst = a * src`.
    pub fn gain(dst: &mut [f32], src: &[f32], a: f32) {
        for (d, s) in dst.iter_mut().zip(src) {
            *d = a * *s;
        }
    }

    /// `dst += a * src`.
    pub fn mix_add(dst: &mut [f32], src: &[f32], a: f32) {
        for (d, s) in dst.iter_mut().zip(src) {
            *d += a * *s;
        }
    }

    /// Linear crossfade, ramp counted from index 0.
    pub fn crossfade(dst: &mut [f32], a: &[f32], b: &[f32], g0: f32, step: f32) {
        crossfade_from(dst, a, b, g0, step, 0);
    }

    /// As [`crossfade`] with the ramp index starting at `first` (used for the
    /// tail of a vector loop).
    pub fn crossfade_from(dst: &mut [f32], a: &[f32], b: &[f32], g0: f32, step: f32, first: usize) {
        for (i, ((d, a), b)) in dst.iter_mut().zip(a).zip(b).enumerate() {
            let g = g0 + step * ((first + i) as f32);
            *d = *a * (1.0 - g) + *b * g;
        }
    }

    /// Interleaved complex multiply-accumulate.
    pub fn cmac_interleaved(acc: &mut [f32], x: &[f32], h: &[f32]) {
        for ((a, x), h) in acc
            .chunks_exact_mut(2)
            .zip(x.chunks_exact(2))
            .zip(h.chunks_exact(2))
        {
            a[0] += x[0] * h[0] - x[1] * h[1];
            a[1] += x[0] * h[1] + x[1] * h[0];
        }
    }

    /// Split complex multiply-accumulate.
    pub fn cmac_split(
        acc_re: &mut [f32],
        acc_im: &mut [f32],
        x_re: &[f32],
        x_im: &[f32],
        h_re: &[f32],
        h_im: &[f32],
    ) {
        for i in 0..acc_re.len() {
            acc_re[i] += x_re[i] * h_re[i] - x_im[i] * h_im[i];
            acc_im[i] += x_re[i] * h_im[i] + x_im[i] * h_re[i];
        }
    }

    /// FIR with reversed taps over a history buffer.
    pub fn fir(out: &mut [f32], x: &[f32], taps_rev: &[f32]) {
        for (n, o) in out.iter_mut().enumerate() {
            let mut acc = 0.0_f32;
            for (k, h) in taps_rev.iter().enumerate() {
                acc += *h * x[n + k];
            }
            *o = acc;
        }
    }

    /// Dot product in the fixed sixteen-lane order of the module docs.
    pub fn dot(a: &[f32], b: &[f32]) -> f32 {
        let n = a.len();
        let mut l = [0.0_f32; 16];
        let mut i = 0;
        while i + 16 <= n {
            for j in 0..16 {
                l[j] += a[i + j] * b[i + j];
            }
            i += 16;
        }
        if i + 8 <= n {
            for j in 0..8 {
                l[j] += a[i + j] * b[i + j];
            }
            i += 8;
        }
        let mut s8 = [0.0_f32; 8];
        for j in 0..8 {
            s8[j] = l[j] + l[j + 8];
        }
        let s4 = [s8[0] + s8[4], s8[1] + s8[5], s8[2] + s8[6], s8[3] + s8[7]];
        let mut sum = (s4[0] + s4[2]) + (s4[1] + s4[3]);
        while i < n {
            sum += a[i] * b[i];
            i += 1;
        }
        sum
    }

    /// `y = W x (+ bias)`.
    pub fn matvec(y: &mut [f32], w: &[f32], x: &[f32], bias: Option<&[f32]>) {
        let cols = x.len();
        for (r, o) in y.iter_mut().enumerate() {
            let mut v = dot(&w[r * cols..r * cols + cols], x);
            if let Some(b) = bias {
                v += b[r];
            }
            *o = v;
        }
    }

    /// Four-channel biquad cascade (interleaved audio, transposed direct form
    /// II; see the module docs for the state layout).
    pub fn biquad4(coeffs: &[[f32; 5]], z: &mut [f32], io: &mut [f32]) {
        for frame in io.chunks_exact_mut(4) {
            for (c, v) in frame.iter_mut().enumerate() {
                let mut x = *v;
                for (s, k) in coeffs.iter().enumerate() {
                    let (z1, z2) = (z[s * 8 + c], z[s * 8 + 4 + c]);
                    let y = k[0] * x + z1;
                    z[s * 8 + c] = (k[1] * x + z2) - k[3] * y;
                    z[s * 8 + 4 + c] = k[2] * x - k[4] * y;
                    x = y;
                }
                *v = x;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Generic kernels over a vector type. Each is `#[inline(always)]` so that it
// is compiled inside the `#[target_feature]` entry point that instantiates it.
// ---------------------------------------------------------------------------

/// The few vector operations the kernels need. Implemented per instruction
/// set below. Every method is `unsafe` because it is only sound to call where
/// the instruction set is available, and the loads and stores take raw
/// pointers that must be valid for `LANES` (or `2 * LANES`) floats.
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
trait Vf: Copy {
    /// Floats per vector: 4 or 8.
    const LANES: usize;
    unsafe fn splat(x: f32) -> Self;
    unsafe fn load(p: *const f32) -> Self;
    unsafe fn store(p: *mut f32, v: Self);
    unsafe fn add(a: Self, b: Self) -> Self;
    unsafe fn sub(a: Self, b: Self) -> Self;
    unsafe fn mul(a: Self, b: Self) -> Self;
    /// `a * b + c`, fused where the instruction set has it.
    unsafe fn fma(a: Self, b: Self, c: Self) -> Self;
    /// `c - a * b`, fused where the instruction set has it.
    unsafe fn fnma(c: Self, a: Self, b: Self) -> Self;
    /// Read `LANES` interleaved pairs from `2 * LANES` floats.
    unsafe fn load2(p: *const f32) -> (Self, Self);
    /// Write `LANES` interleaved pairs.
    unsafe fn store2(p: *mut f32, re: Self, im: Self);
    /// Sum four accumulators holding sixteen logical lanes in the order of the
    /// module docs (without the tail).
    unsafe fn reduce16(acc: &[Self; 4]) -> f32;
}

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
mod generic {
    use super::{Vf, scalar};

    const IOTA: [f32; 8] = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];

    #[inline(always)]
    pub(super) unsafe fn gain<V: Vf>(dst: &mut [f32], src: &[f32], a: f32) {
        let n = dst.len();
        let (d, s) = (dst.as_mut_ptr(), src.as_ptr());
        let mut i = 0;
        // SAFETY: i + LANES <= n and both slices have n elements.
        unsafe {
            let va = V::splat(a);
            while i + 4 * V::LANES <= n {
                for m in 0..4 {
                    let o = i + m * V::LANES;
                    V::store(d.add(o), V::mul(va, V::load(s.add(o))));
                }
                i += 4 * V::LANES;
            }
            while i + V::LANES <= n {
                V::store(d.add(i), V::mul(va, V::load(s.add(i))));
                i += V::LANES;
            }
        }
        scalar::gain(&mut dst[i..], &src[i..], a);
    }

    #[inline(always)]
    pub(super) unsafe fn mix_add<V: Vf>(dst: &mut [f32], src: &[f32], a: f32) {
        let n = dst.len();
        let (d, s) = (dst.as_mut_ptr(), src.as_ptr());
        let mut i = 0;
        // SAFETY: i + LANES <= n and both slices have n elements.
        unsafe {
            let va = V::splat(a);
            while i + 4 * V::LANES <= n {
                for m in 0..4 {
                    let o = i + m * V::LANES;
                    let y = V::add(V::load(d.add(o)), V::mul(va, V::load(s.add(o))));
                    V::store(d.add(o), y);
                }
                i += 4 * V::LANES;
            }
            while i + V::LANES <= n {
                let y = V::add(V::load(d.add(i)), V::mul(va, V::load(s.add(i))));
                V::store(d.add(i), y);
                i += V::LANES;
            }
        }
        scalar::mix_add(&mut dst[i..], &src[i..], a);
    }

    #[inline(always)]
    pub(super) unsafe fn crossfade<V: Vf>(
        dst: &mut [f32],
        a: &[f32],
        b: &[f32],
        g0: f32,
        step: f32,
    ) {
        let n = dst.len();
        let (d, pa, pb) = (dst.as_mut_ptr(), a.as_ptr(), b.as_ptr());
        let mut i = 0;
        // SAFETY: i + LANES <= n; IOTA has 8 >= LANES floats; slices have n elements.
        unsafe {
            let (vg0, vstep, one) = (V::splat(g0), V::splat(step), V::splat(1.0));
            let iota = V::load(IOTA.as_ptr());
            while i + V::LANES <= n {
                let idx = V::add(V::splat(i as f32), iota);
                let g = V::add(vg0, V::mul(vstep, idx));
                let y = V::add(
                    V::mul(V::load(pa.add(i)), V::sub(one, g)),
                    V::mul(V::load(pb.add(i)), g),
                );
                V::store(d.add(i), y);
                i += V::LANES;
            }
        }
        scalar::crossfade_from(&mut dst[i..], &a[i..], &b[i..], g0, step, i);
    }

    #[inline(always)]
    pub(super) unsafe fn cmac_interleaved<V: Vf>(acc: &mut [f32], x: &[f32], h: &[f32]) {
        let n = acc.len(); // floats
        let (pa, px, ph) = (acc.as_mut_ptr(), x.as_ptr(), h.as_ptr());
        let mut i = 0;
        // SAFETY: each step touches 2 * LANES floats starting at i, and i + 2 * LANES <= n.
        unsafe {
            while i + 4 * V::LANES <= n {
                for m in 0..2 {
                    let o = i + m * 2 * V::LANES;
                    let (xr, xi) = V::load2(px.add(o));
                    let (hr, hi) = V::load2(ph.add(o));
                    let (ar, ai) = V::load2(pa.add(o));
                    let re = V::add(ar, V::fnma(V::mul(xr, hr), xi, hi));
                    let im = V::add(ai, V::fma(xi, hr, V::mul(xr, hi)));
                    V::store2(pa.add(o), re, im);
                }
                i += 4 * V::LANES;
            }
            while i + 2 * V::LANES <= n {
                let (xr, xi) = V::load2(px.add(i));
                let (hr, hi) = V::load2(ph.add(i));
                let (ar, ai) = V::load2(pa.add(i));
                let re = V::add(ar, V::fnma(V::mul(xr, hr), xi, hi));
                let im = V::add(ai, V::fma(xi, hr, V::mul(xr, hi)));
                V::store2(pa.add(i), re, im);
                i += 2 * V::LANES;
            }
        }
        scalar::cmac_interleaved(&mut acc[i..], &x[i..], &h[i..]);
    }

    #[inline(always)]
    pub(super) unsafe fn cmac_split<V: Vf>(
        acc_re: &mut [f32],
        acc_im: &mut [f32],
        x_re: &[f32],
        x_im: &[f32],
        h_re: &[f32],
        h_im: &[f32],
    ) {
        let n = acc_re.len();
        let (pr, pi) = (acc_re.as_mut_ptr(), acc_im.as_mut_ptr());
        let (xr, xi, hr, hi) = (x_re.as_ptr(), x_im.as_ptr(), h_re.as_ptr(), h_im.as_ptr());
        let mut i = 0;
        // SAFETY: i + LANES <= n and all six slices have n elements.
        unsafe {
            while i + V::LANES <= n {
                let (a, b) = (V::load(xr.add(i)), V::load(xi.add(i)));
                let (c, d) = (V::load(hr.add(i)), V::load(hi.add(i)));
                let re = V::add(V::load(pr.add(i)), V::fnma(V::mul(a, c), b, d));
                let im = V::add(V::load(pi.add(i)), V::fma(b, c, V::mul(a, d)));
                V::store(pr.add(i), re);
                V::store(pi.add(i), im);
                i += V::LANES;
            }
        }
        scalar::cmac_split(
            &mut acc_re[i..],
            &mut acc_im[i..],
            &x_re[i..],
            &x_im[i..],
            &h_re[i..],
            &h_im[i..],
        );
    }

    #[inline(always)]
    pub(super) unsafe fn fir<V: Vf>(out: &mut [f32], x: &[f32], taps: &[f32]) {
        let n = out.len();
        let t = taps.len();
        let (po, px, ph) = (out.as_mut_ptr(), x.as_ptr(), taps.as_ptr());
        let l = V::LANES;
        let mut i = 0;
        // SAFETY: the caller guarantees x.len() >= n + t - 1, so for output
        // index i + j (< n) and tap k (< t) the load at i + k + m*l reads
        // elements up to i + k + 4*l - 1 <= n - 1 + t - 1.
        unsafe {
            while i + 4 * l <= n {
                let z = V::splat(0.0);
                let mut a = [z; 4];
                for k in 0..t {
                    let hk = V::splat(*ph.add(k));
                    for (m, am) in a.iter_mut().enumerate() {
                        *am = V::fma(V::load(px.add(i + k + m * l)), hk, *am);
                    }
                }
                for (m, am) in a.iter().enumerate() {
                    V::store(po.add(i + m * l), *am);
                }
                i += 4 * l;
            }
            while i + l <= n {
                let mut a = V::splat(0.0);
                for k in 0..t {
                    a = V::fma(V::load(px.add(i + k)), V::splat(*ph.add(k)), a);
                }
                V::store(po.add(i), a);
                i += l;
            }
        }
        // Scalar tail, same order of additions.
        while i < n {
            let mut acc = 0.0_f32;
            for (k, h) in taps.iter().enumerate() {
                acc += *h * x[i + k];
            }
            out[i] = acc;
            i += 1;
        }
    }

    #[inline(always)]
    pub(super) unsafe fn dot<V: Vf>(a: &[f32], b: &[f32]) -> f32 {
        let n = a.len();
        let (pa, pb) = (a.as_ptr(), b.as_ptr());
        let l = V::LANES;
        let regs = 16 / l;
        let mut i = 0;
        // SAFETY: every load reads inside [i, i + 16) or [i, i + 8) with i + 16 <= n (resp. 8).
        let mut sum = unsafe {
            let mut acc = [V::splat(0.0); 4];
            while i + 16 <= n {
                for (j, aj) in acc.iter_mut().enumerate().take(regs) {
                    *aj = V::fma(V::load(pa.add(i + j * l)), V::load(pb.add(i + j * l)), *aj);
                }
                i += 16;
            }
            if i + 8 <= n {
                for (j, aj) in acc.iter_mut().enumerate().take(regs / 2) {
                    *aj = V::fma(V::load(pa.add(i + j * l)), V::load(pb.add(i + j * l)), *aj);
                }
                i += 8;
            }
            V::reduce16(&acc)
        };
        while i < n {
            sum += a[i] * b[i];
            i += 1;
        }
        sum
    }

    #[inline(always)]
    pub(super) unsafe fn matvec<V: Vf>(y: &mut [f32], w: &[f32], x: &[f32], bias: Option<&[f32]>) {
        let cols = x.len();
        for (r, o) in y.iter_mut().enumerate() {
            // SAFETY: the instruction set is available (caller contract).
            let mut v = unsafe { dot::<V>(&w[r * cols..r * cols + cols], x) };
            if let Some(b) = bias {
                v += b[r];
            }
            *o = v;
        }
    }

    /// Four channels, one per lane. `V` must have four lanes. Up to four
    /// sections keep coefficients and state in registers for the whole block
    /// (the state recurrence is the latency-critical chain, and a trip through
    /// memory per sample costs more than the arithmetic).
    #[inline(always)]
    pub(super) unsafe fn biquad4<V: Vf>(coeffs: &[[f32; 5]], z: &mut [f32], io: &mut [f32]) {
        debug_assert!(V::LANES == 4);
        assert!(z.len() == 8 * coeffs.len());
        // SAFETY: the instruction set is available (caller contract) and the
        // length invariant was just checked.
        unsafe {
            match coeffs.len() {
                1 => biquad4_regs::<V, 1>(coeffs, z, io),
                2 => biquad4_regs::<V, 2>(coeffs, z, io),
                3 => biquad4_regs::<V, 3>(coeffs, z, io),
                4 => biquad4_regs::<V, 4>(coeffs, z, io),
                _ => biquad4_mem::<V>(coeffs, z, io),
            }
        }
    }

    /// Transposed direct form II step on one section; the order of operations
    /// is the one in `scalar::biquad4`.
    #[inline(always)]
    unsafe fn step<V: Vf>(k: &[V; 5], z1: &mut V, z2: &mut V, x: V) -> V {
        // SAFETY: only arithmetic on registers.
        unsafe {
            let y = V::add(V::mul(k[0], x), *z1);
            let n1 = V::sub(V::add(V::mul(k[1], x), *z2), V::mul(k[3], y));
            let n2 = V::sub(V::mul(k[2], x), V::mul(k[4], y));
            *z1 = n1;
            *z2 = n2;
            y
        }
    }

    #[inline(always)]
    unsafe fn biquad4_regs<V: Vf, const S: usize>(
        coeffs: &[[f32; 5]],
        z: &mut [f32],
        io: &mut [f32],
    ) {
        let (pz, p) = (z.as_mut_ptr(), io.as_mut_ptr());
        let frames = io.len() / 4;
        // SAFETY: z has 8 * S floats (checked by the caller); frame f touches
        // io[4f..4f + 4] and f < io.len() / 4.
        unsafe {
            let zero = V::splat(0.0);
            let mut k = [[zero; 5]; S];
            let (mut z1, mut z2) = ([zero; S], [zero; S]);
            for s in 0..S {
                for (j, kj) in k[s].iter_mut().enumerate() {
                    *kj = V::splat(coeffs[s][j]);
                }
                z1[s] = V::load(pz.add(8 * s));
                z2[s] = V::load(pz.add(8 * s + 4));
            }
            for f in 0..frames {
                let mut x = V::load(p.add(4 * f));
                for s in 0..S {
                    x = step(&k[s], &mut z1[s], &mut z2[s], x);
                }
                V::store(p.add(4 * f), x);
            }
            for s in 0..S {
                V::store(pz.add(8 * s), z1[s]);
                V::store(pz.add(8 * s + 4), z2[s]);
            }
        }
    }

    #[inline(always)]
    unsafe fn biquad4_mem<V: Vf>(coeffs: &[[f32; 5]], z: &mut [f32], io: &mut [f32]) {
        let (pz, p) = (z.as_mut_ptr(), io.as_mut_ptr());
        let frames = io.len() / 4;
        for f in 0..frames {
            // SAFETY: frame f touches io[4f..4f + 4]; section s owns
            // z[8s..8s + 8] and z.len() == 8 * coeffs.len().
            unsafe {
                let mut x = V::load(p.add(4 * f));
                for (s, c) in coeffs.iter().enumerate() {
                    let k = [
                        V::splat(c[0]),
                        V::splat(c[1]),
                        V::splat(c[2]),
                        V::splat(c[3]),
                        V::splat(c[4]),
                    ];
                    let (mut z1, mut z2) = (V::load(pz.add(8 * s)), V::load(pz.add(8 * s + 4)));
                    x = step(&k, &mut z1, &mut z2, x);
                    V::store(pz.add(8 * s), z1);
                    V::store(pz.add(8 * s + 4), z2);
                }
                V::store(p.add(4 * f), x);
            }
        }
    }
}

/// Builds the entry points and the table for one instruction set. The entry
/// functions carry the `#[target_feature]` attributes passed in `$attr`.
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
macro_rules! isa_module {
    ($name:ident, $isa:expr, $vec:ty, $bq:ty, $(#[$attr:meta])*) => {
        mod $name {
            use super::*;

            $(#[$attr])*
            unsafe fn gain(d: &mut [f32], s: &[f32], a: f32) {
                // SAFETY: only reached through `table`, after the set was detected.
                unsafe { generic::gain::<$vec>(d, s, a) }
            }
            $(#[$attr])*
            unsafe fn mix_add(d: &mut [f32], s: &[f32], a: f32) {
                // SAFETY: as above.
                unsafe { generic::mix_add::<$vec>(d, s, a) }
            }
            $(#[$attr])*
            unsafe fn crossfade(d: &mut [f32], a: &[f32], b: &[f32], g0: f32, st: f32) {
                // SAFETY: as above.
                unsafe { generic::crossfade::<$vec>(d, a, b, g0, st) }
            }
            $(#[$attr])*
            unsafe fn cmac_i(a: &mut [f32], x: &[f32], h: &[f32]) {
                // SAFETY: as above.
                unsafe { generic::cmac_interleaved::<$vec>(a, x, h) }
            }
            $(#[$attr])*
            unsafe fn cmac_s(
                ar: &mut [f32],
                ai: &mut [f32],
                xr: &[f32],
                xi: &[f32],
                hr: &[f32],
                hi: &[f32],
            ) {
                // SAFETY: as above.
                unsafe { generic::cmac_split::<$vec>(ar, ai, xr, xi, hr, hi) }
            }
            $(#[$attr])*
            unsafe fn fir(o: &mut [f32], x: &[f32], t: &[f32]) {
                // SAFETY: as above.
                unsafe { generic::fir::<$vec>(o, x, t) }
            }
            $(#[$attr])*
            unsafe fn dot(a: &[f32], b: &[f32]) -> f32 {
                // SAFETY: as above.
                unsafe { generic::dot::<$vec>(a, b) }
            }
            $(#[$attr])*
            unsafe fn matvec(y: &mut [f32], w: &[f32], x: &[f32], b: Option<&[f32]>) {
                // SAFETY: as above.
                unsafe { generic::matvec::<$vec>(y, w, x, b) }
            }
            $(#[$attr])*
            unsafe fn biquad4(c: &[[f32; 5]], z: &mut [f32], io: &mut [f32]) {
                // SAFETY: as above.
                unsafe { generic::biquad4::<$bq>(c, z, io) }
            }

            /// The table. Only called when the instruction set is present
            /// (always on aarch64 and for SSE2 on x86_64; after
            /// `is_x86_feature_detected!` for AVX and FMA).
            pub(super) fn table() -> Kernels {
                // SAFETY (all closures): the table is only built after the
                // instruction set was confirmed, see above.
                Kernels {
                    isa: $isa,
                    gain: |d, s, a| unsafe { gain(d, s, a) },
                    mix_add: |d, s, a| unsafe { mix_add(d, s, a) },
                    crossfade: |d, a, b, g, s| unsafe { crossfade(d, a, b, g, s) },
                    cmac_i: |a, x, h| unsafe { cmac_i(a, x, h) },
                    cmac_s: |ar, ai, xr, xi, hr, hi| unsafe { cmac_s(ar, ai, xr, xi, hr, hi) },
                    fir: |o, x, t| unsafe { fir(o, x, t) },
                    dot: |a, b| unsafe { dot(a, b) },
                    matvec: |y, w, x, b| unsafe { matvec(y, w, x, b) },
                    biquad4: |c, z, io| unsafe { biquad4(c, z, io) },
                }
            }
        }
    };
}

#[cfg(target_arch = "aarch64")]
#[allow(unused_unsafe)]
mod neon_vec {
    use super::Vf;
    use core::arch::aarch64::*;

    #[derive(Clone, Copy)]
    pub(super) struct N(float32x4_t);

    // SAFETY (whole impl): NEON is part of the aarch64 baseline, and every
    // pointer argument is valid for the stated number of floats by the
    // callers in `generic`. Loads and stores are unaligned-safe (`vld1q`).
    impl Vf for N {
        const LANES: usize = 4;
        #[inline(always)]
        unsafe fn splat(x: f32) -> Self {
            N(unsafe { vdupq_n_f32(x) })
        }
        #[inline(always)]
        unsafe fn load(p: *const f32) -> Self {
            N(unsafe { vld1q_f32(p) })
        }
        #[inline(always)]
        unsafe fn store(p: *mut f32, v: Self) {
            unsafe { vst1q_f32(p, v.0) }
        }
        #[inline(always)]
        unsafe fn add(a: Self, b: Self) -> Self {
            N(unsafe { vaddq_f32(a.0, b.0) })
        }
        #[inline(always)]
        unsafe fn sub(a: Self, b: Self) -> Self {
            N(unsafe { vsubq_f32(a.0, b.0) })
        }
        #[inline(always)]
        unsafe fn mul(a: Self, b: Self) -> Self {
            N(unsafe { vmulq_f32(a.0, b.0) })
        }
        #[inline(always)]
        unsafe fn fma(a: Self, b: Self, c: Self) -> Self {
            // vfmaq_f32(acc, x, y) = acc + x * y
            N(unsafe { vfmaq_f32(c.0, a.0, b.0) })
        }
        #[inline(always)]
        unsafe fn fnma(c: Self, a: Self, b: Self) -> Self {
            // vfmsq_f32(acc, x, y) = acc - x * y
            N(unsafe { vfmsq_f32(c.0, a.0, b.0) })
        }
        #[inline(always)]
        unsafe fn load2(p: *const f32) -> (Self, Self) {
            let v = unsafe { vld2q_f32(p) };
            (N(v.0), N(v.1))
        }
        #[inline(always)]
        unsafe fn store2(p: *mut f32, re: Self, im: Self) {
            unsafe { vst2q_f32(p, float32x4x2_t(re.0, im.0)) }
        }
        #[inline(always)]
        unsafe fn reduce16(acc: &[Self; 4]) -> f32 {
            unsafe {
                // logical lane i + 8: registers 2 and 3
                let lo = vaddq_f32(acc[0].0, acc[2].0);
                let hi = vaddq_f32(acc[1].0, acc[3].0);
                let s4 = vaddq_f32(lo, hi);
                let t = vadd_f32(vget_low_f32(s4), vget_high_f32(s4));
                vget_lane_f32::<0>(t) + vget_lane_f32::<1>(t)
            }
        }
    }
}

#[cfg(target_arch = "aarch64")]
use neon_vec::N;

#[cfg(target_arch = "aarch64")]
isa_module!(neon, Isa::Neon, N, N,);

#[cfg(target_arch = "x86_64")]
#[allow(unused_unsafe)]
mod x86_vec {
    use super::Vf;
    use core::arch::x86_64::*;

    /// Four lanes, SSE2 only.
    #[derive(Clone, Copy)]
    pub(super) struct S(__m128);
    /// Eight lanes, AVX with FMA.
    #[derive(Clone, Copy)]
    pub(super) struct A(__m256);

    // SAFETY (whole impl): SSE2 is part of the x86_64 baseline; pointer
    // arguments are valid by the callers in `generic`; loads are `loadu`.
    impl Vf for S {
        const LANES: usize = 4;
        #[inline(always)]
        unsafe fn splat(x: f32) -> Self {
            S(unsafe { _mm_set1_ps(x) })
        }
        #[inline(always)]
        unsafe fn load(p: *const f32) -> Self {
            S(unsafe { _mm_loadu_ps(p) })
        }
        #[inline(always)]
        unsafe fn store(p: *mut f32, v: Self) {
            unsafe { _mm_storeu_ps(p, v.0) }
        }
        #[inline(always)]
        unsafe fn add(a: Self, b: Self) -> Self {
            S(unsafe { _mm_add_ps(a.0, b.0) })
        }
        #[inline(always)]
        unsafe fn sub(a: Self, b: Self) -> Self {
            S(unsafe { _mm_sub_ps(a.0, b.0) })
        }
        #[inline(always)]
        unsafe fn mul(a: Self, b: Self) -> Self {
            S(unsafe { _mm_mul_ps(a.0, b.0) })
        }
        #[inline(always)]
        unsafe fn fma(a: Self, b: Self, c: Self) -> Self {
            unsafe { Self::add(c, Self::mul(a, b)) }
        }
        #[inline(always)]
        unsafe fn fnma(c: Self, a: Self, b: Self) -> Self {
            unsafe { Self::sub(c, Self::mul(a, b)) }
        }
        #[inline(always)]
        unsafe fn load2(p: *const f32) -> (Self, Self) {
            unsafe {
                let (v0, v1) = (_mm_loadu_ps(p), _mm_loadu_ps(p.add(4)));
                (
                    S(_mm_shuffle_ps::<0x88>(v0, v1)),
                    S(_mm_shuffle_ps::<0xDD>(v0, v1)),
                )
            }
        }
        #[inline(always)]
        unsafe fn store2(p: *mut f32, re: Self, im: Self) {
            unsafe {
                _mm_storeu_ps(p, _mm_unpacklo_ps(re.0, im.0));
                _mm_storeu_ps(p.add(4), _mm_unpackhi_ps(re.0, im.0));
            }
        }
        #[inline(always)]
        unsafe fn reduce16(acc: &[Self; 4]) -> f32 {
            unsafe {
                let lo = _mm_add_ps(acc[0].0, acc[2].0);
                let hi = _mm_add_ps(acc[1].0, acc[3].0);
                let s4 = _mm_add_ps(lo, hi);
                // (s0 + s2, s1 + s3, ..)
                let t = _mm_add_ps(s4, _mm_movehl_ps(s4, s4));
                let r = _mm_add_ss(t, _mm_shuffle_ps::<0x55>(t, t));
                _mm_cvtss_f32(r)
            }
        }
    }

    // SAFETY (whole impl): only used inside `#[target_feature(enable =
    // "avx,fma")]` entry points, which are only called after detection.
    impl Vf for A {
        const LANES: usize = 8;
        #[inline(always)]
        unsafe fn splat(x: f32) -> Self {
            A(unsafe { _mm256_set1_ps(x) })
        }
        #[inline(always)]
        unsafe fn load(p: *const f32) -> Self {
            A(unsafe { _mm256_loadu_ps(p) })
        }
        #[inline(always)]
        unsafe fn store(p: *mut f32, v: Self) {
            unsafe { _mm256_storeu_ps(p, v.0) }
        }
        #[inline(always)]
        unsafe fn add(a: Self, b: Self) -> Self {
            A(unsafe { _mm256_add_ps(a.0, b.0) })
        }
        #[inline(always)]
        unsafe fn sub(a: Self, b: Self) -> Self {
            A(unsafe { _mm256_sub_ps(a.0, b.0) })
        }
        #[inline(always)]
        unsafe fn mul(a: Self, b: Self) -> Self {
            A(unsafe { _mm256_mul_ps(a.0, b.0) })
        }
        #[inline(always)]
        unsafe fn fma(a: Self, b: Self, c: Self) -> Self {
            A(unsafe { _mm256_fmadd_ps(a.0, b.0, c.0) })
        }
        #[inline(always)]
        unsafe fn fnma(c: Self, a: Self, b: Self) -> Self {
            // -(a * b) + c
            A(unsafe { _mm256_fnmadd_ps(a.0, b.0, c.0) })
        }
        #[inline(always)]
        unsafe fn load2(p: *const f32) -> (Self, Self) {
            unsafe {
                let (v0, v1) = (_mm256_loadu_ps(p), _mm256_loadu_ps(p.add(8)));
                // Within each 128-bit lane: [v0.0 v0.2 v1.0 v1.2]. Pairs end up
                // in the order the unpacks in `store2` undo.
                (
                    A(_mm256_shuffle_ps::<0x88>(v0, v1)),
                    A(_mm256_shuffle_ps::<0xDD>(v0, v1)),
                )
            }
        }
        #[inline(always)]
        unsafe fn store2(p: *mut f32, re: Self, im: Self) {
            unsafe {
                _mm256_storeu_ps(p, _mm256_unpacklo_ps(re.0, im.0));
                _mm256_storeu_ps(p.add(8), _mm256_unpackhi_ps(re.0, im.0));
            }
        }
        #[inline(always)]
        unsafe fn reduce16(acc: &[Self; 4]) -> f32 {
            unsafe {
                // two registers hold the sixteen logical lanes
                let s8 = _mm256_add_ps(acc[0].0, acc[1].0);
                let s4 = _mm_add_ps(_mm256_castps256_ps128(s8), _mm256_extractf128_ps::<1>(s8));
                let t = _mm_add_ps(s4, _mm_movehl_ps(s4, s4));
                let r = _mm_add_ss(t, _mm_shuffle_ps::<0x55>(t, t));
                _mm_cvtss_f32(r)
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
use x86_vec::{A, S};

#[cfg(target_arch = "x86_64")]
isa_module!(sse2, Isa::Sse2, S, S,);

// The AVX table runs the four-channel biquad on the SSE register (four lanes);
// the cascade is latency-bound, and eight channels would need a different API.
#[cfg(target_arch = "x86_64")]
isa_module!(avx, Isa::AvxFma, A, S, #[target_feature(enable = "avx,fma")]);

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
        }
        fn vec(&mut self, n: usize) -> Vec<f32> {
            (0..n).map(|_| self.next()).collect()
        }
    }

    const LENS: [usize; 12] = [0, 1, 3, 7, 8, 9, 15, 16, 17, 33, 255, 1023];

    /// Same bit pattern, or both NaN.
    fn same(a: f32, b: f32) -> bool {
        a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
    }

    /// Equal within `tol * scale`; NaN matches NaN, infinities match exactly.
    fn close(a: f32, b: f32, scale: f32, tol: f32) -> bool {
        if a.is_nan() || b.is_nan() {
            return a.is_nan() && b.is_nan();
        }
        if a.is_infinite() || b.is_infinite() {
            return a == b;
        }
        (a - b).abs() <= tol * scale.max(1e-30)
    }

    fn check_exact(a: &[f32], b: &[f32], what: &str) {
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b).enumerate() {
            assert!(same(*x, *y), "{what}: index {i}: {x} vs {y}");
        }
    }

    #[test]
    fn detection_reports_a_path_and_scalar_is_always_there() {
        let k = Kernels::detect();
        let all = Kernels::available();
        assert_eq!(all[0].isa(), Isa::Scalar);
        assert!(all.iter().any(|a| a.isa() == k.isa()));
        #[cfg(target_arch = "aarch64")]
        assert_eq!(k.isa(), Isa::Neon);
        eprintln!("paths: {:?}, detected {:?}", all, k);
    }

    #[test]
    fn gain_mix_crossfade_are_bit_exact() {
        let mut r = Rng(0x1234_5678_9abc_def1);
        for k in Kernels::available() {
            for &n in &LENS {
                for off in [0usize, 1, 3] {
                    let xs = r.vec(n + off + 4);
                    let ys = r.vec(n + off + 4);
                    let (x, y) = (&xs[off..off + n], &ys[off..off + n]);
                    let mut want = vec![0.0; n];
                    let mut got = vec![0.0; n + off + 4];
                    scalar::gain(&mut want, x, 0.37);
                    k.gain(&mut got[off..off + n], x, 0.37);
                    check_exact(&want, &got[off..off + n], "gain");

                    let mut want = y.to_vec();
                    let mut got_buf = ys.clone();
                    scalar::mix_add(&mut want, x, -1.25);
                    k.mix_add(&mut got_buf[off..off + n], x, -1.25);
                    check_exact(&want, &got_buf[off..off + n], "mix_add");

                    let mut want = vec![0.0; n];
                    scalar::crossfade(&mut want, x, y, 0.1, 0.9 / (n.max(1) as f32));
                    let mut got = vec![0.0; n];
                    k.crossfade(&mut got, x, y, 0.1, 0.9 / (n.max(1) as f32));
                    check_exact(&want, &got, "crossfade");

                    let mut w = x.to_vec();
                    let mut g = x.to_vec();
                    scalar::gain(&mut w.clone(), x, 2.0);
                    let src = w.clone();
                    scalar::gain(&mut w, &src, 2.0);
                    k.scale(&mut g, 2.0);
                    check_exact(&w, &g, "scale");
                }
            }
        }
    }

    #[test]
    fn crossfade_ends_at_the_right_gains() {
        let k = Kernels::detect();
        let a = vec![1.0_f32; 64];
        let b = vec![3.0_f32; 64];
        let mut d = vec![0.0; 64];
        k.crossfade(&mut d, &a, &b, 0.0, 1.0 / 64.0);
        assert_eq!(d[0], 1.0);
        assert!((d[63] - (1.0 + 2.0 * 63.0 / 64.0)).abs() < 1e-6);
    }

    #[test]
    fn complex_mac_matches_scalar() {
        let mut r = Rng(42);
        for k in Kernels::available() {
            for &n in &LENS {
                for off in [0usize, 1] {
                    let xs = r.vec(2 * n + 2);
                    let hs = r.vec(2 * n + 2);
                    let a0 = r.vec(2 * n + 2);
                    let (x, h) = (&xs[off..off + 2 * n], &hs[off..off + 2 * n]);
                    let mut want = a0[off..off + 2 * n].to_vec();
                    scalar::cmac_interleaved(&mut want, x, h);
                    let mut got = a0.clone();
                    k.cmac_interleaved(&mut got[off..off + 2 * n], x, h);
                    for i in 0..2 * n {
                        let g = got[off + i];
                        if k.isa().fused() {
                            assert!(close(want[i], g, 4.0, 4.0 * f32::EPSILON), "{i}");
                        } else {
                            assert!(same(want[i], g), "{:?} {i}", k.isa());
                        }
                    }
                    // split
                    let (xr, xi): (Vec<f32>, Vec<f32>) = x.chunks(2).map(|c| (c[0], c[1])).unzip();
                    let (hr, hi): (Vec<f32>, Vec<f32>) = h.chunks(2).map(|c| (c[0], c[1])).unzip();
                    let (mut ar, mut ai): (Vec<f32>, Vec<f32>) =
                        a0[off..off + 2 * n].chunks(2).map(|c| (c[0], c[1])).unzip();
                    let (mut wr, mut wi) = (ar.clone(), ai.clone());
                    scalar::cmac_split(&mut wr, &mut wi, &xr, &xi, &hr, &hi);
                    k.cmac_split(&mut ar, &mut ai, &xr, &xi, &hr, &hi);
                    for i in 0..n {
                        if k.isa().fused() {
                            assert!(close(wr[i], ar[i], 4.0, 4.0 * f32::EPSILON));
                            assert!(close(wi[i], ai[i], 4.0, 4.0 * f32::EPSILON));
                        } else {
                            assert!(same(wr[i], ar[i]) && same(wi[i], ai[i]));
                        }
                        // interleaved and split agree with each other
                        assert!(close(wr[i], want[2 * i], 4.0, 1e-6));
                    }
                }
            }
        }
    }

    #[test]
    fn complex_mac_on_complex_slices() {
        let k = Kernels::detect();
        let mut r = Rng(7);
        let n = 37;
        let mk = |r: &mut Rng| -> Vec<Complex<f32>> {
            (0..n).map(|_| Complex::new(r.next(), r.next())).collect()
        };
        let (x, h, mut acc) = (mk(&mut r), mk(&mut r), mk(&mut r));
        let mut want = acc.clone();
        for ((a, x), h) in want.iter_mut().zip(&x).zip(&h) {
            a.re += x.re * h.re - x.im * h.im;
            a.im += x.re * h.im + x.im * h.re;
        }
        k.cmac_complex(&mut acc, &x, &h);
        for (a, w) in acc.iter().zip(&want) {
            assert!(close(a.re, w.re, 4.0, 4.0 * f32::EPSILON));
            assert!(close(a.im, w.im, 4.0, 4.0 * f32::EPSILON));
        }
    }

    #[test]
    fn dot_and_matvec_match_scalar() {
        let mut r = Rng(99);
        for k in Kernels::available() {
            for &n in &LENS {
                for off in [0usize, 1, 2] {
                    let a = r.vec(n + off);
                    let b = r.vec(n + off);
                    let (a, b) = (&a[off..], &b[off..]);
                    let want = scalar::dot(a, b);
                    let got = k.dot(a, b);
                    let scale: f32 = a.iter().zip(b).map(|(x, y)| (x * y).abs()).sum();
                    if k.isa().fused() {
                        assert!(
                            close(want, got, scale, n.max(1) as f32 * f32::EPSILON),
                            "{:?} n={n}: {want} vs {got}",
                            k.isa()
                        );
                    } else {
                        assert!(same(want, got), "{:?} n={n}: {want} vs {got}", k.isa());
                    }
                }
            }
            // matvec, odd shapes, with and without bias
            for (rows, cols) in [(1, 1), (3, 7), (5, 17), (13, 64), (7, 1023)] {
                let w = r.vec(rows * cols + 1);
                let x = r.vec(cols);
                let bias = r.vec(rows);
                for b in [None, Some(&bias[..])] {
                    let mut want = vec![0.0; rows];
                    let mut got = vec![0.0; rows];
                    scalar::matvec(&mut want, &w[1..], &x, b);
                    k.matvec(&mut got, &w[1..], &x, b);
                    for i in 0..rows {
                        let scale: f32 = w[1 + i * cols..1 + (i + 1) * cols]
                            .iter()
                            .zip(&x)
                            .map(|(p, q)| (p * q).abs())
                            .sum::<f32>()
                            + 1.0;
                        if k.isa().fused() {
                            assert!(close(want[i], got[i], scale, cols as f32 * f32::EPSILON));
                        } else {
                            assert!(same(want[i], got[i]));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fir_matches_scalar_for_any_tap_count() {
        let mut r = Rng(5);
        for k in Kernels::available() {
            for taps in [1usize, 2, 3, 7, 16, 33, 255] {
                for &n in &[1usize, 3, 7, 40, 129, 1023] {
                    for off in [0usize, 1] {
                        let xs = r.vec(n + taps - 1 + off);
                        let x = &xs[off..];
                        let h = r.vec(taps);
                        let mut want = vec![0.0; n];
                        scalar::fir(&mut want, x, &h);
                        let mut got = vec![0.0; n];
                        k.fir(&mut got, x, &h);
                        for i in 0..n {
                            let scale: f32 = (0..taps).map(|j| (h[j] * x[i + j]).abs()).sum();
                            if k.isa().fused() {
                                assert!(close(want[i], got[i], scale, taps as f32 * f32::EPSILON));
                            } else {
                                assert!(same(want[i], got[i]), "{:?} taps {taps} n {n}", k.isa());
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fir_is_a_convolution_with_the_history_buffer() {
        // h = [1, 2, 3] applied to a ramp, with two samples of history.
        let k = Kernels::detect();
        let h = [1.0_f32, 2.0, 3.0];
        let taps_rev = [3.0_f32, 2.0, 1.0];
        let x = [0.0_f32, 0.0, 1.0, 2.0, 3.0, 4.0]; // two history zeros, block 1..4
        let mut y = [0.0; 4];
        k.fir(&mut y, &x, &taps_rev);
        // y[n] = sum_j h[j] * block[n - j]
        let block = [1.0_f32, 2.0, 3.0, 4.0];
        for n in 0..4 {
            let mut want = 0.0;
            for j in 0..3 {
                if n >= j {
                    want += h[j] * block[n - j];
                }
            }
            assert_eq!(y[n], want);
        }
    }

    #[test]
    fn nan_and_inf_propagate_like_scalar() {
        let mut r = Rng(11);
        let n = 100;
        let mut a = r.vec(n);
        let mut b = r.vec(n);
        a[10] = f32::NAN;
        a[40] = f32::INFINITY;
        b[41] = 2.0;
        a[77] = f32::NEG_INFINITY;
        b[77] = 1.0;
        for k in Kernels::available() {
            let mut w = vec![0.0; n];
            let mut g = vec![0.0; n];
            scalar::gain(&mut w, &a, 0.5);
            k.gain(&mut g, &a, 0.5);
            check_exact(&w, &g, "gain nan");
            scalar::mix_add(&mut w, &b, 2.0);
            k.mix_add(&mut g, &b, 2.0);
            check_exact(&w, &g, "mix nan");
            let (w, g) = (scalar::dot(&a, &b), k.dot(&a, &b));
            assert!(w.is_nan() && g.is_nan());
            let mut ow = vec![0.0; 60];
            let mut og = vec![0.0; 60];
            scalar::fir(&mut ow, &a, &b[..41]);
            k.fir(&mut og, &a, &b[..41]);
            for i in 0..60 {
                assert!(
                    ow[i].is_nan() == og[i].is_nan()
                        && (ow[i].is_nan() || ow[i] == og[i] || (ow[i] - og[i]).abs() < 1e-4),
                    "fir {i}: {} {}",
                    ow[i],
                    og[i]
                );
            }
            let mut wa = vec![0.0; n];
            let mut ga = vec![0.0; n];
            scalar::cmac_interleaved(&mut wa, &a, &b);
            k.cmac_interleaved(&mut ga, &a, &b);
            for i in 0..n {
                assert_eq!(wa[i].is_nan(), ga[i].is_nan(), "cmac nan {i}");
                if !wa[i].is_nan() {
                    assert!(close(wa[i], ga[i], 4.0, 1e-6), "cmac {i}");
                }
            }
        }
    }

    #[test]
    fn set_section_ignores_an_index_past_the_end() {
        let mut bank = BiquadBank4::new(&[[1.0, 0.0, 0.0, 0.0, 0.0]]);
        bank.set_section(1, [0.5, 0.0, 0.0, 0.0, 0.0]);
        bank.set_section(0, [0.5, 0.0, 0.0, 0.0, 0.0]);
        let mut io = [1.0f32; 4];
        bank.process_interleaved(&mut io);
        assert_eq!(io, [0.5; 4]);
    }

    fn sections_n(n: usize) -> Vec<[f32; 5]> {
        sections().into_iter().cycle().take(n).collect()
    }

    fn sections() -> Vec<[f32; 5]> {
        // A few stable sections (poles inside the unit circle).
        vec![
            [0.2, 0.4, 0.2, -0.5, 0.2],
            [0.9, -1.2, 0.5, -0.3, 0.1],
            [1.0, 0.1, 0.0, -0.2, 0.05],
        ]
    }

    #[test]
    fn biquad_bank_is_bit_exact_and_matches_biquad() {
        let mut r = Rng(1);
        for k in Kernels::available() {
            for (nsec, &frames) in [1usize, 2, 3, 4, 5, 8]
                .iter()
                .zip([1usize, 3, 7, 1023, 64, 9].iter())
            {
                let secs = sections_n(*nsec);
                let io = r.vec(frames * 4);
                let mut want = io.clone();
                let mut z = vec![0.0; secs.len() * 8];
                scalar::biquad4(&secs, &mut z, &mut want);
                let mut bank = BiquadBank4::with_kernels(&secs, k);
                let mut got = io.clone();
                bank.process_interleaved(&mut got);
                check_exact(&want, &got, "biquad4");

                // planar entry point, in two calls (state carries over)
                let mut planar: Vec<Vec<f32>> = (0..4)
                    .map(|c| (0..frames).map(|i| io[4 * i + c]).collect())
                    .collect();
                let mut bank = BiquadBank4::with_kernels(&secs, k);
                let split = frames / 2;
                for (lo, hi) in [(0, split), (split, frames)] {
                    let [p0, p1, p2, p3] = &mut planar[..] else {
                        unreachable!()
                    };
                    bank.process([
                        &mut p0[lo..hi],
                        &mut p1[lo..hi],
                        &mut p2[lo..hi],
                        &mut p3[lo..hi],
                    ]);
                }
                for i in 0..frames {
                    for c in 0..4 {
                        assert!(same(planar[c][i], want[4 * i + c]));
                    }
                }
            }
        }
        // A unit section is the identity.
        let mut bank = BiquadBank4::new(&[[1.0, 0.0, 0.0, 0.0, 0.0]]);
        let mut io = [0.5_f32, -0.5, 0.25, 1.0, 0.1, 0.2, 0.3, 0.4];
        let before = io;
        bank.process_interleaved(&mut io);
        assert_eq!(io, before);
    }

    #[test]
    fn channels_are_independent_in_the_bank() {
        let mut bank = BiquadBank4::new(&sections());
        let mut io = vec![0.0_f32; 4 * 64];
        io[0] = 1.0; // impulse on channel 0 only
        bank.process_interleaved(&mut io);
        for i in 0..64 {
            assert_eq!(io[4 * i + 1], 0.0);
            assert_eq!(io[4 * i + 2], 0.0);
            assert_eq!(io[4 * i + 3], 0.0);
        }
        assert!(io.iter().step_by(4).any(|v| *v != 0.0));
    }

    /// An eight-lane vector in plain arrays, unfused. It runs the generic
    /// kernels at the AVX width on any machine, so the width-dependent logic
    /// (unrolling, the sixteen-lane order, the tails) is tested even where AVX
    /// itself is not available (Rosetta reports no AVX).
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    #[allow(clippy::needless_range_loop)]
    mod emu {
        use super::super::{Vf, generic, scalar};
        use super::*;

        #[derive(Clone, Copy)]
        struct E8([f32; 8]);
        fn zip(a: E8, b: E8, f: impl Fn(f32, f32) -> f32) -> E8 {
            let mut o = [0.0; 8];
            for i in 0..8 {
                o[i] = f(a.0[i], b.0[i]);
            }
            E8(o)
        }
        // SAFETY (impl): pointers are valid for the stated counts by the callers.
        impl Vf for E8 {
            const LANES: usize = 8;
            unsafe fn splat(x: f32) -> Self {
                E8([x; 8])
            }
            unsafe fn load(p: *const f32) -> Self {
                let mut o = [0.0; 8];
                for (i, v) in o.iter_mut().enumerate() {
                    *v = unsafe { p.add(i).read_unaligned() };
                }
                E8(o)
            }
            unsafe fn store(p: *mut f32, v: Self) {
                for i in 0..8 {
                    unsafe { p.add(i).write_unaligned(v.0[i]) };
                }
            }
            unsafe fn add(a: Self, b: Self) -> Self {
                zip(a, b, |x, y| x + y)
            }
            unsafe fn sub(a: Self, b: Self) -> Self {
                zip(a, b, |x, y| x - y)
            }
            unsafe fn mul(a: Self, b: Self) -> Self {
                zip(a, b, |x, y| x * y)
            }
            unsafe fn fma(a: Self, b: Self, c: Self) -> Self {
                unsafe { Self::add(c, Self::mul(a, b)) }
            }
            unsafe fn fnma(c: Self, a: Self, b: Self) -> Self {
                unsafe { Self::sub(c, Self::mul(a, b)) }
            }
            unsafe fn load2(p: *const f32) -> (Self, Self) {
                let (mut re, mut im) = ([0.0; 8], [0.0; 8]);
                for i in 0..8 {
                    re[i] = unsafe { p.add(2 * i).read_unaligned() };
                    im[i] = unsafe { p.add(2 * i + 1).read_unaligned() };
                }
                (E8(re), E8(im))
            }
            unsafe fn store2(p: *mut f32, re: Self, im: Self) {
                for i in 0..8 {
                    unsafe {
                        p.add(2 * i).write_unaligned(re.0[i]);
                        p.add(2 * i + 1).write_unaligned(im.0[i]);
                    }
                }
            }
            unsafe fn reduce16(acc: &[Self; 4]) -> f32 {
                let s8: Vec<f32> = (0..8).map(|i| acc[0].0[i] + acc[1].0[i]).collect();
                let s4: Vec<f32> = (0..4).map(|i| s8[i] + s8[i + 4]).collect();
                (s4[0] + s4[2]) + (s4[1] + s4[3])
            }
        }

        #[test]
        fn eight_lane_generic_kernels_equal_scalar_exactly() {
            let mut r = Rng(77);
            for &n in &LENS {
                let (a, b) = (r.vec(n), r.vec(n));
                // SAFETY: E8 needs no instruction set.
                unsafe {
                    let mut w = vec![0.0; n];
                    let mut g = vec![0.0; n];
                    scalar::gain(&mut w, &a, 0.3);
                    generic::gain::<E8>(&mut g, &a, 0.3);
                    check_exact(&w, &g, "e8 gain");
                    scalar::mix_add(&mut w, &b, 0.7);
                    generic::mix_add::<E8>(&mut g, &b, 0.7);
                    check_exact(&w, &g, "e8 mix");
                    scalar::crossfade(&mut w, &a, &b, 0.2, 0.001);
                    generic::crossfade::<E8>(&mut g, &a, &b, 0.2, 0.001);
                    check_exact(&w, &g, "e8 crossfade");
                    assert!(
                        same(scalar::dot(&a, &b), generic::dot::<E8>(&a, &b)),
                        "e8 dot {n}"
                    );
                    let (x, h, acc) = (r.vec(2 * n), r.vec(2 * n), r.vec(2 * n));
                    let (mut w, mut g) = (acc.clone(), acc.clone());
                    scalar::cmac_interleaved(&mut w, &x, &h);
                    generic::cmac_interleaved::<E8>(&mut g, &x, &h);
                    check_exact(&w, &g, "e8 cmac");
                }
            }
            for taps in [1usize, 5, 33] {
                for n in [1usize, 7, 31, 32, 33, 100] {
                    let x = r.vec(n + taps - 1);
                    let h = r.vec(taps);
                    let (mut w, mut g) = (vec![0.0; n], vec![0.0; n]);
                    scalar::fir(&mut w, &x, &h);
                    // SAFETY: as above; x is long enough.
                    unsafe { generic::fir::<E8>(&mut g, &x, &h) };
                    check_exact(&w, &g, "e8 fir");
                }
            }
        }
    }

    /// Timing report: `cargo test -p openplugin-dsp --release simd::tests::bench_report
    /// -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_report() {
        use std::hint::black_box;
        use std::time::Instant;
        let mut r = Rng(3);
        let n = 1024usize;
        let a = r.vec(n);
        let b = r.vec(n);
        let c = r.vec(n);
        let mut d = r.vec(n);
        let mut acc = r.vec(2 * n);
        let h2 = r.vec(2 * n);
        let x2 = r.vec(2 * n);
        let taps = r.vec(64);
        let xs = r.vec(n + 63);
        let mut out = vec![0.0f32; n];
        let w = r.vec(64 * 64);
        let xv = r.vec(64);
        let mut y = vec![0.0f32; 64];
        let secs = sections();
        let mut io4 = r.vec(4 * n);

        let time = |mut f: Box<dyn FnMut() + '_>| -> f64 {
            for _ in 0..200 {
                f();
            }
            let reps = 4000;
            let mut best = f64::MAX;
            for _ in 0..5 {
                let t = Instant::now();
                for _ in 0..reps {
                    f();
                }
                best = best.min(t.elapsed().as_secs_f64() / reps as f64);
            }
            best * 1e9
        };

        let mut rows: Vec<(String, Vec<(Isa, f64)>)> = Vec::new();
        let paths = Kernels::available();
        macro_rules! bench {
            ($name:expr, |$k:ident| $body:expr) => {{
                let mut v = Vec::new();
                for $k in &paths {
                    let $k = *$k;
                    v.push(($k.isa(), time(Box::new(|| $body))));
                }
                rows.push(($name.to_string(), v));
            }};
        }
        bench!("gain 1024", |k| k.gain(&mut d, black_box(&a), 0.5));
        bench!("mix_add 1024", |k| k.mix_add(&mut d, black_box(&a), 0.5));
        bench!("crossfade 1024", |k| k.crossfade(
            &mut d,
            black_box(&a),
            &b,
            0.0,
            1e-3
        ));
        bench!("cmac interleaved 1024 bins", |k| k.cmac_interleaved(
            &mut acc,
            black_box(&x2),
            &h2
        ));
        bench!("fir 64 taps x 1024", |k| k.fir(
            &mut out,
            black_box(&xs),
            &taps
        ));
        bench!("dot 1024", |k| {
            black_box(k.dot(black_box(&a), &c));
        });
        bench!("matvec 64x64", |k| k.matvec(
            &mut y,
            black_box(&w),
            &xv,
            None
        ));
        let mut banks: Vec<BiquadBank4> = paths
            .iter()
            .map(|k| BiquadBank4::with_kernels(&secs, *k))
            .collect();
        {
            let mut v = Vec::new();
            for (k, bank) in paths.iter().zip(banks.iter_mut()) {
                v.push((
                    k.isa(),
                    time(Box::new(|| bank.process_interleaved(black_box(&mut io4)))),
                ));
            }
            rows.push(("biquad x3, 4 ch x 1024 frames".to_string(), v));
        }

        println!("\nkernel timings, ns per call (best of 5), speed-up over scalar");
        for (name, v) in &rows {
            let base = v[0].1;
            let cols: Vec<String> = v
                .iter()
                .map(|(isa, t)| format!("{} {:.0} ns ({:.2}x)", isa.name(), t, base / t))
                .collect();
            println!("{name:34} {}", cols.join("   "));
        }
    }
}
