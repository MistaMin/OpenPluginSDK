# Block convolution

## Summary

`BlockConvolver` plays a real impulse of at most 1024 taps. `fir[0]` multiplies the current sample. Playback is overlap-save: one 2048-point transform per chunk of at most 1024 samples. The plan comes from `rustfft`, which uses Neon on Apple Silicon. The inverse is divided by 2048 in this type. `rustfft` does not apply that scale.

`new` and `set_kernel` allocate. `process` does not.

## Add the crate

The type is exported from `openplugin-dsp` and from the `openplugin` prelude.

```rust
use openplugin::dsp::BlockConvolver;

let mut convolver = BlockConvolver::new(2);
convolver.set_kernel(&impulse); // impulse length <= 1024
```

## Audio thread

Call only `process` from the audio callback. Build the convolver and call `set_kernel` when the impulse changes, which is when a control moves, not on every block.

```rust
convolver.process(channel, &mut samples);
```

`process` splits a longer buffer into chunks of 1024. A chunk shorter than 1024 is valid. There is no extra delay beyond the impulse itself: a 1 at `fir[0]` comes out on the same sample.

`reset` clears the per-channel history. It does not rebuild the kernel spectrum.

## Known limits

- The impulse is truncated at 1024 taps.
- One convolver holds one impulse. Channels share that spectrum and keep separate history.
- The comparison test in `crates/openplugin-dsp/src/convolve.rs` checks this path against a direct 1024-tap walk. Outputs must agree within 0.0002.
