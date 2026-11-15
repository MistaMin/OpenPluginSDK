# OpenPluginSDK Architecture

## Summary

OpenPluginSDK 0.4.0 Beta isolates DSP and parameter modelling from OS windowing and host plugin ABIs. CLAP and LV2 are Rust entries. VST3, Audio Unit, and AAX load through the native shells in [SHELL.md](SHELL.md).

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
L2: Design Rust Interface (openplugin-dri)
    - Calls GoodLookin DRI for the 47 knob styles, 40 plates, meters, faders, and switches
L3: Target Format Adapters (openplugin-targets)
    - CLAP instantiates a Plugin, pulls the parameter plane, honours event sample offsets, and can select a surround or ambisonic layout with an optional sidechain. It publishes tail, tells the host about a new latency (`request_restart`, then `latency.changed` in `activate`) or new parameter info (`params.rescan`) through the plugin's `HostNotifier`, and passes transport into `set_transport`
    - LV2 descriptor: ports 0 and 1 in, 2 and 3 out (an instrument: 0 and 1 out), same parameter plane, then freewheel, atom and sidechain ports, and the latency output last
    - VST3, AU, and AAX layouts in this crate are not loadable. `export_shell!` exports the C ABI. The loadable adapters are `native/vst3`, `native/au`, and `native/aax`. They write the parameter plane and render through scratch. See [SHELL.md](SHELL.md)
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

## Loading heavy data off the audio thread

Impulse responses, samples and tables are built on a background thread and swapped in without a lock. `openplugin_core::worker` has the pattern:

```rust,ignore
use openplugin_core::worker::loader;

// Plugin construction (main thread): the audio side keeps the Slot.
let (mut loader, mut slot) = loader(initial_table, 4);

// UI thread: build the new value on the worker. A full queue returns QueueFull.
loader.submit(move || build_table_from_file(&path))?;

// Audio thread, top of process: one poll, then use the live value.
slot.poll();
let table = slot.get();
```

- `Loader::submit` queues a closure for a single worker thread (started by `loader`, stopped and joined when the `Loader` drops). The job's return value is the new value.
- `Slot::poll` takes a finished value if one is ready and swaps it in. It does not lock, allocate, free, park or call the system. The old value goes back to the worker through a second bounded queue and is dropped there.
- Every queue is bounded (the `capacity` argument). `submit` returns `QueueFull` rather than blocking. If the return queue is full, `poll` keeps the old value and swaps on a later call; it never drops it on the audio thread.
- A panicking job is counted by `Loader::failed` and the current value stays.
- Drop the `Slot` (which owns the live value) from the main thread, not from `process`.
- Results apply one per `poll`, in submit order. Retired values are freed within a few milliseconds, since the audio thread does not wake the worker.

Proofs: `crates/openplugin-core/tests/worker_no_alloc.rs` (counting allocator on the audio thread: zero allocations and frees in `poll`), `worker_stress.rs` (20,000 swaps against a running process loop), and `crates/openplugin-dsp/tests/convolve_ir_swap.rs` (a new `BlockConvolver` impulse loaded during playback). 

Running on a host's thread: `worker::Executor`, `Task`, `Waker` and `with_executor` let a format entry supply the thread. A `loader` created inside `with_executor` hands its background half (a `Task`) to the executor instead of starting a thread; if the executor returns `Err(task)` the loader starts its own thread for it. The audio side only calls `Waker::wake` (an atomic add) after a `submit` and after `poll` retires a value, and the executor calls `Task::work` when it chooses: it runs at most one job or hands over a waiting result, and returns `Step::Done`, `Idle` or `Retry`. The plugin code is the same either way. The LV2 entry is the first executor: with the host's `worker:schedule`, `run` ends by turning wakes into `schedule_work` calls, the host's `work` runs the task on its thread and `work_response` reaches the audio thread before a later `run` (see [FORMATS.md](FORMATS.md#lv2-ports)). Proofs: `crates/openplugin-targets/tests/lv2_worker.rs` and `lv2_worker_no_alloc.rs`, and three unit tests in `worker.rs`.
