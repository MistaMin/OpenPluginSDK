# DSP cookbook

Recipes for the building blocks in `openplugin-dsp`, each shown inside the plugin that uses it. Every block is allocation-free on the audio path. Anything that allocates is called out as prepare-time or load-time.

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
