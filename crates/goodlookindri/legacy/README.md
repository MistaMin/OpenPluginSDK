# Legacy macOS target (10.6.8 to 10.11)

`x86_64-apple-darwin.json` is the built-in `x86_64-apple-darwin` spec from `nightly-2023-09-01`
with two changes:

- `has-thread-local` is `false`. Rust's std then uses `pthread_getspecific` instead of
  `__thread` variables, which need `__tlv_bootstrap` from dyld on 10.7 and later.
- The deployment target is 10.6 (`llvm-target` and the linker `-platform_version`).

The file is named like the built-in target on purpose: std's build script switches on the
`TARGET` name and falls back to a restricted std for names it does not know.

Build (no `Cargo.lock`, because the old Cargo cannot read lock file version 4):

```sh
rm -f Cargo.lock
MACOSX_DEPLOYMENT_TARGET=10.6 cargo +nightly-2023-09-01 build --release -p goodlookindri-hello \
  -Zbuild-std=std,panic_abort --target legacy/x86_64-apple-darwin.json
```

Checked on a current Mac (Xcode 16 linker): `LC_VERSION_MIN_MACOSX 10.6`, no `__tlv_*`, and
the imported libSystem symbols are ones 10.6 has. It runs under Rosetta and renders the same
bytes as the modern build. **Not yet run on a real 10.6.8 to 10.11 system.**
