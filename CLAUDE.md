# OpenPluginSDK: notes for coding agents

A pure-Rust audio plugin SDK for CLAP, LV2, VST3, Audio Unit and AAX. Read `docs/ARCHITECTURE.md` and `docs/ROADMAP.md` first.

## Rules

- **Rust wherever possible.** C++ only inside `native/`, where a format's SDK requires it. No JUCE, and no code ported from JUCE or from any source under a licence the SDK cannot carry. DSP is written from the cited paper or standard.
- **Platform code.** Follow [docs/PLATFORMS.md](docs/PLATFORMS.md): each platform's code lives in files of its own, and work for one platform edits only that platform's files. Linux and Windows work never edits macOS files (`sys/macos.rs`, `au.rs`, `cocoa.rs`, `native/`). A shared file gets additions behind `#[cfg]`, never a rewrite, and every shared file you change is named in the report.
- **Where to work.** In your own worktree or checkout on your own branch, never in the main checkout. Don't push to `main`; it is protected, and changes arrive through pull requests.
- **Audio path.** No allocation, no locks, no panics; denormal-safe; computes for the real sample rate.
- **Compatibility.** New trait methods get defaults that keep today's behaviour, unless the task says otherwise.
- **Safety.** Quote every path (several contain spaces). No `rm -rf` outside your own build folder. Read a script before running it. Install nothing without the owner's agreement.
- **Commits.** A plain message in the repo's style (`area: what changed`), with no `Co-Authored-By` lines and no mention of AI.

## Checks

- `cargo test --workspace --no-fail-fast`
- `cargo +1.100.0 clippy --workspace --all-targets -- -D warnings` (CI's Rust; it has a stricter `needless_range_loop`)
- `cargo +1.85.0 check --workspace` (the minimum Rust version)
- After touching `native/`: build the shell and run `ctest`, as in `docs/BUILD.md`.
- Wait for conditions (`run_until`, a counter moving), not fixed sleeps: CI runners are slow.
