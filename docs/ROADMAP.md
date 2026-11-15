# Roadmap

## Summary

Each milestone makes the SDK more stable. None of them depends on a product plugin built with it. Where a milestone needs a real plugin to prove something, it uses the SDK's own examples (`examples/editor`, `examples/gain`).

This is the macOS roadmap, from 0.4 to 1.0 and what comes after. Windows and Linux have their own: [ROADMAP_WINDOWS_LINUX.md](ROADMAP_WINDOWS_LINUX.md). Each open item below is one task: what it changes, which files it owns, and what proves it done.

The editor work follows the plan of [GoodLookin DRI](https://github.com/MistaMin/GoodLookinDri), which the SDK draws with: 0.3 interaction, 0.4 host polish, 0.5 qualification.

## Where 0.3.1 Beta stands

| Area | State |
|---|---|
| Formats | CLAP and LV2 are Rust entries. VST3, Audio Unit and AAX are C++ shells over the Rust `op_shell_*` ABI. macOS only for the shells. |
| Editor | One Rust `Editor` in every format, hosted in a Rust `NSView`. macOS only. Fixed size. |
| Drawing | GoodLookin DRI 0.2: lit knobs by default, `SceneCanvas` with a cached plate layer. |
| Proofs | Unit tests, hostile-input and no-allocation suites, a main-thread editor test through CLAP, LV2 and the shell ABI, generic VST3, AU and AAX shell proofs, `VST3EditorProof`, `lv2_validate`, a render through lilv's `lv2apply`, and `clap-validator` with no failures (34 passed, 10 skipped). |
| Not claimed | Any session in Logic, Cubase, Pro Tools, Bitwig or REAPER. Windows and Linux. |

## Minimum Rust version

The SDK builds and is tested on current stable Rust. `rust-version` in `Cargo.toml` is the oldest Rust a plugin may use to depend on it, and CI checks the workspace on exactly that version on every push. It is 1.85: the oldest Rust that builds the dependency tree.

- Raise it only when a dependency needs a newer Rust, or when a language or library feature fixes a real problem in the SDK. Not to follow stable.
- Keep it at or below the Rust that the current Debian stable release ships, so the Linux milestone can build with distribution packages.
- Raise it only in a minor release, and say so in the release notes with the reason.
- The SDK is written in edition 2024, so every unsafe operation inside an `unsafe fn` sits in its own `unsafe` block. Plugins may stay on edition 2021: the export macros write `#[unsafe(no_mangle)]`, which any edition accepts from Rust 1.82.

## 0.3.x: harden what exists

- **Generic shell proofs.** `vst3_proof`, `au_proof` and `aax_process_proof` check FreEQ's parameters and pass only for FreEQ. Replace them with proofs over the SDK's example that check the contract: parameters, state round-trip, tail, latency and bypass.
- **Clippy clean.** `write_info` and `write_param` dereference raw pointers in safe functions. Make them `unsafe`, or validate the pointers first.
- **Sample-rate range.** The default `accepts_sample_rate` refuses rates below 8 kHz, which `clap-validator` reports. Widen it, or document why not.
- **Hostile input everywhere.** Fuzz CLAP events, LV2 ports, `op_shell_*` and state loading with garbage, NaN and out-of-range values. Test that the audio path does not allocate in every format, not only in the FFI session.
- **CI on macOS.** The workspace tests, the main-thread Cocoa test, and the minimum Rust version on every push.

### Status

Done: generic shell proofs over `examples/editor`; clippy clean with warnings as errors, checked in CI; rates from 1 kHz to 768 kHz in every bridge, with every DSP block computing for the real rate (`effective_rate`, `tests/low_rates.rs`); hostile-input suites for CLAP, LV2, the shell ABI and state loading; no-allocation tests for CLAP, LV2 and the shell; macOS CI with the minimum-Rust check, GoodLookinDri pinned to a commit. The minimum is Rust 1.85, the first release that reads the edition-2024 crates in the dependency tree (`coreaudio-sys` through `cpal`); 1.84 cannot build it. It was 1.80, with no recorded reason.

The three gaps left after the first pass are closed:

- Every format proves host bypass: VST3 (a new `kIsBypass` parameter) and the Audio Unit output the input delayed by the reported latency with a 10 ms crossfade, AAX copies the dry signal, and CLAP's `PARAM_IS_BYPASS` reaches `Plugin::set_host_bypass`. VST3 and the Audio Unit are also proven against a test bundle that reports 37 samples of latency.
- AAX saves and restores state through its own versioned chunk, refuses corrupt chunks without touching any parameter, and proves the round trip.
- The C++ process paths of VST3, the Audio Unit and AAX are proven not to allocate, counting every malloc-family call in the process, the Rust engine included, with a self-check and a negative control.
- CI builds and runs the VST3 and Audio Unit shell proofs against pinned public SDKs (VST3 SDK v3.8.0, MIT; AudioUnitSDK 1.4.0, Apache-2.0), and runs `clap-validator` 0.4.1 on the example. The AAX proofs run only locally: the Avid SDK is not redistributable.

Still open, carried into 0.4:

- The C++ shells save parameter values only. A plugin's own state (`Plugin::save_state`) does not reach VST3, Audio Unit or AAX sessions; that needs a state call in the shell ABI shared by all three.
- VST3 applies bypass and parameter changes at the block, not at their sample offset.

## 0.4: format conformance on macOS

### Done

- **Plugin state in the shell ABI.** `op_shell_save_state` and `op_shell_load_state` carry the parameters and the plugin's own chunk, all or nothing, safe beside the audio thread: a gate lets a state call take the plugin between blocks, and a block that starts meanwhile passes the input through dry. The existing `save_bytes`/`load_bytes` race with `process` is fixed by the same gate.
- **VST3 sample-accurate changes.** Parameter changes and the bypass crossfade start at their sample offset; the block is rendered in segments between change points.
- **AAX gaps.** Bypass is the input delayed by the reported latency with a 10 ms crossfade, like VST3 and the Audio Unit. The controls packet is sized by the parameter count (shell bound 4096; proven with 200). AAX has no tail report: that is a format limit, documented from the AAX SDK.
- **Audio Unit editor proof** (`au_editor_proof`), the counterpart of `vst3_editor_proof`.
- **Validators.** CI runs `auval` (normal and strict), pluginval at strictness 10 on the VST3 and the Audio Unit, `lv2_validate`, and a render through lilv's `lv2apply`, besides `clap-validator`. Avid's AAX Validator runs locally with `scripts/aax_validate.sh` (13 passed, 0 failed, 1 skipped: the cycle-count test needs HDX hardware).
- **One build script.** `scripts/build_plugin.sh` builds CLAP, LV2, VST3, Audio Unit and AAX for one plugin, keeps build paths out of the binaries, and signs (Developer ID), notarises and PACE-signs when the environment asks for it ([BUILD.md](BUILD.md)).

### Status

Tasks 1 to 5 are done: plugin state reaches VST3 (a versioned trailer), the Audio Unit (its own class-info key) and AAX (chunk version 2); parameter-only plugins skip the gate; CLAP state is synchronised with `process`. Task 6 is running.

### Tasks

1. **Plugin state in VST3.** `getState`/`setState` carry the `op_shell_save_state` blob after the parameter values and the bypass flag, versioned; a state written by 0.3.x still loads. Owns `native/vst3/*`. Proof: a plugin with its own chunk round-trips through the VST3 component, old states load, corrupt ones are refused unchanged.
2. **Plugin state in the Audio Unit.** The class-info dictionary carries the blob under its own key; an old preset without it still loads. Owns `native/au/*`. Same proof through `kAudioUnitProperty_ClassInfo`.
3. **Plugin state in AAX.** Chunk version 2 carries the blob; version 1 still loads. Owns `native/aax/*`. Same proof through the chunk calls.
4. **No dry block for parameter-only plugins.** A plugin whose state is its parameters alone saves and loads from the parameter plane without the gate, so a host saving during playback never hears a dry block. Owns `crates/openplugin-ffi`, `crates/openplugin-core`. Proof: a parameter-only plugin's audio is untouched by saves during playback; a plugin with a chunk still round-trips.
5. **CLAP state beside the audio thread.** The CLAP adapter calls `save_state`/`load_state` from the main thread without synchronisation, the race the shell gate fixed. Give CLAP the same guarantee. Owns `crates/openplugin-targets/src/clap.rs`. Proof: a concurrency test with `process` running during saves and loads.
6. **Host smoke tests.** A scripted REAPER run with the example (CLAP and VST3) that loads it from a build folder through a separate REAPER configuration, automates a parameter, saves the project, reopens it and checks the value, opens and closes the editor. Nothing is installed into plug-in folders; the Audio Unit run waits for an install the owner approves. Owns `scripts/host_smoke/*`, `docs/HOST_TESTS.md`.

Then **0.4.0**: version bump, notes, CI green.

## 0.5: a dependable editor layer

1. **Resizable editors: contract and Rust hosts.** `Editor` gains a size range and `resize`; the Cocoa view follows its parent; CLAP (`can_resize`, `get_resize_hints`, `adjust_size`, `set_size`) and LV2 (`ui:resize`) honour it; `SceneCanvas` re-renders at the new size. Owns `crates/openplugin-core/src/editor.rs`, `crates/openplugin-dri/src/cocoa.rs`, `crates/openplugin-dri/src/render.rs`, the gui parts of `clap.rs` and `lv2.rs`, the `op_editor_*` functions. Proof: the main-thread Cocoa test resizes through CLAP and LV2 and checks the pixels.
2. **Resizable editors in the C++ shells.** VST3 (`IPlugView::onSize`, `canResize`, `checkSizeConstraint`), the Audio Unit view and the AAX view follow the editor's size range. Owns `native/*/` view code. Proof: `vst3_editor_proof` and `au_editor_proof` resize and check the view. After task 1.
3. **Hit-testing helpers.** A control registry in `openplugin-dri`: knobs, faders, switches and keys with hit areas, drag and wheel handling, double-click reset, fine adjustment, and gestures through `EditorHost`. Owns a new module in `openplugin-dri`. Proof: unit tests driving mouse sequences into gestures.
4. **Keyboard input and focus.** Key events in the `Editor` contract, focus between controls, keys that the host should keep (transport, shortcuts) passed back. Owns `editor.rs`, `cocoa.rs`. After task 1.
5. **Repaint only what changed.** Controls report damage; `SceneCanvas` re-rasterises damaged rectangles on top of the cached layer; the Cocoa view invalidates only those. Owns `render.rs`, `cocoa.rs`. Proof: identical pixels to a full repaint, and a measured frame time. After tasks 1 and 4.
6. **Editors from design files.** Build an editor from a GoodLookin design CSV: each row a control from task 3, bound to a parameter by id, laid out with `host::map_rect`. Owns a new module in `openplugin-dri` and an example. After task 3.

Then **0.5.0**.

Status: tasks 1, 2, 3, 4 and 6 are done. Task 5 is running.

## 0.6: every format carries everything

The Rust `Plugin` trait already has notes, MIDI output, transport, sidechain, channel layouts and offline rendering, and CLAP passes them all through. The shell ABI (`native/include/op_shell.h`) carries audio, parameters, state, latency and tail only, so VST3, the Audio Unit and AAX get none of them, and the LV2 entry has no MIDI or time either. 1.0 freezes the shell ABI, so this comes first.

1. **Shell ABI: events, transport, buses and kind.** `op_shell.h` gains a sample-offset event list into `op_shell_process` (note on and off, raw MIDI, parameter changes), MIDI output pulled after the block, a transport struct, a sidechain input, layout queries and selection over `supported_layouts`, an offline flag, and the plugin's kind (effect, instrument, note effect) from `PluginInfo`. Versioned so a shell built for 0.5 refuses a mismatched library. Owns `native/include/op_shell.h`, `crates/openplugin-ffi`, `PluginInfo` in `openplugin-core`. Proof: FFI host tests drive notes, MIDI out, transport, sidechain and offline through the ABI with no allocation.
2. **VST3 parity.** Event bus in and out (`IEventList`), `ProcessContext` to the transport, an aux input bus for the sidechain, `setBusArrangements` over the plugin's layouts, `kOffline` process mode, the instrument or note-effect category from the kind. Owns `native/vst3/*`. After task 1. Proof: `vst3_proof` plays notes, reads MIDI out, checks the transport and the sidechain; pluginval strictness 10.
3. **Audio Unit parity.** `aumu` and `aumf` types from the kind, `MusicDeviceMIDIEvent` and `kAudioUnitProperty_MIDIOutputCallback`, transport from `HostCallbackInfo`, a sidechain input element, channel layouts through `SupportedNumChannels` and `AudioChannelLayout`, `kAudioUnitProperty_OfflineRender`. Owns `native/au/*`. After task 1. Proof: `au_proof` and `auval -strict` for an effect, an instrument and a note effect.
4. **AAX parity.** MIDI input and output nodes, `AAX_ITransport`, a sidechain input, stem formats from the layouts, the plugin category from the kind. Owns `native/aax/*`. After task 1. Proof: the AAX proofs and `scripts/aax_validate.sh`.
5. **LV2 parity.** Atom MIDI input and output, `time:Position` to the transport, a sidechain port (`lv2:isSideChain`), `lv2:freeWheeling` to the offline flag, `lv2:InstrumentPlugin` from the kind. Owns `crates/openplugin-targets/src/lv2.rs`. Proof: tests through the LV2 entry and `lv2_validate`; a synth renders through `lv2apply`.
6. **Parameter groups.** `ParamInfo` gains a group path; CLAP module paths, VST3 units, Audio Unit clumps, AAX pages and LV2 port groups show it. After tasks 1 to 5. Proof: each format's proof reads the groups back.
7. **Telling the host about changes.** A plugin reports a new latency or new parameter info at run time: CLAP `latency.changed` and `params.rescan`, VST3 `restartComponent`, Audio Unit property notifications, AAX `SetSignalLatency`, LV2 latency port. After tasks 1 to 5. Proof: each format's proof changes latency mid-session and sees the host told.
8. **One channel order in every format.** A plugin always sees surround channels in the SDK's order (the CLAP order: L R C LFE, then sides, backs and heights), whatever the host uses. Each shell reorders at the edge: VST3 speaker arrangements, Audio Unit channel layout tags, Pro Tools' film order (L C R Ls Rs LFE) in AAX, LV2 port order. Helpers in `layout.rs` find a channel by role (`index_of(SURROUND_LFE)`) and iterate the non-LFE channels, so plugin code never hard-codes an index. Adds the Atmos beds 5.1.2, 5.1.4, 7.1.2 and 9.1.6. Owns `layout.rs` and the layout parts of each entry. After tasks 1 to 5. Proof: a test plugin that tags each channel with its role passes every layout through every format, and every role arrives at the right index.
9. **Downmix and upmix matrices.** A module in `openplugin-dsp` folds any SDK layout to stereo or mono (ITU-R BS.775 for 5.x; documented, matching folds for 6.1, 7.1 and the Atmos beds), with the centre, surround and LFE levels as parameters, and the matching passive upmix. Owns a new `openplugin-dsp` module. Proof: unit tests against the published coefficients, energy checks, no allocation.
10. **Stereo plugins on surround buses.** A plugin written for stereo opts in to an adapter that runs it on a surround bus: one engine per channel pair (L/R, sides, backs, height pairs), the centre as mono, the LFE processed, passed or muted by the plugin's choice, every parameter linked, and an optional shared detector so dynamics move every channel together. A `fold to stereo` mode uses task 9 for surround in, stereo out. Opt-in only: wideners and reverbs that need the speaker positions keep their own surround code. Owns a new `openplugin-core` adapter module and the layout lists the entries publish. After tasks 8 and 9. Proof: a stereo test plugin on 5.1 and 7.1.4 matches running the engines by hand, with linked gain reduction, in every format.

Then **0.6.0**.

## 0.7: host integration and editor polish

1. **The host's parameter menu.** Right-click on a control asks the host for its menu (automation, MIDI learn): CLAP `context-menu`, VST3 `IComponentHandler3`. Formats without one show the SDK's own small menu. Owns a hook in `EditorHost`, the editor parts of each format. Proof: CLAP and VST3 proofs open the host menu for a parameter.
2. **Track name and colour.** CLAP `track-info` and VST3 `IInfoListener` reach the plugin and the editor. Owns `openplugin-core`, the CLAP and VST3 entries. Proof: tests set the track and read it back.
3. **Loading files off the audio thread.** A worker in `openplugin-core` runs jobs (impulse responses, samples) on a background thread and hands the result to `process` through a lock-free swap, freeing the old one off the audio thread; LV2 uses its `worker` extension. Owns a new core module, `lv2.rs` worker parts. Proof: a convolution example loads a new IR during playback with no allocation or lock on the audio thread.
4. **Per-note expression.** MPE, CLAP note expressions and polyphonic modulation, VST3 note expressions, routed to the voices in `openplugin-dsp`. Owns the note parts of each entry, `openplugin-dsp/src/voice.rs`. After 0.6. Proof: tests send per-note pitch, pressure and timbre to one voice and see only that voice change.
5. **Accessibility.** Controls from the registry appear to VoiceOver with name, value and role, and respond to its increment and decrement. Owns `cocoa.rs` and the registry. Proof: the main-thread test reads the accessibility tree.
6. **Typing a value, tooltips, files.** Double-click or a key opens a text field that parses through `ParamInfo::parse`; controls show tooltips; the editor can open a file dialog and accept dropped files. Owns `cocoa.rs`, the registry. Proof: the main-thread test types a value and drops a file.

Then **0.7.0**.

## 1.0: stable and publishable on macOS

1. **Crate metadata.** Every crate inherits the workspace's `authors`, `homepage`, `keywords` and `categories`; unused dependencies (`log` in core, `cpal` in targets) are removed or justified; `cargo publish --dry-run` passes for every crate that does not depend on an unpublished path.
2. **GoodLookin DRI as a versioned dependency.** The SDK names GoodLookin DRI by version and by path, so it builds here and publishes once GoodLookin DRI is on crates.io. Publishing GoodLookin DRI is its owner's step.
3. **API documentation.** `missing_docs` on for every public item, examples in the docs that compile, docs.rs metadata.
4. **API review.** A list of every public item with keep, rename, hide or remove, for the owner to decide the freeze. Nothing is removed without that decision.
5. **Changelog and breaking-change policy.** `CHANGELOG.md` from 0.1 to the release, and a written policy for breaking changes after 1.0.
6. **One sound in every format.** A test renders the same input and automation through CLAP, LV2, VST3, the Audio Unit and AAX and checks the outputs match the Rust engine's, sample for sample within float tolerance. Owns `scripts/format_parity/*`. Proof: the test passes for the examples and fails on a deliberately altered shell.
7. **Installer.** `scripts/build_plugin.sh` can wrap the built bundles in a `.pkg` (`pkgbuild` and `productbuild`), one component per format, signed with a Developer ID Installer identity and notarised when the environment asks. Owns `scripts/*`, `docs/BUILD.md`. Proof: `pkgutil --payload-files` lists every bundle at its install path; the package is never installed by the test.

Status: tasks 1, 2, 4 and 5 are done. Task 3 comes last, after 0.7, because it touches every public item.

The freeze itself and the crates.io publish are the owner's decisions.

## After 1.0

- **Binaural rendering.** Any SDK layout to headphones through head-related impulse responses, convolved per channel with `openplugin-dsp`'s convolver, loaded through the 0.7 worker. Needs an HRTF set whose license allows shipping it, or one the user supplies; choosing the set is the owner's decision.
- **AUv3.** An app extension for iPad, iPhone and the Mac, built on the shell ABI. It is a new native shell and an app container, so it is its own milestone.
- **Windows and Linux.** See [ROADMAP_WINDOWS_LINUX.md](ROADMAP_WINDOWS_LINUX.md).

