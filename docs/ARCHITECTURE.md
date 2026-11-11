# OpenPluginSDK Architecture

## Summary

OpenPluginSDK provides a layered architecture that isolates core DSP and parameter modeling from OS windowing and host plugin ABIs.

## Layer Model

```
L0: Core Contracts (openplugin-core)
    - Plugin trait, ParamInfo, Transport, Unit, Taper, Events
    - ParamPlane and ParamCursor: atomic parameter bits, audio thread pulls changes
    - MIDI parser with running status (note-on velocity 0 stays a note-on)
    - Seqlock meter snapshots, SPSC gesture queue, one-pole smoother
    - Versioned OPST parameter snapshot, OPCH session chunk, and A/B compare
    - Speaker layouts from mono through 7.1.4 and seventh-order ambisonics, plus an optional sidechain
L1: Algorithms & DSP (openplugin-dsp)
    - Biquad filters, Dynamics (Compressor/Limiter/Gate), Saturation, Metering
    - BlockConvolver: overlap-save of a real impulse, at most 1024 taps, 2048-point rustfft, no allocation in process
    - Antiderivative Anti-Aliasing (ADAA1 Residual, ADAA2 Taylor)
    - Polyphase Half-Band Resamplers (2x, 4x Oversampling)
    - Linear-phase 1x/2x/4x oversampler (64-sample latency at 2x and 4x), dry delay, slow RMS gain comp
    - Note-to-Hz, samples-per-beat, phase oscillator, sample voice, fixed polyphony
L2: Direct Rendering Interface (openplugin-dri)
    - Calls GoodLookinDri for the 47 knob styles, 40 plates, meters, faders, and switches
L3: Target Format Adapters (openplugin-targets)
    - CLAP instantiates a Plugin, pulls the parameter plane, honours event sample offsets, and can select a surround or ambisonic layout with an optional sidechain. It publishes tail, asks the host to restart when latency changes, and passes transport into `set_transport`
    - LV2 descriptor: ports 0 and 1 in, 2 and 3 out, same parameter plane
    - VST3, AU, and AAX are descriptor layouts. The working shells are still in OpenGridder
      and should talk to the plugin through ParamPlane, the way OpenGridder's insert bridge does
    - openplugin-ffi is a Rust host session. FfiInstance publishes the parameter
      plane, renders stereo, planar, interleaved, and sidechain blocks through scratch
      allocated in prepare (up to 64 channels), applies factory presets, and saves an OPCH
      session that still accepts an OPST plane.
      Faceplate rasters stay in the product.
L4: Public Facade (openplugin)
    - Single-entrypoint prelude and ergonomics
```

## Real-Time Safety Guarantees

1. **Audio Callback Isolation:** The `Plugin::process` callback must never block on locks, allocate heap memory, or perform file/network I/O. Hosts publish parameters with `ParamPlane::store` and the audio thread applies them with `ParamCursor::pull`. Meter frames go out through a `Seqlock`. Neither crosses a mutex.
2. **Denormal Protection:** Recursive filter structures (`Biquad`) flush denormal values (`< 1e-25`) to prevent CPU spikes in silent passages.
3. **Hostile Input Boundary:** Parameter values provided by third-party DAWs are clamped to `[min, max]` and checked for `NaN`/`inf` before reaching DSP blocks.
