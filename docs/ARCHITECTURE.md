# OpenPluginSDK Architecture

## Summary

OpenPluginSDK provides a layered architecture that isolates core DSP and parameter modeling from OS windowing and host plugin ABIs.

## Layer Model

```
L0: Core Contracts (openplugin-core)
    - Plugin trait, ParamInfo, Transport, Unit, Taper, Events
L1: Algorithms & DSP (openplugin-dsp)
    - Biquad filters, Dynamics (Compressor/Limiter/Gate), Saturation, Metering
    - Antiderivative Anti-Aliasing (ADAA1 Residual, ADAA2 Taylor)
    - Polyphase Half-Band Resamplers (2x, 4x Oversampling)
L2: Direct Rendering Interface (openplugin-dri)
    - GoodLookin procedural vector engines, 47 knob styles, 40 plates, VU ballistics
L3: Target Format Adapters (openplugin-targets)
    - CLAP, VST3, AU, AAX, Standalone
L4: Public Facade (openplugin)
    - Single-entrypoint prelude and ergonomics
```

## Real-Time Safety Guarantees

1. **Audio Callback Isolation:** The `Plugin::process` callback must never block on locks, allocate heap memory, or perform file/network I/O.
2. **Denormal Protection:** Recursive filter structures (`Biquad`) flush denormal values (`< 1e-25`) to prevent CPU spikes in silent passages.
3. **Hostile Input Boundary:** Parameter values provided by third-party DAWs are clamped to `[min, max]` and checked for `NaN`/`inf` before reaching DSP blocks.
