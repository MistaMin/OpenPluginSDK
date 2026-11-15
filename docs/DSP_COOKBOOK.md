# DSP cookbook

Short recipes for the building blocks in `openplugin-dsp`. Each section is
self-contained: what to reach for, a snippet, what is proven and where the
proof lives. All blocks take the real host sample rate (it goes through
`effective_rate`, 1 kHz to 768 kHz, 48 kHz for NaN), do not allocate, and are
safe to call from the audio thread.

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
