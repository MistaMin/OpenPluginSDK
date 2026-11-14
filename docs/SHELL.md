# Host shell

## Summary

This is the 0.3.1 Beta host contract. `export_shell!` exports one plugin as a C ABI named `op_shell_*`. A thin native adapter calls that ABI. The same symbols feed VST3, Audio Unit, and AAX. The Rust layouts in `openplugin-targets` are not loadable. The adapters are `native/vst3`, `native/au`, and `native/aax`. The adapters in this release are macOS.

`create` allocates the plugin and its scratch. `set_plain`, `set_normalized`, and `process` do not allocate. The audio thread applies the parameter plane inside `process`. `new` is not used from the audio thread.

The shell is mono or stereo, 32-bit float, and one plugin per binary. It hosts an editor only when the plugin exports the optional editor ABI (see Editor). AAX bypass copies the dry buffer and does not delay it. AAX development IDs are a hash of the plugin id. `Plugin::aax_ids` replaces them with fixed four-character codes, which a release needs. The AAX packet holds at most 64 parameters.

## Export

```rust
openplugin_targets::export_clap!(EditorGain);
openplugin_targets::export_lv2!(EditorGain);
openplugin_targets::export_shell!(EditorGain);

// With an editor (macOS): the same three, plus `editor = make_editor`.
openplugin_targets::export_shell!(EditorGain, editor = make_editor);
```

The crate type includes `staticlib`. The native build links that archive. It does not sign or copy a plugin into a system folder.

## Allocation

| Call | Thread | Allocates |
|---|---|---|
| `op_shell_info`, `op_shell_param`, `op_shell_to_plain`, `op_shell_to_normalized` | message | no, after the first descriptor read |
| `op_shell_create` | message, while inactive | yes |
| `op_shell_destroy`, `op_shell_reset` | message, while inactive | `destroy` frees |
| `op_shell_set_plain`, `op_shell_set_normalized` | audio | no |
| `op_shell_process` | audio | no |

`process` copies through scratch allocated in `create`, so an input pointer may be the output pointer. A null right pointer is mono. A block longer than the size passed to `create` fails. The adapters split a host block into that size. The prepared size is at most 65536 frames, and the AAX callback uses 2048.

Class IDs are 16 bytes from a 128-bit mix of the plugin id. VST3 prints them with `FUID::toString`, 32 hex digits. The processor id and the controller id differ. AAX manufacturer, product, mono, and stereo ids are four-byte values from that mix with bit `0x20` set in each byte so none is zero.

## Editor

A plugin with an editor exports `op_editor_*` (`native/include/op_editor.h`) next to `op_shell_*`, and the shells are configured with `-DOP_EDITOR=ON`. Without that option the shells build as before and a host shows its generic controls. `export_shell!(P, editor = make_editor)` writes these symbols in Rust over `openplugin_dri::cocoa::EditorView`, for any `openplugin_core::Editor` (see [EDITOR.md](EDITOR.md)). The Rust view draws with AppKit, CoreGraphics and libobjc. A static library does not carry its framework links, so `native/CMakeLists.txt` names those frameworks for every target that links the archive, proofs included. That is necessary even with `-DOP_EDITOR=OFF` when the plugin still exports the editor ABI, as `examples/editor` does: the option only decides whether the shells host the view.

| Call | Does |
|---|---|
| `op_editor_size` | size in points; the view is not resizable |
| `op_editor_create(host)` | makes the editor, bound to no engine; `host` carries the three gesture callbacks |
| `op_editor_view`, `op_editor_attach(parent)` | the `NSView`, and adding it to a host parent view |
| `op_editor_bind(instance)` | shows the engine from `op_shell_create`; null or a destroyed instance shows host values only |
| `op_editor_set_plain(index, plain)` | a parameter value from the host |
| `op_editor_destroy` | removes the view and frees the editor |

Every call is on the main thread. A gesture goes editor to shell to host through `begin_edit`, `perform_edit` and `end_edit`, with plain values, and the host returns the value to the engine as an ordinary parameter change. The editor never writes to the engine.

The shells recreate the engine when the host reconfigures them, so each one binds the open editor again:

- VST3: the processor sends the controller a message (`openplugin.engine`) when it creates or destroys its engine. This needs the processor and controller in one process, connected by the host. A host that does not connect them gets a working editor without meters.
- Audio Unit: the Cocoa view polls its Audio Unit 20 times a second for the engine and the parameter values. On the same timer it logs any failed initialize or render status, because a host renders silence for a failed block and reports nothing. The view class names carry the product name and version, because Objective-C class names are global in the host process.
- AAX: the view reads the engine through `GetCustomData` and sets values through `AAX_IParameter` (`Touch`, `SetValueWithFloat`, `Release`).

`op_editor_bind` must tolerate a pointer that is no longer a live instance. `openplugin_ffi::shell` keeps a table of live instances. `op_editor_bind` hands the editor the instance's `Plugin::editor_state` only when the pointer is in that table.

## Audio Unit reset

A host may call `AudioUnitReset` from its main thread while its render thread is inside the engine, and `op_shell_reset` is exclusive with `process`. The Audio Unit shell therefore only marks the reset, and the render thread performs it at the start of its next block.

## Audio Unit bypass

The Audio Unit shell owns `kAudioUnitProperty_BypassEffect`. `AUEffectBase` would copy the input to the output with no delay and reset the plugin when bypass ends, so the shell turns that off (`ShouldBypassEffect` is false, and the property is handled in `SetProperty`). The plugin keeps running while bypassed, so leaving bypass finds it warm. The shell mixes two signals:

- the dry signal, the input delayed by `op_shell_latency` samples, the same value the unit reports as `kAudioUnitProperty_Latency`;
- the plugin output.

A switch moves the mix linearly over 10 ms. Settled bypass is the delayed input exactly. The delay line and the scratch buffers are allocated in `Initialize`, so rendering allocates nothing. A reset (`AudioUnitReset`) clears the delay line on the render thread, with the engine reset described above. The cost is that a bypassed unit still pays for its plugin's CPU.

## Tail

`export_shell!` always exports `op_shell_tail_seconds(instance)`: seconds of ring-out after the input stops, infinity for an endless tail. The value is `Plugin::tail` as of the last processed block. `tail` is called once per block on the audio thread, so it must be cheap and must not allocate. `Tail::Unknown` reports zero. The call works from any thread and never blocks. With `-DOP_TAIL=ON`, VST3 reports it from `getTailSamples`. The Audio Unit reports it as its tail time, with an endless tail given as ten minutes. Without the option VST3 reports no tail and the Audio Unit does not support the property. The SDK example has a finite zero tail, so `-DOP_TAIL=ON` exercises both adapters without a ringing sound.

## Trace

For a host that goes silent and says nothing. Create `<name>-trace.on` in the temporary folder before the first instance is created. `<name>` is the plugin name in lower case, with dashes for spaces and punctuation. The shell then counts creates, destroys, resets, blocks, failed blocks and peaks. The editor's main-thread value updates append a line to `<name>-trace.log` at most four times a second, with the first eight parameter values the engine holds. Without the file, `process` reads one atomic and nothing else. `openplugin_ffi::shell::trace::write` adds a line from plugin code.

## Build

The SDK checkouts stay outside this repository. The proofs build `examples/editor`, the `EditorGain` example. Its two parameters are `Gain` (dB) and `Warm` (toggle), and every expected value in the proofs comes from `op_shell_info`, `op_shell_param`, `op_shell_latency` and `op_shell_tail_seconds`, not from constants in the proof.

```bash
CARGO_BUILD_JOBS=2 cargo build --release --lib -p example-editor
cmake -S native -B build-shell -DCMAKE_BUILD_TYPE=Release \
  -DOP_RUST_LIB="$PWD/target/release/libexample_editor.a" \
  -DOP_VST3_SDK=/path/to/vst3sdk \
  -DOP_AU_SDK=/path/to/AudioUnitSDK \
  -DOP_AAX_SDK=/path/to/aax-sdk \
  -DOP_PRODUCT="Example Gain" -DOP_BUNDLE_ID=com.opengrid.example \
  -DOP_VERSION=0.3.1 -DOP_MANUFACTURER=OpenGrid \
  -DOP_AU_SUBTYPE=ExGn -DOP_AU_MANUFACTURER=OpGd \
  -DOP_TAIL=ON -DOP_EDITOR=OFF
cmake --build build-shell -j 2
ctest --test-dir build-shell --output-on-failure
```

`OP_TAIL=ON` is what makes `vst3_proof` and `au_proof` exercise the tail report. A build with `-DOP_TAIL=OFF` still builds and passes the other checks; those two proofs print that the tail check was not built instead of claiming it ran. In this workspace `build-shell` is on the external build volume, not inside the repository, because the internal disk is full.

## Proof

The example, in this process only, on 14 November 2026. Sample rate 48 kHz. The tone is a 1 kHz sine. `Gain` is 6 dB unless a check says otherwise; `Warm` is off. The proofs load the built VST3 and Audio Unit bundles, or call the AAX process callback, and compare the adapter with a second instance made straight through the C ABI.

| Check | Result |
|---|---|
| VST3 mono and stereo | parameter count, names, units, defaults, steps and automation flags match `op_shell_info`/`op_shell_param`; 32 hex class IDs equal the ABI processor and controller IDs; latency equals `op_shell_latency` (0 samples); tail equals `op_shell_tail_seconds` (0 samples, `kNoTail`); audio at the default and at 6 dB equals a direct ABI render sample for sample; in-place buffers match out-of-place; save, change and restore through `getState`/`setState`/`setComponentState` restores the audio exactly; truncated and wrong-count state are rejected; the `kIsBypass` parameter exists (automatable, off by default, id outside `0..n-1`), bypassed output is the input delayed by the reported latency, leaving bypass restores the processed signal, bypass survives `getState`/`setState`/`setComponentState`, and a state written without it loads as not bypassed |
| VST3 bypass with latency | The SDK examples report no latency, so `vst3_bypass_latency_proof` builds the same shell over a stub ABI (`native/vst3/latency_stub.cpp`, 37 samples) and checks that bypassed output equals the input delayed by exactly 37 samples, in odd block sizes that wrap the delay line, in place and not, mono and stereo, and that the 10 ms crossfade stays between the processed and the delayed dry signal |
| VST3 sample-accurate changes | `vst3_proof` renders one block through the shell and the same block through the C ABI split at the same offsets (`op_shell_set_normalized` then `op_shell_process` per segment), mono and stereo, separate and in place: one change mid-block, a change at the last sample, a 24-point ramp in one queue, changes in two queues at different and at the same offsets, and a block of 3 prepared blocks plus 1234 samples with changes across the prepared-size boundaries. The outputs are equal (max difference 0). The output equals the unchanged render exactly up to the first change point and differs at that sample, and differs from the same values applied at sample 0. Bypass switched on at 300, on at 100 and off at 700, and on at 250 with a gain change at 600 matches a written-out 10 ms crossfade model (max difference 0), is the processed signal exactly before the change, and moves at the change sample. `vst3_bypass_latency_proof` does the same over the 37-sample stub (on at 300, off at 700 of one block; max difference from the model 1.6e-8). A block with 3 x 4096 points keeps the last value of each queue (checked through `getState`) |
| VST3 no allocation | `vst3_noalloc_proof` (and `vst3_noalloc_latency_proof` over the stub) counts allocations only while armed around `IAudioProcessor::process`: `operator new` in the test, and every malloc-family call in the process through libmalloc's `malloc_logger`, which includes the bundle and the Rust engine. It runs mono and stereo, separate and in-place buffers, a block with parameter changes, dense ramps (a point at every sample in every queue), a 4096-sample block with three times 4096 points (past the scratch capacity), and bypass switched on and off. A self-check first allocates through each route while armed and requires the counter to see it, so the proof cannot pass blind. Result: 0 allocations in `process` |
| Audio Unit mono and stereo | parameter list, order, names, ranges, defaults, units and log flags match the ABI; a NaN and an out-of-range value are rejected; latency equals `op_shell_latency` (0 s); tail equals `op_shell_tail_seconds` (0 s); audio at the default and at 6 dB equals a direct ABI render sample for sample; a `kAudioUnitProperty_ClassInfo` save, change and restore restores the audio exactly; bypass is reported and settable, the bypassed output is the input delayed by the reported latency exactly, the switch has no step larger than the tone itself, and switching back restores the processed signal |
| Audio Unit bypass with latency | `au_bypass_proof` loads a test bundle (`native/au/latency_shell.cpp`, a C++ stand-in plugin that reports 37 samples, because the example reports 0): mono and stereo, the bypassed output is the input delayed by the 37 samples the unit reports, a reset empties the delay line, and switching back restores the processed signal |
| Audio Unit render allocation | `au_alloc_proof` (example) and `au_alloc_latency_proof` (latency test bundle): every malloc-family call and the global `operator new` in the process are counted while armed around `AudioUnitRender` only. Mono and stereo, 100, 512 and 4096 frames, parameter changes, bypass on, bypassed and off, and a render after a reset: zero allocations and zero frees. A self-check first shows the counter sees `malloc`, `calloc`, `realloc`, `strdup`, a CoreFoundation allocation, `operator new`, `std::vector` and a Rust allocation, and a negative control (an allocation put into the crossfade) fails the proof |
| AAX mono and stereo | prepare rejects an invalid rate and a third channel; parameter count and every default come from the ABI and reach the engine; audio at the default, at 6 dB and with `Warm` on equals a direct ABI render sample for sample; bypass copies the dry buffers exactly; a NaN parameter and an oversized packet write silence |
| AAX state chunk, mono and stereo | the shell's own `AAX_CEffectParameters` chunk calls save every parameter and the master bypass; a fresh instance loads the chunk and reports bit-identical values and renders identical audio; 20 corrupt cases (empty, truncated, extended, wrong magic, unknown version, changed layout hash, NaN, infinite, out of range) are refused with every value unchanged; the restored bypass copies the dry buffers exactly |
| AAX allocation | 564 armed process calls (mono and stereo; 1, 512 and 3000 frames; default, changing, non-finite and oversized packets; bypass on and off) made zero allocations, counted by a replaced `operator new`/`delete` and by libsystem_malloc's `malloc_logger` hook; a self-check shows both counters see an allocation |
| AAX bundle | seven ACF exports and a non-zero `ACFGetSDKVersion` |

The allocation count comes from `native/au/alloc_hook.cpp`, a shared library the proof links that interposes the malloc family with dyld. The interposer is test code and is never linked into a shell. In CI the `audio-unit` job builds the example, configures `native/` with only the Audio Unit shell against a pinned public AudioUnitSDK (1.4.0) and runs `ctest -R '^au_'`.

Run one proof to see its lines: `build-shell/VST3Proof "build-shell/Example Gain.vst3"`, `build-shell/AUProof "build-shell/Example Gain.component"`, `build-shell/AAXProof`, `build-shell/AAXStateProof`, `build-shell/AAXAllocProof`, or `build-shell/AAXBundleProof "build-shell/Example Gain.aaxplugin"`. `ctest --output-on-failure` prints them only when a proof fails.

## Known limits

- Without `OP_EDITOR` there is no editor view: hosts use a generic parameter list, and the AAX view is an empty rectangle. The editor ABI is macOS (`NSView`) only.
- The AAX shell has a page table only with `-DOP_AAX_PAGES=ON`: the build runs `OpenPluginAAXPages`, which writes `<Product>Pages.xml` into the bundle with the parameters in declared order. It needs manufacturer, product and plug-in IDs of four printable characters, so the plugin must set them; the hashed development IDs are refused. Without the option AAX Validator's `test.page_table.load` fails with "Failed to load page tables library".
- AAX has no tail report. The tail contract is checked for VST3 and the Audio Unit only.
- AAX state is one shell chunk (id `OpSt`, payload version 1; layout in `native/aax/OpenPluginAAXState.h`). It holds every parameter value and the master bypass, replaces the SDK's default chunk, and a chunk with the wrong length, magic, version, parameter layout, a non-finite or out-of-range value is refused without changing any parameter. It stores parameter values only: a plugin-owned state blob through the ABI is not part of it (a cross-format task). `aax_state_proof` drives the real chunk calls on parameter objects built without an AAX controller; Pro Tools itself does not take part.
- The AAX process callback allocates nothing; `aax_alloc_proof` checks it. The proof covers the callback and the Rust engine it calls, not `EffectInit` or `ResetFieldData`.
- The AAX proofs (`aax_process_proof`, `aax_state_proof`, `aax_alloc_proof`, `aax_bundle_proof`) cannot run in CI. They link the Avid AAX SDK, which is not redistributable, so they run only on a machine that has the SDK checked out (`-DOP_AAX_SDK`).
- AAX host latency is published by `AAX_CEffectParameters::EffectInit` through `SetSignalLatency`, which needs an AAX host controller. `aax_process_proof` checks that the process engine's `op_shell_latency` matches the ABI; it does not instantiate the AAX controller.
- The AAX parameter manager built by `EffectInit` is assembled from the same `op_shell_param` table, but `EffectInit` needs an AAX controller. `aax_state_proof` builds the same parameters through the controller-free part of `EffectInit` (`proofInit`); the direct process proof checks the packet count, the per-parameter IDs and every default value.
- Every shell implements host bypass itself. VST3 exposes the standard bypass parameter (`kIsBypass`, id `0x42595053`, outside the plugin's `0..n-1`); while bypassed it outputs the input delayed by `op_shell_latency` with a 10 ms crossfade on switching, starting at the sample offset of the change, and the engine keeps running so switching back is click-free. The delay line is allocated in `setActive`. The bypass flag is stored as a trailing `int32` after the parameter values in the component state; a state without it (written before bypass existed) loads as not bypassed, and an older shell ignores the trailing flag. The Audio Unit shell handles `kAudioUnitProperty_BypassEffect` itself (see Audio Unit bypass). AAX bypass is the shell's dry copy and is checked.
- `vst3_proof`, `vst3_noalloc_proof`, `vst3_bypass_latency_proof`, `au_proof`, `au_bypass_proof`, `au_alloc_proof`, `aax_process_proof`, `aax_state_proof`, `aax_alloc_proof` and `aax_bundle_proof` are in-process tests. They load the built bundles in the test process, or call the process callback, and compare the adapter with the C ABI. None of them is a Cubase, Logic, Pro Tools or format-validator session.
- `VST3EditorProof` (built with `OP_EDITOR`) loads the VST3 in process and opens the editor. It re-creates the engine with the editor open. If `OP_EDITOR_PROOF_CLICK` is `x;y;param`, it checks the edits a click makes.
- VST3 parameter changes, bypass included, are sample-accurate. `process` collects every point of every input queue, orders them by sample offset (ties keep arrival order), and renders the block in segments between change points. The values that change at a segment start are set (`op_shell_set_normalized`) before `op_shell_process` for that segment, so a value applies from the segment's first sample; the bypass crossfade starts at the sample of its change. A segment is also capped at the prepared block size, and a block larger than that still works. The scratch for the points is allocated in `setActive` (4096 points, or the parameter count plus one if that is larger). A block with more points than that is thinned per queue: each queue keeps an even share of the capacity, keeping evenly spaced points and always its last one, so the last value of every parameter and of bypass is never lost and a long ramp is approximated by fewer steps; a queue beyond the capacity is ignored. A host sends one queue per parameter, so this limit is reached only by more than about 4000 points per block. Points inside one queue at the same offset are merged by the host SDK, not by the shell. The engine reads parameters at the start of each `op_shell_process`, so the accuracy is that of the plugin: a plugin that smooths a parameter still smooths it from the sample where the new value starts. Starting processing (`setProcessing(true)`) resets the bypass crossfade to the current bypass state. The no-allocation proof is macOS-only (it relies on `malloc_logger`) and covers `process`, not `setupProcessing` or `setActive`, which allocate.
- AAX bypass is a dry copy. It does not keep a reported latency.
- The AAX binary links the external Avid SDK. That binary is not an MIT-only artifact. The SDK license is copied into the bundle resources. Nothing in the build signs it or installs it.
