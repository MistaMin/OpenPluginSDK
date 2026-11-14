# Supported Plugin Formats

## Summary

CLAP is the format bridge that instantiates a plugin and runs `Plugin::process`. It applies parameter events at their sample offset through the shared parameter plane. LV2 uses the same plane. Ports 0 and 1 are inputs. Ports 2 and 3 are outputs. Every parameter is a control input from port 4 on, in descriptor order. `lv2::write_bundle` writes `manifest.ttl` and the plugin's `.ttl` from the parameter table. VST3, AU, and AAX source files in `openplugin-targets` are descriptor layouts. The loadable shells are `native/vst3`, `native/au`, and `native/aax`. A plugin exports them with `export_shell!`. The adapters write `ParamPlane` and call `process` on the audio thread. The contract, the build, and the in-process proof are in [SHELL.md](SHELL.md).

A Rust host uses `openplugin-ffi`. `FfiInstance` reads `ParamSpec` (`steps`, discrete, log skew, unit), writes `store_param`, and calls `process_stereo`, `process_planar`, or `process_interleaved`. Those calls copy through scratch, so the same slices may be the input and the output. Presets come from `Plugin::preset_values`, which is plain descriptor-order values and does not touch the live DSP object. `save_bytes` / `load_bytes` are an OPCH session: the OPST parameter plane plus the plugin's own chunk. A bare OPST plane still loads. `set_offline_flag` is applied on the next process, which forces momentary parameters back to their defaults. Planar renders accept up to 64 channels, which covers 7.1.4 and seventh-order ambisonics. Build the session with Cargo. One `FfiInstance` is one plugin.

OpenPluginSDK's target crates line up with the five formats:

| Format | Operating Systems | Primary Host Environments | Implementation Strategy |
|---|---|---|---|
| **CLAP** | Windows, macOS, Linux | Bitwig Studio, Reaper, FL Studio | C ABI (`clap_plugin`, `clap_plugin_entry`), sample-accurate params, `clap.gui` on macOS with `editor =`. Host bypass (`PARAM_IS_BYPASS` to `Plugin::set_host_bypass`, latency-aligned with `SoftBypass`, kept over state save and load) is proven through the exported `clap_plugin` in `tests/clap_bypass.rs`. CI runs `clap-validator` 0.4.1 on the `examples/editor` bundle. No DAW load is claimed |
| **LV2** | Windows, macOS, Linux | Ardour, Reaper, Carla | `lv2_descriptor`, stereo audio and one control port per parameter, `ui:CocoaUI` with `editor =`, `export_lv2!` |
| **VST3** | macOS in this release | In-process factory and editor proofs. A DAW load is not claimed | `native/vst3` over `op_shell_*`. Processor and controller. Parameter changes and the host bypass parameter (`kIsBypass`) are sample-accurate: the block is rendered in segments between change points. Bypass has latency-aligned dry output. Editor view with `-DOP_EDITOR=ON` |
| **AU (v2)** | macOS | In-process component proof. A Logic load is not claimed | `native/au`, `AUEffectBase`, plain parameters. Bypass delays the dry signal by the reported latency and crossfades over 10 ms. Cocoa view with `-DOP_EDITOR=ON`, proved in process by `au_editor_proof` (view, re-initialize, a click seen as begin/value/end gestures, teardown with the unit first) |
| **AAX** | macOS | In-process process callback and ACF exports. A Pro Tools load is not claimed | `native/aax` linked to an external AAX SDK. Native mono and stereo. Editor view with `-DOP_EDITOR=ON` |
| **Standalone**| Windows, macOS, Linux | Live performance, test harnesses | CPAL audio driver + window runner |

## Validators in CI

- **CLAP.** `clap-validator` 0.4.1 on the `examples/editor` bundle.
- **LV2.** The `lv2-validate` job installs `lv2` and `lilv` with Homebrew, assembles `example_editor.lv2` from the `examples/editor` dylib and the files `lv2_ttl` writes, runs `lv2_validate` on `manifest.ttl` and `libexample_editor.ttl`, then renders a stereo sine through `lv2apply` with `Gain` at 6 dB and checks that the peak doubled.
- **VST3 and AU.** The `shell-validators` job runs pluginval 1.0.4 at strictness 10 on the VST3 bundle and the installed Audio Unit, and `auval -v aufx ExGn OpGd` (also `-strict`) on the Audio Unit. See `docs/SHELL.md`.
- **AAX.** Not validated in CI: the Avid SDK and AAX Validator are not redistributable.

## Sample rates

Every format bridge accepts a finite sample rate from **1 000 Hz to 768 000 Hz** inclusive. The bounds live in `openplugin_core::MIN_SAMPLE_RATE` and `openplugin_core::MAX_SAMPLE_RATE`; `openplugin_core::sample_rate_supported` is the shared predicate.

- **CLAP.** `clap_plugin::activate` returns false outside the range, or when the plugin's own `Plugin::accepts_sample_rate` refuses the rate.
- **LV2.** `instantiate` returns null outside the range. The exception is `0.0`, which an LV2 host passes when it does not know the rate yet; the instance then prepares at 48 kHz. An accepted rate is prepared as given.
- **VST3, AU and AAX.** The native adapters call `op_shell_create`, which returns null for an out-of-range rate or a rate the plugin refuses.
- **Rust hosts.** `FfiInstance::prepare` returns false outside the range. `FfiInstance::new` cannot fail, so it uses 48 kHz for an invalid rate.
- **Plugins.** `Plugin::accepts_sample_rate` defaults to the range and a plugin may narrow it. A plugin cannot widen it: the format bridge checks the range first.

**Why 1 kHz.** `clap-validator`'s `process-varying-sample-rates` test activates and processes at 1 234.5678 Hz, and its fuzzer draws rates from 1 000 Hz. The old 8 kHz floor made that test fail. 1 kHz is also the floor the SDK's DSP assumes: biquads, dynamics and meters clamp their coefficients to 1 kHz, and below that filter coefficients no longer describe audio.

**Why 768 kHz.** It is 16 x 48 kHz, the top rate `clap-validator` tries, and above every PCM host rate. A wider upper bound would admit only rates no host uses.

Some `openplugin-dsp` helpers that are designed around studio rates (K-weighting loudness, gain compensation, block meters, bypass and delta smoothing) keep an internal 8 kHz floor. Between 1 kHz and 8 kHz they are approximate. The format contract above promises activation and finite processing there, not calibrated DSP.

## AAX

The adapter links an AAX SDK that stays outside this repository. The SDK is not copied into the tree. The built bundle is not an MIT-only artifact. The build copies the SDK license into the bundle resources. Nothing in the build signs the bundle or copies it into a plug-in folder.

- One native mono variant and one native stereo variant. There is no AAX DSP description.
- Each plug-in parameter is an `AAX_CParameter`. Plain values travel in one packet of a header and one float per parameter, posted by the shell; the shell takes up to 4096 parameters (the AAX SDK sets no limit). A parameter id must be shorter than 32 characters. Log parameters use `AAX_CLogTaperDelegate`.
- Master bypass outputs the input delayed by the reported latency, crossfaded over 10 ms on switching. There is no tail report: AAX has no mechanism for one (see [SHELL.md](SHELL.md)).
- Without `-DOP_EDITOR=ON` the view is an empty rectangle. With it, the view hosts the plugin's editor ([EDITOR.md](EDITOR.md)).
- Manufacturer, product, and plug-in IDs are a hash of the plugin id, documented in [SHELL.md](SHELL.md). Set fixed codes with `Plugin::aax_ids` before a release. A page table (`-DOP_AAX_PAGES=ON`) needs them.
- State is one versioned chunk holding every parameter value and the master bypass. A malformed or wrong-length chunk is refused and changes nothing. See [SHELL.md](SHELL.md).
- `aax_process_proof` calls the process callback. `aax_state_proof` saves and loads the chunk. `aax_alloc_proof` and `aax_alloc_latency_proof` check that the callback does not allocate, with bypass. `aax_bypass_proof` checks the delayed, crossfaded bypass against a latency stub, and `aax_params_proof` checks 200 parameters through the packet, the process callback and the chunk. `aax_bundle_proof` checks the seven ACF exports and `ACFGetSDKVersion`. None is a Pro Tools session, and none runs in CI because the Avid SDK is not redistributable.
