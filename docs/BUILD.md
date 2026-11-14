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

## Checks run on the example

On 15 November 2026, `examples/editor` was built for every format with `--archs arm64` and no signing identity. Then, independently of the script:

- `codesign --verify --deep --strict` passed on the CLAP, VST3, Audio Unit and AAX bundles and on the LV2 library.
- `lv2_validate` passed on the LV2 bundle's `manifest.ttl` and `ExampleGain.ttl` (0 errors).
- `clap-validator` 0.4.1 passed the CLAP bundle: 44 tests run, 34 passed, 0 failed, 10 skipped.
- No binary contained a build, workspace, SDK or home path, and the LV2 library's install name was `@loader_path/ExampleGain.dylib`.
- A `--archs universal` build of the CLAP and LV2 produced an `x86_64 arm64` binary that passed the same `clap-validator` run.

Not covered: the standalone path (the example has no standalone binary), real Developer ID signing, notarisation and PACE signing (shown with `--dry-run` only), and Intel hardware.
