# Building a plugin

`scripts/build_plugin.sh` turns a plugin crate into distributable macOS bundles in one command: CLAP, LV2, VST3, Audio Unit, AAX and, if the plugin has a binary for it, a standalone app. It is plain bash over `cargo`, `cmake`, `lipo`, `codesign` and `xcrun`. Signing, notarisation and PACE signing are optional; without them every bundle is ad-hoc signed, which is enough to load on the machine that built it.

```bash
scripts/build_plugin.sh --help
```

## What it does

1. Builds the plugin crate with `cargo build --release --target <triple>` once per architecture, and joins the results with `lipo` for `universal`.
2. CLAP: copies the `cdylib` to `<Name>.clap/Contents/MacOS/<Name>` and writes `Info.plist` and `PkgInfo`.
3. LV2: copies the `cdylib` to `<Stem>.lv2/<Stem>.dylib` and runs the plugin's TTL binary (`--ttl-bin`) as `<ttl-bin> <folder> <Stem>`, which writes `manifest.ttl` and `<Stem>.ttl`. `<Stem>` is the product name reduced to letters, digits and `_`, so the files are safe inside an LV2 IRI. [`examples/editor/src/bin/lv2_ttl.rs`](../examples/editor/src/bin/lv2_ttl.rs) is a complete TTL binary.
4. VST3, Audio Unit, AAX: configures `native/CMakeLists.txt` over the plugin's `staticlib` and builds only the bundle targets (not the SDK's own proofs). See [SHELL.md](SHELL.md).
5. Standalone: builds the plugin's binary (`--standalone-bin`) and wraps it in `<Name>.app`.
6. Sets install names (`@loader_path/...`) on the libraries, so none records a build path, and remaps source paths in both Rust (`--remap-path-prefix`) and C++ (`-ffile-prefix-map`). Paths that are remapped: the home folder, the folder that holds the workspace (path dependencies usually live there), the SDK, the Cargo home, the workspace, the plugin crate, the build folder, and the three third-party SDKs.
7. Signs, optionally PACE-signs and notarises, then checks: `codesign --verify --deep --strict` on every bundle, and a scan of every binary for the build, workspace, SDK and home paths. A leak fails the run.

All bundles end up in `--out`. Cargo and CMake work in `--work-dir`. The script never installs anything into a plug-in folder and never uses `sudo`.

## Example

The SDK's own example, every format, ad-hoc signed, on this machine's architecture:

```bash
scripts/build_plugin.sh \
  --manifest-path examples/editor/Cargo.toml --version 0.3.1 \
  --name "Example Gain" --bundle-id com.opengrid.example --manufacturer OpenGrid \
  --au-subtype ExGn --au-manufacturer OpGd \
  --ttl-bin lv2_ttl --editor --tail --aax-pages \
  --vst3-sdk /path/to/vst3sdk --au-sdk /path/to/AudioUnitSDK --aax-sdk /path/to/aax-sdk \
  --out dist --work-dir build/openplugin
```

Add `--archs universal` for an arm64 and x86_64 build. Add `--dry-run` to see every command without running any.

## Options

Each option has an environment variable (shown in `--help`), so a plugin can keep its settings in a small wrapper script or a CI file.

| Option | Meaning |
|---|---|
| `--manifest-path` | The plugin's `Cargo.toml`. Required |
| `--package`, `--lib-name` | cargo package, and library stem if it is not the package name with `-` as `_`. Default: from the manifest |
| `--name` | Product name, used for the bundle file names. Default: the package name |
| `--bundle-id` | Bundle identifier prefix. The VST3, Audio Unit and AAX shells append `.vst3`, `.au`, and so on |
| `--version` | Bundle version, numbers only (a `-beta` suffix is cut off). Default: the manifest's `version`; give it when the manifest inherits the version from the workspace |
| `--manufacturer` | Manufacturer display name for the Audio Unit |
| `--au-subtype`, `--au-manufacturer` | Four-character codes. Required for `au` and for `--aax-pages`, unless the plugin pins its own with `Plugin::au_ids`: then both are left out (the shell build asks the plugin), and codes that differ from the pinned ones stop the build |
| `--au-version` | The Audio Unit component version: a number, or `release` for `major * 65536 + minor * 256 + patch` of `--version`. A host takes a higher number for an update; the default is `release`. `OP_AU_VERSION` |
| `--au-type` | `auto` (the default: from the plugin's kind), `aufx`, `aumu` or `aumf`. `OP_AU_TYPE` |
| `--formats` | `clap,lv2,vst3,au,aax,standalone` or `all`. Default: `clap,lv2,vst3,au,aax` |
| `--ttl-bin` | The plugin's bin that writes the LV2 files. Required for `lv2` |
| `--standalone-bin` | The plugin's bin that runs standalone. Required for `standalone` |
| `--features` | cargo features for the plugin crate |
| `--editor`, `--tail`, `--aax-pages` | Maps to `OP_EDITOR`, `OP_TAIL` and `OP_AAX_PAGES` in the shells. Off by default |
| `--pkg` | Also build an installer package, `<out>/<Name>-<version>.pkg` (see Installer package). `OP_PKG=1` |
| `--archs` | `arm64`, `x86_64` or `universal`. Default: the host's |
| `--deployment-target` | Minimum macOS version. Default 11.0 |
| `--vst3-sdk`, `--au-sdk`, `--aax-sdk` | SDK checkouts, or `VST3_SDK`, `AU_SDK`, `AAX_SDK`. Each is needed only for its own format. They are never copied into the output |
| `--out`, `--work-dir`, `--jobs` | Output folder, build folder, parallel jobs |
| `--dry-run`, `--no-verify` | Print without running; skip the final checks |

## Signing, notarisation and PACE

None of these is required. They are read from the environment only, and the script never prints a secret: it prints the identity name and profile name you give it, nothing else.

| Variable | Effect |
|---|---|
| `CODESIGN_IDENTITY` | A `Developer ID Application: ...` identity. Each bundle is signed with `codesign --force --deep --timestamp --options runtime`. Unset: ad-hoc (`--sign -`) |
| `NOTARY_PROFILE` | A `notarytool` keychain profile (made once with `xcrun notarytool store-credentials`). Each bundle is zipped with `ditto`, submitted with `--wait`, and stapled. The LV2 folder is submitted but cannot be stapled. Needs `CODESIGN_IDENTITY` |
| `WRAPTOOL`, `PACE_ACCOUNT`, `PACE_WCGUID` | Path to PACE `wraptool`, the iLok account and the wrap configuration GUID. The AAX bundle is signed with `wraptool sign`, sealed again with `codesign`, and checked with `wraptool verify`. Needs `CODESIGN_IDENTITY`. `wraptool` asks for the account password itself; the script never takes one |

The order for AAX is: Apple signature, PACE wrap, Apple signature again, then notarisation. `--dry-run` shows these steps with placeholder values and contacts nobody:

```bash
CODESIGN_IDENTITY="Developer ID Application: Example Co (ABCDE12345)" \
NOTARY_PROFILE=example-notary WRAPTOOL=/opt/pace/wraptool \
PACE_ACCOUNT=example-account PACE_WCGUID=00000000-0000-0000-0000-000000000000 \
scripts/build_plugin.sh --dry-run --manifest-path examples/editor/Cargo.toml ...
```

The AAX bundle links the Avid SDK and is not redistributable as source. The VST3 bundle carries the VST3 SDK licence file and the AAX bundle the AAX one, as the shells' CMake does.

## Installer package

`--pkg` (or `OP_PKG=1`) makes `build_plugin.sh` call `scripts/make_pkg.sh` after the bundles are signed. It wraps the formats that were built, and only those, in a macOS installer:

- One component package per format, from `pkgbuild`, each with its own identifier (`<bundle-id>.pkg.<format>`) and bundles marked not relocatable, so a plug-in always lands in its folder.
- One distribution package, from `productbuild`, with a Distribution XML in which the user ticks the formats to install.

| Format | Install location |
|---|---|
| CLAP | `/Library/Audio/Plug-Ins/CLAP` |
| LV2 | `/Library/Audio/Plug-Ins/LV2` |
| VST3 | `/Library/Audio/Plug-Ins/VST3` |
| Audio Unit | `/Library/Audio/Plug-Ins/Components` |
| AAX | `/Library/Application Support/Avid/Audio/Plug-Ins` |
| Standalone app | `/Applications` |

Signing is read from the environment, like the rest of the script:

| Variable | Effect |
|---|---|
| `INSTALLER_IDENTITY` | A `Developer ID Installer: ...` identity. The component and distribution packages are signed with it and a secure timestamp. Unset: the package is unsigned, the script says so, and macOS will warn when it is opened |
| `NOTARY_PROFILE` | With `--pkg`, the distribution package is also submitted with `notarytool`, waited for, stapled and validated. Needs `INSTALLER_IDENTITY` (and `CODESIGN_IDENTITY`, as for the bundles) |

`make_pkg.sh` also runs on its own over a folder of bundles (`scripts/make_pkg.sh --help`). Its staging folder is `<work-dir>/pkg`; it deletes only paths inside it and refuses an empty one. Every path is quoted, so folders with spaces work. The script only builds and signs packages: it never runs `installer`, opens a package or uses `sudo`.

`scripts/test_pkg.sh --work-dir DIR` builds the example plugin with `--pkg`, expands the package with `pkgutil --expand-full` into the work folder and checks, for every format, that the component package exists, installs into the right folder, is not relocatable, has a choice in the Distribution XML, and holds the bundle in its payload. Nothing is installed. It needs `VST3_SDK`, `AU_SDK` and `AAX_SDK` for those formats, or `--formats clap,lv2`.

## Checks run on the example

On 15 November 2026, `examples/editor` was built for every format with `--archs arm64` and no signing identity. Then, independently of the script:

- `codesign --verify --deep --strict` passed on the CLAP, VST3, Audio Unit and AAX bundles and on the LV2 library.
- `lv2_validate` passed on the LV2 bundle's `manifest.ttl` and `ExampleGain.ttl` (0 errors).
- `clap-validator` 0.4.1 passed the CLAP bundle: 44 tests run, 34 passed, 0 failed, 10 skipped.
- No binary contained a build, workspace, SDK or home path, and the LV2 library's install name was `@loader_path/ExampleGain.dylib`.
- A `--archs universal` build of the CLAP and LV2 produced an `x86_64 arm64` binary that passed the same `clap-validator` run.

- `--pkg` on the example, every format except standalone, from a work folder under `/Volumes/Developer 2/...` (a space in the path), unsigned: `scripts/test_pkg.sh` passed, with all five bundles in their component packages at the install locations above.

### Universal binaries

On 15 November 2026, on an Apple Silicon Mac with Rosetta, the same example was built for every format (CLAP, LV2, VST3, Audio Unit, AAX, with the editor, tail and AAX page table) with `--archs universal`. The Rust library is built for `aarch64-apple-darwin` and `x86_64-apple-darwin` and joined with `lipo`. The VST3 SDK, the AudioUnitSDK sources and the AAX SDK's `AAXLibrary` sources are compiled by `native/CMakeLists.txt` with `CMAKE_OSX_ARCHITECTURES=arm64;x86_64`, so each is built for both slices in the build folder (no SDK file is changed). Nothing in the build had to change. `lipo -archs` on each bundle binary:

| Bundle | `lipo -archs` |
|---|---|
| `.clap` | `x86_64 arm64` |
| `.lv2` library | `x86_64 arm64` |
| `.vst3` | `x86_64 arm64` |
| `.component` (Audio Unit) | `x86_64 arm64` |
| `.aaxplugin` | `x86_64 arm64` |

`build_plugin.sh` now makes this check itself: after signing it runs `lipo -archs` on every binary in every bundle and fails if the slices differ from `--archs`.

The x86_64 slice ran under Rosetta (`arch -x86_64`), the arm64 slice natively:

| Check | arm64 | x86_64 (Rosetta) |
|---|---|---|
| Native ctest proofs, 18 tests (`vst3_*` 5, `au_*` 5, `aax_*` 8; the Audio Unit proofs register the bundle in the test process and install nothing) | 18 of 18 pass | 18 of 18 pass |
| Format parity (`scripts/format_parity/run.sh`, `--arch x86_64` for Intel): CLAP, LV2, VST3, Audio Unit and AAX match the reference with a maximum difference of 0; the three gain-error controls and the timing control fail as they must | pass | pass |
| `clap-validator` (built from the pinned commit for each architecture) on the universal CLAP bundle | 44 run, 34 passed, 0 failed, 10 skipped | 44 run, 34 passed, 0 failed, 10 skipped |

To run the proofs in the other slice, configure the proof build with `"-DCMAKE_OSX_ARCHITECTURES=arm64;x86_64"`, build everything, run `ctest`, then re-run `cmake` with `"-DCMAKE_TEST_LAUNCHER=arch;-x86_64"` (it only rewrites the test list) and run `ctest` again. The format parity script needs `--arch x86_64` because it builds its host and plugin per architecture.

CI runs all of the above except AAX (its SDK is not redistributable) in the `universal` job on the arm64 runner. The job installs Rosetta with `softwareupdate --install-rosetta --agree-to-license` only if `arch -x86_64` does not already work. That job has not run on GitHub yet; its steps were run locally.

Not covered by the universal checks: Intel hardware (Rosetta is not a real Intel CPU), a signed or notarised universal build, `auval` and `pluginval` on the x86_64 slice, and the editor proofs (they need a window server).

Not covered: installing the package, a signed or notarised package (shown with `make_pkg.sh --dry-run` only), the standalone path (the example has no standalone binary), real Developer ID signing, notarisation and PACE signing (shown with `--dry-run` only), and Intel hardware.

## Linux

`scripts/build_plugin_linux.sh` builds a plugin crate into the formats that work on Linux: CLAP, LV2, VST3 and a standalone executable. It is plain bash over `cargo` and binutils, takes the same option names and environment variables as `build_plugin.sh` where they mean the same thing (`--manifest-path`, `--package`, `--lib-name`, `--name`, `--version`, `--ttl-bin`, `--standalone-bin`, `--features`, `--formats`, `--out`, `--work-dir`, `--jobs`, `--dry-run`, `--no-verify`), and refuses `au` and `aax`, which do not exist on Linux. `vst3` needs `--vst3-sdk` and CMake, and takes `--editor`, `--tail` and `--manufacturer` as on macOS. `--formats all` is `clap,lv2,standalone`, as on macOS, and so needs `--standalone-bin`. The macOS-only options (`--bundle-id`, `--archs` and so on) are refused by name. It runs only on Linux and never installs anything.

```bash
scripts/build_plugin_linux.sh \
  --manifest-path examples/editor/Cargo.toml --version 0.4.0 \
  --name "Example Gain" --ttl-bin lv2_ttl \
  --out dist --work-dir build/openplugin-linux
```

| Format | Layout |
|---|---|
| CLAP | `<out>/<Name>.clap`: the shared library itself, one ELF file, not a folder |
| LV2 | `<out>/<Stem>.lv2/` with `<Stem>.so`, and `manifest.ttl` and `<Stem>.ttl` written by `<ttl-bin> <folder> <Stem>`. `<Stem>` is reduced as on macOS |
| VST3 | `<out>/<Name>.vst3/Contents/<arch>-linux/<Name>.so` (`x86_64-linux`, `aarch64-linux`): the shell of `native/vst3/linux` over the plugin's `staticlib`, with the VST3 SDK licence in `Contents/Resources`. It exports only `GetPluginFactory`, `ModuleEntry` and `ModuleExit` (the check fails on anything else), and links libstdc++ statically. With `--editor` the editor is an X11 view run by the host's `Linux::IRunLoop` |
| Standalone | `<out>/<Name>`: the plugin's `--standalone-bin`, one ELF executable; and `<out>/<id>.desktop`, a freedesktop Desktop Entry (`Type=Application`, `Exec="<Name>"`, `StartupWMClass=OpenPlugin`, the X11 view's class). `<id>` is `<Name>` in lower case with every character but letters, digits, `_`, `.` and `-` as `-`. `Exec` names the executable on `PATH`: the script installs nothing, so the folder is the installer's choice |

The library is a release build of the host architecture, with the same `--remap-path-prefix` list as on macOS. `--strip` runs `strip --strip-all` on each copy; the dynamic symbols stay. Unless `--no-verify` is given, the script then checks each library and fails the run on any of these:

- `strings -a` finds the build, workspace, its parent, plugin crate, SDK or home path.
- `readelf -d` lists a `NEEDED` library other than glibc's (`libc`, `libm`, and `libdl`, `libpthread`, `librt` for older toolchains), `libgcc_s` and the loader. `--allow-lib SONAME` (or `OP_ALLOW_LIBS`) accepts another. `ldd` reports a library as not found.
- An `RPATH` or `RUNPATH` points into the build tree, the workspace or the home folder.
- `nm -D --defined-only` does not show `clap_entry` (CLAP), `lv2_descriptor` (LV2) or the three VST3 entry points, or the VST3 module exports anything else.
- The LV2 bundle lacks `manifest.ttl` or `<Stem>.ttl`, or `manifest.ttl` does not name `<Stem>.so` as `lv2:binary`.
- The standalone file is not an ELF executable (it gets the same path, library and run path checks, and no export check), its desktop entry is missing, or `desktop-file-validate` (when installed) rejects the entry.

A standalone bin is the plugin's own: a `main` that builds the editor and calls `openplugin::dri::x11::run_window(title, editor, None)`, which on macOS is `openplugin::dri::cocoa::run_window`. It opens no audio device on either platform. The window is an X11 one, so in a Wayland session it runs through Xwayland.

### Checks run on Linux

On 15 November 2026, on Arch Linux (x86_64, Intel Core i7-2630QM, Rust 1.100), `examples/editor` was built with the command above, with and without `--strip`, once into a folder with a space in its name:

- The script's checks passed. Both libraries need only `libgcc_s.so.1`, `libm.so.6`, `libc.so.6` and `ld-linux-x86-64.so.2`, have no run path, and contain no build or home path.
- `lv2_validate manifest.ttl ExampleGain.ttl` in the bundle: `Found 0 errors among 84 files (checked 1972 restrictions)`. The one skipped file is `/usr/lib/lv2/core.lv2/lv2core.doap.ttl`, a specification file that Arch's `lv2` package names in `lv2_validate` but does not ship; no bundle file is skipped.
- `clap-validator` 0.4.1, built with `cargo install` from commit `152b982` (the one the `universal` job builds), on `Example Gain.clap`: 44 tests run, 0 failed, 10 skipped, and 34 passed or 33 passed with 1 warning depending on timing (seven full runs: 34 passed in four, 33 passed and 1 warning in three). The skips are the ones seen on macOS: no preset discovery factory (3), no 64-bit audio (2), no note input port (3), no `audio-ports-activation` (1), no `configurable-audio-ports` (1). The warning is `process-audio-denormals`, which compares the time to process denormal input with normal input: 1.28 to 2.45 times longer in the full runs, and 4.6 to 5.6 times longer in 15 runs of that test alone (`--include process-audio-denormals`), each of which warned. No format entry sets flush-to-zero around `process` (`openplugin_dsp::utility::DenormalGuard` exists but is not used there). The machine was shared with other builds during these runs. On macOS (arm64, and x86_64 under Rosetta) the test passes.

CI runs the same commands in the `linux-package` job on `ubuntu-latest`. Its `apt-get install lv2-dev sordi` step has not run on GitHub yet; everything after it was run on the Arch machine.

## Linux: VST3 shell

VST3 is the one format on Linux that needs the native shell: CLAP and LV2 are Rust alone, and there is no Audio Unit or AAX. `native/CMakeLists.txt` hands a Linux build machine to `native/cmake/linux.cmake`, which holds the options, the Rust library and its system libraries, and includes `native/vst3/linux/vst3.cmake`. The compiler is GCC or Clang. `scripts/build_plugin_linux.sh --formats vst3` drives this build for a plugin crate; what follows is the build underneath it, which is also how the SDK's own proofs are built. From the repository root, with the VST3 SDK outside the tree:

```bash
cargo build --release --lib -p example-editor -p example-role-tags -p example-expression-synth
cmake -S native -B build-shell -DCMAKE_BUILD_TYPE=Release \
  -DOP_RUST_LIB="$PWD/target/release/libexample_editor.a" \
  -DOP_VST3_SDK=/path/to/vst3sdk \
  -DOP_PRODUCT="Example Gain" -DOP_BUNDLE_ID=com.opengrid.example \
  -DOP_VERSION=0.3.1 -DOP_MANUFACTURER=OpenGrid \
  -DOP_TAIL=ON -DOP_EDITOR=OFF
cmake --build build-shell -j 4
ctest --test-dir build-shell --output-on-failure
```

- **The module** is `<Name>.vst3/Contents/<arch>-linux/<Name>.so` (`x86_64-linux`, `aarch64-linux`) in the build folder, with the VST3 SDK licence in `Contents/Resources`. Its entry points are `ModuleEntry` and `ModuleExit` (the SDK's `linuxmain.cpp`). There is no `Info.plist`, `PkgInfo`, `moduleinfo.json` or signature. Install by copying the outer folder to `~/.vst3` or `/usr/lib/vst3`; nothing here does that.
- **Exports.** A module exports `GetPluginFactory`, `ModuleEntry` and `ModuleExit` and nothing else (`native/vst3/linux/exports.map`): the plugin's `op_shell_*`, the VST3 SDK and the C++ runtime stay inside it, so two plug-ins in one host share no symbol.
- **The C++ runtime** is linked into the module (`-static-libstdc++`), so it loads in a host whose `libstdc++` is older than the build machine's. That needs the static library on the build machine (part of GCC on Arch, Debian and Ubuntu; the `libstdc++-static` package on Fedora). The module needs `libc`, `libm` and `libgcc_s`; the glibc of the build machine is the oldest it runs on.
- **Class ids** are the 16 bytes of the plugin's FUID strings, as on macOS, so a project saved on one system finds the plug-in on the other (Windows reorders them to the same strings).
- **Timers.** Linux has no run loop of its own for a plug-in; the host lends one (`Linux::IRunLoop`). The shell's main-thread timer (the poll that announces a new latency or new parameter info) runs on the loop the host gives through `IPluginFactory3::setHostContext`. A host that gives none there gets no announcements; the plug-in works otherwise.
- **System libraries.** `OP_RUST_NATIVE_LIBS` lists what the Rust library links (`gcc_s;util;rt;pthread;m;dl;c`). When a plugin needs more, take the list from `cargo rustc --release --lib -p <crate> --crate-type staticlib -- --print native-static-libs`.
- **`-DOP_EDITOR=ON`** makes the shell host the plugin's editor (`native/include/op_editor_x11.h`): the X11 view, embedded with `kPlatformTypeX11EmbedWindowID`, which is how Linux hosts embed editors in a Wayland session too (through Xwayland). The view is driven from the run loop of the host's `IPlugFrame`: one timer of 33 ms and the X connection's descriptor. A frame that lends no run loop cannot show the editor, and `attached` fails there. The editor works in points and the host in pixels; the view converts at its own scale, which it reads from the display (`Xft.dpi` over 96). It does not offer `IPlugViewContentScaleSupport`: a host cannot set the scale, as with the CLAP view. The stand-in plugins have no editor, so, as on macOS, that build has fewer proofs and adds `vst3_editor_proof`, which opens windows and needs an X display (`DISPLAY`; a desktop session, Xwayland or Xvfb) and libxcb's headers (`libxcb1-dev`; `libxcb` on Arch) for the proof's own X client.

The proofs are the macOS proof sources unchanged. `native/vst3/linux/compat` stands in for the CoreFoundation calls they make (a bundle loaded with `dlopen`, and the main run loop, which is also the `IRunLoop` each module is lent), `native/linux/compat/main_thread.h` for `pthread_main_np`, and `native/linux/alloc_hook.cpp` for libmalloc's `malloc_logger`: the proof executable replaces `malloc` and its relatives for the whole process (glibc allows that), so the count covers the module, its C++ runtime and the Rust library. glibc only.

### Checks run on the example

These checks ran on #21's first Linux shell; the shell in `native/vst3/linux` today is the one that came with #37, with #21's proofs, view sizing and CI job added, and CI repeats the proofs and the validator on it. On 16 November 2026, on Arch Linux (x86_64, GCC 16.2.1, Clang 23.1.3, CMake 4.4.4, Rust 1.100), with `examples/editor` as "Example Gain", against VST3 SDK 3.8.1_build_84 (Arch's `vst3sdk` package) and v3.8.0_build_66 (the one CI pins):

- `ctest`, editor off: 12 of 12 with GCC against both SDK versions, and 12 of 12 with Clang. They are the 12 `vst3_*` proofs of the macOS build. `vst3_noalloc_events_proof` takes about 40 seconds, as on the other systems: it prepares a point per sample for 2081 parameters before it measures.
- `ctest`, editor on, on the desktop's X display (Xwayland under Hyprland, scale 1): 3 of 3. `vst3_editor_proof`: no attach without a run loop; the view opens at 240 x 160 pixels with one timer and one descriptor registered, and the window shows the editor's picture; right click, click, the resize range (180 x 120 to 480 x 320), an editor-initiated size (360 x 240) through `resizeView`, a refusing frame, two views on one controller, and teardown with the processor first.
- The VST3 SDK's own `validator` (built from the same checkout, `cmake --build build-shell --target validator`): 47 tests passed, 0 failed on each of `Example Gain.vst3`, `Expression Synth.vst3` and `Role Tags.vst3`; with `-e` (extensive) on the packaged bundle, 537 passed, 0 failed.
- `scripts/build_plugin_linux.sh --formats vst3 --editor --tail --strip`: the script's checks passed. The module needs only `libgcc_s.so.1`, `libm.so.6`, `libc.so.6` and the loader, exports the three entry points, and contains no build, SDK or home path.
- `openplugin-port scan` on the packaged bundle reads the plug-in: name, vendor and its three parameters, no warnings.
- `cargo test -p example-editor --test x11_shell` on the same display: the `op_editor_*` calls of `op_editor_x11.h` as a shell makes them, without CMake or the VST3 SDK (attach, the scale, the descriptor and the pump, a click, resize in points, destroy).

CI runs all of this in the `vst3-shell-linux` job on `ubuntu-latest`: the build, the twelve proofs, the validator on the three modules, the editor build with `vst3_editor_proof` under Xvfb, and the packaged bundle through the validator's extensive tests. It is not a required check yet.

In hosts on the same machine, with the packaged example copied to `~/.vst3`, `~/.clap` and `~/.lv2` (`build_plugin_linux.sh --formats clap,lv2,vst3 --editor --tail --strip`):

- pluginval 1.0.4, `--strictness-level 10 --validate-in-process`, on the VST3 with the desktop's display, so with its editor tests: `SUCCESS`.
- Ardour 9.10 (a source build). `ardour-vst3-scanner` finds the VST3. In a Lua session on the dummy backend the VST3 and the LV2 are listed, instantiated and added to the master bus; the VST3 shows Gain, Warm and Bypass, and Gain set to 6 reads back 6; the saved session reopens with Gain at 6 in both. In Ardour's own window (the session opened with both plug-in windows marked open): the VST3 editor and the LV2 editor each appear inside Ardour's plug-in window, drawn with the knob at the saved 6.0 dB, and a click on the Warm switch flips it in each. No audio was rendered in Ardour.
- SoundCraft 0.20.1 (`soundcraft-cli`, and the app through its control channel): its scans list the VST3 and the CLAP. On a soloed track of its demo session, a 4 second bounce peaks at -9.227 dB without the plug-in and with the VST3 at 0 dB (the same loudness too), and at -3.227 dB with the VST3 or the CLAP at Gain +6 dB: 6.000 dB more. A session saved with the VST3 at -12 dB and Warm on reopens and bounces to the same bytes as the session it was saved from. In the app the insert shows SoundCraft's own panel with Gain and Warm; its "Open Plugin Editor" button is disabled, because SoundCraft on Linux does not host embedded plug-in editors.

Ardour makes the plug-in's place as wide as its own toolbar (374 pixels for a 240 x 160 editor) and gives the view that width with the old height. The view takes the width and the height of its shape and asks Ardour's frame for that size once, so the editor is 374 x 249 there and keeps its 3:2 shape (`vst3_editor_proof` has the case, with a frame that grants the size and one that refuses it).

Not covered: a display with a scale other than 1 for the VST3 view, aarch64, and a C library other than glibc.

## Windows: CLAP

On Windows a `.clap` is the plug-in DLL under another name, with no bundle folder. `scripts/windows/bundle_clap.ps1` builds examples in release and copies `target\release\<lib>.dll` to `<Out>\<Name>.clap` (default `target\clap`; nothing is installed in a plug-in folder):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\bundle_clap.ps1                      # editor, design_editor, cpp_bridge, role_tags
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\bundle_clap.ps1 -Example editor -Out dist
```

A crate whose `[lib]` has no `cdylib` (`examples/role_tags`: `staticlib` and `rlib`) is built with `cargo rustc --lib --crate-type cdylib`. The product name is the example's folder in CamelCase (`design_editor` becomes `DesignEditor.clap`). The CLAP `gui` extension is offered on macOS only for now, so a Windows `.clap` has no editor.

On 15 November 2026, on Windows 11 x64 (Rust 1.100.0 MSVC), `clap-validator` 0.4.1 `validate`, out of process and with `--in-process` (the same counts, warnings aside):

| Plug-in | Run | Passed (or warning) | Failed | Skipped |
|---|---|---|---|---|
| `Editor.clap` | 44 | 33 | 0 | 11 |
| `DesignEditor.clap` | 44 | 33 | 0 | 11 |
| `CppBridge.clap` | 44 | 33 | 0 | 11 |
| `RoleTags.clap` | 44 | 31 | 2 | 11 |

- One more skip than on macOS: `scan-rtld-now` runs on Unix only.
- `process-audio-denormals` could end with a warning on these runs (denormal input took 1.3 to 3.4 times longer on x86_64). Since the CLAP wrapper flushes denormals around every call into the plug-in (`openplugin_core::denormal`), `Editor.clap` passes it with no warning: on 16 November 2026, after the rebase onto that change, `clap-validator validate` gave 44 run, 33 passed, 0 failed, 0 warnings, 11 skipped.
- `RoleTags.clap` fails `param-set-events` and `param-set-no-cookies` on every platform: its one parameter is a toggle whose default is 0, and the validator's fixed-seed value for it is also 0, so the test sees no change. With the default set to 1 for a trial build, both pass; `openplugin-port capture --set Spare=1` reads 1 back through `clap_plugin_params::flush`. The example is a test plug-in for the channel order proof and is not changed for this.

REAPER 7.80 (a portable copy with its own `-cfgfile` and the Dummy Audio device) scans all four, instantiates each, and renders a project with `Editor.clap` (Gain 6 dB, Warm on, set from ReaScript) with `reaper.exe -renderproject`, from an SSH session as well as in the desktop session. The 32-bit float render is bit-identical to `openplugin-port null --save` (the SDK's own offline render of the same input and settings), and the state REAPER saved in the `.rpp` reopens to Gain 6 dB and Warm on.

## Windows: VST3 and AAX shells

`native/CMakeLists.txt` hands a Windows build machine to `native/cmake/windows.cmake`, which holds the options, the Rust library and its system libraries, and includes one file per format: `native/vst3/win/vst3.cmake` and `native/aax/win/aax.cmake`. The compiler is MSVC (Visual Studio or its Build Tools) for x64. There is no Audio Unit on Windows. `scripts\windows\build_plugin.ps1` drives this build for a plugin crate (see Windows: packaging below); what follows is the build underneath it, which is also how the SDK's own proofs are built. In PowerShell, from the repository root, with the SDK checkouts outside the tree:

```powershell
cargo build --release --lib -p example-editor -p example-role-tags -p example-expression-synth
cmake -S native -B build-shell -A x64 `
  "-DOP_RUST_LIB=$PWD\target\release\example_editor.lib" `
  "-DOP_VST3_SDK=C:\path\to\vst3sdk" "-DOP_AAX_SDK=C:\path\to\aax-sdk-2-8-1" `
  "-DOP_PRODUCT=Example Gain" "-DOP_BUNDLE_ID=com.opengrid.example" `
  "-DOP_VERSION=0.3.1" "-DOP_MANUFACTURER=OpenGrid" `
  "-DOP_TAIL=ON" "-DOP_EDITOR=OFF"
cmake --build build-shell --config Release -j 4
ctest --test-dir build-shell -C Release --output-on-failure
```

- **Quote each `-D`.** PowerShell splits an unquoted `-DOP_VERSION=0.3.1` at the first dot, and CMake then reads `.3.1` as a path.
- **Release and Debug.** `--config Release` is what ships. `--config Debug` (with `ctest -C Debug`) builds and passes the same proofs: unoptimized C++ with debug information and the SDKs' own checks, for stepping through a shell. Every configuration links the release C runtime statically (`/MT`, `MultiThreaded`), Debug included, because the Rust library is built against it (`+crt-static`) and Rust has no build against the debug runtime; one binary cannot mix the two. So a Debug shell has no debug heap and no checked iterators, and it links the same `target\release\<name>.lib` as Release. The VST3 SDK's own CMake asks for `/MDd` in Debug and `/MD` otherwise; `native/vst3/win/vst3.cmake` takes that off the SDK's libraries and gives them `/MT`, and `native/aax/win/aax.cmake` does the same for the AAX library. The bundles in the build folder are those of the configuration built last.
- **One format.** `-DOP_BUILD_AAX=OFF` or `-DOP_BUILD_VST3=OFF` leaves a format out, and its SDK is then not needed. The AAX shell wants AAX SDK 2.8.1: with 2.10 two of the proofs do not build.
- **System libraries.** `OP_RUST_NATIVE_LIBS` lists what the Rust library links (`user32`, `gdi32`, `shell32`, `comdlg32`, `comctl32` and the ones `std` needs). When a plugin needs more, take the list from `cargo rustc --release --lib -p <crate> --crate-type staticlib -- --print native-static-libs`. It is a cache variable: a build folder configured earlier keeps its old list until it is given again or removed with `-UOP_RUST_NATIVE_LIBS`.
- **`-DOP_EDITOR=ON`** makes the shells host the plugin's editor (`native/include/op_editor_win.h`). The stand-in plugins have no editor, so, as on macOS, that build has fewer proofs and adds `vst3_editor_proof`, which opens real windows and needs a desktop session (`scripts\windows\run_in_desktop.ps1` from SSH).

What is built, in the build folder:

| Format | Layout | Notes |
|---|---|---|
| VST3 | `<Name>.vst3\Contents\x86_64-win\<Name>.vst3`, the VST3 SDK licence in `Contents\Resources` | The inner file is the DLL. Its entry points are `InitDll` and `ExitDll`. No `Info.plist`, `PkgInfo` or `moduleinfo.json`. Install by copying the outer folder to `%COMMONPROGRAMFILES%\VST3` |
| AAX | `<Name>.aaxplugin\Contents\x64\<Name>.aaxplugin` | Unsigned: Pro Tools loads it only after PACE signing. The AAX Validator loads it as it is (see [AAX_VALIDATOR.md](AAX_VALIDATOR.md)) |

Nothing is signed, installed or copied into a plug-in folder.

A VST3 class has the same id on Windows as on macOS. The plugin gives each class id as the 16 bytes of its FUID string. On macOS those bytes are the TUID; on Windows the VST3 SDK is COM compatible and a TUID is a GUID, so the Windows shell stores the first three fields in little-endian order (`native/vst3/win/OpenPluginVST3Win.cpp`). A host reads the same FUID on both, and a project saved on one system finds the plug-in on the other. `crates/openplugin-port/tests/scan.rs` holds the example's two ids as fixed strings.

The editor's sizes: the editor works in points, a Windows host in pixels. The VST3 view converts at its scale, which is the one the host gives through `IPlugViewContentScaleSupport` or, without that, the DPI of the host's window over 96.

### Checks run on the example

On 16 November 2026, on Windows 11 x64 (Rust 1.100.0 MSVC, MSVC 14.51, CMake 4.4, VST3 SDK v3.8.0_build_66, AAX SDK 2.8.1), with `examples/editor` as "Example Gain":

- `ctest -C Release`, editor off: 27 of 27, the 12 `vst3_*` proofs of the macOS build and the 15 `aax_*` proofs. `vst3_noalloc_events_proof` takes one to three minutes: it prepares a point per sample for 2081 parameters before it measures.
- `ctest -C Release`, editor on, in the desktop session: 18 of 18. `vst3_editor_proof` ran on a display at 150 %: the view opened at 360 x 240 pixels for 240 x 160 points, resized through its range (270 x 180 to 720 x 480), opened at 480 x 320 after `setContentScaleFactor(2.0)` and went to 360 x 240, asking the frame, after `setContentScaleFactor(1.5)`.
- pluginval 1.0.4, `--strictness-level 10`, on the VST3 bundle: `SUCCESS` without the editor (`--skip-gui-tests`, over SSH) and with the editor and pluginval's editor tests (in the desktop session).
- `openplugin-port scan` and the port's VST3 fixture tests (`OPENPLUGIN_PORT_FIXTURES` at the build folder): the scan reproduces the example's parameters, buses and class ids; capture and null tests pass, the latency one against `Latency Stub.vst3`.
- REAPER 7.80 with a configuration of its own (`-cfgfile`, `-newinst`) and `scripts/host_smoke/reaper_smoke.lua` unchanged: the VST3 is found and inserted, Gain reads in dB, offline renders have the expected peaks (0.2500 at 0 dB, 0.4988 at +6 dB, 0.0628 at -12 dB, 0.9953 at +12 dB, 0.7596 with Warm on), state and a Gain envelope survive save and reopen, and the plug-in's own editor opens in REAPER's window, twice.
- `ctest -C Debug` on a `--config Debug` build: 27 of 27 with the editor off and 18 of 18 with it on. `vst3_noalloc_events_proof` takes about six minutes there, because the proof's own setup is unoptimized. The Debug modules import `VCRUNTIME140.dll` and `MSVCP140.dll`, not the `...D.dll` debug runtime.

CI runs the VST3 part without the editor in the `vst3-shell-windows` job on `windows-latest`: the build, the twelve proofs and the port's fixture tests. That job has not run on GitHub yet; its steps were run locally.

Not covered: ARM64, a signed or installed bundle, Pro Tools (PACE signing), and Cubase or another Steinberg host.

## Windows: packaging

`scripts\windows\build_plugin.ps1` turns a plugin crate into Windows plug-ins in one command: a CLAP file, an LV2 bundle, a VST3 bundle and an AAX bundle, for the machine's processor (x64, or ARM64 on Windows on Arm, where AAX is not built). It is the Windows counterpart of `scripts/build_plugin.sh` and `scripts/build_plugin_linux.sh`, with the same options where they mean the same thing, spelled the PowerShell way (`--manifest-path` is `-ManifestPath`), and the same environment variables. It needs Rust for the MSVC target, CMake, and Visual Studio or its Build Tools with the C++ workload; it finds the compiler, `rc` and `dumpbin` itself (`vswhere`, `vcvars64.bat`). Nothing is installed, signed or copied into a plug-in folder.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\build_plugin.ps1 `
  -ManifestPath examples\editor\Cargo.toml -Name "Example Gain" -Version 0.3.1 `
  -Manufacturer OpenGrid -BundleId com.opengrid.example -Editor -Tail -AaxPages `
  -Vst3Sdk C:\path\to\vst3sdk -AaxSdk C:\path\to\aax-sdk-2-8-1 `
  -Out dist -WorkDir build\openplugin-windows
```

| Option | Meaning |
|---|---|
| `-ManifestPath` | The plugin's `Cargo.toml`. Required |
| `-Package`, `-LibName` | cargo package, and the library stem if it is not the lib target's name. Default: from `cargo metadata` |
| `-Name` | Product name: the file names and the product in the version resource. Default: the package name |
| `-Version` | Numbers only (a `-beta` suffix is cut off). Default: the manifest's, also when it is inherited from the workspace |
| `-Manufacturer` | The vendor: the company in the version resource. Give the name the plugin reports. Default `OpenPlugin` |
| `-BundleId` | Passed on for parity with macOS; no Windows format reads it |
| `-Formats` | `clap,lv2,vst3,aax` or `all`. Default: `clap`, plus `lv2` when `-TtlBin` is given, `vst3` when a VST3 SDK is given and `aax` when an AAX SDK is given. Quote the list in PowerShell (`-Formats 'clap,lv2,vst3'`) |
| `-TtlBin` | The plugin's bin that writes the LV2 description files, called as `<bin> <folder> <stem>`; the bundle is `<Stem>.lv2` with the DLL as `<Stem>.dll` |
| `-Editor`, `-Tail`, `-AaxPages` | `OP_EDITOR`, `OP_TAIL` and `OP_AAX_PAGES` in the shells. Off by default |
| `-Features` | cargo features for the plugin crate |
| `-AllowLib` | A DLL the plug-in may import besides the system ones and the Visual C++ runtime. Repeatable |
| `-Vst3Sdk`, `-AaxSdk` | SDK checkouts, or `VST3_SDK`, `AAX_SDK`. Each is needed only for its own format |
| `-Out`, `-WorkDir`, `-Jobs` | Output folder (default `dist`), build folder, parallel jobs |
| `-DryRun`, `-NoVerify` | Print without running; skip the final checks |

What it does:

1. Builds the crate once with `cargo rustc --release --lib`: the `cdylib` is the CLAP, the `staticlib` goes into the shells. Source paths are remapped (`--remap-path-prefix`: the home folder, the folder that holds the workspace, the SDK, the Cargo and rustup homes, the workspace, the plugin crate, the build folder), and the DLL's PDB is recorded by name only (`/PDBALTPATH`).
2. CLAP: copies the DLL to `<Name>.clap`. A CLAP on Windows is the DLL itself.
3. VST3 and AAX: configures `native/CMakeLists.txt` over the static library, as `Release`, and builds only the bundle targets. `OP_TRIM_PATHS` takes the build machine's folders off `__FILE__` in the C++ code (`/d1trimfile`), because an assert in an SDK header carries its path.
4. Every binary gets a version resource from `native/win/version.rc.in`: product name, version (as text and as the four numbers), company, a file description that names the format, and the original file name. The script compiles it with `rc` and hands the `.res` to the linker for the CLAP; CMake does the same for the VST3 and AAX modules (`op_version_resource`), in every build, not only through this script.
5. Checks every binary, and fails the run on any miss: it is a 64-bit DLL; it holds none of the build machine's folders, as bytes or as UTF-16, with either slash; it imports only system DLLs and nothing of the Visual C++ runtime (`dumpbin /dependents`, through `scripts\windows\check_imports.ps1`; `-AllowLib` cannot allow `vcruntime*`, `msvcp*` or `concrt*`); it exports its format's entry points (`clap_entry`; `GetPluginFactory`, `InitDll`, `ExitDll`; the seven `ACF*` functions); and `(Get-Item x).VersionInfo` shows the product, version and manufacturer given.

**No Visual C++ Redistributable.** The C and C++ runtime is linked statically into every Windows binary, so a plug-in needs nothing installed besides Windows 10 or 11 (whose Universal CRT is part of the system). The Rust side asks for it in `.cargo/config.toml` (`-C target-feature=+crt-static` for `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`), and `build_plugin.ps1` passes the same flag itself, because its `CARGO_ENCODED_RUSTFLAGS` replaces the config file's flags and a plugin in another repository does not read it. The shells, the VST3 SDK's libraries and the AAX library are built `MultiThreaded` (`native/cmake/windows.cmake`). A plugin crate built some other way needs the flag too, in its own `.cargo/config.toml` or `RUSTFLAGS`, and a C or C++ dependency it builds with the `cc` crate follows it (`/MT`).

`scripts\windows\check_imports.ps1 -Path <files or folders>` lists the imports of every `.dll`, `.exe`, `.clap`, `.vst3` and `.aaxplugin` with `dumpbin /dependents` and fails on `vcruntime*`, `msvcp*`, `concrt*`, `vccorlib*`, `msvcr1*` and `ucrtbased.dll`. `-SelfTest` is its negative control: it builds one DLL with `/MD`, which must be caught, and the same source with `/MT`, which must pass. The `windows`, `vst3-shell-windows` and Windows `x86-core-dsp` CI jobs run it over every binary they build. The AAX shell is not built in CI (its SDK is not redistributable): its CMake asks for the static runtime, and `build_plugin.ps1` checks the built `.aaxplugin`, but that has not yet been run on a machine.

### Checks run on the example

On 16 November 2026 the command above built `examples/editor` into a folder with a space in its name (`build-package\out dir`, build folder `build-package\work dir`), every format, with the editor, the tail and the AAX page table. The script's own checks passed for the three binaries, each with the version resource `Example Gain` 0.3.1 by `OpenGrid`. Then, on its output:

- `clap-validator` 0.4.1 `validate` on `Example Gain.clap`: 44 run, 33 passed, 0 failed, 0 warnings, 11 skipped.
- pluginval 1.0.4, `--strictness-level 10` with its editor tests, in the desktop session, on `Example Gain.vst3`: `SUCCESS`.
- The AAX Validator on `Example Gain.aaxplugin` (`scripts\windows\aax_validate.ps1 -Bundle`): 14 results, 13 passed, 0 failed, 1 skipped (`test.cycle_counts`).

Before `OP_TRIM_PATHS` the AAX module held the AAX SDK's folder three times (asserts in `AAX_CParameter.h`), which is what the path check is for. Not covered: a signed bundle or an installer, ARM64, and a plugin whose crate has no `staticlib`.
