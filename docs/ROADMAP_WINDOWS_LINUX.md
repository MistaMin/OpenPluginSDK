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

## W2: Linux

- **Standalone:** a native Wayland window for the standalone app.
- **Plugin editors:** an X11 view used only for embedding in a host. On Linux, VST3 defines one window type, X11 (`kPlatformTypeX11EmbedWindowID`); LV2 has no Wayland UI type in common use; Linux DAWs host plugin editors through XWayland even in a Wayland session. When hosts embed native Wayland surfaces, through CLAP's `wayland` window API or a later VST3 or LV2 type, add that path and keep X11 only for the hosts that need it.
- **Formats:** CLAP and LV2 tested in a Linux host; the VST3 shell on Linux.
- **Proofs:** the main-thread editor test on the Linux display; clap-validator, pluginval and `lv2_validate` on Linux.

## W3: Windows

- **Editor:** an `HWND` view.
- **Shells:** VST3 and AAX on Windows. AAX stays C++, because the Avid SDK has no C interface.
- **Proofs:** the shell proofs, clap-validator and pluginval on Windows; the AAX Validator locally.

## W4: CI on all three systems

- Every proof that can run headless on every system, and a documented local run for the rest.
