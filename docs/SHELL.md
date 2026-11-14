# Host shell

## Summary

This is the 0.2.0 Beta host contract. `export_shell!` exports one plugin as a C ABI named `op_shell_*`. A thin native adapter calls that ABI. The same symbols feed VST3, Audio Unit, and AAX. The Rust layouts in `openplugin-targets` are not loadable. The adapters are `native/vst3`, `native/au`, and `native/aax`. The adapters in this release are macOS.

`create` allocates the plugin and its scratch. `set_plain`, `set_normalized`, and `process` do not allocate. The audio thread applies the parameter plane inside `process`. `new` is not used from the audio thread.

The shell is mono or stereo, 32-bit float, and one plugin per binary. It hosts an editor only when the plugin exports the optional editor ABI (see Editor). AAX bypass copies the dry buffer and does not delay it. AAX development IDs are a hash of the plugin id. Replace them with registered Avid IDs before a signed release. The AAX packet holds at most 64 parameters.

## Export

```rust
openplugin_targets::export_clap!(EqPlugin);
openplugin_targets::export_lv2!(EqPlugin);
openplugin_targets::export_shell!(EqPlugin);
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

A plugin with an editor exports `op_editor_*` (`native/include/op_editor.h`) next to `op_shell_*`, and the shells are configured with `-DOP_EDITOR=ON`. Without that option the shells build as before and a host shows its generic controls. `export_shell!` does not emit these symbols: the plugin crate writes them, because the editor is the plugin's own.

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

`op_editor_bind` must tolerate a pointer that is no longer a live instance. The plugin keeps a table of live instances for that.

## Audio Unit reset

A host may call `AudioUnitReset` from its main thread while its render thread is inside the engine, and `op_shell_reset` is exclusive with `process`. The Audio Unit shell therefore only marks the reset, and the render thread performs it at the start of its next block.

## Tail

With `-DOP_TAIL=ON` the plugin also exports `op_shell_tail_seconds(instance)`: seconds of ring-out after the input stops, infinity for an endless tail. It may be called from any thread and must not block. VST3 reports it from `getTailSamples`; the Audio Unit reports it as its tail time, with an endless tail given as ten minutes. Without the option VST3 reports no tail and the Audio Unit does not support the property.

## Build

The SDK checkouts stay outside this repository. FreEQ is the proof plugin.

```bash
cargo build --release --lib --manifest-path openplugin-eq/Cargo.toml
cmake -S native -B build-shell -DCMAKE_BUILD_TYPE=Release \
  -DOP_RUST_LIB=openplugin-eq/target/release/libopenplugin_eq.a \
  -DOP_VST3_SDK=/path/to/vst3sdk \
  -DOP_AU_SDK=/path/to/AudioUnitSDK \
  -DOP_AAX_SDK=/path/to/aax-sdk \
  -DOP_PRODUCT=FreEQ -DOP_BUNDLE_ID=com.mistamin.freeq -DOP_VERSION=0.1.0 \
  -DOP_MANUFACTURER=MistaMin -DOP_AU_SUBTYPE=FrEQ -DOP_AU_MANUFACTURER=MstM
cmake --build build-shell --config Release
ctest --test-dir build-shell --output-on-failure
```

`vst3_proof` and `au_proof` load the bundle in the test process. `aax_process_proof` calls the process callback. `aax_bundle_proof` checks the seven ACF exports and `ACFGetSDKVersion`. None of those tests is a Cubase, Logic, or Pro Tools session.

## Proof

FreEQ, 11 November 2026, in this process only. Sample rate 48 kHz. The tone is a 1 kHz sine. Band 4 is a peak at 1 kHz, Q 1, +6 dB. The other bands are peaks at 0 dB. Phase is minimum. Trim is 0 dB.

| Check | Result |
|---|---|
| VST3 mono and stereo | 6.00 dB, unity when the bells are flat, state restore, in-place buffers, 32 hex class IDs, 26 parameters |
| Audio Unit mono and stereo | 6.00 dB, unity, latency 0, a NaN parameter is rejected |
| AAX mono and stereo | 6.00 dB, unity, dry bypass, a non-finite packet writes silence |
| AAX bundle | seven ACF exports and a non-zero `ACFGetSDKVersion` |

## Known limits

- Without `OP_EDITOR` there is no editor view: hosts use a generic parameter list, and the AAX view is an empty rectangle. The editor ABI is macOS (`NSView`) only.
- The AAX shell has a page table only with `-DOP_AAX_PAGES=ON`: the build runs `OpenPluginAAXPages`, which writes `<Product>Pages.xml` into the bundle with the parameters in declared order. It needs manufacturer, product and plug-in IDs of four printable characters, so the plugin must set them; the hashed development IDs are refused. Without the option AAX Validator's `test.page_table.load` fails with "Failed to load page tables library".
- AAX has no tail report.
- VST3 parameter changes are applied at the block, using the last point in each queue.
- AAX bypass is a dry copy. It does not keep a reported latency.
- The AAX binary links the external Avid SDK. That binary is not an MIT-only artifact. The SDK license is copied into the bundle resources. Nothing in the build signs it or installs it.
