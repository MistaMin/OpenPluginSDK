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

## Utility

`openplugin_dsp::utility` holds the small blocks every plugin needs. They are plain functions or small structs; nothing allocates.

```rust
use openplugin_dsp::utility::*;
use openplugin_dsp::{Gain, PanLaw, Panner};

struct MyPlugin {
    gain: Gain,
    panner: Panner,
    width: f32,
}

impl MyPlugin {
    fn prepare(&mut self, sample_rate: f32) {
        self.gain = Gain::new(sample_rate, 0.0);     // 10 ms one-pole smoothing, core OnePole
        self.panner = Panner::new(sample_rate, PanLaw::Minus4p5);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // Flush denormals for this call only; the host's mode comes back on drop.
        let _guard = DenormalGuard::new();
        stereo_width_in_place(left, right, self.width);   // 0 = mono, 1 = unchanged, 2 = double side
        ms_encode_in_place(left, right);                  // left = mid, right = side
        // ... process mid and side separately ...
        ms_decode_in_place(left, right);
        self.panner.balance(left, right);                 // unity at centre
        self.gain.process_stereo(left, right);            // set_db(x) glides, jump_db(x) snaps
    }
}
```

Choices:

- Mid/side is `M = (L + R) / 2`, `S = (L - R) / 2`; decode is `L = M + S`, `R = M - S`. Mono has `S = 0` and keeps its level. The round trip is bit-exact when the sums are exact, which is almost always.
- Pan laws (`PanLaw`): `ConstantPower` (-3.01 dB at centre, `cos`/`sin`), `Minus4p5` (centre exactly -4.5 dB), `Linear` and `Minus6` (centre -6.02 dB, `1 - x` and `x`). All are 0 dB on the side the signal is panned to. `pan_mono` takes a mono source to two outputs; `balance` takes a stereo source, leaves the near channel at unity and turns the far one down, so centre does nothing.
- `Gain` and `Panner` smooth with the core one-pole. Call `set_db` / `set_pan` from the parameter, never recompute coefficients per sample yourself. `db_to_gain(-120.0)` and below is exactly zero.
- `DenormalGuard` sets flush-to-zero (aarch64 `FPCR.FZ`; x86_64 `MXCSR` FTZ and DAZ) with a few instructions and restores the previous setting when dropped. Make one at the top of `process`. It is `!Send` on purpose: the setting belongs to one thread. On other targets it does nothing (`guard.active()` says so). A feedback loop that decays into denormals returns exactly zero with the guard and keeps the denormal slow path without it.

## Metering

All meters take blocks, allocate in `new` only, and treat NaN and infinity as silence.

### Loudness (EBU Mode)

```rust
use openplugin_dsp::{EbuMeter, EbuScale, Target};
use openplugin_dsp::downmix::roles_of;
use openplugin_core::SpeakerLayout;

// prepare: roles come from the bus layout, never from channel indices.
let roles = roles_of(SpeakerLayout::Surround51).unwrap();
let mut meter = EbuMeter::new(sample_rate, roles);   // -23 LUFS target, +9 scale
meter.set_scale(EbuScale::Plus18);
meter.set_target(Target::ATSC_A85);

// process: one slice per channel.
meter.push(&[l, r, c, lfe, ls, rs]);
let (m, s, i) = (meter.momentary_lu(), meter.short_term_lu(), meter.integrated_lu()); // LU re target
let (m_abs, i_abs) = (meter.momentary_lufs(), meter.integrated_lufs());               // LUFS
let (max_m, max_s) = (meter.max_momentary_lufs(), meter.max_short_term_lufs());
let range = meter.loudness_range_lu();
let tp = meter.max_true_peak_dbtp();
meter.set_running(false);   // stand-by: M, S and maxima follow, I and LRA hold
meter.reset();              // integrated, LRA and true peak together
```

- Weights follow BS.1770-4 by role: L, R, C at 1.0; Ls, Rs at 1.41; LFE left out. Side and back surrounds and the 6.1 back centre take 1.41, heights 1.0: those two are choices, since BS.1770-4 does not give them (see `loudness.rs`). `Loudness::new` is stereo at weight 1 as before.
- Momentary (400 ms) and short-term (3 s) are not gated and are evaluated every 10 ms, so maxima are exact to 10 ms. Integrated is gated at -70 LUFS and -10 LU. The range is EBU Tech 3342: 3 s values every 100 ms, gates at -70 LUFS and -20 LU, 95th minus 10th percentile, in 0.1 LU bins. The meter shows the range as unsettled for the first 60 s (`range_settled`).
- Targets are `Target::EBU_R128` (-23 LUFS +/- 0.5 LU, -1 dBTP) and `Target::ATSC_A85` (-24 LKFS +/- 2 dB, -2 dBTP). Streaming targets are not listed because their values could not be checked.
- Proven in `tests/ebu_conformance.rs` against Tech 3341 v4 cases 1 to 6 and 9 to 14 (+/-0.1) and Tech 3342 v4 cases 1 to 4 (+/-1 LU); the authentic-programme cases (3341 cases 7 and 8, 3342 cases 5 and 6) need the EBU files and are not covered.

### True peak, PPM, K-system, VU

```rust
use openplugin_dsp::{KMeter, KScale, Ppm, PpmType, TruePeakMeter, VuIec};

let mut tp = TruePeakMeter::new(sample_rate, 2);     // 4x below 96 kHz, 2x below 192 kHz
tp.set_hold_ms(2000.0);
tp.process(0, left); tp.process(1, right);
let (held, max) = (tp.held_dbtp(0), tp.max_dbtp(0));

let mut ppm = Ppm::new(sample_rate, PpmType::Ebu);   // Ebu, Din, Nordic, Bbc
ppm.process(left);
let shown = PpmType::Ebu.display(ppm.level_dbfs(), -18.0);   // number on the EBU scale

let mut k = KMeter::new(sample_rate, KScale::K20, 2);
k.process(0, left);
let reading = k.rms_k(0);                         // 0 at -20 dBFS (AES17 RMS), +20 at full-scale sine

let mut vu = VuIec::new(sample_rate);             // 300 ms to 99 %, ~1.3 % overshoot
vu.process(left);
```

- `PpmType` carries the integration and return time of each IEC 60268-10 type (Din and Nordic 5 ms, Ebu and Bbc 10 ms; 20 dB in 1.7 s, BBC 24 dB in 2.8 s) and the scale mapping. Measured bursts against the standard's table are in the `ppm.rs` documentation; the model is one stage and reads within about 1.5 dB of the table everywhere and within 0.4 dB at the defining points.
- The K-meter reads RMS over 600 ms with a +3 dB offset so a full-scale sine reads 0 dBFS; zones are green to 0, yellow to +4, red above (`KZone`).
- Use `VuIec` where the 300 ms rise matters; the older `VuMeter` is a one-pole and takes about 1.4 s to reach 99 %.

### Correlation and goniometer

```rust
use openplugin_dsp::scope_feed;

// prepare (any thread): the writer goes to the audio thread, the reader to the editor.
let (mut writer, reader) = scope_feed(sample_rate);

// audio thread
writer.push(left, right);          // no allocation, never waits for the editor

// editor thread, on a timer (about 60 Hz)
let frame = reader.read();         // 256 points (x = side, y = mid), correlation, balance, sequence
if frame.sequence != last_seen { /* redraw */ }
```

The two halves share one core `Seqlock`: the writer does two atomic increments and a 2 KB copy per frame, so a slow or stalled editor costs the audio thread nothing; a reader retries only while a publish is in progress, and an editor that stops reading just misses frames. Correlation is +1 for mono, 0 for uncorrelated and -1 for out of phase, smoothed over 300 ms.

## Reverb

Three blocks, all wet-only (they output the reverb, not the dry signal: mix them with the dry path yourself, or run them on a send), stereo in and stereo out, allocated in `new`, free of allocation, denormals and NaN afterwards (a non-finite input sample counts as zero).

- `FdnReverb` is a feedback delay network after Jot and Chaigne (AES 1991): 8 or 16 delay lines of prime length (so mutually prime), a Householder or Hadamard feedback matrix, and per-line absorption filters set from the decay time at three frequencies by Jot's rule (a line of `m` samples gets gain `10^(-3 m / (fs RT60))`). Smooth, dense, and the decay times you ask for are the ones you measure.
- `PlateReverb` is Dattorro's plate (JAES 1997), written from the paper's topology with its delay lengths rescaled from 29761 Hz to the real rate. Brighter and more diffuse, with the characteristic fast build-up; one decay time plus damping.
- `EarlyReflections` is a tapped delay with panned taps and five built-in patterns (`SmallRoom`, `LargeRoom`, `Hall`, `Dense`, `Slap`) or your own list of up to 32 taps. `FdnReverb` carries one (`early_mut()`, level starts at 0); it also works alone, or in front of a `PlateReverb`.

```rust
use openplugin_dsp::{EarlyPattern, FdnMatrix, FdnReverb};

struct MyReverb {
    reverb: FdnReverb,
    mix: f32,
}

impl MyReverb {
    fn prepare(&mut self, sample_rate: f32) {
        // Allocates here, never in process.
        self.reverb = FdnReverb::new(sample_rate, 16);
        self.reverb.set_matrix(FdnMatrix::Hadamard);
        // -60 dB times at 125 Hz, 1 kHz and 8 kHz.
        self.reverb.set_rt60(2.8, 2.0, 1.1);
        self.reverb.set_size(1.0);
        self.reverb.set_predelay_ms(20.0);
        self.reverb.set_modulation(0.25, 0.8); // gentle; 0.0 turns it off
        self.reverb.set_width(1.0);
        let early = self.reverb.early_mut();
        early.set_pattern(EarlyPattern::LargeRoom);
        early.set_level(0.4);
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let [l, r, ..] = io else { return };
        for i in 0..frames {
            let (wl, wr) = self.reverb.process_frame(l[i], r[i]);
            l[i] += self.mix * wl; // dry plus wet
            r[i] += self.mix * wr;
        }
    }

    // Tell the host how long the sound goes on after the input stops.
    fn tail(&self) -> openplugin_core::Tail {
        if self.reverb.is_frozen() {
            openplugin_core::Tail::Infinite
        } else {
            openplugin_core::Tail::Finite(self.reverb.tail_samples() as u32)
        }
    }
}
```

`process(&mut left, &mut right)` does the same over two slices, replacing them with the wet signal. The plate has the same shape: `PlateReverb::new(sample_rate)`, `set_rt60(seconds)`, `set_damping_hz`, `set_bandwidth_hz`, `set_predelay_ms`, `set_modulation`, `set_width`, `set_freeze`.

Choices and limits:

- Decay times are clamped (FDN 0.05 to 600 s, plate 0.1 to 120 s) and a reverb is stable at every setting: the loop gain is held below one for any combination of times, sizes and frequencies. `set_freeze(true)` fades the input out over about 10 ms, removes damping and sets every band to 10 000 s, so the tail holds with bounded level; report `Tail::Infinite` while frozen.
- Measured decay (Schroeder backward integration, -5 to -35 dB fit, octave bands, 44.1, 48 and 96 kHz): the FDN is within 3.1 % of the setting at 125 Hz, 1 kHz and 8 kHz, with and without modulation, 8 or 16 lines, either matrix; the plate within 3.6 % at 1 s, 2.5 s and 5 s. The numbers are in `tests/reverb_proof.rs`. Settings far apart between neighbouring bands (for example 4 s next to 0.4 s) can be 5 to 7 % off at the band between, because the absorption is made of first-order shelves.
- `set_reference_hz` moves the three frequencies at which the FDN times hold (default 125, 1000 and 8000 Hz).
- `set_size` scales the FDN delay lengths (0.2 to 4) and, by Jot's rule, does not change the decay time; the lines glide to the new lengths (at most a 2 % pitch bend), so a size knob does not click. `set_predelay_ms` glides the same way.
- Modulation exists to break up fixed modes (metallic ringing). It uses allpass-interpolated reads, which keep unit gain at all frequencies, so it does not shorten the high band. Keep depth at or below 0.5 for a natural tail; at 0 the output is exactly repeatable.
- Level: the FDN's wet level grows with decay time, as in a real room. Set the mix by ear and leave headroom.
- `set_sample_rate` (FDN) reallocates and clears the tail. Do not call it from `process`.

## Synthesis

The synthesis blocks make an instrument: band-limited oscillators, wavetables, envelopes, noise, a voice allocator and a sinc resampler for samples. Every one takes the real sample rate, allocates nothing once built, and treats NaN or a negative frequency as zero. `crates/openplugin-dsp/tests/synth_conformance.rs` is a complete eight-voice synth made of them (saw, square, pulse, triangle, pink noise, glide, mono/legato, sustain pedal) that passes `openplugin-testing`'s conformance kit.

### Band-limited oscillators

```rust
use openplugin_dsp::{BandlimitedOsc, BlWave, BlepOrder};

struct Voice { osc: BandlimitedOsc }

impl Voice {
    fn new() -> Self {
        // Order Four is the default: 2 samples of latency, 20 dB or more below the naive saw's aliasing at every pitch.
        Self { osc: BandlimitedOsc::new(BlWave::Pulse).with_order(BlepOrder::Four) }
    }

    fn render(&mut self, hz: f32, width: f32, sample_rate: f32, out: &mut [f32]) {
        for v in out {
            self.osc.set_pulse_width(width); // may change every sample (PWM)
            *v = self.osc.next(hz, sample_rate);
        }
    }
}
```

Hard sync: the master's `last_wrap()` is the fraction of the sample at which it wrapped; give it to the slave.

```rust
let mut master = BandlimitedOsc::new(BlWave::Saw);
let mut slave = BandlimitedOsc::new(BlWave::Saw);
// per sample; sweep slave_hz for the classic sync sound:
master.next(master_hz, sample_rate);
let y = slave.next_sync(slave_hz, sample_rate, master.last_wrap());
```

How it works: the naive waveform is corrected at each step (PolyBLEP, Välimäki and Huovilainen 2007) and each slope change (PolyBLAMP, for the triangle) by the residual of a band-limited step, a polynomial of the distance in samples (Välimäki, Pekonen and Nam 2012). The kernels are B-splines of width 2, 4, 6 or 8 samples (`BlepOrder`), checked in the tests against an independent recursion. The output is `latency()` samples late (1 to 4); that is what lets the correction reach the samples before a step and what makes sync and PWM exact between samples.

Choices:

- Aliased power relative to the fundamental, measured with coherent FFTs at 44.1 kHz (`tests/synth_proof.rs`, dB; each wider order buys about 9 dB):

  | wave, f0 | naive | order 2 | order 4 | order 6 | order 8 | wavetable |
  | --- | --- | --- | --- | --- | --- | --- |
  | saw 100 Hz | -23.4 | -39.5 | -49.4 | -58.6 | -67.5 | -90.8 |
  | saw 1 kHz | -13.5 | -30.2 | -40.5 | -50.2 | -59.5 | -135.0 |
  | saw 5 kHz | -6.5 | -23.2 | -33.9 | -44.1 | -54.1 | -144.3 |
  | square 2 kHz | -13.8 | -32.4 | -44.6 | -56.2 | -67.7 | |
  | square 5 kHz | -9.1 | -23.9 | -34.1 | -44.1 | -54.2 | |
  | triangle 1 kHz | -48.1 | -60.4 | -70.1 | -79.3 | -88.3 | |
  | triangle 5 kHz | -26.3 | -38.0 | -48.0 | -58.1 | -68.1 | |

  Hard sync (500 Hz master, slave 2.71x): -1.8 dB naive, -27.6 dB band-limited at order 4. The sync jump is the weakest case; use a wider order or oversample for bright sync leads.
- PolyBLEP lowers aliasing, it does not remove it. For a clean saw at the top of the range pick order 6 or 8, or use the wavetable oscillator.
- DC and level are the waveform's: saw, square and triangle have zero mean, a pulse has mean `2 w - 1`, peaks are 1.0 within the BLEP overshoot.

### Wavetables

```rust
use openplugin_core::worker::{loader, Loader, Slot};
use openplugin_dsp::{WavetableOsc, WavetableSet};

struct Synth {
    jobs: Loader<WavetableSet>,
    table: Slot<WavetableSet>,
    osc: WavetableOsc,
}

impl Synth {
    fn new() -> Self {
        // The audio thread starts with a cheap sine.
        let (jobs, table) = loader(WavetableSet::sine(), 2);
        Self { jobs, table, osc: WavetableOsc::new() }
    }

    /// Main thread: building runs FFTs and allocates, so it goes to the worker.
    fn load_cycle(&mut self, cycle: Vec<f32>) {
        let _ = self.jobs.submit(move || {
            WavetableSet::from_cycle(&cycle, 2048).unwrap_or_else(|_| WavetableSet::sine())
        });
    }

    fn process(&mut self, hz: f32, sample_rate: f32, out: &mut [f32]) {
        self.table.poll(); // swaps in a finished table without a lock or an allocation
        for v in out {
            *v = self.osc.next(self.table.get(), hz, sample_rate);
        }
    }
}
```

A `WavetableSet` holds one octave level per halving of the harmonic count (256, 128, ... 1 for 2048 points); the oscillator reads the level with the most harmonics that stay below Nyquist at the current `hz / sample_rate`, with cubic interpolation. Tables are sample-rate independent, so the same set serves 44.1 kHz and 384 kHz. `from_harmonics`, `saw` and `square` build a set from a spectrum. The measured floor is in the table above.

### Envelopes

```rust
use openplugin_dsp::{Adsr, Curve, Gate, RetriggerMode};

let mut env = Adsr::new(sample_rate);
env.set_ahdsr(0.005, 0.0, 0.15, 0.6, 0.4); // attack, hold, decay (s), sustain level, release (s); hold 0 is an ADSR
env.set_curves(Curve::Linear, Curve::Exponential, Curve::Exponential);
env.set_mode(RetriggerMode::Legato); // a gate_on while held is ignored

// In process: gate events with block offsets, applied at their sample.
env.process(&mut level[..frames], &[(0, Gate::On), (200, Gate::Off)]);
```

Choices:

- Segments are whole samples (`round(seconds * sample_rate)`) and reach their target on the last one: the peak is sample `attack`, the sustain level is sample `attack + hold + decay`, and the level is exactly 0.0 `release` samples after `gate_off`. The tests check this at five sample rates and both curve shapes.
- The exponential shape is the RC curve normalised to end on target, so a long release never lingers near zero. `set_curvature` (default 5) sets how bent it is.
- A retrigger attacks from the current level, so there is no click; the attack time is scaled by the distance left to climb.
- The sustain level is read each sample while sustaining. If it is automated, pass it through a smoother.

### Noise

```rust
use openplugin_dsp::{PinkNoise, WhiteNoise};

let mut white = WhiteNoise::new(0x5EED);            // uniform in [-1, 1), repeatable
let mut pink = PinkNoise::new(sample_rate, 0x5EED); // RMS 0.2, power falls 3 dB per octave
pink.set_sample_rate(new_rate);                      // in prepare, if the rate changes
let sample = white.next() * 0.5 + pink.next();
```

Pink noise is white noise through a pole/zero ladder whose poles and zeros are recomputed for the real sample rate (a generalisation of Paul Kellet's filter, whose coefficients are fixed for 44.1 kHz). Measured over 10 Hz to 100 kHz at 384 kHz (four decades): slope -2.99 dB per octave, no half-octave band more than about 0.5 dB off the fit; 10 Hz to 25 kHz at 96 kHz: -2.98 dB per octave. The slope holds to about 0.3 of the sample rate and then bends down toward Nyquist. Below 4 Hz the spectrum levels off, so there is no drift.

### Voice allocation

```rust
use openplugin_dsp::{Adsr, BandlimitedOsc, GlideMode, StealPolicy, VoiceAllocator};

#[derive(Default)]
struct SynthVoice { osc: Option<BandlimitedOsc>, env: Option<Adsr> }

let mut voices = VoiceAllocator::<SynthVoice, 8>::new(sample_rate);
voices.set_policy(StealPolicy::Quietest);
voices.set_glide(0.08, GlideMode::LegatoOnly);

// note_on(offset, note, velocity):
if let Some(a) = voices.note_on(note, velocity) {
    let v = voices.voice_mut(a.voice).unwrap();
    if a.retrigger { /* v.state.env: gate_on() */ }
}
// note_off:
if let Some(i) = voices.note_off(note).released { /* gate_off on voice i's envelope */ }
// per sample, per active voice: v.advance(1); let hz = v.hz();
// when a released voice's envelope is idle: voices.free(i);
// once per block, for StealPolicy::Quietest: voices.set_level(i, envelope_level * velocity);
```

Choices:

- Stealing order: the same held note retriggers its voice; then a free voice; then a voice already in its release tail; then a held voice, picked by the policy: `Oldest`, `Quietest` (lowest level set with `set_level`), or `SameNote` (also reuses a tail of the same note, else the oldest).
- `VoiceMode::Mono` and `Legato` use voice 0 with a held-key stack and last-note priority: letting go of the top key returns to the older one. Mono retriggers the envelope on each change; Legato only when no key was held (`Allocation::retrigger`).
- Glide is linear in semitones and takes exactly `glide * sample_rate` samples (rounded) whatever the interval, and lands on the note.
- The sustain pedal (`set_sustain_pedal`) defers releases; it and `all_notes_off` call back with each voice to release, so nothing allocates.
- The per-voice state `S` is yours (oscillators, envelope, filter) and is never touched by the allocator.

### Sample playback

```rust
use openplugin_dsp::{playback_ratio, Quality, SincKernel, SincPlayer};

// prepare (allocates the table once; share it between voices):
let kernel = SincKernel::new(Quality::High);
// note on:
let mut player = SincPlayer::new();
player.start(playback_ratio(root_hz, note_hz, file_sample_rate, host_rate));
player.set_loop(Some((loop_start, loop_end))); // or None for a one-shot
// per sample:
let y = player.next(&kernel, &sample_data);
// pitch bend: player.set_ratio(new_ratio);
```

The kernel is a Kaiser-windowed sinc stored as a table of 256 to 2048 phases per input sample (a polyphase decomposition) with linear interpolation between phases. Reading up in pitch (ratio above 1) stretches the kernel so material that would fold above the new Nyquist is filtered first; the ratio is limited to 1/64 to 16. Weights are normalised, so a constant reads back exactly.

| `Quality` | taps | passband edge (cycles/sample) | ripple | stopband (measured) |
| --- | --- | --- | --- | --- |
| `Low` | 16 | 0.27 | 0.016 dB | -59.1 dB |
| `Medium` | 32 | 0.34 | 0.0015 dB | -80.0 dB |
| `High` | 64 | 0.40 | 0.0002 dB | -99.9 dB |
| `Best` | 128 | 0.44 | 0.00002 dB | -118.9 dB |

Folding test (a 0.30 cycles/sample tone read at ratio 1.7 would land above Nyquist): the residue is -60.4, -84.4, -101.3 and -132.3 dB for the four qualities, and a tone that stays in band keeps unit gain to within 0.14 % (`Low`) or better. `SincConfig` sets the taps, attenuation and phases by hand.

## Neural models

`openplugin_dsp::neural` runs small neural networks for amp and pedal models, one sample at a time: `Dense` (identity, tanh, ReLU, sigmoid), `Gru`, `Lstm` and a causal dilated `Conv1d`, chained by a `Model` with input size 1 (audio) or 2 (audio and a conditioning value such as a gain knob) and output size 1, with an optional residual (`output + input`). The equations are written from the papers: Cho et al. 2014 (GRU, reset gate before the recurrent matrix; frameworks that apply it after, as cuDNN does, compute a different function), Hochreiter and Schmidhuber 1997 with the forget gate of Gers, Schmidhuber and Cummins 2000 (LSTM, no peepholes), van den Oord et al. 2016 (dilated causal convolution; the gated activation inside a WaveNet block is not modelled). It is a separate implementation, not a port of any existing amp-model runtime. The exact formulas, the limits and the JSON format are in the module docs.

### Load off the audio thread, run on it

```rust
use openplugin_core::worker::loader;
use openplugin_dsp::neural::Model;

// constructor: the audio side starts with no model
let (mut loader, mut slot) = loader::<Option<Model>>(None, 2);
// UI thread, when the user picks a file (parse + allocate happen on the worker):
let text = std::fs::read_to_string(path)?;
loader.submit(move || Model::from_json(&text).ok())?;
// audio thread, top of process:
slot.poll();
if let Some(model) = slot.get_mut() {
    for s in buf.iter_mut() { *s = model.process_sample(*s, drive); }
}
```

`Model::from_json` returns a `ModelError` that names the field (`layers[0].w_h has 26 values, expected 27`). Call `model.reset()` on transport stop or a parameter jump. `process_sample` and `process_block` do not allocate (a counting allocator test proves it) and treat NaN or infinite input as 0.

### JSON (version 1)

```json
{ "version": 1, "sample_rate": 48000, "input_size": 1, "residual": true,
  "layers": [
    { "type": "gru",   "in": 1, "hidden": 2, "w_x": [6 numbers], "w_h": [12], "b": [6] },
    { "type": "dense", "in": 2, "out": 1, "activation": "identity", "weights": [2], "bias": [1] } ] }
```

Matrices are flat, row-major, one row per output unit. GRU row blocks are z, r, n; LSTM blocks are i, f, g, o; a `conv1d` weight is `out x kernel x in`, tap 0 (the current sample) first. Unknown or missing fields, NaN or infinite numbers, wrong lengths, sizes over 512 (kernel 64, dilation 4096, 4 million weights, 64 MiB of text) and layers that do not chain are errors. A complete example is `crates/openplugin-dsp/tests/data/tiny_model.json`.

### Sample rate

A model is trained at one rate (`Model::sample_rate()`), and its memory is counted in samples. `rate_ratio(host)` is `host / trained`. When it is not 1, resample to the trained rate (`LinearPhaseOversampler` for integer ratios, `SincKernel` otherwise), run the model, and resample back; the model never resamples, and the resampler's latency is yours to report. Running a 48 kHz model at 96 kHz unchanged halves every time constant and shifts the sound up.

### Silence and DC

A model with non-zero biases outputs a small DC level for silent input, like the trained amp. A plugin that must be silent for silence (the conformance kit's reset check demands it) should train without biases or high-pass the output. Recurrent states and the output are flushed to zero below 1e-30, so decay never reaches denormals.

### Proof and cost

* Output against an independent plain-`f64` reference of the same equations, written in the test from the JSON alone: ten stacks (Dense with all four activations, GRU, LSTM, Conv1d with dilation, hidden sizes 1 to 64, two-input conditioning, residual), 10 000 samples each, worst absolute error 1.4e-7 (tolerance 1e-5).
* Single-unit GRU and LSTM with chosen weights, three steps against hand-computed values, and a convolution whose impulse response is its kernel at the dilated taps.
* Hostile files: wrong sizes, mismatched lengths, NaN and infinity spellings, `1e999`, sizes of 2^40, kernel and dilation over the limits, 100 000-deep nesting, all refused with an error and without allocating the declared size.
* `openplugin-testing` conformance kit on an LSTM amp with random weights loaded through `worker::loader`.

The matrix-vector product stores the weights transposed at load, so each product is `out[0..rows] += column * x[c]` over contiguous slices: pure vertical multiply-adds that the compiler vectorises (NEON here), with no horizontal sum per row. A first version that took one dot product per row (eight accumulators, then a horizontal reduction) was 1.8 times slower on LSTM 64 (1945 us per block against 1098 us).

CPU per 512-sample block at 48 kHz, one hidden layer plus a one-output dense layer, `--release`, best of 30 timings, Apple Silicon (`cargo test --release -p openplugin-dsp --test neural -- --ignored --nocapture`). Real-time fraction is time per block divided by the 10.67 ms the block lasts:

| layer | us / block | real-time fraction |
| --- | --- | --- |
| LSTM 16 | 176 | 0.017 |
| LSTM 32 | 407 | 0.038 |
| LSTM 64 | 1098 | 0.103 |
| GRU 16 | 155 | 0.015 |
| GRU 32 | 368 | 0.035 |
| GRU 64 | 817 | 0.077 |

So one LSTM 64 voice uses about a tenth of a core; a stereo plugin running two of them needs a fifth. `set_fast_activations(true)` swaps `tanh` for a [7/6] rational (error about 1e-4 at its clamp) and the logistic function built on it; on this machine it saves 0 to 9 percent (LSTM 64 1085 us, GRU 64 743 us), so the matrix products, not the activations, dominate and it is off by default. In the reference test its error is below 2e-3, but the activations in that test are small, where the rational is most accurate; do not read the 1e-7 it prints as its bound.

Not covered: peephole LSTMs, bidirectional or multi-layer cuDNN weight packing (export each layer separately), the gated blocks and skip sums inside WaveNet, batch norm, and f64 or quantised weights.

## FIR design and phase

All of this designs on a worker thread (it allocates); only the finished taps reach the audio thread. Sources: Oppenheim and Schafer, "Discrete-Time Signal Processing" (windowed sinc, Kaiser's formulas, the real cepstrum and minimum phase); Parks and Burrus, "Digital Filter Design" (least squares); Smith, "Spectral Audio Signal Processing" (spectral factorisation of a linear-phase design); Vaidyanathan, "Multirate Systems and Filter Banks" (half-band filters). Parks-McClellan (Remez) is not implemented.

### Windowed sinc and Kaiser

```rust
use openplugin_dsp::{design_kaiser, design_windowed, kaiser_attenuation_db, FirKind, WindowKind};

// 80 dB, 500 Hz transition, cutoff (-6 dB point) 6 kHz at 48 kHz: 483 taps, odd.
let taps = design_kaiser(FirKind::LowPass { cutoff_hz: 6000.0 }, 48_000.0, 80.0, 500.0)?;
// Any window from windows.rs, your own length (odd for HighPass / BandStop):
let hp = design_windowed(FirKind::HighPass { cutoff_hz: 80.0 }, 48_000.0, 1023, WindowKind::Blackman)?;
// From a ripple and attenuation pair (the stricter deviation rules):
let a = kaiser_attenuation_db(0.01 /* dB peak ripple */, 60.0);
```

Measured at 48 kHz (peak over the whole stopband; ripple over the whole passband):

| spec (A dB, transition) | Kaiser estimate | stopband peak | passband ripple | achieved transition |
| --- | --- | --- | --- | --- |
| 60 dB, 1000 Hz | 175 taps | -59.6 dB | 0.0087 dB | 1085 Hz |
| 80 dB, 500 Hz | 483 taps | -80.2 dB | 0.0009 dB | 521 Hz |
| 100 dB, 2000 Hz | 155 taps | -100.0 dB | 0.0001 dB | 2049 Hz |

The formula is an estimate: attenuation lands within 0.4 dB and the transition within 9 % of the request. Pad the spec by a dB if it is a hard limit. High-pass, band-pass and band-stop at 70 dB / 800 Hz come out at 261 taps and meet the stopband to within 2 dB of the request.

### Least squares

```rust
use openplugin_dsp::{design_least_squares, ls_grid, LsBand};
let bands = [LsBand::flat(0.0, 5000.0, 1.0, 1.0), LsBand::flat(7000.0, 24_000.0, 0.0, 1.0)];
let fit = design_least_squares(101, 48_000.0, &ls_grid(&bands, 48_000.0, 101))?;
// fit.taps, fit.rms_error, fit.condition_estimate
```

Bands may slope (`gain_lo` to `gain_hi`) and carry weights; arbitrary grids go in as `LsPoint`s. It solves the normal equations by Cholesky. With the grid covering the band, conditioning is about 5; leave a wide gap with many taps (801 taps, a 10 kHz gap) and the estimate rises to 3e5; a longer or wider gap fails with `FirError::Singular` rather than returning noise. Measured: 101 taps, 5 kHz / 7 kHz edges, RMS amplitude error 7.9e-5, against 9.0e-4 for a Kaiser design on the same measure; weighting the stopband by 100 lowers the stopband peak.

### Linear-phase EQ from a curve

```rust
use openplugin_dsp::{linear_phase_eq, linear_phase_latency, EqPoint, WindowKind};
let curve: [EqPoint; 4] = [(100.0, 0.0), (1000.0, 12.0), (3000.0, 0.0), (8000.0, -9.0)];
let taps = linear_phase_eq(&curve, 48_000.0, 2049, WindowKind::Kaiser { beta: 8.0 })?;
let latency = linear_phase_latency(taps.len()); // (N-1)/2 = 1024, report via Plugin::latency
```

The curve is linear in dB over log frequency and flat outside its ends. Taps are exactly symmetric; the measured group delay error is 5e-12 samples; the magnitude is within 0.15 dB of the curve at test points for 2049 taps. Resolution is limited by the length: about `4 fs / N` smoothing.

### Minimum-phase and mixed-phase EQ

```rust
use openplugin_dsp::{minimum_phase_eq, mixed_phase_eq, partial_energy};
let min = minimum_phase_eq(&curve, 48_000.0, 2049)?;       // almost no latency
let mix = mixed_phase_eq(&curve, 48_000.0, 2049, 0.5)?;    // half the pre-ringing, about half the delay
let kept = partial_energy(&min, 256);                       // share of energy in the first 256 taps
```

The cepstral method uses an FFT of at least 32 times the length; a 4x larger FFT changes the taps by 1.5e-8, so aliasing is not the limit. At 2049 taps against a linear-phase design of the same curve: magnitude within 0.02 dB of the target; group delay at 1 kHz 33 samples against 1024; 99.9965 % of the energy in the first 256 taps against 2.5e-10 for linear phase. Mixed phase at amount 1, 0.5, 0 has 3.8, 258 and 512 samples of delay at 400 Hz. Minimum phase changes the phase, so apply the same filter to both channels of a stereo pair, and expect phase differences against the dry signal in parallel mixes.

### Handing taps to a running convolver

```rust
use openplugin_core::worker::loader;
use openplugin_dsp::partitioned::{ConvolverConfig, Layout};

// prepare:
let config = ConvolverConfig::new(Layout::Mono, 4096);
let mut conv = config.build()?;                       // audio-side state
let (mut loader, mut slot) = loader(config.build_ir(&[&[1.0]])?, 4);

// UI / message thread, when the curve changes:
let cfg = config.clone();
loader.submit(move || cfg.build_ir(&[&minimum_phase_eq(&curve, 48_000.0, 1024).unwrap()]).unwrap())?;

// audio thread, every block:
slot.poll();                                          // picks up the new ConvolutionIr
conv.process_mono(slot.get(), block);
```

`tests/fir_swap_no_alloc.rs` runs this with a counting allocator: no allocation or free on the audio thread across the swap and the blocks after it, and the output equals direct convolution with the designed taps (max difference 6e-8). The swap switches the impulse between blocks, so a sharp change can click; crossfade two convolvers if the EQ moves fast. A linear-phase EQ adds `(N-1)/2` samples of latency.

### Minimum-phase half-band oversampling

```rust
use openplugin_dsp::MinPhaseHalfband;
let mut os = MinPhaseHalfband::design(44_100.0, 20_000.0, 80.0)?; // prepare, allocates
os.upsample_2x(&input, &mut doubled);       // same call shape as Oversampler2x
/* process at 2x */
os.downsample_2x(&doubled, &mut output);
```

It is a new type; `Oversampler2x` is unchanged. The prototype (Kaiser, linear phase, asked for `2A + 10` dB) is spectrally factored into a minimum-phase filter of about half its length. Measured at 44.1 kHz base rate:

| passband edge / rejection | taps (at 2x) | stopband (24.1 kHz and up) | passband ripple | group delay, DC to edge (base samples) | linear-phase equivalent |
| --- | --- | --- | --- | --- | --- |
| 18 kHz / 60 dB | 48 | -61.8 dB | 2e-6 dB | 0.97 to 2.56 | 10.0 |
| 20 kHz / 80 dB | 123 | -81.5 dB | below 1e-6 dB | 1.40 to 6.50 | 27.0 |
| 20 kHz / 100 dB | 153 | -101.0 dB | below 1e-6 dB | 1.72 to 7.89 | 34.5 |

Signal path at 80 dB: a 10 kHz tone upsamples to unity (1.0002) with its image at 34.1 kHz 78 dB down (the measurement window limits this reading; the filter response above is the reference), a 2x-rate tone that folds onto 10 kHz leaves -85 dB, and a 5 kHz round trip has unity gain. The delay is not constant, so the pair is not linear phase: use it where phase at the band edge does not matter (non-linear processing, most effects). Taps are `f32`, which bounds usable rejection to about 130 dB.

## Virtual analog

Zero-delay-feedback filters, wave digital filters and a tape model. Sources are in the module docs; where a paper's exact equation was not available the derivation is ours and says so. All are allocation-free, `f64` inside, treat a non-finite input as 0 and recover from it.

### Ladder, diode ladder and Korg-35 filters

```rust
use openplugin_dsp::{DiodeLadder, Korg35, Korg35Mode, TransistorLadder};

let mut ladder = TransistorLadder::new(sample_rate); // 4 poles, tanh per stage
ladder.set_cutoff(800.0);      // Hz; |H| = 1/(4-k) here
ladder.set_resonance(0.9);     // k = 4 r, self-oscillation at 1.0
ladder.set_drive(2.0);         // gain before the first tanh
ladder.set_poles(4);           // tap 1..=4
// per sample (cutoff and resonance may change every sample):
let y = ladder.process_sample(x);

let mut diode = DiodeLadder::new(sample_rate);   // k = r * 901/49, oscillates at 1.195 fc
let mut korg = Korg35::new(sample_rate);         // 2-pole Sallen-Key, K = 2 r
korg.set_mode(Korg35Mode::HighPass);
```

The transistor ladder places a `tanh` at the input of each stage and in the feedback sum, and solves the one unknown (the loop input) per sample by Newton's method inside a bracket that always contains the root, so it cannot diverge; the diode ladder solves its 4x4 system by damped Newton; the Korg-35 solves a scalar monotone equation. All three are capped (16, 16 and 12 iterations; typically 2 to 4).

Measured (tests in `crates/openplugin-dsp/tests/va_proof.rs`), small signal against the linear analog prototype with the bilinear frequency mapping (rates 44.1, 48, 96, 192 kHz, cutoffs 1 to 4 kHz, several resonances):

| filter | worst magnitude error | worst phase error |
| --- | --- | --- |
| transistor ladder, `1/((1+s)^4 + k)` | 0.0041 dB | below 1e-4 rad |
| diode ladder, `1/(s^4+7s^3+15s^2+10s+1+k)` | 0.0001 dB | |
| Korg-35 low-pass and high-pass, `1/(s^2+(2-K)s+1)` | 0.0002 dB | |

At 192 kHz the transistor ladder's resonance peak (k = 3.6) is at 973.3 Hz and 11.14 dB against 973.3 Hz and 11.14 dB for the unwarped analog prototype. Self-oscillation: below the critical resonance a kick decays to nothing (below 1e-8 after 3 s); above it a bounded oscillation grows (transistor ladder r 1.05: 996 Hz at 1 kHz cutoff, amplitude 0.08; diode ladder r 1.1: 1104 Hz, about 1.2 times the cutoff as derived; Korg-35 r 1.05: 1008 Hz). With white noise in and cutoff (20 Hz to 0.45 fs), resonance (0 to 1.25) and drive (0 to 8) random every sample for 60 000 samples at 44.1 and 192 kHz, the outputs stay finite and below 4.2; the ladders use at most 10 iterations.

### Wave digital filters and the diode clipper

```rust
use openplugin_dsp::{Capacitor, DiodePair, DiodeParams, Parallel, ResistiveVoltageSource, WdfPort};

let mut tree = Parallel::new(ResistiveVoltageSource::new(2200.0), Capacitor::new(10e-9, sample_rate));
let root = DiodePair::new(DiodeParams::D1N4148);
// per sample:
tree.a.set_voltage(f64::from(x));
let v = root.process(&mut tree);   // reflect up, solve the root, send down
// after changing a component: tree.a.set_resistance(..); tree.refresh();
```

`Series`, `Parallel` and `Inverter` are generic adaptors (adapted towards the root), with `Resistor`, `Capacitor`, `Inductor` and `ResistiveVoltageSource` as leaves, so a circuit is a type and the audio path has no allocation or dynamic dispatch. The root is a `DiodePair` (Werner et al. 2015 via `wright_omega`, plus two Newton steps on the exact `sinh` equation) or a `SingleDiode`. `DiodeClipper` is the finished example.

Against an independent trapezoidal implicit solve of `C dv/dt = (vs - v)/R - 2 Is sinh(v/nVt)` run at 3.07 MHz, maximum error as a share of the output peak (2.2 kohm, 10 nF, 1N4148):

| input | 48 kHz | 96 kHz | 192 kHz |
| --- | --- | --- | --- |
| 200 Hz, 0.3 V | 0.086 % | 0.020 % | 0.005 % |
| 1 kHz, 2 V | 1.9 % | 0.53 % | 0.17 % |
| 1 kHz, 8 V | 5.6 % | 2.8 % | 1.3 % |
| 3 kHz, 4 V | 10.6 % | 5.1 % | 2.2 % |

The remaining error is the bilinear capacitor meeting a clipping edge shorter than a sample, so it falls with the rate and a clipper wants oversampling. Against the same scheme at its own rate the WDF agrees to 3e-8 V, which shows the wave algebra and root are exact.

### Tape (Jiles-Atherton hysteresis)

```rust
use openplugin_dsp::{LinearPhaseOversampler, TapeHysteresis};

let mut os = LinearPhaseOversampler::prepare(4, 1, max_block)?;
let mut tape = TapeHysteresis::new(4.0 * sample_rate); // the model runs at the oversampled rate
tape.set_drive(3.0);       // input gain
tape.set_saturation(0.8);  // field scale, earlier knee
tape.set_width(0.5);       // pinning, loop width
os.process(&[input], &mut [&mut output[..]], |s| s[0] = tape.process_sample(s[0]))?;
// report os.latency_samples() as the plugin latency
```

The model is the Jiles-Atherton equations in normalised form, integrated in the field variable with fourth-order Runge-Kutta (substeps of at most 0.25 field units, 1 to 32), after Chowdhury (DAFx-19). Parameter values are our own, not the paper's; no head-gap or loss filter. Measured: at drive 1 the output against input is a closed loop (closure 1e-6) with remanent values of -0.233 and +0.233 at the input's zero crossings, and the loop area grows from 0.20 (width 0) to 0.53 (width 1). Harmonics grow with drive (odd harmonics dominate): THD 7.0 %, 9.6 %, 11.0 %, 13.3 %, 19.7 % at drives 0.25, 0.5, 1, 2, 4, and the output rms rises monotonically. Aliasing of a 7001 Hz sine at 48 kHz (energy outside the harmonics of the tone, relative to the fundamental): tape at drive 3, saturation 0.8: -17.8 dB at 1x and -43.2 dB with 4x `LinearPhaseOversampler`; transistor ladder at 12 kHz cutoff, drive 10: -14.9 dB and -41.1 dB.

## Physical modelling

Karplus-Strong strings, digital waveguides and modal banks make sound from a physical description instead of a table: a pitch and a decay time go in, a plucked, struck or blown tone comes out. All of them compute for the real sample rate, run in `f64`, never allocate after construction, flush denormals, ignore non-finite settings and read non-finite input as 0. Sources are in the module docs of `pluck.rs`, `waveguide.rs`, `modal.rs` and `phys_common.rs`; nothing is ported.

### Plucked string (Karplus-Strong, Jaffe-Smith)

```rust
use openplugin_dsp::{hz_from_midi, PluckedString, Tuning};

// prepare (allocates; one per voice):
let mut string = PluckedString::new(sample_rate, 25.0); // lowest pitch it must reach
string.set_stretch(0.1);       // Jaffe-Smith S: 0.5 is the original average, near 0 stays bright
string.set_decay(2.5);         // seconds for the first partial to fall 60 dB
string.set_pick_position(0.15);
// note on:
string.set_frequency(hz_from_midi(note as f32)); // the first partial
string.pluck(velocity);        // 0..=1: velocity sets level and brightness
// per sample (or string.process(&mut block)):
let y = string.next();
```

The loop is an integer delay, a first-order allpass for the fractional delay (`Tuning::Allpass`, the default; `Tuning::Integer` is the original algorithm), the loss filter `(1 - S) + S z^-1` with a loop gain chosen for the T60, and up to eight dispersion allpasses when `set_stiffness(B)` is above 0. The tuning allpass is designed for the exact phase delay at the fundamental, counting the loss filter's delay (`a = sin(w (1 - d) / 2) / sin(w (1 + d) / 2)`), so the pitch is right to a hundredth of a cent. `next_with(x)` adds an outside excitation into the loop (a bow, an exciter).

Limits to know: the loop gain never exceeds 1, so when the loss filter alone already decays faster than asked (high notes with `S` near 0.5) the real T60 is shorter than the setting; lower `S` for long high notes. `set_frequency` clamps to the prepared lowest pitch and to a quarter of the sample rate. The pick-position comb is an integer number of samples (`D = round(beta N)`) applied to an excitation that fills only the integer part of the loop, so its notches are shallower than the waveguide string's exact ones (27 dB below the neighbouring partials in the test, beta 0.2).

Measured (`tests/physical_proof.rs`, release build; frequencies by a Hann-windowed DFT maximised by golden-section search):

| Check | Result |
| --- | --- |
| Pitch, allpass tuning, MIDI 28 to 100 in steps of 4, at 44.1, 48, 96 and 192 kHz | worst error 0.008 cent |
| Pitch, integer delay only, same grid | quantisation up to 33.4 cents off the note; the signal matches its own loop model (`partial_frequency`) within 0.12 cent |
| Partials 2 to 4 (allpass tuning), notes 36 to 72 | within 0.002 cent of the loop's phase condition; within 0.03 cent of exact harmonics |
| T60 of the first partial, 0.6 / 1.5 / 4 s, notes 36 to 72, 48 and 96 kHz | within 0.01 % |
| Stiffness, `B = 1e-4`, 196 Hz at 96 kHz, partials 1 to 8 vs `f_k = k f_1 sqrt((1 + B k^2) / (1 + B))` | worst 0.20 cent with 4 sections (0.12 with 8) |
| `B = 4e-4` | 1.56 cents with 4 sections (0.95 with 8); the formula itself moves partial 8 by 21.5 cents |
| `B = 1e-3` | 5.2 cents with 4 sections (3.2 with 8); partial 8 moves 52.8 cents |
| `B = 3e-3` | 17 cents with 4 sections (10 with 8): beyond what one coefficient can do |

The dispersion coefficient is a least-squares fit at partials 2 to 12, weighted towards the low ones, so the error grows with the partial number and with `B`; piano-like values (`B` from 1e-4 to 1e-3 in the mid range) are the intended use, and the first partial is always tuned exactly. `stiff_partial_hz(f1, b, k)` gives the formula.

### Digital waveguide string and bore

```rust
use openplugin_dsp::{BoreEnd, WaveguideBore, WaveguideString};

let mut s = WaveguideString::new(sample_rate, 25.0);
s.set_frequency(196.0);
s.set_decay(3.0);
s.set_pluck_position(0.2);   // fraction of the string
s.set_pickup_position(0.35);
s.pluck(0.8);                // triangular displacement, two half-amplitude waves
let y = s.next();            // or s.next_with(x): x enters at set_input_position

let mut tube = WaveguideBore::new(sample_rate, 25.0, BoreEnd::Closed, BoreEnd::Open);
tube.set_frequency(261.6);   // lowest resonance
let p = tube.next_with(excitation); // pressure at the right end
```

The string is a right-going and a left-going delay line with inverting ends; the loss filter, tuning and optional dispersion are lumped at the bridge. The bore has closed (+1) and open (-1) ends: open-open and closed-closed resonate at `f, 2f, 3f`, closed-open at `f, 3f, 5f` (the loop is then half as long for the same pitch; `physical_length(343.0)` gives the tube length). Positions are fractions of the string length, interpolated linearly between samples; pluck, input and pickup positions put comb notches at partials `k = m / beta`.

| Check | Result |
| --- | --- |
| Pitch, MIDI 28 to 100 in steps of 6, 44.1 to 192 kHz | worst 0.017 cent |
| Partial amplitudes vs `sin(pi k beta) sin(pi k q) / k^2` (pluck beta 0.2, pickup q 0.35), partials 2 to 9 | within 0.005 dB |
| Pluck notches (partials 5 and 10) | 67.7 and 52.6 dB below the theory without a notch |
| Input at 1/4 of the string, pickup at 0.35: partials 4 and 8 | 94.5 and 91.2 dB below partial 1 |
| T60, three notes from 1 to 2.5 s | within 0.1 % |
| Stiffness `B = 4e-4`, partials 1 to 8 | worst 1.56 cents |
| Bore, open-open and closed-closed, harmonics 1 to 4 at 44.1 and 96 kHz | fundamental exact; harmonics within 0.01 cent |
| Bore, closed-open | even harmonics 123 and 126 dB below the odd ones; fundamental exact |

### Modal resonator bank

```rust
use openplugin_dsp::{Exciter, ModalBank, ModalPreset, PresetSettings, Strike};

let mut bank = ModalBank::new(sample_rate, 32);          // allocates once
bank.load_preset(ModalPreset::BarFreeFree, &PresetSettings {
    f1: 440.0, t60: 3.0, decay_tilt: 0.7, gain_tilt: 0.0, modes: 5,
});
// or by hand: bank.set_mode(i, freq_hz, t60_seconds, gain);
let mut hit = Exciter::new(sample_rate, Strike::Mallet { contact_ms: 0.5 });
hit.trigger(velocity);
// per sample:
let y = bank.next(hit.next());
```

Each mode is `y[n] = 2 r cos(w) y[n-1] - r^2 y[n-2] + g sin(w) x[n]` with `r = 10^(-3 / (T60 fs))`, whose impulse response is exactly `g r^n sin(w (n + 1))`: the frequency, the 60 dB time and the amplitude are the settings, not approximations. Coefficients and states are `f64` in arrays (a 30 Hz mode at 192 kHz would detune in `f32`) and the bank sums four modes at a time: 0.8 ns per mode per sample (64 modes, 48 kHz, Apple Silicon, release). A mode at 0.49 of the sample rate or above is silenced.

Presets give ratios from formulas: `BarFreeFree` and `BarClampedFree` (roots of `cos x cosh x = +-1` by Newton: 1, 2.757, 5.404, 8.933 and 1, 6.267, 17.55), `BarSimplySupported` (`n^2`), `Plate { aspect }` (simply supported rectangle, `m^2 + (n a/b)^2`, Leissa 1969) and `Membrane` (zeros of Bessel `J_m`, tabulated in `MEMBRANE_ZEROS` from Abramowitz and Stegun table 9.5; 28 modes up to 14.93). The test evaluates `J_m` by its integral form and checks that every tabulated value is a zero and that no zero below 14.93 is missing. `decay_tilt` makes higher modes die faster (`T60_k = T60_1 (f_1 / f_k)^tilt`) and `gain_tilt` shapes their level.

`Exciter` makes `Strike::Impulse`, `Strike::NoiseBurst { ms }` and `Strike::Mallet { contact_ms }` (a half-sine force pulse of unit area; a short contact excites the high modes, a long one hardly does).

| Check | Result |
| --- | --- |
| Free-free bar, clamped-free bar, 1:1.6 plate, 12-mode membrane; 44.1 and 96 kHz; isolated modes | frequency within 0.009 cent, T60 within 0.02 % of the specification |
| Mode 5 of a free-free bar, level relative to mode 0 | impulse 1.000, 0.15 ms mallet 0.49, 2 ms mallet 0.002 |

### Test instrument

`tests/physical_conformance.rs` is a six-voice plucked-string instrument (notes in, mono out, decay, brightness and pick position as parameters; note-off shortens the decay) that passes the `openplugin-testing` conformance kit (228 sessions, including the allocation check). `tests/physical_no_alloc.rs` changes every setting and strikes every model while a counting allocator is armed: zero allocations and zero frees.

## Multiband and dynamic EQ

Five blocks built from the crossovers, the SVF, the envelope followers, the gain computer, the limiter and the linear-phase oversampler. All are allocation-free after construction, have `f64` filter state where they filter, treat non-finite input as `0.0`, ignore NaN settings, and are proven in `crates/openplugin-dsp/tests/multiband.rs`, `multiband_no_alloc.rs` and `multiband_conformance.rs`.

### Multiband compressor

```rust
use openplugin_dsp::{DynamicsParams, LrOrder, MultibandCompressor};

// prepare (allocates once): 3 bands, LR4, split at 200 Hz and 3 kHz
let mut mb = MultibandCompressor::new(sample_rate, channels, 3, LrOrder::Lr4, &[200.0, 3000.0]);
mb.set_band_params(0, DynamicsParams { threshold_db: -24.0, ratio: 3.0, ..Default::default() });
mb.set_band_link(0.3);          // 0 independent .. 1 every band follows the deepest
mb.set_band_solo(1, false);
mb.set_band_bypass(2, false);
mb.set_band_trim_db(2, -1.5);   // slewed over 5 ms, like solo changes
// process (any block size):
mb.process(&mut [&mut left, &mut right]);
let gr0 = mb.gain_reduction_db(0); // for a meter
```

Each band is a `Dynamics` (compressor, expander or gate), so every parameter of `DynamicsParams` is per band. With every band bypassed, or at unity gain, the sum is flat: measured within 0.0025 dB (limit 0.01) from 20 Hz to 20 kHz for LR2, LR4 and LR8, 2, 3 and 4 bands, at 44.1, 48, 96 and 192 kHz. No latency. Neighbouring bands leak into each other (an LR4 band is only 42 dB down at 1.7 octaves), so a deep reduction in one band shifts what a tone near its edge measures; the static-curve test uses LR8 with the tone three octaves from either edge, and then agrees with the ideal `(1 - 1/ratio)` line within 0.01 dB.

Measured per-band static curve (LR8, mid band of a 100 Hz / 8 kHz split, 1 kHz tone, threshold -26 dB, ratio 4, hard knee, linear-domain decoupled peak detector): gain 0.00, -7.49, -14.99, -19.49 dB at -26, -16, -6, 0 dB input (ideal 0, -7.5, -15, -19.5). Use `Domain::Linear` with `Topology::Decoupled` when the reading must be exact.

### Dynamic EQ band

```rust
use openplugin_dsp::{DynEqKind, DynEqMode, DynamicEqBand, DynamicEqParams};

let mut band = DynamicEqBand::new(sample_rate, channels);
band.set_params(DynamicEqParams {
    kind: DynEqKind::Bell, mode: DynEqMode::Above,
    freq_hz: 3500.0, q: 2.0,
    gain_db: 0.0,                 // resting gain
    threshold_db: -24.0, ratio: 4.0, knee_db: 6.0,
    range_db: -9.0,               // cut up to 9 dB; positive boosts
    attack_ms: 8.0, release_ms: 120.0,
    ..Default::default()
});
band.process(&mut [&mut left, &mut right]);                              // detector on the band's own signal
band.process_sidechain(&mut [&mut left, &mut right], &[&key_l, &key_r]); // or an external key
```

Bell, low shelf and high shelf on the `Svf`. The detector is band-limited to the band (band-pass for a bell, low-pass for a low shelf, high-pass for a high shelf), RMS, calibrated so a sine reads its peak: the threshold is in dBFS of a sine. `Above` engages over the threshold, `Below` under it (upward processing). The engagement `min(|range|, (1 - 1/ratio) excess)` is smoothed in dB by `Ballistics`. The SVF coefficients are recomputed every 125 us (6 samples at 48 kHz) rather than every sample: a bell needs a `tan` and a `powf`, the engagement is smoothed at audio rate, and the TPT filter has no coefficient-change transient. A change below 0.001 dB is not applied. `DynamicEqParams::static_gain_db(level)` is the curve for drawing.

Measured (1 kHz bell, Q 1, threshold -30 dB, ratio 4, range -12 dB): steady gain 0.00, -3.83, -7.58, -11.33, -12.00 dB at -30, -25, -20, -15, -10 dB, against the ideal 0, -3.75, -7.5, -11.25, -12. A step to -20 dB with attack 40 ms reached 63.2 % of its target at 41.0 ms; the release back to quiet left 36.8 % at 120.7 ms (set 120). A quiet external key leaves a tone alone and a loud key cuts it the full 12 dB. A high shelf with an infinite ratio and a 9 dB range cuts 8 kHz by 8.5 dB (the shelf's own gain there) and leaves 200 Hz.

### Transient shaper

```rust
use openplugin_dsp::TransientShaper;

let mut ts = TransientShaper::new(sample_rate);
ts.set_attack(0.6);     // -1..1: soften .. emphasise onsets
ts.set_sustain(-0.3);   // -1..1: shorten .. lift the tail
ts.set_max_gain_db(12.0);
ts.process(&mut [&mut left, &mut right]); // one linked gain
```

A fast and a slow envelope follower (defaults 1/20 ms and 25/150 ms, linear, decoupled peak; Zoelzer, "DAFX", 2nd ed., section 4.2 for the followers); their dB difference `d` is positive on onsets and negative in decays. The gain is `attack * d` on onsets and `-sustain * d` on tails, limited to +-12 dB, faded out below -70 dB so noise floors are not shaped, and smoothed over 0.3 ms. Keep the fast release a few periods of the lowest frequency, or the gain follows the waveform. Measured on a 1 kHz tone burst: attack +1 raised the first 5 ms by 12.0 dB (the limit), +0.5 by 11.7 dB, -0.5 and -1 lowered it by 4.7 and 4.8 dB, steady state unchanged (0.000 dB); on a 60 ms exponential decay sustain +1 lifted the tail by 11.5 dB and -1 lowered it by 11.5 dB with the onset unchanged. The effect is program dependent (the gain tracks the envelope difference in dB); lower `max_gain_db` for gentler shaping.

### De-esser

```rust
use openplugin_dsp::{DeEssDetect, DeEssMode, DeEsser};

let mut de = DeEsser::new(sample_rate, channels);
de.set_frequency(6500.0);  de.set_q(1.5);
de.set_threshold_db(-30.0); de.set_ratio(4.0); de.set_range_db(12.0);
de.set_mode(DeEssMode::SplitBand);       // or WideBand
de.set_detect(DeEssDetect::BandPass);    // or HighPass
de.set_listen(false);                    // true: hear the detector
de.process(&mut [&mut left, &mut right]);
```

Detection is a band-pass (or high-pass) of the input or of a key, into a `Dynamics` compressor. Wide-band multiplies everything by the gain. Split-band with a band-pass detector outputs `x + (g - 1) band`: exact at `g = 1` and exactly `g` at the band centre. With a high-pass detector it outputs `low + g high` of an LR4 split (only half the reduction at the corner itself, where both halves are -6 dB). Measured with a 0.3 amplitude tone, threshold -30 dB, ratio 6, range 15 dB: 7 kHz reduced by 15.0 dB in both modes (band-pass), 200 Hz untouched (0.00 dB); high-pass detection at 4 kHz reduced 7 kHz by 15.0 dB wide-band and 12.3 dB split-band. With a 200 Hz tone riding on the sibilant, split-band leaves it alone (0.00 dB) and wide-band ducks it 15 dB.

### True-peak clipper

```rust
use openplugin_dsp::TruePeakClipper;

// prepare (allocates): 4x, stereo, host blocks up to 512
let mut clip = TruePeakClipper::prepare(sample_rate, 4, 2, 512).unwrap();
clip.set_ceiling_db(-1.0);
clip.set_drive_db(6.0);
clip.set_knee_db(3.0);          // 0 = hard clip, else a tanh knee
clip.set_guard_db(0.0);         // optional extra margin under the ceiling
clip.process(&mut [&mut left, &mut right]);
let latency = clip.latency_samples(); // return from Plugin::latency
```

The curve is linear up to `ceiling - knee`, then `x_k + w tanh((x - x_k)/w)` with `w` the knee width (C1, never reaching the ceiling), or a hard clamp for knee 0. It runs at 2x or 4x on `LinearPhaseOversampler`, then through the crate's lookahead true-peak `Limiter` (1 ms lookahead, 20 ms release) that removes what the decimation filter's ringing adds back, and a final sample clamp. `latency_samples()` is the oversampler's 64 plus the limiter's delay (121 at 48 kHz). Why the limiter: without it a hard-clipped 11 kHz sine at 4x measured 1.8 dB over the ceiling, and clamping samples to the ceiling is no cure (samples pinned at an fs/4 tone's 45-degree phase have an inter-sample peak 3 dB higher).

True-peak result against an independent 16x Hann-windowed-sinc interpolator (ceilings -1 and -6 dBFS, knees 0 and 6 dB, 2x and 4x): the sample peak never exceeds the ceiling; the true-peak excess over the ceiling is held to a stated tolerance of **0.6 dB** for tones, sweeps and square waves (measured: 1 kHz sine -0.03 dB, fs/4 sine at 45 degrees 0.00, 11 kHz sine 0.00, 3 kHz square +0.10, a 20 Hz to 24 kHz sweep +0.50 at the -6 dB ceiling) and **1.7 dB** for full-band white noise driven hard into the clip (measured +1.48). The noise figure is the limiter's known open item (its detector under-reads full-band noise), not the clip. Below the ceiling the block is a delayed wire (error 1e-6).
