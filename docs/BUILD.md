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
| `--au-subtype`, `--au-manufacturer` | Four-character codes. Required for `au` and for `--aax-pages` |
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

`scripts/build_plugin_linux.sh` builds a plugin crate into the formats that work on Linux today: CLAP and LV2. It is plain bash over `cargo` and binutils, takes the same option names and environment variables as `build_plugin.sh` where they mean the same thing (`--manifest-path`, `--package`, `--lib-name`, `--name`, `--version`, `--ttl-bin`, `--features`, `--formats`, `--out`, `--work-dir`, `--jobs`, `--dry-run`, `--no-verify`), and refuses `vst3`, `au`, `aax` and `standalone`, which are not built on Linux yet. The macOS-only options (`--bundle-id`, `--editor`, `--archs` and so on) are refused by name. It runs only on Linux and never installs anything.

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

The library is a release build of the host architecture, with the same `--remap-path-prefix` list as on macOS. `--strip` runs `strip --strip-all` on each copy; the dynamic symbols stay. Unless `--no-verify` is given, the script then checks each library and fails the run on any of these:

- `strings -a` finds the build, workspace, its parent, plugin crate, SDK or home path.
- `readelf -d` lists a `NEEDED` library other than glibc's (`libc`, `libm`, and `libdl`, `libpthread`, `librt` for older toolchains), `libgcc_s` and the loader. `--allow-lib SONAME` (or `OP_ALLOW_LIBS`) accepts another. `ldd` reports a library as not found.
- An `RPATH` or `RUNPATH` points into the build tree, the workspace or the home folder.
- `nm -D --defined-only` does not show `clap_entry` (CLAP) or `lv2_descriptor` (LV2).
- The LV2 bundle lacks `manifest.ttl` or `<Stem>.ttl`, or `manifest.ttl` does not name `<Stem>.so` as `lv2:binary`.

### Checks run on Linux

On 15 November 2026, on Arch Linux (x86_64, Intel Core i7-2630QM, Rust 1.100), `examples/editor` was built with the command above, with and without `--strip`, once into a folder with a space in its name:

- The script's checks passed. Both libraries need only `libgcc_s.so.1`, `libm.so.6`, `libc.so.6` and `ld-linux-x86-64.so.2`, have no run path, and contain no build or home path.
- `lv2_validate manifest.ttl ExampleGain.ttl` in the bundle: `Found 0 errors among 84 files (checked 1972 restrictions)`. The one skipped file is `/usr/lib/lv2/core.lv2/lv2core.doap.ttl`, a specification file that Arch's `lv2` package names in `lv2_validate` but does not ship; no bundle file is skipped.
- `clap-validator` 0.4.1, built with `cargo install` from commit `152b982` (the one the `universal` job builds), on `Example Gain.clap`: 44 tests run, 0 failed, 10 skipped, and 34 passed or 33 passed with 1 warning depending on timing (seven full runs: 34 passed in four, 33 passed and 1 warning in three). The skips are the ones seen on macOS: no preset discovery factory (3), no 64-bit audio (2), no note input port (3), no `audio-ports-activation` (1), no `configurable-audio-ports` (1). The warning is `process-audio-denormals`, which compares the time to process denormal input with normal input: 1.28 to 2.45 times longer in the full runs, and 4.6 to 5.6 times longer in 15 runs of that test alone (`--include process-audio-denormals`), each of which warned. No format entry sets flush-to-zero around `process` (`openplugin_dsp::utility::DenormalGuard` exists but is not used there). The machine was shared with other builds during these runs. On macOS (arm64, and x86_64 under Rosetta) the test passes.

CI runs the same commands in the `linux-package` job on `ubuntu-latest`. Its `apt-get install lv2-dev sordi` step has not run on GitHub yet; everything after it was run on the Arch machine.

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
- `process-audio-denormals` can end with a warning instead of a pass on `Editor.clap`, `DesignEditor.clap` and `CppBridge.clap` (denormal input took 1.3 to 3.4 times longer; the higher figures with the machine busy with builds). x86_64 handles denormals in microcode, and neither the examples nor the CLAP wrapper set flush-to-zero (`openplugin_dsp::DenormalGuard` does, for a plug-in that wants it). A warning is not a failure.
- `RoleTags.clap` fails `param-set-events` and `param-set-no-cookies` on every platform: its one parameter is a toggle whose default is 0, and the validator's fixed-seed value for it is also 0, so the test sees no change. With the default set to 1 for a trial build, both pass; `openplugin-port capture --set Spare=1` reads 1 back through `clap_plugin_params::flush`. The example is a test plug-in for the channel order proof and is not changed for this.

REAPER 7.80 (a portable copy with its own `-cfgfile` and the Dummy Audio device) scans all four, instantiates each, and renders a project with `Editor.clap` (Gain 6 dB, Warm on, set from ReaScript) with `reaper.exe -renderproject`, from an SSH session as well as in the desktop session. The 32-bit float render is bit-identical to `openplugin-port null --save` (the SDK's own offline render of the same input and settings), and the state REAPER saved in the `.rpp` reopens to Gain 6 dB and Warm on.
