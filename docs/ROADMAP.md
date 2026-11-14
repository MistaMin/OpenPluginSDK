# Roadmap

## Summary

Each milestone makes the SDK more stable. None of them depends on a product plugin built with it. Where a milestone needs a real plugin to prove something, it uses the SDK's own examples (`examples/editor`, `examples/gain`).

The editor work follows the plan of [GoodLookin DRI](https://github.com/MistaMin/GoodLookinDri), which the SDK draws with: 0.3 interaction, 0.4 host polish, 0.5 qualification.

## Where 0.3.0 Beta stands

| Area | State |
|---|---|
| Formats | CLAP and LV2 are Rust entries. VST3, Audio Unit and AAX are C++ shells over the Rust `op_shell_*` ABI. macOS only for the shells. |
| Editor | One Rust `Editor` in every format, hosted in a Rust `NSView`. macOS only. Fixed size. |
| Drawing | GoodLookin DRI 0.2: lit knobs by default, `SceneCanvas` with a cached plate layer. |
| Proofs | Unit tests, a main-thread editor test through CLAP, LV2 and the shell ABI, `VST3EditorProof`, `lv2_validate`, a render through lilv's `lv2apply`, and `clap-validator` with one failure (the 8 kHz minimum rate). |
| Not claimed | Any session in Logic, Cubase, Pro Tools, Bitwig or REAPER. Windows and Linux. |

## Minimum Rust version

The SDK builds and is tested on current stable Rust. `rust-version` in `Cargo.toml` is the oldest Rust a plugin may use to depend on it, and CI checks the workspace on exactly that version on every push. It is 1.85: the oldest Rust that builds the dependency tree.

- Raise it only when a dependency needs a newer Rust, or when a language or library feature fixes a real problem in the SDK. Not to follow stable.
- Keep it at or below the Rust that the current Debian stable release ships, so the Linux milestone can build with distribution packages.
- Raise it only in a minor release, and say so in the release notes with the reason.

## 0.3.x: harden what exists

- **Generic shell proofs.** `vst3_proof`, `au_proof` and `aax_process_proof` check FreEQ's parameters and pass only for FreEQ. Replace them with proofs over the SDK's example that check the contract: parameters, state round-trip, tail, latency and bypass.
- **Clippy clean.** `write_info` and `write_param` dereference raw pointers in safe functions. Make them `unsafe`, or validate the pointers first.
- **Sample-rate range.** The default `accepts_sample_rate` refuses rates below 8 kHz, which `clap-validator` reports. Widen it, or document why not.
- **Hostile input everywhere.** Fuzz CLAP events, LV2 ports, `op_shell_*` and state loading with garbage, NaN and out-of-range values. Test that the audio path does not allocate in every format, not only in the FFI session.
- **CI on macOS.** The workspace tests, the main-thread Cocoa test, and the minimum Rust version on every push.

### Status

Done: generic shell proofs over `examples/editor`; clippy clean with warnings as errors, checked in CI; rates from 1 kHz to 768 kHz in every bridge, with every DSP block computing for the real rate (`effective_rate`, `tests/low_rates.rs`); hostile-input suites for CLAP, LV2, the shell ABI and state loading; no-allocation tests for CLAP, LV2 and the shell; macOS CI with the minimum-Rust check, GoodLookinDri pinned to a commit. The minimum is Rust 1.85, the first release that reads the edition-2024 crates in the dependency tree (`coreaudio-sys` through `cpal`); 1.84 cannot build it. It was 1.80, with no recorded reason.

Open:

- The VST3 and Audio Unit proofs do not check bypass, and the AAX proof does not check a state round-trip.
- The shell proofs need the third-party SDKs, so they are not in CI.
- The no-allocation tests cover the Rust side; the C++ shells' own process paths are not measured.

## 0.4: format conformance on macOS

- **Validators in CI** against the example: `clap-validator`, `auval`, AAX Validator, pluginval and `lv2_validate`.
- **Audio Unit editor proof** to match `VST3EditorProof`.
- **Shell gaps:**
  - AAX reports no tail.
  - AAX bypass is a dry copy that is not delayed by the reported latency.
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
- Publishable on crates.io. The SDK depends on GoodLookin DRI by local path today, so `openplugin = "0.3.0-beta"` from crates.io does not resolve.
- Complete API documentation, and a changelog from 1.0 on.
