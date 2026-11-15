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
