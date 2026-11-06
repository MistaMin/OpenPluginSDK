# Supported Plugin Formats

## Summary

OpenPluginSDK compiles identical DSP and UI definitions into five standard formats:

| Format | Operating Systems | Primary Host Environments | Implementation Strategy |
|---|---|---|---|
| **CLAP** | Windows, macOS, Linux | Bitwig Studio, Reaper, FL Studio | C ABI (`clap_plugin`, `clap_plugin_entry`) |
| **VST3** | Windows, macOS, Linux | Cubase, Nuendo, Ableton Live, Studio One | COM interfaces (`IComponent`, `IEditController`) |
| **AU (v2/v3)** | macOS | Logic Pro, GarageBand, Final Cut Pro | AudioComponent C FFI + Cocoa bridge |
| **AAX** | Windows, macOS | Avid Pro Tools Ultimate / Studio | Avid AAX SDK C++ interop (`AAX_CEffectParameters`) |
| **Standalone**| Windows, macOS, Linux | Live performance, test harnesses | CPAL audio driver + window runner |

## AAX Support Notes

- AAX export utilizes the local Avid AAX SDK (located in developer environment at `aax-sdk-2-8-1`).
- Provides Native and DSP processing descriptions.
- Parameter state maps directly to `AAX_CParameter`.
- UI view embeds directly into `AAX_IEffectGUI`.
