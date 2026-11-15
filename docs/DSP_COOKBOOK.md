# DSP cookbook

Short recipes for using the building blocks in `openplugin-dsp` inside a plugin. Each recipe is written to be pasted into a `Plugin` implementation: set up in `prepare`, keep the work in `process`, and allocate nothing in the audio path. Every block is written from textbooks and papers, and its module documentation names the source.

## Dither

Dither is the last stage of a plugin. Everything before it runs in floating point; dither is the one step that turns the float signal into a shorter word, so nothing that follows may change the signal's level or add gain. Use it when the host is going to write 16 bit (or any fixed word) from your output, or when the plugin's job is a deliberate bit-crusher.

```rust
use openplugin_dsp::{Dither, DitherKind, NoiseShaping};

struct MyPlugin {
    // ... the rest of the plugin ...
    dither: Dither,
}

impl MyPlugin {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        // Allocates here, never in process. The seed makes renders repeatable.
        self.dither = Dither::new(channels, sample_rate, 16, 0x5EED);
        self.dither.set_kind(DitherKind::Tpdf);
        self.dither.set_shaping(NoiseShaping::Perceptual);
    }

    fn reset(&mut self) {
        self.dither.reset();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        // ... all other processing first ...
        for (channel, buf) in io.iter_mut().enumerate() {
            self.dither.process(channel, &mut buf[..frames]);
        }
    }
}
```

Choices:

- `DitherKind::Tpdf` is the default and the right answer for output. `Rpdf` is cheaper in noise (total error power one third lower) but leaves noise that follows the signal level. `None` only rounds, and exists to show what dither prevents.
- `NoiseShaping::None` keeps flat noise at `Δ²/4` (`Δ = 2^(1-bits)`). `FirstOrder` pushes noise up in frequency. `Perceptual` is an order 9 curve designed at the session rate from the threshold of hearing; it puts a dip of about 18 dB around 3.5 kHz and raises the noise above 10 kHz. Total noise power rises, so use it only when the target is 16 bit or shorter and the audience is listeners. At a high host rate the shaped noise sits mostly above the audible band.
- Silence stays silent: after `set_blank_after` samples (default 4096) of exact `0.0` the output is exact zero, and the dither resumes on the first non-zero sample.
- To write integers yourself use `process_i16`, `process_i24` or `process_i32`, which dither and convert in one pass. A plugin whose host is float should not dither at all unless it is a deliberate effect.

## Convolution

`openplugin_dsp::partitioned` plays impulses of seconds (reverbs, cabinets, rooms) with no added latency. `BlockConvolver` stays for kernels of up to 1024 taps ([CONVOLVE.md](CONVOLVE.md)).

### What you get

- `ConvolverConfig`: layout, stage sizes and the longest impulse. It fixes every buffer size, so it is chosen at prepare time.
- `LongConvolver`: the running state (input history, frequency-domain delay lines). `build()` allocates; `process_mono`, `process_stereo` and `process` do not.
- `ConvolutionIr`: the prepared impulse, one FFT per partition. `build_ir()` allocates and does the heavy work, so run it on a worker thread. It is `Send`.
- Layouts: `Mono` (one impulse), `Stereo` (two impulses, `[left, right]`, channels independent), `TrueStereo` (four impulses `[LL, LR, RL, RR]`; `XY` is from input `X` to output `Y`, so `out_L = in_L*LL + in_R*RL`).
- Latency is 0. `Plugin::latency` returns 0 for this block.

### How it is cut up

The first `N1` taps run in direct form on every sample. The rest is cut into partitions, small first. With the default stage sizes `[64, 512, 4096]`:

| taps | partition size | FFT size |
|---|---|---|
| 0 to 63 | direct form | none |
| 64 to 511 | 64 | 128 |
| 512 to 4095 | 512 | 1024 |
| 4096 to the end | 4096 | 8192 |

`ConvolverConfig::uniform(layout, 64, max_len)` is the single-size version. Measured on an Apple Silicon laptop (release build, `tests/long_convolution_bench.rs`, mono, 3 s impulse at 48 kHz, host block 64): 4 us per block on average (0.3 % of the 1333 us budget) against 67 us for the uniform scheme, with the worst block at 60 us. A 10 s impulse at 192 kHz costs 16 us per 64-sample block on average (5 % of the budget), worst block 110 us. True stereo costs about three times the mono figure.

Everything runs on the audio thread. The large partitions do their transforms once per 4096 samples, so one block in 64 costs more than the rest (the multiply-adds are spread over the preceding block, the transforms are not). The worst block is therefore many times the average, about 15 times in the 48 kHz case above, but still 4.5 % of its deadline. If a very long impulse at 192 kHz makes the worst block too high for you, use a smaller last size (`with_stages(&[64, 512, 2048])`). Offloading the large stages to a background thread is not done here.

### A reverb plugin

The parameter plane and the audio thread never touch the impulse data directly. The UI thread picks a file, a `Loader` job decodes and transforms it, and `process` swaps the result in.

```rust
use openplugin_core::worker::{loader, Loader, Slot};
use openplugin_dsp::partitioned::{ConvolutionIr, ConvolverConfig, Layout, LongConvolver};

const MAX_TAPS: usize = 6 * 48_000; // six seconds at 48 kHz

struct Reverb {
    config: ConvolverConfig,
    convolver: LongConvolver,
    ir: Slot<ConvolutionIr>,
    loader: Loader<ConvolutionIr>,
}

impl Reverb {
    /// Prepare time. Allocates.
    fn new() -> Self {
        let config = ConvolverConfig::new(Layout::Stereo, MAX_TAPS);
        let convolver = config.build().unwrap();
        // Until a file is loaded the plugin passes audio through (a unit impulse).
        let unit = config.build_ir(&[&[1.0], &[1.0]]).unwrap();
        let (loader, ir) = loader(unit, 2);
        Self { config, convolver, ir, loader }
    }

    /// UI thread: a new impulse arrived (decoded already, left and right).
    fn load(&mut self, left: Vec<f32>, right: Vec<f32>) {
        let config = self.config.clone();
        // The FFTs run on the worker thread. Err means too many jobs are queued.
        let _ = self.loader.submit(move || {
            config.build_ir(&[&left, &right]).expect("impulse within max_len")
        });
    }

    /// Audio thread. No allocation, no lock.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.ir.poll(); // swaps in a finished impulse, if any
        self.convolver.process_stereo(self.ir.get(), left, right);
    }
}
```

Notes for a real plugin:

- Impulses longer than `MAX_TAPS` are refused by `build_ir` (`ConvolveError::TooLong`). Trim them in the job, or size `MAX_TAPS` from the longest file you ship.
- Memory is about 16 bytes per tap per impulse in the `ConvolutionIr`, plus the same order again in the convolver's delay lines. A 10 s stereo impulse at 192 kHz is about 60 MB of impulse data.
- Swapping an impulse keeps the input history, so there is no gap. The output of the block that crosses the swap mixes old and new partitions. Fade the wet signal for a few milliseconds if that is audible.
- After a transport stop, call `convolver.reset()` (it does not allocate) so the old tail does not play.
- A sample rate change needs a new impulse: resample it with `plan_rate`/`effective_rate` and call `load` again.
- Input NaN and infinity are replaced by zero inside the convolver, and denormals are flushed, so a bad block cannot leave a lasting artefact in the delay lines.
- For a dry/wet mix, keep the dry signal in your own buffer. The convolver processes in place and adds no latency, so no delay compensation is needed.

### Sources

Gardner, "Efficient convolution without input-output delay", JAES 43(3), 1995. Soo and Pang, "Multidelay block frequency domain adaptive filter", IEEE Trans. ASSP 38(2), 1990. Wefers, "Partitioned convolution algorithms for real-time auralization", 2015.

## Dynamics

Sources: D. Giannoulis, M. Massberg, J. D. Reiss, "Digital Dynamic Range Compressor Design - A Tutorial and Analysis", J. Audio Eng. Soc. 60(6), 2012 (envelope followers, gain computer, soft knee); Recommendation ITU-R BS.1770-4 Annex 2 (true peak).

The pieces, from small to large:

| Block | What it is |
| --- | --- |
| `Ballistics` | Attack / release / hold smoother for any scalar. `Topology::Branching` (classic) or `Decoupled`. |
| `EnvelopeFollower` | `Detection::Peak` or `Rms`, smoothed in `Domain::Linear` or `Domain::Log`; output in dB. |
| `GainComputer` | Static curve in dB: compressor, expander, gate; soft knee, ratio up to infinity, range, gate hysteresis. |
| `Dynamics` | Any number of channels, one linked detector, one gain for all of them; sidechain input and filter hook. |
| `Limiter` | Lookahead brickwall with a true-peak ceiling and a reported latency. |
| `TruePeak` | The 4x (BS.1770 structure) or 8x detector the limiter uses, usable on its own for a meter. |

Time constants are true time constants: after `attack_ms` a step has covered 63.2 % of its way (tested to 3 %).

### A stereo compressor

```rust
use openplugin_dsp::{Dynamics, DynamicsParams, Smoothing};

// prepare():
let mut comp = Dynamics::new(sample_rate);
comp.set_params(DynamicsParams {
    threshold_db: -18.0,
    ratio: 4.0,          // f32::INFINITY limits
    knee_db: 6.0,        // 0 = hard knee
    attack_ms: 10.0,
    release_ms: 120.0,
    makeup_db: 3.0,
    smoothing: Smoothing::Gain, // ballistics on the gain, in dB (the paper's "smooth" design)
    ..DynamicsParams::default()
});

// process(): left and right get the same gain, so the image does not move.
comp.process(&mut [left, right]);          // left, right: &mut [f32]
let gr = comp.gain_reduction_db();         // for a meter
```

`Link::Max` (default) follows the loudest channel; `Link::Average` follows the mean. The same `Dynamics` works for mono or 5.1/7.1: it keeps no per-channel state. For independent channels, use one `Dynamics` per channel.

Change settings between blocks with `set_params`. The ballistics state survives unless `mode`, `domain` or `smoothing` changes.

### A gate with hysteresis, hold and range

```rust
use openplugin_dsp::{Dynamics, DynamicsMode, DynamicsParams, Smoothing};

let mut gate = Dynamics::new(sample_rate);
gate.set_params(DynamicsParams {
    mode: DynamicsMode::Gate,
    threshold_db: -40.0,   // opens at -40 dB
    hysteresis_db: 6.0,    // and stays open down to -46 dB: no chatter
    range_db: 60.0,        // closed = -60 dB, not silence
    attack_ms: 0.5,        // opening
    hold_ms: 50.0,         // stays open this long after the level last exceeded the output
    release_ms: 120.0,     // closing
    knee_db: 0.0,
    smoothing: Smoothing::Gain,
    ..DynamicsParams::default()
});
```

`DynamicsMode::Expander` is the same with a finite `ratio`: below the threshold the output falls `ratio` dB per dB. In `Smoothing::Gain`, attack means "gain moving toward open" for an expander or gate and "toward more attenuation" for a compressor.

### Sidechain input and sidechain filter

An external key replaces the channels in the detector:

```rust
comp.process_sidechain(&mut [left, right], &[key_left, key_right]);
```

The filter hook is a closure called for every detector sample before it is rectified and linked. It receives the channel index so that each channel can have its own filter state. Plug in any `f32 -> f32` filter: a biquad, a state-variable filter, a crossover band, or your own.

```rust
// A 100 Hz high-pass so bass does not pump the compressor.
// `hp` is one filter per channel; use the crate's SVF / biquad, or any f32 -> f32.
comp.process_filtered(&mut [left, right], None, |ch, x| hp[ch].process(x));

// De-esser: band-pass the key, compress the full signal.
comp.process_filtered(&mut [left, right], None, |ch, x| band[ch].process(x));

// External key, filtered:
comp.process_filtered(&mut [left, right], Some(&[key_l, key_r]), |ch, x| hp[ch].process(x));
```

Only the detector hears the filtered signal; the audio is untouched. Zero-phase is not needed for a detector, but a filter that delays by many samples shifts when the compressor reacts.

Own channel loop? `Dynamics::next_gain(detector_sample)` takes the already-linked, filtered, rectified detector value and returns the linear gain.

### Peak versus RMS, and where to smooth

`detection: Detection::Rms` with `rms_ms` (10 to 50 ms is usual) reads the program level and reacts to loudness; `Peak` reacts to the crest and suits limiting-style jobs. `Smoothing::Level` smooths the detected level before the gain computer (this is what the older `Compressor` does); `Smoothing::Gain` smooths the gain in dB afterwards, which gives release times that do not depend on how far over the threshold the signal is. `Topology::Decoupled` makes attack and release independent of each other and the output never overshoots a peak.

### Static curve for a plot

```rust
use openplugin_dsp::{DynamicsMode, GainComputer};
let curve = GainComputer::new(DynamicsMode::Compressor, -20.0, 4.0, 6.0);
let out_db = curve.static_output_db(-10.0);   // -17.5 with a hard knee, a little higher with this knee
```

### A limiter with a true-peak ceiling

```rust
use openplugin_dsp::Limiter;

// prepare() -- allocates:
let mut limiter = Limiter::prepare(sample_rate, channels, 1.5 /* lookahead ms */, 80.0 /* release ms */);
limiter.set_ceiling_db(-1.0);                  // dBTP
let latency = limiter.latency_samples();       // return this from Plugin::latency

// process(): in place, any block length.
limiter.process(&mut [left, right]);
let gr = limiter.gain_reduction_db();
```

How it works: each frame, every channel is measured with an 8x interpolated true-peak detector (the BS.1770-4 Annex 2 idea, a longer filter; see `true_peak.rs`), the largest value across channels sets one gain for all channels, a sliding minimum over the lookahead makes the gain arrive before the peak, and a moving average over the lookahead turns the corner into linear ramps, so the gain never clicks and mathematically cannot exceed what the peak requires. The audio is delayed by the lookahead plus the detector's delay (`latency_samples()`), and the gain recovers exponentially with the release time.

What the ceiling means, measured against an independent 16x interpolator in `tests/dynamics.rs`:

| Signal | Excess over the ceiling |
| --- | --- |
| fs/4 sine at 45 degrees, bursts of it | below 0.002 dB |
| noise band-limited to 0.4 fs, square-ish bursts, sweep | below 0.03 dB |
| sines at 0.43 fs | 0.05 dB |
| hard (not band-limited) square wave | 0.07 dB |
| full-band white noise | 1.7 dB |

Full-band white noise is the one weak case: it carries energy right up to Nyquist, where an ideal brick-wall reconstruction and any short detector filter disagree. A real converter's reconstruction filter rolls off before Nyquist and sees less. If you need a margin on such material, set the ceiling lower.

`set_true_peak(false)` limits sample peaks only: cheaper, but intersample peaks (an fs/4 sine at 45 degrees is 3 dB over) pass through.

Return the limiter's latency from `Plugin::latency`, and run a `SoftBypass` or `Delta` with the same figure so that bypass and the dry path line up.

### Using the pieces directly

```rust
use openplugin_dsp::{EnvelopeFollower, Detection, Domain};
let mut env = EnvelopeFollower::new(sample_rate);
env.set_detection(Detection::Rms);
env.set_domain(Domain::Log);
env.set_attack_ms(5.0);
env.set_release_ms(200.0);
let level_db = env.process(sample);       // smoothed level in dB
let raw_db = env.detect(sample);          // level before the ballistics (the RMS window still applies)
```

## Filters

Pick by what you need to do with the cutoff.

| Need | Use |
| --- | --- |
| Cutoff, resonance or gain moves while audio runs (envelopes, LFOs, a knob being dragged, per-sample modulation) | `Svf` |
| A static EQ band that must still match its analog curve near Nyquist (high bell, high shelf, low-pass at 18 kHz and 44.1 kHz) | `MatchedBiquad` |
| The classic RBJ cookbook EQ, unchanged | `Biquad` |
| Band split for multiband processing | `Crossover` (`LinkwitzRiley` for one split) |
| Steep, maximally flat low/high-pass (6 to 48 dB/octave) | `Butterworth` |
| Gentle 6 dB/octave tone control, smoothing | `OnePoleFilter` |
| Remove DC offset | `DcBlocker` |
| Phase shaping without changing magnitude | `Allpass1`, `Allpass2` |

All of them process `f32` and keep `f64` state. Parameter setters are
`configure` (SVF, matched) or `set` (the rest); a NaN or infinite parameter is
ignored and the previous setting kept, out-of-range values are clamped.

### State-variable filter (modulation-safe)

`Svf` is the zero-delay-feedback trapezoidal SVF (Zavalishin, *The Art of VA
Filter Design*; Simper's Cytomic technical paper). Modes: `LowPass`,
`HighPass`, `BandPass` (0 dB peak), `Notch`, `Peak`, `AllPass`, `Bell`,
`LowShelf`, `HighShelf`. It is the bilinear transform of the analog prototype,
pre-warped so the corner lands on `freq`, and the coefficients may change on
every sample.

```rust
use openplugin_dsp::{Svf, SvfMode};

let mut filter = Svf::new();
for x in block.iter_mut() {
    // cutoff follows a smoothed parameter, every sample
    filter.configure(SvfMode::LowPass, sample_rate, smoothed_cutoff.next(target), q, 0.0);
    *x = filter.process_sample(*x);
}
```

`process_split` returns `(low, band, high)` at once; they add up to the input.
Proven in `crates/openplugin-dsp/tests/filters.rs`: the response of every mode
equals the pre-warped analog prototype to 0.004 dB (44.1k, 48k, 96k, 192k, 8k),
stays within 0.5 dB of the raw prototype up to fs/16, and 6 seconds of noise
through per-sample random cutoff, Q and gain changes stays finite (peak
output 39 for unit noise at Q up to 20).

### Analog-matched biquad (no cramping)

`MatchedBiquad` follows Vicanek, *Matched Second Order Digital Filters* (2016):
impulse-invariant poles and a numerator fitted to the analog magnitude at DC,
`f0` and Nyquist. The parameters mean the same as for `Biquad`, so it drops in:

```rust
use openplugin_dsp::{MatchedBiquad, MatchedKind};

let mut bell = MatchedBiquad::new();
bell.configure(MatchedKind::Peaking, sample_rate, 15_000.0, 1.0, 9.0);
```

What it buys (measured, worst dB error against the analog prototype where that
is above -12 dB; the RBJ figure is the same measurement on `Biquad`):

| Shape | `f0` up to | Matched | RBJ |
| --- | --- | --- | --- |
| Peaking | 0.4 fs, Q 0.5 to 8 | 4.7 dB (mostly under 2.5 dB) | up to 11 dB |
| Low-pass | 0.4 fs, Q 0.5 to 8 | 1.2 dB | stops at a zero at Nyquist, over 100 dB error |
| High-pass, Q 0.5 to 0.707 | 0.4 fs | 2.1 dB | up to 15 dB |
| Shelves, Q up to 0.707 | 0.25 fs | 1.7 dB | up to 2.6 dB |
| Band-pass, Q 2 to 8 | 0.1 fs | under 2 dB | similar |
| Notch | 0.1 fs | under 1.2 dB, exact null at `f0` | similar |

Limits, stated plainly: the band-pass is zero at DC and Nyquist like any
`[b0, 0, -b0]` form, high-Q high-passes and shelves with `f0` near 0.4 fs
are no better than RBJ, and the filter is a direct-form biquad, so it is
not the one to modulate every sample (a runaway beyond +-1e6 resets it). The
curve depends only on `f0 / fs`, so the same figures hold at every rate.
`magnitude_at(rate, hz)` evaluates the curve from the coefficients, handy for
an on-screen response plot.

### Butterworth, one-pole, DC blocker, all-pass

```rust
use openplugin_dsp::{Butterworth, ButterworthKind, DcBlocker};

let mut hp = Butterworth::new();
hp.set(ButterworthKind::HighPass, 4, sample_rate, 80.0); // order 1..=8
let mut dc = DcBlocker::new(sample_rate, 20.0);
```

Odd orders add one `OnePoleFilter`; each pair of poles is an `Svf` section
with the Butterworth Q, so the cutoff can be moved at any time. Measured
against `1/sqrt(1 + W^(2N))` (W the pre-warped frequency ratio): within 0.2 %
for orders 1 to 8, exactly -3.01 dB at the cutoff. `DcBlocker` is a bilinear
first-order high-pass (-3 dB at its corner, 20 Hz by default) and removes a
constant 0.7 to under 1e-6 within four seconds. `Allpass1` (-90 degrees at
`freq`) and `Allpass2` (-180 degrees, sharper with Q) are unity gain to
0.1 %.

### Crossovers (Linkwitz-Riley)

```rust
use openplugin_dsp::{Crossover, LrOrder};

let mut xo = Crossover::new(LrOrder::Lr4, 3, sample_rate, &[200.0, 3_000.0]);
let [low, mid, high, _] = xo.process_sample(x);
// later, per block or per sample:
xo.set_frequencies(sample_rate, &[250.0, 3_000.0]);
```

`LrOrder::{Lr2, Lr4, Lr8}` (12, 24, 48 dB/octave); 2, 3 or 4 bands, frequencies
ascending. The bands add up to an all-pass of the input, so the magnitude of
the sum is flat. A band below a later split passes through that split's
all-pass (its own low plus high) so the phase matches the bands above. Measured:
the sum is within 0.01 dB from 20 Hz to 20 kHz for every order and band count at
44.1k, 48k, 96k and 192k, and two-band edges sit at -6.02 dB.

Two things to know. An LR2 high band is polarity-inverted (`low - high` is the
all-pass; `low + high` has a null at the crossover), so sum LR2 bands as the
module returns them. Multiband processing between split and sum should treat
each band as a separate path; the group delay differs per band by design, which
is what the all-pass compensation equalises.

### Robustness rules every filter here follows

- A NaN or infinite input sample is treated as `0.0` and never reaches the
  state, so one bad sample from a host cannot lock a filter up. The filter then
  behaves exactly as if it had been given silence for that sample (tested).
- A result outside the `f32` range (only from enormous finite input) resets the
  state and outputs `0.0`.
- State and output below 1e-30 are flushed to zero: tails reach exactly zero
  and never become subnormal (tested over 1 000 000 samples).
- Nothing allocates, including the `configure`/`set` calls
  (`crates/openplugin-dsp/tests/filters_no_alloc.rs` counts allocations with a
  global allocator while every filter runs and is modulated).

## Spectral

Three pieces: windows with known gain and bandwidth (`WindowKind`), an STFT frame processor with overlap-add (`Stft`), and a lock-free spectrum feed for an editor (`SpectrumAnalyzer`, `spectrum_feed`). Sources: Harris, "On the use of windows for harmonic analysis with the DFT" (Proc. IEEE, 1978) for the windows; Allen (1977) and Griffin and Lim (1984) for analysis, modification and weighted overlap-add; Boehm (2012) for the seqlock.

A spectral effect that zeroes every bin above 5 kHz, with its latency reported to the host:

```rust
use openplugin_dsp::{Complex, Stft, StftConfig};

struct Lowpass {
    stft: Stft,          // built in prepare, never in process
    cutoff_bin: usize,
}

impl Lowpass {
    fn prepare(sample_rate: f32, channels: usize) -> Self {
        let stft = Stft::new(channels, StftConfig::hann(2048)).unwrap(); // hop 512, exact COLA
        Self { cutoff_bin: (5000.0 * 2048.0 / sample_rate) as usize, stft }
    }
}

impl Plugin for Lowpass {
    fn latency(&self) -> u32 {
        self.stft.latency() as u32 // 2048 samples
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let cut = self.cutoff_bin;
        for (ch, buf) in io.iter_mut().enumerate() {
            let mut zero_above = |_: usize, bins: &mut [Complex<f32>]| {
                for b in &mut bins[cut..] {
                    *b = Complex::new(0.0, 0.0);
                }
            };
            self.stft.process_in_place(ch, &mut buf[..frames], &mut zero_above);
        }
    }
}
```

Choices:

- Pick the window and hop so that `analysis * synthesis` sums to a constant. `StftConfig::hann` (Hann analysis, rectangular synthesis, hop a quarter of the frame) is exact. Periodic Hann and Hamming are exact at any hop of at most half the frame, four-term Blackman-Harris at a quarter or less, square-root Hann on both sides at half. `check_ola(frame, hop, analysis, synthesis)` returns the ripple (`(max - min) / mean` of the summed product over one hop); `Stft::ola_error()` reports it for what you built. A bad pair (Hann at hop 3/4 of the frame, Blackman-Harris at half, any Kaiser) shows a ripple above 1e-2 and the output is amplitude modulated at the hop rate. `Stft` divides by the mean, so the gain is one, but it cannot remove ripple.
- Latency is `frame` samples whatever the host block size, including 1. Report it with `Plugin::latency` and give a bypass the same delay (`SoftBypass`). A smaller hop costs more FFTs, not more latency; a smaller frame costs resolution.
- The callback sees bins `0..=frame/2`, bin `k` at `k * sample_rate / frame`, and the conjugate half is rebuilt for you. Changing a bin's magnitude without care for its neighbours leaves time-domain aliasing in the result; smooth gain curves, and keep fades short compared to the frame.
- The transform is a complex FFT of the real frame (not half-length real packing), so cost is that of a `frame`-point complex transform and its inverse per hop and channel. Bin phase is relative to the oldest sample of the frame.
- Analysing instead of processing: `SpectrumAnalyzer::new` returns the analyser and a `SpectrumReader`. Feed the analyser from `process`; it publishes a frame every hop. Level is calibrated: a full-scale sine centred on a bin reads 0 dB, whatever the window. A tone between bins reads low by the window's scalloping loss (Hann up to 1.4 dB); flat-top reads within 0.05 dB, at the price of a main lobe about four bins wide. Use Hann or Blackman-Harris for a general display and flat-top to read a level.
- The editor, on its own thread, calls `reader.read(&mut mags, &mut peaks)` on each paint. It returns the frame number, or `None` before the first frame or if the writer overtook the read 64 times; skip that paint. `log_frequency_bins` turns the linear bins into display columns on a logarithmic axis between two frequencies (a column that spans several bins shows their maximum). Smoothing is `average_ms`; the peak trace holds `peak_hold_ms` then falls at `peak_fall_db_per_s`.
- The audio side never waits for the editor: `publish` is relaxed stores between two sequence updates. A reader costs nothing to the audio thread, and any number can exist (`SpectrumReader` is `Clone`).

## Modulation and delay

Chorus, flanger and phaser are a delay line or a string of allpass filters whose time or break frequency is moved by a low-frequency oscillator. The blocks are `ModDelay` (the fractional delay line), `Lfo`, `Chorus`, `Flanger` and `Phaser`. Each allocates in `new` or `prepare` only, takes the real sample rate, treats a non-finite input sample as `0.0`, flushes values below `1e-30` and clamps every parameter, so a bad value from a host cannot panic or leave NaN in the state.

```rust
use openplugin_core::Transport;
use openplugin_dsp::{Chorus, Feel, LfoRate, LfoShape, NoteDivision};

struct MyChorus {
    chorus: Chorus,
}

impl MyChorus {
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.chorus.prepare(channels, sample_rate); // allocates here, nowhere else
        self.chorus.set_voices(4);
        self.chorus.set_delay_ms(18.0);
        self.chorus.set_depth_ms(3.0);
        self.chorus.set_mix(0.5);
        self.chorus.set_shape(LfoShape::Sine);
        // Free-running at 0.8 Hz, or one cycle per dotted quarter note:
        self.chorus.set_rate(LfoRate::Sync(NoteDivision::new(4, Feel::Dotted)));
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.chorus.set_transport(transport); // once per block, before process
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for (channel, buf) in io.iter_mut().enumerate() {
            self.chorus.process(channel, &mut buf[..frames]);
        }
    }
}
```

`Flanger::new(channels, fs)` and `Phaser::new(channels, fs)` have the same shape: `prepare`, setters, `set_transport`, `process(channel, block)`. Stereo (or wider) is one object with one state per channel, and the LFOs of later channels are offset by up to half a cycle (`set_stereo_spread`).

### Fractional delay

A delay that is not a whole number of samples needs an interpolator. `ModDelay::new(sample_rate, max_delay_ms, Interpolation::...)` sizes the buffer once; `process(x, delay_samples)` takes a delay that may change on every sample. Delay 0 is the sample just written, and each interpolator has a smallest delay (`min_delay()`): linear 0, Hermite 1, Lagrange order `N` `max(N/2 - 0.5, 0)`, Thiran order `N` `N - 0.5`. A request outside `[min_delay, max_delay]` (or NaN) is clamped.

The kinds follow Laakso, Valimaki, Karjalainen and Laine, "Splitting the Unit Delay", IEEE Signal Processing Magazine 13(1), 1996:

| Kind | Cost | What it does well | Where it falls short |
| --- | --- | --- | --- |
| `Linear` | 2 taps | cheapest | low-pass that gets deep at fraction 0.5 (-3 dB at fs/4) |
| `Hermite` | 4 taps | magnitude much flatter than linear (-1.1 dB at fs/4, fraction 0.5), smooth under a sweep | delay is exactly linear's: accurate at low frequency only |
| `Lagrange(N)` | `N+1` taps | delay accurate to high order at low frequency, exact at DC | magnitude falls toward Nyquist |
| `Thiran(N)` | recursive | magnitude exactly 1, delay exact at DC to order `N` | stateful: only for fixed or slowly changing delay |

Measured delay error (phase delay minus the requested delay, in samples, for a delay of 12.25), from the coefficient theory, which the tests also check against the impulse response of a running `ModDelay` to better than 2e-4 samples:

| Kind | 0.05 pi (1.2 kHz at 48 kHz) | 0.1 pi (2.4 kHz) | 0.25 pi (6 kHz) | 0.5 pi (12 kHz) |
| --- | --- | --- | --- | --- |
| Linear / Hermite | -0.00039 | -0.00155 | -0.01002 | -0.04517 |
| Lagrange 3 | -0.00000 | -0.00003 | -0.00123 | -0.01780 |
| Lagrange 5 | -0.00000 | -0.00000 | -0.00016 | -0.00770 |
| Thiran 1 | -0.00144 | -0.00571 | -0.03339 | -0.10911 |
| Thiran 2 | -0.00001 | -0.00013 | -0.00457 | -0.05121 |
| Thiran 3 | -0.00000 | -0.00000 | -0.00063 | -0.02454 |

`phase_delay`, `group_delay` and `Interpolation::frequency_response` give these numbers for any delay and frequency, so a plugin can show them or pick a kind by its own tolerance (`OPENPLUGIN_TABLE=1 cargo test -p openplugin-dsp --test modulation print_error_table -- --nocapture` prints the full table).

For a swept delay (chorus, flanger, vibrato) use Hermite or Lagrange 3. A sine of 1 kHz through a delay swept 5 ms deep at 2 Hz has a largest sample-to-sample step of 0.1390 against a bound of 0.1433 (the tone's slope times one plus the delay's slope), for linear, Hermite and both Lagrange orders: no click. A Thiran filter keeps state that depends on its coefficients, so it stays clean only while the delay moves inside one integer interval (0.1308 against 0.1440 in the test); across integer crossings it jumps (0.2988) and `Chorus` and `Flanger` replace it with Hermite. Use Thiran for tuning a waveguide or a comb to an exact pitch.

A jump in a delay *parameter* is a different problem from a sweep: `Glide` is a one-pole smoother for a delay in samples. A 100 to 600 sample jump on a 500 Hz tone gives a step of 1.06 without it and 0.066 with a 20 ms glide.

### LFO and the host transport

`Lfo` has sine, triangle, saw, square, sample-and-hold and smoothed random shapes, all bipolar and starting at phase 0. `next()` returns the value then advances; `phase_offset` shifts it without disturbing the lock. The random shapes are a function of the cycle number (`splitmix64` of seed and cycle), so a given cycle always gives the same value.

`LfoRate::Hz(f)` free-runs. `LfoRate::Sync(NoteDivision::new(8, Feel::Dotted))` is one cycle per dotted eighth; `LfoRate::Quarters(8.0)` is one cycle per two 4/4 bars. A synced LFO is locked to `Transport::quarter_notes`: call `set_transport` once per block and, while the host plays, the LFO position becomes `quarter_notes / quarters_per_cycle`. It is a function of musical position, not a counter, so a loop, a locate or a tempo change leaves it exactly where the host says, with no drift to correct. Between blocks it advances at `tempo / 60 / length` per second. While the host is stopped it keeps running at the last good tempo and re-locks on play. Tempo 0 (the `Transport` default), NaN or a non-finite position are ignored.

Measured: 400 blocks of 512 samples at 44.1 kHz with a one-quarter-note sine, including a loop back to 0, a locate to quarter note 1234.37 and tempo changes (120, 97.5, 211 BPM): the largest difference between the LFO and `sin(2 pi * host_position)` is 6e-8. A synced random LFO reaches the same value by two different routes (a jump, versus playing there).

### Flanger and chorus

The flanger is a comb filter: `|1 + g e^{-j w d}|` has notches at odd multiples of `fs / (2 d)`; with `d = 48` samples at 48 kHz the tests find them at 500, 1500, 2500 Hz and so on, with depths below -80 dB, and 1.0000 at the peaks. `mix` runs from -1 to 1 and the deepest comb is at +-0.5; a negative mix inverts the wet path and puts a notch at DC. Feedback adds peaks (the loop delay equals the direct delay exactly). `set_through_zero(true)` delays the dry path by `delay_ms` (the plugin must report `latency_samples()`) so the swept path can cross it; with negative mix the two cancel at the crossing: the test sweeps a 90 Hz tone and the output energy falls to 0.0014 of the input's at the crossing.

The chorus sweeps up to eight voices, spaced evenly in LFO phase, and divides the wet sum by the square root of the count. The pitch shift of a swept delay is the slope of the delay: a 1 ms deep sweep at 1 Hz moves a 2 kHz tone by +-12.6 Hz (the theory is 12.57 Hz).

### Phaser

Each stage is the first-order allpass `(a + z^-1) / (1 + a z^-1)`, `a = (tan(pi f / fs) - 1) / (tan(pi f / fs) + 1)`, with phase `-2 atan(tan(w / 2) / tan(pi f / fs))`. Mixed with the dry signal, `N` stages cancel where the total phase is an odd multiple of -180 degrees, at

```text
tan(w_k / 2) = tan(pi f / fs) * tan((2k + 1) pi / (2N))      for 2k + 1 < N
```

`Phaser::notch_frequencies` evaluates this. For 4 to 12 stages, break frequencies 300 Hz, 1 kHz and 2.5 kHz and rates 44.1, 48 and 96 kHz, every notch lands within 2 cents of the prediction and is deeper than -60 dB. Feedback (up to 0.95, either sign) adds a resonance beside each notch; the feedback is one sample late so there is no delay-free loop. The break frequency is swept on a log scale between `set_range`'s limits.

### Limits

- Delay and depth are limited to 100 ms (chorus) and 50 ms (flanger).
- The LFO runs at no more than half the sample rate.
- There is one coefficient computation (a `tan`) per phaser sample per channel; if that matters, drive the phaser from a control-rate sweep in your own wrapper.
- Hermite and Lagrange change the delay of high frequencies, as the table shows; Thiran changes none of the magnitude but is not for sweeps.
