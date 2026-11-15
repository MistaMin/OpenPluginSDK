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

Tasks 1 to 5 are done: plugin state reaches VST3 (a versioned trailer), the Audio Unit (its own class-info key) and AAX (chunk version 2); parameter-only plugins skip the gate; CLAP state is synchronised with `process`. Task 6 is done: `scripts/host_smoke` runs the example in REAPER 7.80 (CLAP, VST3, Audio Unit) and OpenGrid9 (VST3, Audio Unit, LV2), 105 of 106 checks pass, and `auval -strict` passes ([HOST_TESTS.md](HOST_TESTS.md)). The one failure is a real bug: VST3 shows parameter text as the normalized value instead of the plugin's own text; it is being fixed with the VST3 parity task, and 0.4.0 is tagged after that.

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

Status: tasks 1 (shell ABI 2), 5 (LV2) and 9 (downmix and upmix) are done. Tasks 2 to 4 (VST3, Audio Unit, AAX) start now on ABI 2.

Then **0.6.0**.

## 0.7: host integration and editor polish

1. **The host's parameter menu.** Right-click on a control asks the host for its menu (automation, MIDI learn): CLAP `context-menu`, VST3 `IComponentHandler3`. Formats without one show the SDK's own small menu. Owns a hook in `EditorHost`, the editor parts of each format. Proof: CLAP and VST3 proofs open the host menu for a parameter.
2. **Track name and colour.** CLAP `track-info` and VST3 `IInfoListener` reach the plugin and the editor. Owns `openplugin-core`, the CLAP and VST3 entries. Proof: tests set the track and read it back.
3. **Loading files off the audio thread.** A worker in `openplugin-core` runs jobs (impulse responses, samples) on a background thread and hands the result to `process` through a lock-free swap, freeing the old one off the audio thread; LV2 uses its `worker` extension. Owns a new core module, `lv2.rs` worker parts. Proof: a convolution example loads a new IR during playback with no allocation or lock on the audio thread.
4. **Per-note expression.** MPE, CLAP note expressions and polyphonic modulation, VST3 note expressions, routed to the voices in `openplugin-dsp`. Owns the note parts of each entry, `openplugin-dsp/src/voice.rs`. After 0.6. Proof: tests send per-note pitch, pressure and timbre to one voice and see only that voice change.
5. **Accessibility.** Controls from the registry appear to VoiceOver with name, value and role, and respond to its increment and decrement. Owns `cocoa.rs` and the registry. Proof: the main-thread test reads the accessibility tree.
6. **Typing a value, tooltips, files.** Double-click or a key opens a text field that parses through `ParamInfo::parse`; controls show tooltips; the editor can open a file dialog and accept dropped files. Owns `cocoa.rs`, the registry. Proof: the main-thread test types a value and drops a file.

Status: task 3 is done, including LV2: `openplugin_core::worker` (proven allocation-free on the audio side, with an impulse swap during playback) and its `Executor` hook, through which the LV2 entry runs jobs on the host's `worker` thread when the host offers `worker:schedule` (fake host worker tests, fallback, hostile calls, no allocation on `run`, `lv2_validate`), with the loader's own thread as the fallback.

Status: task 5 is done (`Editor::accessibility_nodes`, `accessibility_action` and `accessibility_focus`; `ControlSet` and `DesignEditor` provide them automatically; the Cocoa view exposes `NSAccessibilityElement` children with name, value, role and screen frame, increment, decrement and press as mouse-equivalent gestures, focus following and value-changed notifications; proven by the `a11y` main-thread tests; speech by VoiceOver itself is not verified headlessly).

Status: task 6 is done (`Editor::take_text_edit`, `text_edit_commit`, `text_edit_cancel`, `tooltips`, `wants_drops`, `accepts_drop`, `files_dropped`, `take_file_dialog`, `file_dialog_done`; double-click types a value and Option-double-click resets, configurable; proven by the `typing` main-thread tests of both editor examples; the real sheet, tooltip bubble and drag cursor are not verified headlessly).

Then **0.7.0**.

## 1.0: stable and publishable on macOS

1. **Crate metadata.** Every crate inherits the workspace's `authors`, `homepage`, `keywords` and `categories`; unused dependencies (`log` in core, `cpal` in targets) are removed or justified; `cargo publish --dry-run` passes for every crate that does not depend on an unpublished path.
2. **GoodLookin DRI as a versioned dependency.** The SDK names GoodLookin DRI by version and by path, so it builds here and publishes once GoodLookin DRI is on crates.io. Publishing GoodLookin DRI is its owner's step.
3. **API documentation.** `missing_docs` on for every public item, examples in the docs that compile, docs.rs metadata.
4. **API review.** A list of every public item with keep, rename, hide or remove, for the owner to decide the freeze. Nothing is removed without that decision.
5. **Changelog and breaking-change policy.** `CHANGELOG.md` from 0.1 to the release, and a written policy for breaking changes after 1.0.
6. **One sound in every format.** A test renders the same input and automation through CLAP, LV2, VST3, the Audio Unit and AAX and checks the outputs match the Rust engine's, sample for sample within float tolerance. Owns `scripts/format_parity/*`. Proof: the test passes for the examples and fails on a deliberately altered shell.
7. **Installer.** `scripts/build_plugin.sh` can wrap the built bundles in a `.pkg` (`pkgbuild` and `productbuild`), one component per format, signed with a Developer ID Installer identity and notarised when the environment asks. Owns `scripts/*`, `docs/BUILD.md`. Proof: `pkgutil --payload-files` lists every bundle at its install path; the package is never installed by the test.

8. **Universal binaries, proven.** `--archs universal` already joins arm64 and x86_64 with `lipo`, but only the CLAP and LV2 were checked. Build every format universal, the AAX SDK library included, run the x86_64 slice of every proof, the format-parity test and `clap-validator` under Rosetta, and add a CI job that does the same. Owns `scripts/build_plugin.sh`, `native/CMakeLists.txt` architecture settings, CI. Proof: `lipo -archs` lists both slices in every bundle, and every proof passes on each slice.

Status: tasks 1, 2, 4, 5, 6, 7 and 8 are done (6: `scripts/format_parity`, every format matches the Rust engine exactly on the example, with negative controls; 8: every format builds universal and every proof passes on both slices, x86_64 under Rosetta) (7: `--pkg`, `scripts/make_pkg.sh`, `scripts/test_pkg.sh`; signing and notarising the package proven only as a dry run). Task 3 comes last, after 0.7, because it touches every public item.

The freeze itself and the crates.io publish are the owner's decisions.

## DSP library: building blocks for good products

This track runs beside the milestones. `openplugin-dsp` has a starter set: an RBJ biquad, one compressor, saturation curves with ADAA, 2x and linear-phase oversampling, a short convolver (1024 taps), loudness after BS.1770 and R 128, meters, a fixed delay, a naive oscillator, a sample voice, soft bypass and delta listening. It has no dither, no state-variable or crossover filters, no limiter or gate, no long convolution, no modulated delay or reverb, no band-limited oscillator or envelope, and no STFT. Every block below is allocation-free on the audio path, computes for the real sample rate (`effective_rate`), stays free of denormals, cites the paper or standard it follows, and is written from that source, never ported from JUCE or from code under a licence the SDK cannot carry. A cookbook in `docs/DSP_COOKBOOK.md` shows each one inside a small plugin.

Done: tasks 1 to 4 (dither and noise shaping; SVF, matched biquads, Linkwitz-Riley crossovers and Butterworth cascades; envelopes, gain computer, linked dynamics, true peak and a lookahead limiter; long partitioned convolution with a zero-latency head). Open items from them: the matched biquad is not safe under per-sample modulation (use the SVF), full-band white noise can pass the limiter's true-peak ceiling by up to 1.7 dB, and the long convolver's worst block is about 15 times its average.

Blocks that land before 1.0 are frozen with it. Later ones arrive in minor releases, since adding a module breaks nothing.

1. **Dither and noise shaping.** RPDF and TPDF dither to any word length from 8 to 24 bits, first-order and higher-order noise shaping from published curves, an independent generator per channel with a fixed seed for tests, auto-blanking on digital silence, and dithered paths for the `pcm` conversions. Proof: noise floor and spectrum against theory; zero output for silence; a truncation test shows the distortion dither removes.
2. **Filters.** A topology-preserving state-variable filter (stable under fast modulation), analog-matched biquads with no cramping near Nyquist, Linkwitz-Riley crossovers for two to four bands that sum flat, one-pole, DC blocker, allpass and Butterworth cascades. Proof: responses against the analog prototypes; crossover sum within 0.01 dB; modulation stress without blow-up.
3. **Dynamics.** Envelope followers (peak and RMS, attack, release, hold), soft knee, gate and expander, a lookahead limiter with a true-peak ceiling, sidechain filters, and linked detection across channels. Proof: static curves, time constants, a ceiling never exceeded on true-peak test signals.
4. **Long convolution.** Uniformly and non-uniformly partitioned convolution for impulses of seconds, a zero-latency head, mono, stereo and true-stereo, impulses loaded through the 0.7 worker. Proof: equal to direct convolution; CPU per block measured; no allocation.
5. **Modulation and delay.** Fractional delays (Hermite, Lagrange, Thiran allpass), modulated delay lines, an LFO that syncs to the host transport, and chorus, flanger and phaser building blocks. Proof: group delay against theory; LFO phase locked to transport.
6. **Reverb.** A feedback delay network after Jot, the plate after Dattorro, early-reflection taps, damping per band. Proof: decay time against the setting; no instability at any setting.
7. **Synthesis.** Band-limited oscillators (PolyBLEP and BLAMP), mip-mapped wavetables, ADSR envelopes, white and pink noise, voice allocation with stealing and glide, a windowed-sinc resampler for sample playback. Proof: aliasing measured against the naive oscillator; envelope timing.
8. **Spectral.** An STFT frame with windows (Hann, Blackman-Harris, Kaiser), hop and overlap-add with a constant-overlap check, and lock-free spectrum data for editors. Proof: identity resynthesis; analyser bins against known tones.
9. **Utility.** Mid/side, width, pan laws (-3, -4.5 and -6 dB), balance, polarity, dB gain with smoothing, and a denormal guard that sets flush-to-zero on the audio thread. Proof: unit tests per block.
10. **Metering.** True peak after BS.1770, PPM ballistics (EBU, DIN, Nordic, BBC), VU checked against IEC 60268-17, the K-system, loudness range after EBU Tech 3342, BS.1770 channel weights for surround by channel role (surrounds +1.5 dB, LFE excluded), an EBU Mode meter after EBU Tech 3341, loudness targets (EBU R 128, ATSC A/85, streaming guidance) as presets, and correlation and goniometer feeds for editors. Proof: the EBU Tech 3341 and 3342 conformance signals read within their published tolerances; test signals from the other standards.
11. **Virtual analog.** Zero-delay-feedback ladder and diode filters, Sallen-Key and Korg-35 style filters, after Zavalishin; wave digital filters (adaptors, a diode and a triode model after the published WDF literature) for circuit models; tape after Chowdhury's published hysteresis model (Jiles-Atherton). Proof: responses against circuit simulations or the papers' figures; stability at every setting; aliasing measured with and without oversampling.
12. **Multiband and dynamic EQ.** Multiband compression on the crossovers of task 2, dynamic EQ bands on the filters of task 2 driven by task 3's detectors, a transient shaper, a de-esser, and a clipper that limits true peak. Proof: band sums flat at unity gain; static and dynamic curves; true-peak ceiling held.
13. **FIR design and phase.** Windowed and least-squares FIR design, linear-phase and minimum-phase EQ from a magnitude curve (cepstral method), and minimum-phase oversampling filters. Proof: designed responses against the target; minimum-phase group delay against linear-phase.
14. **Time and pitch.** A phase vocoder with phase locking for time stretch and pitch shift, a low-latency delay-line pitch shifter, and pitch detection (YIN and McLeod's MPM). Proof: known tones shifted and detected within stated cents; transient smearing measured.
15. **Spectral repair.** Spectral gating and noise reduction from a learned noise profile, on the STFT frame of task 8. Proof: SNR improvement on test mixtures; identity when disabled.
16. **Physical modelling.** Karplus-Strong and extended variants, digital waveguides, modal resonator banks. Proof: partial frequencies and decay against theory.
17. **Neural models.** Small dense, GRU and LSTM layers for amp and pedal models, weights loaded from a documented JSON format, inference allocation-free and vectorised. Proof: outputs against a reference implementation of the same weights; CPU per block measured.
18. **Vectorised kernels.** NEON and SSE/AVX paths for the hot loops (biquad cascades across channels, FIR, convolution multiply-accumulate, gain and mix), with a scalar fallback and identical results within stated tolerance, selected at run time. Proof: equality tests against scalar; speed-up measured.

## Performance and stability: tools and a guide

1. **A conformance kit for any plugin.** `openplugin::testing` (or a dev-dependency crate) runs a plugin developer's own `Plugin` through what hosts do to it: every sample rate from 22.05 to 384 kHz, block sizes from 1 to 8192 and varying per call, silence, DC, full-scale noise, NaN and infinity in, parameter fuzz at sample offsets, reset in the middle of a tail, state round-trips, layouts, offline switching. It checks: no allocation, lock or panic on the audio path; no NaN, infinity or denormal out; latency and tail as reported; state round-trip exact; output bounded. One call in a plugin's tests. Proof: passes on the examples; each check fails on a deliberately broken test plugin.
2. **A benchmark harness.** CPU per block for any `Plugin` across sample rates, block sizes and channel counts, as a fraction of real time, with a stable report to compare commits. Proof: reports for the examples; a deliberate slowdown shows.
3. **The performance guide.** `docs/PERFORMANCE.md`: the real-time rules and how the kit checks them; denormals and flush-to-zero; block versus per-sample work; parameter smoothing cost; memory layout (structure of arrays, cache lines); helping the Rust compiler vectorise (chunks, slices without bounds checks, `#[inline]`, LTO, codegen units, target CPU); fast approximations of `tanh`, `exp` and `sin` with stated error; oversampling cost; measuring with Instruments and the harness. Every claim shown with a measured example in the repo.
4. **The stability guide.** `docs/STABILITY.md`: numerically stable filter topologies and when to use `f64`; guarding state against NaN; panics never crossing the format boundary; thread rules for state, editor and audio; host behaviours to survive (block size changes, rate changes without a reset, process before activate, state loads during playback); soak and fuzz testing with the kit. Each rule linked to the test that enforces it in the SDK.

Status: tasks 1 and 2 are done (`openplugin-testing`: `conformance` with 13 checks and `bench` with commit comparison; it found NaN and denormal bugs in two examples and in `Biquad`, now fixed). Tasks 3 and 4 are next.

## Porting toolkit: moving a plugin from another SDK

A team moving a shipped plugin needs three things: the new build must load the old one's sessions and presets, sound the same, and take days rather than months. The toolkit works from the built plugin, through the formats' public interfaces, so it serves JUCE, iPlug2, DPF (DISTRHO), nih-plug, and plugins written straight against the VST3, CLAP or Audio Unit SDKs alike. It never copies or reads another SDK's source: concept maps name the other SDK's public API, and every tool works on bundles, binaries and saved states.

1. **Plugin scanner.** `openplugin-port scan <bundle>` loads an existing CLAP, VST3 or Audio Unit from its path and writes a manifest: class and component IDs, Audio Unit type, subtype and manufacturer, AAX IDs from the bundle's plist where present, bus layouts, latency, tail, and every parameter's ID, name, range, default, steps and value strings. It also writes the `ParamInfo` table and the identity settings for the SDK. Rust, hosting through the formats' C interfaces. Proof: scanning the SDK's own built examples reproduces their tables exactly; scanning any installed third-party plugin read-only succeeds.
2. **Identity pinning.** A plugin can set every ID a host stores explicitly: the VST3 component and controller class IDs, the VST3 parameter ID of each parameter (not only its index), Audio Unit codes, AAX manufacturer, product and plug-in IDs, the CLAP ID and parameter IDs. A host then treats the port as the same plugin. After 0.6. Proof: a session and an automation lane saved with one build reopen in a second build with a different parameter order.
3. **Foreign state import.** `Plugin::load_state` gets a version-aware path for chunks written by the old plugin, with helpers for common containers (XML, binary key-value, plists) and a capture tool that saves the old plugin's state for each of its presets. Proof: a test loads captured states into the port and checks every parameter.
4. **Null test against the original.** Render the old plugin and the port through the same input and automation and compare, reusing the 1.0 format-parity machinery. Proof: identical builds null; a deliberately changed port fails with the first differing sample reported.
5. **Concept maps and guides.** One guide each for JUCE, iPlug2, DPF, nih-plug and the raw VST3, CLAP and Audio Unit SDKs: their processor, parameters, state, editor, buses, MIDI and threading mapped onto the SDK's, with a worked port of a small effect. After 0.7, so the API they describe is settled.
6. **DSP from Faust.** A build helper compiles a Faust `.dsp` with Faust's Rust backend and wraps it as a `Plugin`, with parameters from its UI metadata. Faust's compiler is GPL but its generated code is not; the guide states the licence terms. Proof: a Faust filter builds and renders through every format. Needs the Faust compiler installed by the owner.
7. **C++ DSP bridge.** For teams that ship before rewriting, a documented pattern for calling existing C++ DSP from a Rust plugin through `cxx`, allocation rules included. This one is C++ by nature: its job is to reuse code a team already has, until it is rewritten in Rust.
8. **Scaffolding.** `openplugin-port new --from manifest.toml` writes a plugin crate with the pinned identity, the parameter table, the state importer stub and the null test ready to run.

## After 1.0

- **Binaural rendering.** Any SDK layout to headphones through head-related impulse responses, convolved per channel with `openplugin-dsp`'s convolver, loaded through the 0.7 worker. Needs an HRTF set whose license allows shipping it, or one the user supplies; choosing the set is the owner's decision.
- **AUv3.** An app extension for iPad, iPhone and the Mac, built on the shell ABI. It is a new native shell and an app container, so it is its own milestone.
- **Windows and Linux.** See [ROADMAP_WINDOWS_LINUX.md](ROADMAP_WINDOWS_LINUX.md).

