# Supported Plugin Formats

## Summary

CLAP is the format bridge that instantiates a plugin and runs `Plugin::process`. It follows OpenGridder's from-scratch CLAP shell, and it applies parameter events at their sample offset through the shared parameter plane. LV2 uses the same plane. Its port map is the one in OpenGridder: inputs 0 and 1, outputs 2 and 3. VST3, AU, and AAX in this repository are descriptor layouts. The working shells for those formats are OpenGridder's `native/vst3`, `native/au`, and `native/aax`, and they still belong to the offload plugin. Those shells should exchange parameters with `ParamPlane` instead of locking the audio callback.

Seven Halls, RadarVU, and ArcVu do not link those shells. JUCE links one Rust static library and builds its parameter objects from a spec table. `openplugin-ffi` is that contract. `export_openplugin!(MyPlugin)` emits the symbols in `crates/openplugin-ffi/include/openplugin.h`. The host reads `op_param_info` (`steps`, discrete, log skew, unit), writes `op_set_param`, and calls `op_process_stereo`, `op_process_planar`, or `op_process_interleaved`. Those process calls copy through scratch, so the host may pass the same pointer for input and output. Presets come from `Plugin::preset_values`, which is plain descriptor-order values and does not touch the live DSP object. `op_save` / `op_load` are an OPST snapshot of the parameter plane. `op_set_offline` is applied on the next process, which is how an offline bounce stays deterministic. `cmake/OpenPluginRust.cmake` builds the static library, including a universal lipo when both Apple architectures are requested. One static library exports one plugin.

OpenPluginSDK's target crates line up with the five formats:

| Format | Operating Systems | Primary Host Environments | Implementation Strategy |
|---|---|---|---|
| **CLAP** | Windows, macOS, Linux | Bitwig Studio, Reaper, FL Studio | C ABI (`clap_plugin`, `clap_plugin_entry`), sample-accurate params |
| **LV2** | Windows, macOS, Linux | Ardour, Reaper, Carla | `lv2_descriptor`, stereo ports, `export_lv2!` |
| **VST3** | Windows, macOS, Linux | Cubase, Nuendo, Ableton Live, Studio One | COM interfaces (`IComponent`, `IEditController`) |
| **AU (v2/v3)** | macOS | Logic Pro, GarageBand, Final Cut Pro | AudioComponent C FFI + Cocoa bridge |
| **AAX** | Windows, macOS | Avid Pro Tools Ultimate / Studio | Avid AAX SDK C++ interop (`AAX_CEffectParameters`) |
| **Standalone**| Windows, macOS, Linux | Live performance, test harnesses | CPAL audio driver + window runner |

## AAX Support Notes

- AAX export utilizes the local Avid AAX SDK (located in developer environment at `aax-sdk-2-8-1`).
- Provides Native and DSP processing descriptions.
- Parameter state maps directly to `AAX_CParameter`.
- UI view embeds directly into `AAX_IEffectGUI`.
