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
