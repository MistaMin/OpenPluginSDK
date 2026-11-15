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
