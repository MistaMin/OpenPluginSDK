# DSP cookbook

Short recipes for the building blocks in `openplugin-dsp`. Each section stands alone: copy the snippet, then read the block's rustdoc for the details. Every block here is allocation-free in `process`, computes its coefficients for the real sample rate (1 kHz to 768 kHz), and treats NaN and infinity as silence.

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
