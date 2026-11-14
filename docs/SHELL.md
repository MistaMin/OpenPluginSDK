# Host shell

## Summary

This is the 0.3.1 Beta host contract. `export_shell!` exports one plugin as a C ABI named `op_shell_*`. A thin native adapter calls that ABI. The same symbols feed VST3, Audio Unit, and AAX. The Rust layouts in `openplugin-targets` are not loadable. The adapters are `native/vst3`, `native/au`, and `native/aax`. The adapters in this release are macOS.

`create` allocates the plugin and its scratch. `set_plain`, `set_normalized`, and `process` do not allocate. The audio thread applies the parameter plane inside `process`. `new` is not used from the audio thread.

The shell is mono or stereo, 32-bit float, and one plugin per binary. It hosts an editor only when the plugin exports the optional editor ABI (see Editor). AAX development IDs are a hash of the plugin id. `Plugin::aax_ids` replaces them with fixed four-character codes, which a release needs. The AAX controls packet is sized from the parameter count; the shell takes up to 4096 parameters.

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
| `op_shell_save_state`, `op_shell_load_state` | message, any time | yes (the blob) |

`process` copies through scratch allocated in `create`, so an input pointer may be the output pointer. A null right pointer is mono. A block longer than the size passed to `create` fails. The adapters split a host block into that size. The prepared size is at most 65536 frames, and the AAX callback uses 2048.

Class IDs are 16 bytes from a 128-bit mix of the plugin id. VST3 prints them with `FUID::toString`, 32 hex digits. The processor id and the controller id differ. AAX manufacturer, product, mono, and stereo ids are four-byte values from that mix with bit `0x20` set in each byte so none is zero.

## Plugin state

`op_shell_save_state` and `op_shell_load_state` carry the whole plugin state across the ABI: the parameter plane and the plugin's own chunk (`Plugin::save_state` / `load_state`) in one opaque blob, the OPCH session of `openplugin_core::encode_session`. A shell stores the blob as its host state and hands it back later. The shells do not call these yet; today they save parameter values only.

```c
uint32_t op_shell_save_state(void* instance, uint8_t* out, uint32_t capacity, uint32_t* needed);
uint32_t op_shell_load_state(void* instance, const uint8_t* data, uint32_t length);
// op_shell_save_state results: OP_STATE_FAILED 0, OP_STATE_OK 1, OP_STATE_TOO_SMALL 2
```

- Save: `out` NULL with `capacity` 0 is a size query. The call returns `OP_STATE_TOO_SMALL` and writes the length to `*needed` (which may be NULL). Allocate that many bytes and call again. `OP_STATE_OK` means the blob is written and `*needed` is its length. If the state grew in between, the second call returns `OP_STATE_TOO_SMALL` again with the new size, so loop until OK. A short buffer is never written. `OP_STATE_FAILED` is a null instance, a null `out` with capacity above 0, a plugin that panics, a state over 4 GiB, or a timeout (below).
- Load: returns 1 or 0. It is all or nothing. The envelope (magic, version, every length, the parameter count, finite values) is checked before the plugin is touched. A corrupt, truncated, extended or foreign blob, an empty one, a null pointer, or a chunk the plugin's `load_state` refuses returns 0 and leaves the parameters and the plugin as they were. A plugin whose `load_state` returns false must have changed nothing itself: the SDK cannot undo a plugin's partial write. A bare OPST parameter plane (what the shells wrote before) still loads, and leaves the plugin's chunk alone.
- ABI: `OP_SHELL_ABI` stays 1. Nothing existing changed, and a shell that does not call the new symbols builds as before. Both are declared in `op_shell.h` without a guard; a plugin built with `export_shell!` always exports them.

### Threading

A host calls get and set state on the main thread while its audio thread may be inside `op_shell_process`. `Plugin::save_state` takes `&mut self`, so a state call needs the plugin object to itself. One atomic gate decides who has it, and the audio thread never waits:

- `op_shell_process` (and `op_shell_reset`) enter the gate with one compare-exchange, no lock and no allocation. If a state call holds or has asked for the gate, the block is skipped: the input is copied to the output (a dry block, a null right input follows the left), the plugin is not entered, and `process` still returns 1. A `reset` that cannot enter is remembered and runs at the start of the next block, so the Audio Unit's render-thread reset is safe beside a state call.
- A state call takes a mutex against other state calls, sets a request bit, and waits for a block already in flight to end. New blocks see the request bit and skip, so a busy audio thread cannot starve it. The wait is bounded at 2 seconds (`openplugin_ffi::STATE_WAIT`); after that the call fails and changes nothing. The wait happens on the calling (main) thread only.
- Request bit and in-flight bit are one atomic word: for any block and any state call, either the block sees the request and skips, or the call sees the block and waits. No block runs against a half-restored plugin; a load applies the chunk and the plane before it releases the gate, so the next block sees both.

Cost: a block is lost (replaced by dry audio) only when it starts during a call, for as long as `save_state` or `load_state` takes. A host that needs gap-free audio calls these while the transport is stopped or the unit is inactive, when no block is running. Never call them from the audio thread: they allocate and may wait. Before a save the gate holder applies the parameter plane to the plugin, so the chunk matches the values saved beside it; after a load every parameter is pushed to the plugin again on the next block.

The tests are `crates/openplugin-ffi/tests/shell_state.rs` (round trip, size protocol, hostile blobs), `shell_state_threads.rs` (a block during a held save is dry, a save waits for a block in flight, the bounded timeout, and two threads alternating two states against a rendering thread: no torn block, no torn blob) and `shell_state_no_alloc.rs` (zero allocations and frees on the audio thread while a save or a load holds the gate and during repeated calls).

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
| AAX mono and stereo | prepare rejects an invalid rate and a third channel; parameter count and every default come from the ABI and reach the engine; audio at the default, at 6 dB and with `Warm` on equals a direct ABI render sample for sample; bypass outputs the dry buffers exactly once its crossfade is done; a NaN parameter and an oversized packet write silence |
| AAX state chunk, mono and stereo | the shell's own `AAX_CEffectParameters` chunk calls save every parameter and the master bypass; a fresh instance loads the chunk and reports bit-identical values and renders identical audio; 20 corrupt cases (empty, truncated, extended, wrong magic, unknown version, changed layout hash, NaN, infinite, out of range) are refused with every value unchanged; the restored bypass outputs the dry buffers exactly |
| AAX allocation | 720 armed process calls (mono and stereo; 1, 512 and 3000 frames; default, changing, non-finite and oversized packets; bypass on and off, with crossfades across calls, and refused packets while bypassed) made zero allocations, counted by a replaced `operator new`/`delete` and by libsystem_malloc's `malloc_logger` hook; a self-check shows both counters see an allocation |
| AAX bypass, latency stub | `aax_bypass_proof`: the process callback over a stand-in ABI that reports 37 samples (the example reports 0), mono and stereo, in place and not, blocks of 1 to 3000 frames, switching both ways: bypassed from the first block the output is the input delayed by 37 samples, bit for bit; switching matches a per-sample reference within 1e-6 over a 480-sample (10 ms) crossfade, with the engine's own output exactly before the switch and after the fade back; reset empties the delay line; a refused packet is silence unless bypassed |
| AAX 200 parameters | `aax_params_proof`: the real `AAX_CEffectParameters` and process callback over a stand-in ABI that declares 200 parameters: the packet is 8 + 4 * 200 bytes, is posted once per change with every value, all 200 reach the engine and the audio uses the first and the last, the chunk (820 bytes) restores all 200 and the bypass, a 64-value or truncated chunk is refused, a restored bypass delays by 37 samples, a packet claiming 201 values is silence |
| AAX allocation, latency stub | `aax_alloc_latency_proof`: the same 720 armed calls over the 200-parameter stub with 37 samples of latency, so the delay line, the crossfade and a 200-value packet run while the counters are armed: zero allocations |
| AAX bundle | seven ACF exports and a non-zero `ACFGetSDKVersion` |

The allocation count comes from `native/au/alloc_hook.cpp`, a shared library the proof links that interposes the malloc family with dyld. The interposer is test code and is never linked into a shell. In CI the `audio-unit` job builds the example, configures `native/` with only the Audio Unit shell against a pinned public AudioUnitSDK (1.4.0) and runs `ctest -R '^au_'`.

The `shell-validators` CI job builds the VST3 and Audio Unit shells for the example (product "Example Gain") and runs two external tools against them. `auval -v aufx ExGn OpGd` runs on the component installed into the runner's `~/Library/Audio/Plug-Ins/Components`, after `AudioComponentRegistrar` is restarted and `auval -a` lists it; `auval -strict` runs next. pluginval 1.0.4 (pinned, SHA-256 checked before it runs) runs at strictness 10, with `--skip-gui-tests`, on the VST3 bundle and on the installed component. A failed test fails the job. Only the CI runner installs the component.

Run one proof to see its lines: `build-shell/VST3Proof "build-shell/Example Gain.vst3"`, `build-shell/AUProof "build-shell/Example Gain.component"`, `build-shell/AAXProof`, `build-shell/AAXStateProof`, `build-shell/AAXAllocProof`, or `build-shell/AAXBundleProof "build-shell/Example Gain.aaxplugin"`. `ctest --output-on-failure` prints them only when a proof fails.

## Known limits

- Without `OP_EDITOR` there is no editor view: hosts use a generic parameter list, and the AAX view is an empty rectangle. The editor ABI is macOS (`NSView`) only.
- The AAX shell has a page table only with `-DOP_AAX_PAGES=ON`: the build runs `OpenPluginAAXPages`, which writes `<Product>Pages.xml` into the bundle with the parameters in declared order. It needs manufacturer, product and plug-in IDs of four printable characters, so the plugin must set them; the hashed development IDs are refused. Without the option AAX Validator's `test.page_table.load` fails with "Failed to load page tables library".
- AAX has no tail report. This is a format limit, not a gap in the shell. The AAX SDK 2.8.1 (`Interfaces/AAX_Properties.h`, `AAX_Enums.h`, `AAX_IController.h`, `AAX_IEffectParameters.h`, `AAX_IHostProcessor.h` and the `Documentation/Doxygen/dox` guides) has no call, property or notification by which a plug-in states a tail length or a ring-out time; the only latency-like report is `SetSignalLatency`. The one related switch, `AAX_eProperty_Constraint_AlwaysProcess`, stops the host disabling the plug-in's processing when the chain goes silent. It is per effect, costs the whole chain's performance (the SDK says to avoid it unless the host interferes), and carries no duration, so the shell does not set it. `op_shell_tail_seconds` is therefore not used by the AAX shell. The tail contract is checked for VST3 and the Audio Unit only.
- AAX state is one shell chunk (id `OpSt`, payload version 1; layout in `native/aax/OpenPluginAAXState.h`). It holds every parameter value and the master bypass, replaces the SDK's default chunk, and a chunk with the wrong length, magic, version, parameter layout, a non-finite or out-of-range value is refused without changing any parameter. It stores parameter values only: the plugin-owned state the ABI now carries (see Plugin state) is not wired into it yet. `aax_state_proof` drives the real chunk calls on parameter objects built without an AAX controller; Pro Tools itself does not take part.
- The AAX process callback allocates nothing, with or without bypass; `aax_alloc_proof` (the example) and `aax_alloc_latency_proof` (the 200-parameter, 37-sample stub) check it. The proofs cover the callback and the engine it calls, not `EffectInit` or `ResetFieldData`. The delay line, dry buffers and crossfade state are allocated in `ProcessState::prepare`, which `EffectInit` calls.
- The AAX proofs (`aax_process_proof`, `aax_state_proof`, `aax_alloc_proof`, `aax_bypass_proof`, `aax_params_proof`, `aax_alloc_latency_proof`, `aax_bundle_proof`) cannot run in CI. They link the Avid AAX SDK, which is not redistributable, so they run only on a machine that has the SDK checked out (`-DOP_AAX_SDK`).
- Avid's AAX Validator runs locally against the example bundle with `scripts/aax_validate.sh`; see `docs/AAX_VALIDATOR.md`. It cannot run in CI either.
- AAX host latency is published by `AAX_CEffectParameters::EffectInit` through `SetSignalLatency`, which needs an AAX host controller. `aax_process_proof` checks that the process engine's `op_shell_latency` matches the ABI; it does not instantiate the AAX controller.
- The AAX parameter manager built by `EffectInit` is assembled from the same `op_shell_param` table, but `EffectInit` needs an AAX controller. `aax_state_proof` builds the same parameters through the controller-free part of `EffectInit` (`proofInit`); the direct process proof checks the packet count, the per-parameter IDs and every default value.
- Every shell implements host bypass itself. VST3 exposes the standard bypass parameter (`kIsBypass`, id `0x42595053`, outside the plugin's `0..n-1`); while bypassed it outputs the input delayed by `op_shell_latency` with a 10 ms crossfade on switching, starting at the sample offset of the change, and the engine keeps running so switching back is click-free. The delay line is allocated in `setActive`. The bypass flag is stored as a trailing `int32` after the parameter values in the component state; a state without it (written before bypass existed) loads as not bypassed, and an older shell ignores the trailing flag. The Audio Unit shell handles `kAudioUnitProperty_BypassEffect` itself (see Audio Unit bypass). AAX master bypass also outputs the input delayed by `op_shell_latency` with a 10 ms crossfade; its delay line is allocated in `ProcessState::prepare`.
- `vst3_proof`, `vst3_noalloc_proof`, `vst3_bypass_latency_proof`, `au_proof`, `au_bypass_proof`, `au_alloc_proof`, `aax_process_proof`, `aax_state_proof`, `aax_alloc_proof`, `aax_bypass_proof`, `aax_params_proof`, `aax_alloc_latency_proof`, `aax_bundle_proof` and `au_editor_proof` are in-process tests. They load the built bundles in the test process, or call the process callback, and compare the adapter with the C ABI. None of them is a Cubase, Logic, Pro Tools or format-validator session.
- `AUEditorProof` (built with `OP_EDITOR`, ctest `au_editor_proof`) registers the Audio Unit bundle in process, without installing it, and opens the editor as a host does: it reads `kAudioUnitProperty_CocoaUI`, instantiates the factory class from the bundle, makes the view with `uiViewForAudioUnit:withSize:` in an offscreen window and checks its size and that the editor is attached. It uninitializes and initializes the unit with the view open, then clicks a point (default: the centre of the Warm toggle of `examples/editor`; `OP_EDITOR_PROOF_CLICK` `x;y;param` overrides it, shared with the VST3 proof). An `AUEventListener` must see a begin gesture, a value change and an end gesture for that parameter, and the unit's value must change to the value the listener saw. It then closes the view, opens a second one, and disposes the unit first. The window is never shown, but AppKit is initialized, so the proof expects a macOS session with a window server; it has not been run headless and is not in CI. It does not check the re-bind beyond the click still working afterwards, and it does not cover a real host's container (resizing, another window).
- `VST3EditorProof` (built with `OP_EDITOR`) loads the VST3 in process and opens the editor. It re-creates the engine with the editor open. If `OP_EDITOR_PROOF_CLICK` is `x;y;param`, it checks the edits a click makes.
- VST3 parameter changes, bypass included, are sample-accurate. `process` collects every point of every input queue, orders them by sample offset (ties keep arrival order), and renders the block in segments between change points. The values that change at a segment start are set (`op_shell_set_normalized`) before `op_shell_process` for that segment, so a value applies from the segment's first sample; the bypass crossfade starts at the sample of its change. A segment is also capped at the prepared block size, and a block larger than that still works. The scratch for the points is allocated in `setActive` (4096 points, or the parameter count plus one if that is larger). A block with more points than that is thinned per queue: each queue keeps an even share of the capacity, keeping evenly spaced points and always its last one, so the last value of every parameter and of bypass is never lost and a long ramp is approximated by fewer steps; a queue beyond the capacity is ignored. A host sends one queue per parameter, so this limit is reached only by more than about 4000 points per block. Points inside one queue at the same offset are merged by the host SDK, not by the shell. The engine reads parameters at the start of each `op_shell_process`, so the accuracy is that of the plugin: a plugin that smooths a parameter still smooths it from the sample where the new value starts. Starting processing (`setProcessing(true)`) resets the bypass crossfade to the current bypass state. The no-allocation proof is macOS-only (it relies on `malloc_logger`) and covers `process`, not `setupProcessing` or `setActive`, which allocate.
- The AAX controls packet is posted by the shell itself (`GenerateCoefficients` calls `PostPacket`), not by `AAX_CPacketDispatcher`, because a dispatcher packet has a compile-time size. `aax_params_proof` checks what would be posted (`proofTakePacket`); the `PostPacket` call and the data port size declared in Describe need an AAX host, so a 200-value port in Pro Tools is not shown. The AAX SDK sets no limit on the number of parameters or on a native data port's size; the one hard limit found is that a parameter id must be shorter than 32 characters (`kAAX_ParameterIdentifierMaxSize` in `Interfaces/AAX.h`), which the shell does not yet enforce (ids up to 63 characters pass the ABI). `OP_SHELL_AAX_MAX_PARAMS` (64) in `op_shell.h` and `AAX_MAX_PARAMS` in the Rust crate are no longer used by the AAX shell.
- The AAX binary links the external Avid SDK. That binary is not an MIT-only artifact. The SDK license is copied into the bundle resources. Nothing in the build signs it or installs it.
