# Platform code: who edits what

The SDK is built on macOS first and is being brought to Linux and Windows ([ROADMAP_WINDOWS_LINUX.md](ROADMAP_WINDOWS_LINUX.md)). The rule that keeps one platform's work from breaking another's: **platform code lives in files of its own, and work for one platform edits only that platform's files.** The same pattern runs through Rust's standard library (`std::sys`) and `libloading`: one set of function names, one file per platform behind them, and a single switch that picks the file.

## The pattern

A module that needs the operating system has a folder with one file per platform and a switch in `mod.rs`:

```
crates/openplugin-port/src/sys/
  mod.rs       the switch, and the list of items every platform file provides
  macos.rs     dlopen, CoreFoundation, AudioToolbox
  linux.rs     dlopen, VST3 modules as .so files
  windows.rs   LoadLibraryW, VST3 modules as .vst3 DLLs
```

- `mod.rs` holds no platform code: only `#[cfg]` lines that pick a file, and the documented list of items each file provides.
- Every platform file provides the same items under the same names, so the code that calls them has no `#[cfg]` of its own.
- A feature that exists on one platform only gets a stand-in on the others that compiles and returns a clear error (`openplugin-port/src/au_unsupported.rs` for Audio Units).
- A new platform view or shell is a new file (`openplugin-dri/src/x11/`, `openplugin-dri/src/win32.rs`) next to the existing one (`cocoa.rs`), chosen by one `#[cfg]` line in its crate's `lib.rs`.

## Who edits what

| Files | Edited by |
|---|---|
| `*/sys/macos.rs`, `openplugin-port/src/au.rs` and `au/`, `openplugin-dri/src/cocoa.rs`, everything in `native/` today, the editor tests `examples/*/tests/{cocoa,a11y,menu,typing}.rs` | macOS work only |
| `*/sys/linux.rs`, `openplugin-dri/src/x11/` (the X11 view and its services), Linux-only tests such as `examples/editor/tests/x11.rs` | Linux work only |
| `*/sys/windows.rs`, `openplugin-dri/src/win32.rs`, Windows-only tests | Windows work only |
| `au_unsupported.rs` | Linux or Windows work; it must stay a stand-in |
| Everything else | Shared: see below |

## Changing a shared file

A shared file is anything not listed above: the formats in `openplugin-targets`, the core crates, the DSP library, the tests that run everywhere, the build scripts and the CI workflow.

- Add; don't rewrite. A platform change to a shared file adds a `#[cfg]`-gated `mod` line, a call into the platform file, or a new branch in a `match` on the platform. The platform's code itself goes in its own file.
- Never change what a shared file does on another platform. A Linux change that alters the macOS build of a shared file is a macOS change and needs the macOS checks below.
- Name every shared file you changed in the commit message or the report, with one line on why.
- `native/` is macOS-only today. When the VST3 or AAX shell is brought to Windows, the Windows parts go in their own files or folders (`native/vst3/win/`, `native/aax/win/`) with their own CMake branch, not `#ifdef`s inside the macOS sources.

## Checks before a platform change is merged

Every platform change keeps the other platforms green:

- **macOS** (on the Mac or the macOS CI jobs): `cargo test --workspace`, `cargo +1.100.0 clippy --workspace --all-targets -- -D warnings`, `cargo +1.85.0 check --workspace`, and the native shell proofs (`ctest`) with the editor off and on.
- **Linux** (on the Linux machine or the Linux CI job): the workspace builds, and the tests that need no window pass, with clippy.
- **Windows** (on the Windows machine or the Windows CI job): the same as Linux, once the Windows port builds.
- `cargo check --workspace --target x86_64-pc-windows-gnu` from the Mac catches a broken Windows build early without a Windows machine.

## Where platform work happens

- Each platform's work happens on its own machine, in its own checkout, on its own branch (`linux-port`, `windows-port`), never in the Mac's main checkout.
- The branch is merged into `main` on the Mac after the macOS checks pass and the platform's own checks pass on its machine.
- `main` is protected on GitHub: changes arrive through pull requests whose CI checks pass on every platform the SDK supports.
