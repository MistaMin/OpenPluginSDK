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
