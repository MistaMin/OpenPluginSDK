# Performance guide

How to make a plugin cheap and keep it cheap, for someone writing a `Plugin` with this SDK.

The rule of this guide: **every claim has a measured example in the repo.** The examples are in
[`examples/perf_guide`](../examples/perf_guide) (a workspace member). Each optimisation is a pair of
small kernels, a "before" and an "after", wrapped as a `Plugin` and timed by the SDK's own bench
harness ([`openplugin-testing`](TESTING_PLUGINS.md#benchmarks)). The numbers below come from that
crate. Where something could not be measured on the machine that wrote this guide, the guide says so
instead of quoting a number from memory.

Companion guide: [STABILITY.md](STABILITY.md), for the rules that keep a plugin from producing
NaN, blowing up or crashing the host.

## How the numbers were made

| | |
| --- | --- |
| Machine | Apple M1 Max (Mac Studio), arm64, macOS 15.7.5 |
| Compiler | rustc 1.98.0 (88d9e12ae 2026-08-18) |
| Date | 2026-11-15 |
| Profile | the workspace `release` profile: `opt-level = 3`, `lto = "thin"`, `codegen-units = 1` |
| Harness | `openplugin_testing::bench`, 48 kHz, blocks of 4096 frames, 2 channels, 2000 timed blocks after 300 warm-up blocks, the median of five harness runs |
| Unit | microseconds per block (`us/block`), and `ns/frame` = `us/block` divided by 4096. A block of 4096 frames at 48 kHz lasts 85.3 ms, so 1 us/block is 0.0012 % of one core |

The machine was not idle (other builds ran on it). The medians repeat to about 2 % between runs;
the p99 column in the tool's output does not, and the guide quotes medians only. Timing is
quantised by the clock (41.7 ns on this Mac), which is why the blocks are long: a 0.5 us kernel
measured on a 512-frame block would be eleven ticks. Rerun on your own machine before you believe a
ratio from a different CPU.

Reproduce everything:

```sh
cargo run --release -p example-perf-guide              # timings, then the stability numbers
cargo run --release -p example-perf-guide -- perf --filter tanh,vectorise
cargo run --release -p example-perf-guide -- approx    # the error of the fast functions
cargo test -p example-perf-guide                       # every "after" equals its "before"
```

The tests are part of the proof: [`tests/kernels.rs`](../examples/perf_guide/tests/kernels.rs) checks
that each "after" computes what its "before" computes (bit for bit, or within the stated error), and
[`tests/approx_error.rs`](../examples/perf_guide/tests/approx_error.rs) asserts the error bounds
quoted in the approximation section.

## 1. The real-time rules

`Plugin::process` runs on a thread the host gives a deadline: the block must be finished before the
device needs it. Missing the deadline is a dropout the user hears, and one slow block is enough.
So the rules are not about averages, they are about the worst block:

| Do not, on the audio thread | Why | How the kit checks it |
| --- | --- | --- |
| Allocate or free (`Vec::new`, `push` past capacity, `format!`, `Box::new`, `String`, cloning a `Vec`) | The allocator may take a lock, ask the kernel for pages or walk a long free list. How long it takes is not yours to bound. | `no_alloc`: a counting global allocator armed around every audio-path call. Fails on the first allocation or free. |
| Take a lock (`Mutex`, `RwLock`, a channel `recv`) | If the thread that holds it is descheduled, the audio thread waits for it (priority inversion). | **Not checkable from user space.** The `locks` check always reports `skip`. Use the thread sanitizer or a profiler that shows `pthread_mutex_lock`. The SDK's own answer is `ParamPlane`, `Seqlock` and the SPSC queue below. |
| Make a system call (file, socket, `println!`, `eprintln!`, `thread::sleep`, `log::info!` with a real logger) | Unbounded latency, and `println!` also locks stdout. | Only indirectly: formatting allocates, which `no_alloc` sees. The write itself is not seen. Send a message to another thread through a wait-free queue and log there. |
| Do unbounded work (loop to convergence, a search, a recursion without a depth bound, rebuilding a table when a parameter moves) | The cost depends on the data. | `bench` shows `p99` and `max` next to the median; a block that sometimes costs ten times the median shows there. `compare` flags a slower commit. |
| Panic | A panic that crosses the format boundary takes the host down. See [STABILITY.md](STABILITY.md#panics-never-cross-the-format-boundary). | `no_panic`. |

What the SDK gives you instead of the forbidden things (all in `openplugin-core`, all
allocation-free after construction):

- **Parameters**: `ParamPlane::store` from any thread, `ParamCursor::pull` on the audio thread
  (atomic words, no lock). Sample-accurate changes: `SampleRamp`.
- **Meters and telemetry to the editor**: `Seqlock` (single writer, torn reads impossible). Test:
  `readers_never_observe_a_torn_pair` in [`seqlock.rs`](../crates/openplugin-core/src/seqlock.rs).
- **Messages between threads**: the bounded SPSC queue in
  [`spsc.rs`](../crates/openplugin-core/src/spsc.rs) (`concurrent_ordering` test).
- **Building something big off the audio thread**: `worker::Loader` and `Slot`, which build a
  convolution impulse or a wavetable on a worker and swap it in with one poll per block (pattern in
  [ARCHITECTURE.md](ARCHITECTURE.md#loading-heavy-data-off-the-audio-thread)). Proofs:
  [`worker_no_alloc.rs`](../crates/openplugin-core/tests/worker_no_alloc.rs) (zero allocations and
  frees in `poll`) and [`worker_stress.rs`](../crates/openplugin-core/tests/worker_stress.rs).
- **Scratch memory**: allocate it in `prepare`, which the host calls off the audio thread.

### Allocation, measured

The "before" allocates a `Vec` per channel per block; the "after" takes a scratch buffer from
`prepare` ([`kernels.rs`](../examples/perf_guide/src/kernels.rs), `AllocPerBlock` and
`Preallocated`):

```rust
// before: the allocator is called on the audio thread
fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
    for channel in io.iter_mut() {
        let copy: Vec<f32> = channel[..frames].to_vec();          // allocates and frees
        for (d, s) in channel[..frames].iter_mut().zip(&copy) { *d = *s * 0.5; }
    }
}

// after: sized once, in prepare
fn prepare(&mut self, _: f32, max_block: usize, _: usize) { self.scratch = vec![0.0; max_block]; }
fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
    for channel in io.iter_mut() {
        let copy = &mut self.scratch[..frames];
        copy.copy_from_slice(&channel[..frames]);
        for (d, s) in channel[..frames].iter_mut().zip(copy.iter()) { *d = *s * 0.5; }
    }
}
```

| Case | us/block |
| --- | ---: |
| `Vec` per block (before) | 0.96 |
| scratch from `prepare` (after) | 0.92 |

The median barely moves: the system allocator on this Mac is fast when nothing is contended. That
is the point of the rule. The allocator's cost is the *tail*, not the median, and the median is what
a benchmark shows. This is why the rule is enforced by a counter and not by a stopwatch: the test
`allocation_per_block_fails_the_kit_and_the_preallocated_scratch_passes` in
[`tests/kernels.rs`](../examples/perf_guide/tests/kernels.rs) runs the conformance kit on both and
asserts that exactly `no_alloc` fails for the first and nothing fails for the second.

### How the kit checks the rest

[TESTING_PLUGINS.md](TESTING_PLUGINS.md) lists every check. The ones about this guide:

- `no_alloc`, `no_panic`, `finite_output`, `no_denormals`, `bounded_output` and `hostile_input` are
  asserted on every sample of every run (all sample rates 22.05 to 384 kHz, block sizes 1 to 8192,
  fixed and random per call).
- Each check is proved to fail on a deliberately broken plugin:
  [`crates/openplugin-testing/tests/broken.rs`](../crates/openplugin-testing/tests/broken.rs)
  (`allocation_in_process_fails_no_alloc`, `panic_fails_no_panic`, `nan_output_fails_finite_output`,
  `poisoned_by_nan_fails_hostile_input`, and so on). `no_denormals` has no entry there; its failing
  case is `subnormal_output_fails_the_kit_and_the_flush_passes` in the perf guide's tests.
- The SDK's own audio paths carry the same proof: counting-allocator tests in
  [`openplugin-ffi`](../crates/openplugin-ffi/tests/shell_no_alloc.rs),
  [`openplugin-targets`](../crates/openplugin-targets/tests/clap_no_alloc.rs) (CLAP, LV2) and
  [`openplugin-dsp`](../crates/openplugin-dsp/tests/filters_no_alloc.rs) (filters, dynamics,
  convolution, dither, downmix).

## 2. Measuring

**The bench harness** reports CPU time per block as a fraction of real time. A four-line `main` per
plugin ([TESTING_PLUGINS.md](TESTING_PLUGINS.md#benchmarks)); `--out before.json` on the old commit,
`--baseline before.json` on the new one, exit code 1 when a case is slower than the tolerance
(default 25 %). The perf guide wraps a `Kernel` (a plugin with no parameters) in a `Rig` so that
the same harness times a loop and a whole plugin:

```rust
let report = openplugin_testing::bench::bench(|| Rig(Preallocated::default()), &options);
```

Rules for reading it: use `--release` (the report prints `DEBUG` otherwise, and the numbers in
section 7 show why), use blocks long enough that the clock tick is small against the work, compare
medians, and rerun a case that looks wrong before believing it.

**Instruments (Time Profiler)** answers *where* the time goes, which the harness does not. On macOS:

1. Give the release build line tables: `CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release -p example-perf-guide`.
2. Record: `xcrun xctrace record --template 'Time Profiler' --time-limit 10s --output perf.trace --launch -- target/release/perf_guide perf --filter tanh`
   (or open Instruments, choose Time Profiler, pick the binary).
3. Open `perf.trace`. Use *Call Tree* with *Invert Call Tree* and *Hide System Libraries* to see the
   self time per function; the `Rig::process` frame shows the kernel.
4. Profile the same plugin with the kit's input in mind: a profile of silence hides denormal slowness
   and hides everything that depends on the data.

The `xctrace record` command above was run on the machine of this guide and produced a trace. The
trace's `time-profile` table came back empty when exported from a non-interactive shell, so the
guide quotes no profile; read the trace in Instruments itself. For a quick answer without
Instruments, the harness itself is the profiler: comment out half of `process` and compare.

## 3. Denormals and flush-to-zero

A subnormal (denormal) `f32` is a nonzero number smaller than `1.18e-38`. A recursive filter or a
reverb tail that decays toward zero passes through that range on its way, and some CPUs execute
arithmetic on subnormals through a slow microcode path. The rule: never let state or output sit in
the subnormal range.

The SDK's DSP flushes state in its filters, envelope followers and convolvers
(`flush`: `|x| < 1e-30` becomes `0.0`, see
[`svf.rs`](../crates/openplugin-dsp/src/svf.rs) and the test `tails_flush_to_zero_without_denormals`
in [`filters.rs`](../crates/openplugin-dsp/tests/filters.rs)). It is not exported as a public helper
yet; the guard is coming, and until then the two-line version in your own code is:

```rust
y *= 0.97;
if y.abs() < 1.0e-30 { y = 0.0; }   // before it reaches the subnormal range
```

Note that `Biquad` guards NaN but does **not** flush; use `Svf`, `MatchedBiquad` or flush yourself
if a `Biquad` tail must reach exact zero.

The kit checks the output: `no_denormals` fails if any output sample is subnormal, including while a
tail decays. Test: `subnormal_output_fails_the_kit_and_the_flush_passes`.

**What the benchmark measured on this machine** (`Decay`: a one-pole tail excited at the top of every
512 samples; with `start = 1e-36` most of each run is subnormal):

| Case | us/block |
| --- | ---: |
| normal range, no flush (control) | 9.79 |
| subnormal range, no flush (before) | 9.79 |
| subnormal range, flushed per sample (after) | 25.29 |

On Apple Silicon, **subnormals cost nothing**: the first two rows are the same to three digits, and
the flush guard makes this loop 2.6 times slower, because the compare and select lengthen a
dependency chain that was already the bottleneck. The same binary built for `x86_64` and run under
Rosetta gave the same picture (9.92, 9.92, 26.29 us/block), but Rosetta translates to Arm
instructions, so it says nothing about an Intel or AMD CPU. **The penalty that denormals are known
for on x86 could not be measured here**, so this guide does not put a number on it.

What follows from that, honestly:

- Do not add a per-sample flush to a loop you have not measured. Where it is needed, flush the
  *state* of the recursion (the part that decays), not every output, and where the loop allows it,
  once per sub-block instead of once per sample.
- Do keep the kit's `no_denormals` check in your tests. A plugin that is fast on an M1 can be the
  slow one on a Windows laptop, and the check is how you find out without owning the laptop.
- Hardware flush-to-zero (an FPCR or MXCSR mode bit) is a process-wide setting that belongs to the
  host's thread. The SDK does not set it, and a plugin should not: it changes the arithmetic of
  everything else on that thread. State and output flushed in code travels with the plugin.

## 4. Block-rate work against sample-rate work

Work that does not change per sample should not run per sample. Typical offenders: `exp`, `tan`,
`powf` and `sin` to turn a parameter into a coefficient, `10^(dB/20)`, a division that is the same
every sample.

[`SweptLowpass`](../examples/perf_guide/src/kernels.rs) is a one-pole low-pass whose cutoff is swept by
an LFO. The "before" recomputes the coefficient with `exp` for every sample; the "after" does it every
16th sample (control rate):

```rust
// before: every sample
coeff = 1.0 - (-TAU * cutoff / rate).exp();
z += coeff * (x - z);

// after: every 16 samples
if i % 16 == 0 { coeff = 1.0 - (-TAU * cutoff / rate).exp(); }
z += coeff * (x - z);
```

| Case | us/block | ns/frame |
| --- | ---: | ---: |
| `exp` every sample (before) | 43.46 | 10.61 |
| `exp` every 16 samples (after) | 26.96 | 6.58 |

1.6 times faster, and the remaining 6.6 ns is the LFO's `sin` and the filter itself. The output
differs from the per-sample version by less than 2e-3 (test
`sixteen_sample_coefficient_updates_follow_the_per_sample_sweep`): a stale coefficient for 16
samples (0.3 ms at 48 kHz) is below what a smoothed parameter does anyway.

The general pattern, which `StableFilter` in the stability guide uses: smooth the parameter per
sample if you must, recompute the coefficients per sub-block of 16 to 64 samples, and never per
block of 4096, which steps audibly. A parameter that has not changed needs no recomputation at all:
compare against the last value you built coefficients for.

## 5. Parameter smoothing costs

Smoothing exists so that a parameter jump does not click ([STABILITY.md](STABILITY.md#smooth-what-the-listener-would-hear-as-a-step)).
`openplugin_core::OnePole` is the SDK's smoother; its `next` is a multiply and an add. The cost is
the loop it forces: a one-pole smoother has a per-sample feedback, so the gain curve cannot be
computed in parallel, and a gain that nobody is touching still pays for it.

[`SmoothedGain`](../examples/perf_guide/src/kernels.rs) in four forms (the "moving" rows alternate
the target between 0.25 and 1.0 every block, a parameter under constant automation):

```rust
// before: always step the one-pole, fill a gain curve, multiply
for g in &mut ramp[..frames] { *g = pole.next(target); }
for (s, g) in channel.iter_mut().zip(&ramp[..frames]) { *s *= *g; }

// after, settled: once it has arrived, a constant multiply
if (pole.current() - target).abs() < 1.0e-5 {
    pole.reset(target);
    for s in &mut channel[..frames] { *s *= target; }
}

// after, moving: a linear ramp across the block, no feedback to serialise it
let step = (target - gain) / frames as f32;
for (i, s) in channel[..frames].iter_mut().enumerate() { *s *= gain + step * i as f32; }
```

| Case | us/block | ns/frame |
| --- | ---: | ---: |
| settled: one-pole every sample (before) | 14.04 | 3.43 |
| settled: skip when settled (after) | 0.46 | 0.11 |
| moving: one-pole every sample | 14.04 | 3.43 |
| moving: linear ramp per block | 3.04 | 0.74 |

Skipping the smoother when it has settled is 30 times cheaper, and most parameters are settled
nearly all the time. A linear ramp per block is 4.6 times cheaper than the one-pole while moving,
at the price of a ramp that is straight instead of exponential and of reaching the target exactly at
the end of the block (the block size becomes the smoothing time; with 4096-frame blocks, that is
85 ms). Choose the one-pole when the time constant must not depend on the host's block size, which
is usually the right answer, and then skip it when settled. Test:
`settled_skip_and_ramps_reach_the_same_gain`.

## 6. Memory layout: structure of arrays

A loop over many elements that touches a few fields of each should read those fields from arrays of
their own. In an array of structures every cache line carries the fields the loop does not use.

[`PartialsAos` and `PartialsSoa`](../examples/perf_guide/src/kernels.rs) hold 16384 partials (an
additive engine, a bank of voices or bands). Each partial has an amplitude, a decay and a gain that
the loop uses, and 13 more floats that it does not (phase, frequency, pan, filter state): a 64-byte
struct, a 1 MiB array.

```rust
// before: Vec<Partial>, 64 bytes per element, 12 of them used
struct Partial { amp: f32, decay: f32, gain: f32, other: [f32; 13] }
for p in &mut self.parts { p.amp = p.amp * p.decay + p.gain * 1.0e-6; }

// after: one array per field
for ((a, d), g) in self.amp.iter_mut().zip(&self.decay).zip(&self.gain) {
    *a = *a * *d + *g * 1.0e-6;
}
```

| Case | us/block | ns/element |
| --- | ---: | ---: |
| array of structures (before) | 16.71 | 1.02 |
| structure of arrays (after) | 2.88 | 0.18 |

5.8 times faster, and the second loop vectorises while the first reads one strided field at a time.
(An earlier version of this benchmark summed the amplitudes into one running total, and both layouts
tied at 17 us: a sequential floating-point sum is a dependency chain the compiler may not reorder, and
it hid the layout. Measure the loop you ship, not the one you meant.) Test:
`structure_of_arrays_computes_what_the_array_of_structures_does`.

Where the fields are used together every time, an array of structures is not slower, and it is
simpler. The change pays when there are many elements, a big struct, and a loop that reads few
fields. For audio buffers the SDK already gives you the good layout: `io: &mut [Vec<f32>]` is one
contiguous slice per channel (planar), not interleaved frames.

## 7. Helping the compiler vectorise

The Rust compiler turns a simple loop over slices into SIMD instructions when it can prove the loop
is safe to run several elements at once. What stops it: a bounds check on each access, a call it
cannot inline, a loop-carried dependency, and a build that does not optimise.

[`CurveIndexed`, `CurveZip`, `CurveChunks`](../examples/perf_guide/src/kernels.rs) compute
`y = clamp(x * c[i] + 0.1, -1, 1)` three ways:

```rust
// before: nothing proves i < io[c].len() or i < self.curve.len(): every access is checked
for c in 0..io.len() {
    for i in 0..frames {
        io[c][i] = (io[c][i] * self.curve[i] + 0.1).clamp(-1.0, 1.0);
    }
}

// after (1): slice once, then iterate in lock step; one length check per channel
let curve = &self.curve[..frames];
for channel in io.iter_mut() {
    for (s, c) in channel[..frames].iter_mut().zip(curve) {
        *s = (*s * *c + 0.1).clamp(-1.0, 1.0);
    }
}

// after (2): chunks_exact(8); the chunk length is a constant, the tail is handled apart
for (d, c) in (&mut ds).zip(&mut cs) { for k in 0..8 { d[k] = (d[k] * c[k] + 0.1).clamp(-1.0, 1.0); } }
for (d, c) in ds.into_remainder().iter_mut().zip(cs.remainder()) { /* same, one at a time */ }
```

| Case | us/block | ns/frame |
| --- | ---: | ---: |
| indexed, bounds checks (before) | 6.50 | 1.59 |
| sliced + `zip` (after) | 1.13 | 0.28 |
| `chunks_exact(8)` (after) | 2.33 | 0.57 |
| sliced + `zip`, computed in `f64` | 3.46 | 0.84 |

- Slicing to `frames` once and iterating (`zip`) is 5.8 times faster than indexing. This is the
  habit to keep: `let ch = &mut channel[..frames];` first, then iterators.
- `chunks_exact` was **not** faster than `zip` here, it was twice as slow. LLVM already vectorises
  the `zip` loop, and the manual chunking only adds a remainder loop. Reach for it when the loop has
  a reduction (a sum or a peak) that you want in eight independent lanes by hand, not for a
  per-sample map.
- Computing in `f64` costs a factor of three here: half as many lanes per vector, plus a conversion
  on the way in and out. See section 10.
- All four give the same samples: test `every_vectorisation_form_gives_the_same_samples`.

### Build settings

The same kernels, rebuilt with one setting changed (`us/block`, the same harness, the same machine):

| Case | workspace profile | `opt-level=2` | `opt-level=1` | `lto=off`, `codegen-units=16` | `target-cpu=native` |
| --- | ---: | ---: | ---: | ---: | ---: |
| indexed (before) | 5.54 | 6.46 | 6.46 | 5.79 | 5.50 |
| sliced + `zip` | 1.04 | 1.00 | **5.54** | 1.04 | 1.04 |
| `fast_tanh` | 4.79 | 4.79 | **22.83** | 4.79 | 4.79 |
| `fast_exp` | 5.38 | 5.38 | **21.33** | 5.38 | 5.42 |
| structure of arrays | 2.75 | 2.75 | 17.58 | 2.75 | 2.75 |
| settled smoother skip | 0.46 | 0.46 | 2.75 | 0.46 | 0.46 |
| `f32::tanh` (libm call) | 47.58 | 47.54 | 47.75 | 47.58 | 47.58 |

(The first column is a second run of the baseline, so it differs by a few percent from the tables above. Commands: `CARGO_PROFILE_RELEASE_OPT_LEVEL=1 cargo build --release -p example-perf-guide`,
`CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 ...`, and
`RUSTFLAGS="-C target-cpu=native" ...`, each into its own target directory.)

What the table says:

- **`opt-level`**: 3 and 2 are identical on these kernels. `opt-level=1` does not vectorise: every
  optimisation in this guide is gone, and the fast approximations become *slower than the library
  function* (`fast_exp` 21.3 against `f32::exp` 13.5 us). Debug builds are worse: with no optimisation
  at all, `f32::tanh` and `fast_tanh` are 87.8 and 81.9 us, the `zip` loop is 57 us and
  `chunks_exact` is 120 us. A debug-build benchmark has the wrong ranking, not only the wrong
  numbers. Profile with `--release`.
- **LTO and `codegen-units`**: no difference for any of these kernels. Every function they call in
  another crate (`OnePole::next`, the oversamplers' inner functions) is small and marked
  `#[inline]`, and a loop that stays inside one crate does not cross a codegen unit. So this
  benchmark shows that these settings are *not needed* for such code and does not show what they buy
  for code that is not `#[inline]` and is called per sample across crates. `lto = "thin"` and
  `codegen-units = 1` are what the workspace ships; measure your own plugin with the harness (it is
  two environment variables) before changing them.
- **`target-cpu`**: no difference on this machine, because NEON is part of the baseline `aarch64`
  target. On `x86_64` the baseline is SSE2, and `-C target-cpu=x86-64-v3` allows AVX2: it can widen
  the vectors. That was not measured here (no x86 hardware, and Rosetta does not run AVX). A plugin
  built for distribution should pick a conservative target level, not `native`, which produces a
  binary that crashes with an illegal instruction on an older customer machine.
- **`#[inline]`**: put it on small functions that are called from the audio loop in *other* crates
  (the SDK's DSP primitives carry it, so a per-sample `Biquad::process_sample` or `OnePole::next`
  inlines into your loop). Do not put it on big functions. This guide has no before/after for
  `#[inline]` itself, so it makes no speed claim for it.

## 8. Fast approximations of `tanh`, `exp` and `sin`

The standard-library `tanh`, `exp` and `sin` on `f32` are calls into the system math library. They
are accurate to the last bit or two and they are not vectorised. An approximation written as plain
arithmetic is inlined and vectorised. Four are in
[`approx.rs`](../examples/perf_guide/src/approx.rs): `fast_tanh` (a [7/6] rational), `fast_tanh_cheap`
(a [3/2] rational), `fast_exp` (range reduction plus a degree-6 polynomial) and `fast_sin`
(range reduction plus an odd polynomial to `x^11`). They are plain Rust, no intrinsics.

Maximum error, measured on a dense grid against the `f64` function at the same `f32` input
(`cargo run --release -p example-perf-guide -- approx`; asserted by
[`tests/approx_error.rs`](../examples/perf_guide/tests/approx_error.rs)):

| Function | Domain | Max absolute error | Max relative error | Standard `f32` function, same domain |
| --- | --- | ---: | ---: | ---: |
| `fast_tanh` | -12 to 12 | 9.6e-5 (at the clamp, x = -4.97) | n/a | 3.7e-8 |
| `fast_tanh_cheap` | -12 to 12 | 2.4e-2 | n/a | 3.7e-8 |
| `fast_exp` | -80 to 80 | n/a | 4.1e-6 | 6.0e-8 |
| `fast_sin` | -pi to pi | 1.9e-7 | n/a | |
| `fast_sin` | -100 to 100 | 5.4e-6 (grows with `|x|`) | n/a | 3.2e-8 |

Behaviour outside the domain is defined and tested (`the_approximations_keep_their_shape_outside_the_accurate_range`):
`fast_tanh` and `fast_tanh_cheap` clamp and saturate to exactly +-1; `fast_exp` clamps its input to
[-87, 88], so it never returns infinity or a subnormal; `fast_sin` is odd and exact at 0. None of
them repairs a NaN: guard the input.

Speed (`Map { f, pre }` applies the function to every sample of a block of noise; two channels):

```rust
// before                                  // after
*s = (*s * 4.0).tanh();                    *s = fast_tanh(*s * 4.0);
```

| Function | std (us/block) | fast (us/block) | Speed-up | Max error you accept |
| --- | ---: | ---: | ---: | --- |
| `tanh` | 47.63 | 5.42 (`fast_tanh`), 4.00 (`fast_tanh_cheap`) | 8.8x, 11.9x | 9.6e-5, 2.4e-2 |
| `exp` | 13.83 | 5.83 | 2.4x | 4.1e-6 relative |
| `sin` | 59.08 | 5.21 | 11.3x | 1.9e-7 (one turn) |

Rules of use:

- A saturator's error of 1e-4 (-80 dB) in the shape of the curve is inaudible against the harmonics
  it creates, so a drive stage can use `fast_tanh`. If the curve is part of your plugin's *definition* (a null test
  against another build, a calibrated meter), use the standard function and pay for it.
- Coefficients (computed per block or per sub-block, section 4) should use the standard functions:
  you pay for them rarely, and a filter's pole position is sensitive to a 1e-5 relative error where a
  waveshaper is not.
- Keep a phase wrapped to one turn. `fast_sin` of a phase that grows without bound loses accuracy as
  `|x|` grows (the table: 5.4e-6 at 100 rad). An oscillator that accumulates a wrapped phase pays no
  such loss. [`fast_sin`'s reduction](../examples/perf_guide/src/approx.rs) is a convenience, not a
  licence to feed it time in seconds times 2 pi.
- The approximations above are for shaping, not for oscillators whose pitch matters: a 2e-7 sine
  error is -134 dB, which is fine, and a pitch error is not an amplitude error at all.

## 9. Oversampling costs

Oversampling a non-linear stage by 2 or 4 pushes its aliasing above the audible band, at a price. The
price has three parts: the non-linear stage runs 2 or 4 times as often, the two filters (up and
down) cost per *oversampled* sample, and the filters add latency that the plugin must report.

[`Sat1x`, `SatIir2x`, `SatLinearPhase`](../examples/perf_guide/src/kernels.rs): the same
`tanh(4x)` saturator at 1x, through the SDK's IIR half-band `Oversampler2x`, and through the SDK's
linear-phase `LinearPhaseOversampler` at 2x and 4x (64 samples of latency). Stereo, a 4096-frame block
(85.3 ms of audio):

| Case | us/block | ns/frame | Share of one core, per instance |
| --- | ---: | ---: | ---: |
| 1x | 47.54 | 11.6 | 0.06 % |
| 2x IIR half-band (`Oversampler2x`) | 315.46 | 77.0 | 0.37 % |
| 2x linear phase | 2827.63 | 690.3 | 3.3 % |
| 4x linear phase | 10006.33 | 2443.0 | 11.7 % |

- 2x through the IIR pair costs 6.6 times the 1x saturator; 2x linear phase costs 59 times.
  Oversampling is where plugins spend their CPU. Measure it before you ship it as a default.
- The linear-phase resampler is dominated by its filter, not by the saturator: it convolves with a
  long windowed sinc at every oversampled sample. Choose it when phase linearity is the requirement
  (a mastering chain, a parallel path that must null) and offer the IIR path or no oversampling as the
  default; consider making the factor a parameter with 1x as the default, as most plugins do.
- Oversample only the non-linear stage, not the whole plugin. Filters that run at the host rate stay
  at the host rate.
- An alternative that costs nothing in latency: anti-derivative anti-aliasing for a few memoryless
  curves, `Adaa2Taylor` and `ResidualAdaa1` in `openplugin-dsp` ([`adaa.rs`](../crates/openplugin-dsp/src/adaa.rs)).
  It is not benchmarked in this guide.
- Report the added latency (`Plugin::latency`) and keep a dry/bypass path aligned with it
  (`SoftBypass`). The kit's `latency` check verifies that the number does not change while processing;
  `verify_latency_by_impulse` verifies it is true.

## 10. `f32` or `f64`

Two different questions.

**Buffers and per-sample maps: `f32`.** The host hands you `f32`. A loop in `f32` fits twice as many
lanes in a vector register and moves half as many bytes. The measured cost of doing the same
`clamp(x*c+0.1)` in `f64` and rounding back was 3.1 times (section 7: 3.46 against 1.13 us/block).

**State of a recursive filter: `f64` when it matters, and it costs nothing.** The same transposed
direct form II low-pass (1 kHz, Q 0.707, [`Tdf2F32` and `Tdf2F64`](../examples/perf_guide/src/kernels.rs)):

| Case | us/block |
| --- | ---: |
| TDF2 low-pass, `f32` state | 35.04 |
| TDF2 low-pass, `f64` state | 35.54 |

A recursion is a dependency chain: each sample waits for the last, so the filter runs at the chain's
latency, and `f64` has the same latency as `f32`. There is nothing to vectorise across samples, so
`f64` loses nothing. It *gains* precision, which for a low cutoff is the difference between a working
and a broken filter: [STABILITY.md](STABILITY.md#f64-state-for-low-cutoffs) measures a 20 Hz and a 5 Hz
low-pass at 192 kHz, where `f32` state settles to 0.9986 and 0.14 instead of 1. Use `f64` state in
filters, integrators and phase accumulators, and `f32` for everything that is a map over a buffer.
(Test: `f32_and_f64_state_agree_for_a_one_kilohertz_low_pass` shows the two agree where `f32` is
adequate.)

## 11. Checklist

Before you ship a plugin:

1. `conformance(...)` passes in `--release` and in debug with `Options::quick()`.
2. `bench` has a stored baseline (`--out`), and CI runs `--baseline`.
3. `Plugin::prepare` allocates; `process` does not.
4. Coefficients are computed per block or sub-block, not per sample, and not when nothing changed.
5. A smoother that has settled costs nothing.
6. Loops slice to `frames` first and use iterators.
7. State that decays is flushed, or the kit's `no_denormals` check is in the tests.
8. Oversampling is a measured choice.
9. You have looked at p99 and max, not only the median.
