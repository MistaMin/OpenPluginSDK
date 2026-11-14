# Roadmap

## Summary

Each milestone makes the SDK more stable. None of them depends on a product plugin built with it. Where a milestone needs a real plugin to prove something, it uses the SDK's own examples (`examples/editor`, `examples/gain`).

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

- **Validators in CI** against the example: `clap-validator`, `auval`, AAX Validator, pluginval and `lv2_validate`.
- **Plugin state in the C++ shells:** a state call in the `op_shell_*` ABI, so `Plugin::save_state` reaches VST3, Audio Unit and AAX sessions.
- **Audio Unit editor proof** to match `VST3EditorProof`.
- **Shell gaps:**
  - AAX reports no tail.
  - AAX bypass is a dry copy that is not delayed by the reported latency (VST3 and the Audio Unit now delay it).
  - The AAX packet holds at most 64 parameters.
  - VST3 applies a parameter change at the block, using the last point in each queue, not at its sample offset.
- **Scripted host smoke tests** with the example: load, automate, save, reopen, open the editor.
- **One shell build script**, with optional signing and notarisation steps. The build does neither today.

## 0.5: a dependable editor layer

- Resizable editors and keyboard focus in the `Editor` contract.
- Hit-testing helpers for knobs, faders and switches, so an editor does not hand-code hit rectangles.
- Repaint only what changed, on top of `SceneCanvas`'s cached layer.
- An editor built from a GoodLookin design file, with each control bound to a parameter by id.

## 0.6: Windows and Linux

- **Windows:** an `HWND` editor view. VST3 and AAX shells on Windows. AAX stays C++, because the Avid SDK has no C interface.
- **Linux, standalone:** a native Wayland window. Wayland is the default session on current GNOME and KDE desktops.
- **Linux, plugin editors:** an X11 view, used only for embedding in a host. On Linux, VST3 defines one window type, X11 (`kPlatformTypeX11EmbedWindowID`). LV2 has no Wayland UI type in common use. Linux DAWs host plugin editors through XWayland even in a Wayland session. When hosts embed native Wayland surfaces, through CLAP's `wayland` window API or a later VST3 or LV2 type, the SDK adds that path. The X11 path then stays only for the hosts that need it.
- CLAP and LV2 tested on Linux. CI on macOS, Windows and Linux.

## 1.0: stable and publishable

- A frozen public API and a written policy for breaking changes.
- Publishable on crates.io. The SDK depends on GoodLookin DRI by local path today, so `openplugin = "0.3.1-beta"` from crates.io does not resolve.
- Complete API documentation, and a changelog from 1.0 on.
