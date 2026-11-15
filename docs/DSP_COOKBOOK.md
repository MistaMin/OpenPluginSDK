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
use openplugin_dsp::{KMeter, KScale, Ppm, PpmType, TruePeak, VuIec};

let mut tp = TruePeak::new(sample_rate, 2);     // 4x below 96 kHz, 2x below 192 kHz
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
