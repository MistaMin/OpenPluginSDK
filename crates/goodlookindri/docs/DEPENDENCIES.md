# Dependencies and licensing

GoodLookin DRI is MIT licensed, copyright 2026 Marcos Quirino Deida. See the root [LICENSE](../LICENSE).

The core crate depends only on the Rust standard library. The optional `goodlookindri-shell` crate adds one pinned dependency, `fontdue`, for text. `cargo test` uses the built-in test harness. Nothing is downloaded at build time besides the Rust toolchain you already installed.

Documentation images in `docs/images` are renders of the control catalogue, produced by this repository's `render_page` and `render_demo` examples. They are part of this repository and covered by the same MIT license. They are reference pictures, not runtime assets. The library does not load them.

GoodLookin DRI does not vendor a plugin framework, a window system, a font, or an FFT library. The spectrum transform is implemented in `src/spectrum.rs`.

If you replay `Scene` through another graphics library, that library keeps its own license. Linking GoodLookin DRI does not relicense your plugin.
