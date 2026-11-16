# Roadmap: Windows and Linux

## Summary

The macOS roadmap is in [ROADMAP.md](ROADMAP.md). This one brings the SDK to Windows and Linux. It starts when Windows and Linux machines are reachable for building and testing, because editor and host behaviour cannot be proven on macOS, and untested platform code would undo the hardening of 0.3 and 0.4.

## What the work needs

- **Linux:** an SSH login with key authentication, a normal user and a working folder; a logged-in desktop session, so editors and hosts can run on its display and be captured as screenshots over SSH (a virtual display such as Xvfb works for X11, not for Wayland).
- **Windows 10 or 11:** the built-in OpenSSH Server with key authentication; Rust, Visual Studio Build Tools, CMake and Git; a user who stays logged in, because a program started over SSH cannot open windows on the desktop, so editor and host checks run in the desktop session through a scheduled task.
- Both reachable from the Mac that coordinates the work, on the same network or through a VPN such as Tailscale.

## W1: build and test the Rust side everywhere

- CI jobs on GitHub's Windows and Linux runners that build the workspace and run the tests that do not need a window: core, DSP, CLAP, LV2 and the shell ABI, with clippy and the minimum Rust version.
- Fix what does not build: platform-specific code behind `cfg`, paths, the macOS-only editor view excluded.

Status: done on both systems. The workspace builds and every test that needs no window passes on Linux and Windows, with clippy and the minimum Rust version, in CI (`linux`, `msrv-linux`, `windows`, `msrv-windows`).

## W2: Linux

- **Standalone:** a native Wayland window for the standalone app.
- **Plugin editors:** an X11 view used only for embedding in a host. On Linux, VST3 defines one window type, X11 (`kPlatformTypeX11EmbedWindowID`); LV2 has no Wayland UI type in common use; Linux DAWs host plugin editors through XWayland even in a Wayland session. When hosts embed native Wayland surfaces, through CLAP's `wayland` window API or a later VST3 or LV2 type, add that path and keep X11 only for the hosts that need it.
- **Formats:** CLAP and LV2 tested in a Linux host; the VST3 shell on Linux.
- **Proofs:** the main-thread editor test on the Linux display; clap-validator, pluginval and `lv2_validate` on Linux.

Status: done except what needs the owner. The X11 editor view (pure Rust, `x11rb`), CLAP `gui` on X11, LV2 `ui:X11UI`, Linux packaging and the standalone, proven on a real display and in CI under Xvfb, with `lv2_validate`, clap-validator and lilv. Open, for the owner: the VST3 shell on Linux (needs cmake, clang and a VST3 SDK checkout on the Linux machine), pluginval, a Linux DAW, a native Wayland standalone window (X11 under XWayland works today), and whether the CLAP view should stay unmapped until `show`.

## W3: Windows

- **Editor:** an `HWND` view.
- **Shells:** VST3 and AAX on Windows. AAX stays C++, because the Avid SDK has no C interface.
- **Proofs:** the shell proofs, clap-validator and pluginval on Windows; the AAX Validator locally.

Status: done except what needs the owner. The HWND view with the system menu, dialog, tooltips and UI Automation, CLAP `gui` and LV2 `ui:WindowsUI`, the VST3 and AAX shells (Release and Debug), packaging with version resources; shell proofs, clap-validator, pluginval at strictness 10 and the AAX Validator locally; REAPER. No plug-in needs the Visual C++ Redistributable: the C and C++ runtime is linked statically (`+crt-static`, `MultiThreaded`), and CI fails on any `vcruntime*`, `msvcp*` or `concrt*` import (`scripts/windows/check_imports.ps1`, with a `/MD` negative control). The AAX shell's static runtime is not yet proven on a machine (CI does not build AAX). Open, for the owner: PACE signing and Pro Tools on Windows, and repeating the system UI and accessibility checks with real input on an unlocked desktop (`OP_SYSTEM_UI=1 cargo test -p example-editor --test win32_system_ui`, and `--test win32_a11y`); Narrator not tried.

## W4: CI on all three systems

- Every proof that can run headless on every system, and a documented local run for the rest.

Status: the headless proofs run on all three systems in CI and are required checks on `main` (19 jobs): the Linux X11 editor tests under Xvfb, Linux LV2 in lilv, the Windows VST3 shell proofs and clap-validator on Windows. The AAX Validator and pluginval on Windows, and Pro Tools, stay local runs (documented in `docs/BUILD.md` and `docs/AAX_VALIDATOR.md`).
