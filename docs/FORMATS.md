# Supported Plugin Formats

## Summary

CLAP is the format bridge that instantiates a plugin and runs `Plugin::process`. It applies parameter events at their sample offset through the shared parameter plane. LV2 uses the same plane. Ports 0 and 1 are inputs. Ports 2 and 3 are outputs. Every parameter is a control input from port 4 on, in descriptor order. `lv2::write_bundle` writes `manifest.ttl` and the plugin's `.ttl` from the parameter table. VST3, AU, and AAX source files in `openplugin-targets` are descriptor layouts. The loadable shells are `native/vst3`, `native/au`, and `native/aax`. A plugin exports them with `export_shell!`. The adapters write `ParamPlane` and call `process` on the audio thread. The contract, the build, and the in-process proof are in [SHELL.md](SHELL.md).

A Rust host uses `openplugin-ffi`. `FfiInstance` reads `ParamSpec` (`steps`, discrete, log skew, unit), writes `store_param`, and calls `process_stereo`, `process_planar`, or `process_interleaved`. Those calls copy through scratch, so the same slices may be the input and the output. Presets come from `Plugin::preset_values`, which is plain descriptor-order values and does not touch the live DSP object. `save_bytes` / `load_bytes` are an OPCH session: the OPST parameter plane plus the plugin's own chunk. A bare OPST plane still loads. `set_offline_flag` is applied on the next process, which forces momentary parameters back to their defaults. Planar renders accept up to 64 channels, which covers 7.1.4 and seventh-order ambisonics. Build the session with Cargo. One `FfiInstance` is one plugin.

OpenPluginSDK's target crates line up with the five formats:

| Format | Operating Systems | Primary Host Environments | Implementation Strategy |
|---|---|---|---|
| **CLAP** | Windows, macOS, Linux | Bitwig Studio, Reaper, FL Studio | C ABI (`clap_plugin`, `clap_plugin_entry`), sample-accurate params, `clap.gui` on macOS with `editor =` |
| **LV2** | Windows, macOS, Linux | Ardour, Reaper, Carla | `lv2_descriptor`, stereo audio and one control port per parameter, `ui:CocoaUI` with `editor =`, `export_lv2!` |
| **VST3** | macOS in this release | In-process factory and editor proofs. A DAW load is not claimed | `native/vst3` over `op_shell_*`. Processor and controller. Editor view with `-DOP_EDITOR=ON` |
| **AU (v2)** | macOS | In-process component proof. A Logic load is not claimed | `native/au`, `AUEffectBase`, plain parameters. Cocoa view with `-DOP_EDITOR=ON` |
| **AAX** | macOS | In-process process callback and ACF exports. A Pro Tools load is not claimed | `native/aax` linked to an external AAX SDK. Native mono and stereo. Editor view with `-DOP_EDITOR=ON` |
| **Standalone**| Windows, macOS, Linux | Live performance, test harnesses | CPAL audio driver + window runner |

## AAX

The adapter links an AAX SDK that stays outside this repository. The SDK is not copied into the tree. The built bundle is not an MIT-only artifact. The build copies the SDK license into the bundle resources. Nothing in the build signs the bundle or copies it into a plug-in folder.

- One native mono variant and one native stereo variant. There is no AAX DSP description.
- Each plug-in parameter is an `AAX_CParameter`. Plain values travel in one packet of at most 64 floats. Log parameters use `AAX_CLogTaperDelegate`.
- Master bypass copies the dry buffer. It does not delay that copy by the reported latency.
- Without `-DOP_EDITOR=ON` the view is an empty rectangle. With it, the view hosts the plugin's editor ([EDITOR.md](EDITOR.md)).
- Manufacturer, product, and plug-in IDs are a hash of the plugin id, documented in [SHELL.md](SHELL.md). Set fixed codes with `Plugin::aax_ids` before a release. A page table (`-DOP_AAX_PAGES=ON`) needs them.
- `aax_process_proof` calls the process callback. `aax_bundle_proof` checks the seven ACF exports and `ACFGetSDKVersion`. Neither test is a Pro Tools session.
