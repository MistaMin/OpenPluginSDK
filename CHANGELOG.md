# Changelog

All notable changes to OpenPluginSDK are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions are described in [docs/VERSIONING.md](docs/VERSIONING.md).

Entries describe what changes for someone writing a plugin with the SDK, and are written from the git history. Each release names its commit range. Only `v0.1.0-beta.1` is tagged; the later releases are identified by the commit that bumped the version in `Cargo.toml`. Breaking changes are marked **Breaking**.

Before 1.0 the SDK does not promise compatibility between releases: any release may break. Breaking changes are listed under Changed or Removed and marked.

## [Unreleased]

Everything after the 0.3.1 Beta release commit `a708cac`: the 0.4 work on macOS format conformance and the first 0.5 and 1.0 tasks.

### Added

- LV2 parity with CLAP: an `atom:Sequence` input when `note_ports().input` (`midi:MidiEvent` delivered at its frame offset through `note_on_channel`, `note_off` and `midi::deliver`; `time:Position` mapped onto `Transport`), an `atom:Sequence` MIDI output from `pull_midi` when `note_ports().output`, one `lv2:isSideChain` audio input per sidechain channel (`process_buses`), an `lv2:freeWheeling` control port wired to `set_offline`, and `lv2:InstrumentPlugin` with no main audio input for an instrument. `lv2::PortMap` and `lv2::layout_of` describe the layout. Proven through the exported descriptor (`tests/lv2_ports.rs`, `lv2_atoms_hostile.rs`, `lv2_ports_no_alloc.rs`); `lv2_validate` reports 0 errors on the generated files, and `lv2apply` still renders the example.
- `op_shell_save_state` and `op_shell_load_state` in the shell ABI. They carry the parameters and the plugin's own chunk (`Plugin::save_state` / `load_state`) in one blob, all or nothing. `OP_SHELL_ABI` stays 1; nothing existing changed. The VST3 shell carries the blob in its component state (a trailer after the version 1 fields), the Audio Unit in its class-info dictionary (key `OpenPluginState`), and AAX in state chunk version 2; states written by earlier versions still load.
- `openplugin_ffi::STATE_WAIT` (two seconds) and `FfiInstance::try_save_bytes`, the bounded form of `save_bytes`.
- VST3: a `kIsBypass` host bypass parameter. VST3, the Audio Unit and AAX bypass by delaying the dry signal by the reported latency with a 10 ms crossfade, and the plugin keeps running. CLAP's `PARAM_IS_BYPASS` reaches `Plugin::set_host_bypass`.
- AAX: parameter and plugin state saved as a versioned chunk (corrupt chunks are refused without touching any parameter); the controls packet is sized by the parameter count, so plugins with more than 64 parameters work (proven with 200).
- `scripts/build_plugin.sh`, which builds CLAP, LV2, VST3, Audio Unit and AAX bundles for one plugin, keeps build paths out of the binaries, and signs, notarises and PACE-signs when the environment asks for it. Described in `docs/BUILD.md`.
- `scripts/aax_validate.sh`, which runs Avid's AAX Validator against the example bundle locally (13 passed, 0 failed, 1 skipped).
- Proofs: CLAP host bypass through the exported `clap_plugin`; allocation-free `process` for the VST3, Audio Unit and AAX C++ paths (counting every malloc-family call in the process); Audio Unit editor proof (`au_editor_proof`); VST3 bypass and parameter changes proven sample-accurate.
- CI: `clap-validator` 0.4.1, VST3 and Audio Unit shell proofs against pinned public SDKs, `auval` (normal and strict), pluginval at strictness 10, `lv2_validate`, and a render through lilv's `lv2apply`.
- `Plugin::state_is_parameters_only` (default `false`). A plugin that returns `true` saves and loads from the parameter plane without the state gate, in the shells and in CLAP, so a host saving during playback never skips a block. The SDK's examples opt in.
- `openplugin_dri::hit`: a control registry (knobs, faders, toggles, buttons, key banks) that turns mouse input into host gestures, with double-click reset, fine adjustment, wheel and stepped snapping; `Editor::modifiers_changed`.
- `openplugin_dri::DesignEditor`: an editor built from a GoodLookin design CSV, each row bound to a parameter by id; `examples/design_editor`.
- `docs/PUBLISHING.md`, `CHANGELOG.md`, `docs/VERSIONING.md` and `docs/API_REVIEW.md`; crate metadata (authors, homepage, keywords, categories, readme, documentation) inherited by every crate; GoodLookin DRI named by version as well as path.
- `docs/ROADMAP.md` for macOS to 1.0 and `docs/ROADMAP_WINDOWS_LINUX.md`.

### Changed

- **Breaking (LV2 port indices).** Indices after the parameters are new: every LV2 plugin gains a `freewheel` control port at `first_param + parameter count`, and a plugin whose `note_ports()` has `input` (the default) or `output` gains atom ports after it, then sidechain ports. Ports 0 to 3 and the parameters of an effect keep their indices. An LV2 instrument no longer has audio inputs: its outputs are ports 0 and 1 and its parameters start at port 2 (`lv2::AUDIO_PORTS` stays 4, the effect value; use `lv2::PortMap::first_param`). Hosts that saved state by port symbol or by effect index are unaffected; a plugin with note ports now lists `urid:map` as a required feature.
- VST3 applies parameter changes and the bypass crossfade at their sample offset instead of at the start of the block.
- Audio Unit bypass no longer resets the plugin when it ends; the plugin keeps running and the dry signal is delayed by the reported latency (the base class copied the input with no delay).
- CLAP saves and loads beside `process` safely: a gate for plugins with a chunk, the parameter plane for parameter-only plugins.
- The native build signs each shell bundle again (ad hoc) after its post-build copies, so the bundles verify and the Audio Unit registers.
- Unused dependencies removed: `log` and `serde_json` from core, `serde` and `log` from dsp, `serde` from dri, `cpal` and `raw-window-handle` from targets. The `standalone` feature no longer pulls `cpal`.
- `FfiInstance` guards the plugin object with a gate so that saving and loading a session is safe beside `process`. A block that starts while a state call holds the plugin passes the input through dry. This is behaviour, not a signature change, except for the additions above.

### Fixed

- `save_bytes` / `load_bytes` raced with `process` (fixed by the gate above), and so did CLAP's `state.save` / `state.load`.
- VST3 `setState` applied values while it was still reading them, so a bad state left them half-applied; it now validates first.
- The shell bundles' signatures were broken by the post-build copies ("code has no resources but signature indicates they must be present").
- AAX bypass copied the dry signal without the reported latency; it is now delayed by the reported latency like VST3 and the Audio Unit.

## [0.3.1-beta] - 2026-11-14

Commits `22d4007` to `a708cac`. The hardening release (roadmap 0.3.x). GoodLookin DRI 0.2 rendering, then a pass of hostile-input, allocation and conformance proofs.

### Added

- GoodLookin DRI 0.2 drawing in `openplugin-dri`:
  - `DrawList::knob_tier` and the re-exported `KnobTier` pick the knob tier: lit by default, with the classic and flat tiers still available. A `lit_` or `flat_` prefix on a knob style code picks the tier for one knob.
  - `SceneCanvas`, which rasterises a whole scene into the pixels an `Editor` returns, at the display's backing scale. `SceneCanvas::frame_layered` keeps the static plate as pixels and repaints only the moving parts (18.3 ms to 3.0 ms per frame for the example at 2x, identical pixels).
  - `knob_angle`, the pointer angle for a rotary control.
  - `DrawList` records the faceplate colour so lit knobs reflect it; faders ink their scale for the panel.
- `MIN_SAMPLE_RATE`, `MAX_SAMPLE_RATE` and `sample_rate_supported` in `openplugin-core`, and `effective_rate` in `openplugin-dsp`.
- Tests that ship with the crates: hostile-input suites for CLAP events, LV2 ports, the `op_shell_*` ABI and state loading (NaN, infinity, denormals, oversized blocks, garbage indices); no-allocation tests for CLAP, LV2 and the shell; `tests/low_rates.rs`; shell proofs over `examples/editor` that check parameters, state round trip, tail, latency and bypass for any plugin.
- macOS CI: workspace tests, the main-thread Cocoa test, clippy with warnings as errors, and a minimum-Rust job. GoodLookinDri is pinned to a commit in CI.
- `docs/ROADMAP.md`.

### Changed

- **Breaking (Rust API):** `openplugin_ffi::shell::write_info` and `write_param` are now `unsafe fn` with a `# Safety` contract. The C ABI and its null behaviour are unchanged. `export_shell!` wraps the calls; direct callers must add `unsafe`.
- **Breaking (behaviour):** the accepted sample-rate range is now 1 kHz to 768 kHz in every format bridge (CLAP `activate`, LV2 `instantiate`, `op_shell_create`, `FfiInstance::prepare`). The default of `Plugin::accepts_sample_rate` was 8 kHz to 768 kHz. A plugin can narrow the range but not widen it. `FfiInstance::prepare` returns false outside the range instead of substituting a rate; `FfiInstance::new` still uses 48 kHz for an invalid rate. LV2 `instantiate` still accepts `0.0` (unknown rate) and prepares at 48 kHz.
- **Breaking (behaviour):** the DSP blocks compute for the real rate down to 1 kHz (loudness, K-weighting, `LoudnessMatch`, `SoftBypass`, `Delta`, `GainComp`, `BlockLevel`, `MeterBank`). Before, rates under 8 kHz were treated as invalid, computed as 48 kHz, or rounded up to 8 kHz. `MeterBank::new` no longer refuses low rates.
- **Breaking (appearance):** rotary controls sweep +-0.8 pi (+-144 degrees), the sweep GoodLookin design files use, instead of +-135 degrees. Knobs are drawn in the lit tier by default. `DrawList::commands` now comes from `Scene::lower_basic`, with lit knobs drawn again in the classic tier for hosts without images.
- **Breaking (build):** the workspace is edition 2024 and `rust-version` is 1.85, up from 1.80. Rust 1.85 is the oldest Rust that reads the edition-2024 crates in the dependency tree (`coreaudio-sys` through `cpal`); 1.84 cannot build it. A plugin may stay on edition 2021: the export macros write `#[unsafe(no_mangle)]`, which any edition accepts from Rust 1.82.
- Painting holds a process-wide lock, because GoodLookin DRI keeps the knob scale and floor colour process-wide.
- The `Default` impls for `Compressor` and `VuMeter` exist now (a clippy-driven addition). Targeted lint allows on public drawing and processing functions; no signatures changed.

### Fixed

- Several DSP blocks used 48 kHz or 8 kHz for rates under 8 kHz (see Changed).

## [0.3.0-beta] - 2026-11-14

Commits `06436bc` to `2b1d828`. One editor in every format.

### Added

- The editor contract, `openplugin_core::editor`: `Editor`, `EditorFactory`, `EditorHost`, `EditorState`, `Gesture`, `Frame`, `GestureQueue`, `ValueMirror`. A plugin writes one `Editor` and passes `editor = make_editor` to `export_clap!`, `export_lv2!` and `export_shell!`.
- `openplugin_dri::cocoa::EditorView` and `run_window`: a Rust `NSView` driven through the Objective-C runtime, used by CLAP (`clap.gui`), LV2 (`ui:CocoaUI` with instance access) and the VST3, Audio Unit and AAX shells. macOS only.
- `export_shell!` writes the `op_editor_*` functions in Rust and always exports `op_shell_tail_seconds` (from `Plugin::tail`). A live-instance table guards `op_editor_bind`. An opt-in trace file records what a host does with the shell.
- Shell build options (all off by default, so a plugin without an editor builds as before): `OP_EDITOR` hosts the view in VST3, Audio Unit and AAX; `OP_TAIL` reports the tail in VST3 and Audio Unit; `OP_AAX_PAGES` builds `OpenPluginAAXPages` and writes `<Product>Pages.xml` into the AAX bundle (AAX Validator's page-table tests pass).
- `Plugin::aax_ids` (fixed four-character AAX codes), `Plugin::lv2_uri`, and `AaxIds`.
- LV2: one control port per parameter (hosts could not set any before), `ui:CocoaUI`, and `write_bundle`, `manifest_ttl` and `plugin_ttl` for the bundle files.
- Presets: a `key = value` text format read through `ParamInfo::parse`, `PresetBank`, and `build::embed_files` for `build.rs`.
- `ParamInfo::parse`, the inverse of `ParamInfo::format`; CLAP `text_to_value` reads choice names, On/Off and units through it.
- DSP: `KWeighting` and `Loudness` after ITU-R BS.1770-4 (momentary, short-term, gated integrated), `LoudnessMatch` auto gain, and `Delta`.
- `examples/editor`: one plugin exported in every format, with a test that runs on the main thread and drives the editor through CLAP, LV2 and the shell.
- Copyright holder set to Marcos Quirino Deida; test fixtures and examples use "OpenGrid" as vendor.

### Fixed

- CLAP `state.load` restores the parameter snapshot that `state.save` writes for a plugin without its own state; it returned false before.
- Audio Unit: `reset` runs on the render thread instead of racing it; a failed initialize or render status is kept and logged by the view; parameter names are published as CFStrings.
- AAX: continuous parameters keep the default step count (10001 steps fails AAX Validator).
- Audio Unit reports every failed render (base class errors included) instead of rendering silence without a report.

## [0.2.0-beta] - 2026-11-11

Commits `618ed79` to `6671df8`. The host shells.

### Added

- `export_shell!`, the C ABI (`op_shell_*`, `OP_SHELL_ABI` 1) that the macOS VST3, Audio Unit and AAX adapters call, with those adapters in `native/vst3`, `native/au` and `native/aax` and the CMake build for them. macOS only. The FreEQ in-process proof reads 6.00 dB on a band-4 bell.
- `BlockConvolver`: overlap-save convolution with a 2048-point transform for a real impulse of at most 1024 taps, without allocating in `process`.
- `docs/SHELL.md`, `docs/CONVOLVE.md` and `docs/FREEQ.md` (the equaliser benchmark report).

### Changed

- CLAP state load republishes the parameter plane and ignores parameter events outside namespace 0.

## [0.1.1-beta.1] - 2026-11-11

Commits `f3ce570` to `8ab9e16` (all made after the `v0.1.0-beta.1` tag). The host plane and the host contract.

### Added

- New crate `openplugin-ffi`: `FfiInstance`, the Rust host session (parameter specs, the parameter plane, scratch rendering, presets and the OPST snapshot). A C header and `op_*` exports were added and dropped again inside this release, so none shipped.
- Host plane in `openplugin-core`: `ParamPlane` and `ParamCursor` (parameters between threads), `Seqlock` (meter frames), an SPSC queue for gestures, OPST snapshots (`encode_params` / `decode_params`), A/B compare (`AbCompare`), a MIDI 1.0 parser (`midi`), `SampleRamp`, `OnePole`, and `AutomationLane`.
- Surround layouts and an optional sidechain (`SpeakerLayout`, `IoConfig`, `Plugin::supported_layouts`, `process_buses`), published through the CLAP surround and ambisonic extensions. Also `Tail`, `ParamFlags`, `NotePorts`, latency notification, the OPCH session chunk (`encode_session` / `decode_session`), `BusMeters`, `Plugin::set_host_bypass`, `Plugin::accepts_sample_rate`, `SoftBypass`, and a wider `GainComp` limit.
- DSP: residual ADAA1 (`ResidualAdaa1`), triangle ADAA2 (`Adaa2Taylor`) and a polyphase 2x oversampler (`Oversampler2x`); a linear-phase oversampler, dry delay (`DelayLine`), gain compensation, and note, oscillator and voice helpers.
- `Plugin::set_offline` and the main-thread GUI safety rule for editors.
- GoodLookin DRI drawing in `openplugin-dri`: console faders, horizontal sliders and pan knobs; drawing comes from the GoodLookinDri crate.
- CLAP applies parameter events at their sample offset; LV2 exports a stereo descriptor.

### Changed

- **Breaking (struct literals):** `ParamInfo` gained public fields `steps` and `flags`. Code that builds it with `ParamInfo::new` and the `with_*` builders is unaffected; a struct literal no longer compiles.

## [0.1.0-beta.1] - 2026-11-06

Commit `90691a3` (tagged `v0.1.0-beta.1`). The first release.

### Added

- Workspace of `openplugin`, `openplugin-core`, `openplugin-dsp`, `openplugin-dri`, `openplugin-targets`, and the examples `gain` and `console_eq`.
- `Plugin` trait, `PluginInfo`, `ParamInfo` with tapers, `Transport`, and MIDI note hooks.
- Realtime-safe DSP: RBJ biquads, log-domain dynamics, tape and tube saturation, ballistic VU meters.
- Adapter types for CLAP, VST3, Audio Unit, AAX and Standalone. The VST3, Audio Unit and AAX types are layouts only; loadable shells for them arrive in 0.2.0-beta.
- GoodLookin DRI drawing (knobs, faceplates, meters) and `DrawList`.
- Licensed MIT OR Apache-2.0.

[Unreleased]: https://github.com/MistaMin/OpenPluginSDK/compare/a708cac...HEAD
[0.3.1-beta]: https://github.com/MistaMin/OpenPluginSDK/compare/2b1d828...a708cac
[0.3.0-beta]: https://github.com/MistaMin/OpenPluginSDK/compare/6671df8...2b1d828
[0.2.0-beta]: https://github.com/MistaMin/OpenPluginSDK/compare/8ab9e16...6671df8
[0.1.1-beta.1]: https://github.com/MistaMin/OpenPluginSDK/compare/v0.1.0-beta.1...8ab9e16
[0.1.0-beta.1]: https://github.com/MistaMin/OpenPluginSDK/releases/tag/v0.1.0-beta.1
