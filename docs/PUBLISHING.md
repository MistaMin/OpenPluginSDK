# Publishing to crates.io

Nothing here has been run for real. Every crate in `crates/` is ready to publish except for one prerequisite: GoodLookin DRI is not on crates.io yet.

## Prerequisite: GoodLookin DRI

`openplugin-dri` depends on two crates that live outside this repository:

| Crate                | Version | Source                                    |
|----------------------|---------|-------------------------------------------|
| `goodlookindri`      | 0.2.0   | `../GoodLookinDri`                        |
| `goodlookindri-shell`| 0.2.0   | `../GoodLookinDri/shell`                  |

The workspace `Cargo.toml` names both with `path` and `version`. Cargo uses the path here and the version on crates.io, so nothing changes in this repository once they are published. GoodLookin DRI's owner publishes them first, `goodlookindri` before `goodlookindri-shell` (the shell depends on it). If either is released with a different version, update the two `version = "0.2.0"` entries in the workspace `Cargo.toml`.

`openplugin-targets` and `openplugin` reach GoodLookin DRI only through `openplugin-dri`, so the whole chain waits on this step.

## Publish order

Each crate must be on the registry, and visible in the index, before the next one is packaged.

| # | Crate                | Depends on (inside the SDK)                            | Dry run today                                              |
|---|----------------------|--------------------------------------------------------|------------------------------------------------------------|
| 1 | `openplugin-core`    | none                                                   | passes (38 files, 177 KiB, 45 KiB compressed)              |
| 2 | `openplugin-dsp`     | core                                                   | blocked: `openplugin-core` is not on crates.io yet         |
| 3 | `openplugin-ffi`     | core                                                   | blocked: `openplugin-core` is not on crates.io yet         |
| 4 | `openplugin-dri`     | core, `goodlookindri`, `goodlookindri-shell`           | blocked: GoodLookin DRI and `openplugin-core` not published |
| 5 | `openplugin-targets` | core, dri, ffi (dsp as a dev dependency)               | blocked: SDK crates above not published                    |
| 6 | `openplugin`         | core, dsp, dri, targets, ffi                           | blocked: SDK crates above not published                    |

`dsp` and `ffi` do not depend on each other and can swap places. `dsp` is also a dev dependency of `targets`, so it must be published before `targets`.

Every blocker above is an unpublished dependency. Metadata, package contents and package size were checked and need nothing more: each package holds `Cargo.toml`, `README.md`, `src/` and, where present, `tests/` (6 to 38 files).

## What each crate carries

All six crates inherit `version`, `edition`, `rust-version`, `license`, `repository`, `homepage`, `authors`, `keywords` and `categories` from the workspace, and set `description`, `documentation` (docs.rs) and `readme` (`crates/<name>/README.md`). Examples are `publish = false`.

Not included: the licence texts (`LICENSE-MIT`, `LICENSE-APACHE`) sit at the repository root and are not copied into the packages. crates.io accepts that because `license` is set, but copying both files into each crate directory before publishing would ship the texts with the source.

## Version

The version is the pre-release `0.3.1-beta`. Cargo does not pick pre-releases for `openplugin = "0.3"`; users write `openplugin = "0.3.1-beta"`. The roadmap's 1.0 item (frozen API, changelog) is a separate step from getting the crates on the registry.

## Commands

Check the tree first. From the repository root, on a clean checkout of the release commit:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo +1.85.0 check --workspace --all-targets --locked
```

Log in once (this stores a token; use an account that owns the crate names):

```sh
cargo login
```

Then, after `goodlookindri` and `goodlookindri-shell` 0.2.0 are on crates.io, publish in order. Each `cargo publish` waits until the crate shows up in the index, so the next one resolves. Do a dry run for a crate first if you want to see its package:

```sh
cargo publish --dry-run -p openplugin-core
cargo publish -p openplugin-core

cargo publish -p openplugin-dsp
cargo publish -p openplugin-ffi
cargo publish -p openplugin-dri
cargo publish -p openplugin-targets
cargo publish -p openplugin
```

A dry run of crates 2 to 6 only works once the crates before it are published, because cargo resolves the packaged manifest against the registry. After the last one, check that a fresh project builds:

```sh
cargo new check-openplugin && cd check-openplugin
cargo add openplugin@0.3.1-beta
cargo build
```

A published version cannot be replaced. If something is wrong, `cargo yank --version 0.3.1-beta <crate>` hides it from new projects, and the fix goes out as the next version.
