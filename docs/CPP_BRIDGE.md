# Calling existing C++ DSP from a Rust plugin

This guide is for a team that already has DSP in C++ and wants to ship a plugin on OpenPluginSDK before it has rewritten that DSP in Rust. This is C++ by nature, and that is the point: the bridge exists to reuse code you already own and have tested, until you replace it. Everything else in the SDK is Rust; nothing here asks you to write new C++ for the SDK's sake. The SDK's own DSP (`openplugin-dsp`) stays the better home for new work, and [PORTING.md](PORTING.md) is where the rest of a port is described.

The worked example is [`examples/cpp_bridge`](../examples/cpp_bridge): a one-pole low-pass and a smoothed gain written in C++17 (`cpp/dsp.h`, `cpp/dsp.cpp`), a thin shim (`cpp/bridge.h`, `cpp/bridge.cpp`), the [`cxx`](https://crates.io/crates/cxx) declaration and the `Plugin` wrapper (`src/lib.rs`). It is exported as CLAP and LV2, passes the conformance kit and `clap-validator`, and builds as a universal binary with `scripts/build_plugin.sh`. All of the C++ was written for this example; none of it is taken from any other SDK.

## The pattern in one page

```
 host audio thread                         host main thread
        |                                         |
  Plugin::process  --- noexcept, slices --->  (C++ DSP class)  <--- prepare / load_state / save_state
        |                                         ^                  (may allocate, may throw;
  one call per channel,                           |                   shim catches, Rust gets Err)
  no allocation, no throw                  Result<..> from cxx
```

1. **Keep your C++ class as it is.** It needs no Rust or cxx header. `dsp.h` includes only the standard library.
2. **Write free functions in a shim header** (`bridge.h`) that take your class by reference. cxx binds free functions without touching the class, so the class keeps its shape. The shim is the only place that includes `rust/cxx.h`.
3. **Declare the bridge in Rust** with `#[cxx::bridge]`: the class is an opaque `type`, held in a `UniquePtr`. Functions that cannot throw are plain `fn`. Functions that may throw return `Result<..>`.
4. **Wrap it as a `Plugin`.** `Default` makes the C++ object. `prepare` calls the C++ `prepare` (which allocates) and records an error instead of panicking. `process` makes one call per channel with `&mut [f32]`. `set_param` and `param` pass plain `f32` values.
5. **State is bytes.** The C++ side saves a versioned blob; `save_state` returns it as a `Vec<u8>`, `load_state` hands a `&[u8]` back and returns `false` on any `Err`.
6. **Export** with `openplugin::targets::export_clap!(Wrapper)` (and `export_lv2!`) exactly as a pure Rust plugin does.

### Cargo and build

```toml
[lib]
crate-type = ["cdylib", "staticlib", "rlib"]

[dependencies]
openplugin = "0.3"
cxx = "=1.0.198"

[build-dependencies]
cxx-build = "=1.0.198"
```

`build.rs`:

```rust
let mut build = cxx_build::bridge("src/lib.rs");
build.compiler("clang++").std("c++17").include("cpp")
     .file("cpp/dsp.cpp").file("cpp/bridge.cpp");
build.compile("opbridge");
```

- **Licence.** `cxx`, `cxx-build` and their helper crates are `MIT OR Apache-2.0`, the same as the SDK.
- **Version.** `cxx` 1.0.198 is the newest release whose declared minimum Rust version (1.85) matches the SDK's. 1.0.199 and later require Rust 1.88. The example pins both crates with `=`, because cxx and cxx-build must be the same version. Raise the pin when the SDK raises its minimum Rust version.
- **Compiler.** `cxx-build` uses the `cc` crate, which reads the `CXX` environment variable and otherwise `c++`; on macOS both are Apple clang. The example asks for `clang++` unless `CXX` is set. C++17 is set with `.std("c++17")`.

## Rules for the C++ on the audio path

These are the SDK's real-time rules ([PERFORMANCE.md](PERFORMANCE.md), [STABILITY.md](STABILITY.md)) applied to code the SDK cannot see into.

1. **No allocation.** No `new`, `malloc`, `std::vector::push_back` or `resize`, `std::string`, `std::function` that captures, `std::shared_ptr` creation, `std::make_*`, or a standard container copy. Allocate everything in `prepare`, sized for `max_block` and the channel count, and reuse it. `dsp.cpp` allocates its per-channel state in `prepare` with build-then-swap, so a failed `prepare` leaves the old state intact.
2. **No locks.** No `std::mutex`, `std::lock_guard`, `std::condition_variable`, `std::shared_mutex`, no `std::atomic` operation that is not lock-free, no `std::cout` or `printf`, no file or network access. Pass values from other threads through atomics or a wait-free queue (`openplugin_core::spsc`, the `Seqlock`).
3. **No exceptions.** Every function the audio thread calls is `noexcept` in the C++ header and the shim. Do not use `std::vector::at`, `dynamic_cast` to a reference, `std::stoi` and similar throwing calls there. If something does throw out of a `noexcept` function, the C++ runtime calls `std::terminate`, which is loud and immediate; cxx does not catch in a function declared without `Result`, so the process ends instead of unwinding through Rust frames (which is undefined behaviour).
4. **No unbounded loops or recursion**, and no first-use initialisation: a function-local `static` that constructs on first call takes a lock and may allocate. Construct such objects in `prepare`.
5. **Denormals and NaN.** `dsp.cpp` flushes tiny values in its state and output and treats non-finite input as zero. The conformance kit's `hostile_input` and `no_denormals` checks apply to C++ output as much as Rust.
6. **Channels.** `Plugin::process` can be given more channels than `prepare` saw; the contract is that extra channels are untouched. The example's C++ ignores a channel index it did not allocate.

## The boundary

### Slices, not copies

`Plugin::process` gets `&mut [&mut [f32]]`, planar. The wrapper calls `ffi::process(dsp, channel_index, &mut channel[..frames])`; cxx maps `&mut [f32]` to `rust::Slice<float>` (a pointer and a length) so nothing is copied. If your C++ wants interleaved audio or a fixed block size, convert in a buffer allocated in `prepare`; never size it on the audio thread.

### Errors, not panics or throws

`prepare`, `save_state` and `load_state` can fail, so they are `-> Result<..>` in the bridge. cxx catches `std::exception` in the generated shim and returns `Err(cxx::Exception)`, whose `what()` is the message. It does not catch other thrown types, which would terminate; so the shim runs every throwing call through a `guard()` that rethrows `std::exception` and turns anything else into a `std::runtime_error`:

```cpp
template <class F> auto guard(F&& f) -> decltype(f()) {
    try { return f(); }
    catch (const std::exception&) { throw; }
    catch (...) { throw std::runtime_error("unknown (non-std) C++ exception"); }
}
```

On the Rust side, a failed `prepare` leaves the plugin in a not-ready state: `process` passes audio through unchanged and `last_error()` says why. The next successful `prepare` recovers. A bad state chunk makes `load_state` return `false` and changes nothing (the C++ parses and validates the whole chunk before applying any of it).

### Panics

A Rust panic must not leave an `extern "C"` function: since the 2024 edition it aborts the host. The SDK's CLAP and LV2 entries do not wrap the plugin in `catch_unwind` (the shell ABI does), so the example does it in `process`:

```rust
if catch_unwind(AssertUnwindSafe(|| self.run_dsp(io, frames))).is_err() {
    self.ready = false; // pass through from now on
    self.last_error = Some("panic in process".into());
}
```

`catch_unwind` is free until a panic happens. A panic cannot come from the C++ call itself (cxx does not unwind through C++), so this only guards the Rust glue.

### Parameters and state

Parameters are plain `f32` values. The example makes the C++ object the source of truth: `set_param` clamps with the `ParamInfo` and calls the `noexcept` setter, and `param` calls the `noexcept` getter, so a loaded chunk is visible immediately. If your class keeps its parameters in its own types, convert at the edge.

State is a byte blob in both directions. The example's is 16 bytes: a magic, a version, then the values, in host byte order (Apple targets are all little-endian; write explicit little-endian if your chunk must be portable). Start the blob with a version, validate everything, and throw `std::invalid_argument` for anything else. The conformance kit feeds `load_state` truncated and random bytes, and the `state` check requires save, load, save to be byte-identical. If all of your plugin's state is its parameters, return `true` from `state_is_parameters_only` and skip the chunk entirely.

## Threading

The SDK creates the plugin on one thread, calls `prepare` and `load_state`/`save_state` on others, and runs `process` on the audio thread; never two at once on the same object except where the SDK documents it (shell state calls on a plugin that is not `state_is_parameters_only` skip a block, see `Plugin::state_is_parameters_only`). The C++ object needs no thread affinity for this to work, and the example says so with `unsafe impl Send for OnePoleGain {}`. Write that line only after checking that your class has no thread-local state, no GUI or main-thread-only calls, and no globals shared between instances without synchronisation. Two plugin instances in one host run on different audio threads at the same time, so a global in your C++ (a lookup table built on first use, a static scratch buffer) is a data race. Make such tables `const`, built at load, or per instance.

If your C++ starts its own threads, the conformance kit does not see their allocations or locks (it counts the calling thread only). Prefer the SDK's worker (`openplugin_core::worker`) for background work.

## Checking it

### The conformance kit, and what it can see of C++

```rust
openplugin_testing::install_allocator!();

#[test]
fn conforms() {
    conformance(CppBridge::default, Options::default()).assert_pass();
}
```

The `no_alloc` check arms a counting `#[global_allocator]` on the audio thread. That allocator is Rust's: it counts calls that go through `std::alloc`. **C++ allocation does not go through it.** A C++ `new` ends up in `operator new` in libc++, which calls `malloc`; neither touches Rust's global allocator. The example proves this in `tests/faults.rs`:

| Allocation in the C++ `process` | Seen by `no_alloc` as shipped | Seen with the test `operator new` | Seen by `malloc_logger` |
| --- | --- | --- | --- |
| `new float[16]` | no | **yes, check fails** | not tested (`new` ends in `malloc`) |
| `malloc(64)` | no | no | **yes** |

So a green `no_alloc` on a bridged plugin proves nothing about the C++ unless one of the extra measures below is on. There are two:

- **Count `operator new` in the kit.** `cpp/count_new.cpp` replaces the global `operator new` and `delete` and forwards them to Rust's allocator (`src/lib.rs`, module `counted_new`), where the kit's counter sees them. It is compiled only with `--features test-faults`, which also adds C++ fault hooks (`set_fault`). With it a deliberately allocating C++ `process` fails the check: `cpp_new_on_the_audio_path_fails_the_no_alloc_check`. Use the same trick in your test build: it covers `new`, `std::vector`, `std::string` and everything built on them. It does not cover `malloc`/`calloc`/`realloc` called directly, nor allocation inside a system library.
- **Count `malloc` itself.** macOS libmalloc calls the function pointer `malloc_logger` on every allocation and free when it is set (it is what `MallocStackLogging` uses). `tests/faults.rs` sets it, counts allocations on the calling thread while one `process` call runs, and sees the `malloc` fault (`raw_cpp_malloc_is_invisible_to_the_kit_but_visible_to_malloc_logger`: zero for the clean path, one per channel for the faulty one). It is a private but long-stable libmalloc symbol; use it in tests, never in a shipped build. Instruments' Allocations template, or `MallocStackLogging=1` with `malloc_history`, give the same answer on a real host.

Run the fault tests with:

```bash
cargo test -p example-cpp-bridge --features test-faults
```

The `test-faults` feature must never be on in a shipped build: it replaces `operator new` for the whole library. `scripts/build_plugin.sh` does not pass it unless you give `--features`.

The other checks (`finite_output`, `no_denormals`, `hostile_input`, `state`, `tail`, `latency`, `param_fuzz`) work on the audio, so they apply to the C++ unchanged. `locks` is always skipped; use the thread sanitizer or a profiler for it (see [TESTING_PLUGINS.md](TESTING_PLUGINS.md)). For exceptions: a throw from a `noexcept` function terminates, so a plugin that does it fails the whole test binary, loudly.

### Also run

- `clap-validator validate Plugin.clap` (CI pins a version; the example gives 34 passed, 0 failed, 10 skipped).
- Address and undefined behaviour sanitizers: set `RUSTFLAGS="-Zsanitizer=address"` on nightly, or compile the C++ alone with `-fsanitize=address,undefined` in its own test; sanitizing only one side misses bugs on the other.
- Valgrind-style tools do not run on current macOS; use Instruments (Leaks, Allocations, Time Profiler) on a real host.

## ABI and toolchain

- **One C++ standard library.** Link `libc++` (the macOS default, and what `link-cplusplus` selects for Apple targets). Never mix it with `libstdc++`. Do not let C++ standard-library types cross the bridge: only the cxx types (`rust::Slice`, `rust::Vec`, `rust::String`), integers, floats, and `std::unique_ptr` of your opaque class, which cxx owns.
- **One compiler for all of your C++.** Compile the bridge and the DSP with the same clang and the same flags (`-std=c++17`, same `-fno-rtti` / `-fno-exceptions` choice). The bridge needs exceptions on (the guard relies on them) so do not compile with `-fno-exceptions`. A prebuilt C++ static library built with different settings must use the same libc++ and the same exceptions model.
- **Deployment target.** Bridge, DSP and Rust must agree. `build_plugin.sh` exports `MACOSX_DEPLOYMENT_TARGET` (default 11.0) and `cc` passes it to clang++. When it is unset (a plain `cargo build`) the example's `build.rs` passes `-mmacosx-version-min=11.0`, what rustc links with, so there are no "built for newer macOS" linker warnings.
- **Symbols.** The cxx runtime exports `cxxbridge1$...` symbols from the library. Two cxx-based plugins in one host do not clash (macOS uses two-level namespaces), but compile the C++ with `-fvisibility=hidden` (the example does) so that your own classes are not exported.
- **Path leaks.** `build_plugin.sh` fails a build whose binaries contain the build or home path. C++ `__FILE__` and debug info carry paths, so `build.rs` passes `-ffile-prefix-map` for the output and manifest folders.
- **Third-party C++.** Any C++ you link carries its own licence; this guide is only about the bridge. A closed-source static library works the same way as long as it is built with the toolchain above.

### Universal builds

`cxx-build` builds the C++ for whatever target Cargo is building, so a universal plugin is two Cargo builds joined by `lipo`, which `build_plugin.sh --archs universal` already does:

```bash
scripts/build_plugin.sh --manifest-path examples/cpp_bridge/Cargo.toml --version 0.3.1 \
  --name CppBridge --bundle-id com.example.cppbridge \
  --formats clap,lv2 --ttl-bin lv2_ttl --archs universal \
  --out out --work-dir work
```

This builds `--target aarch64-apple-darwin` and `--target x86_64-apple-darwin`, compiles the C++ twice (`cc` adds `-arch`/`--target` itself), `lipo`s the two dylibs, and the script's final check confirms that every binary has both slices (`architectures (x86_64 arm64)`) and no build path. It needs `rustup target add x86_64-apple-darwin`. The example was built this way and the CLAP bundle ran under `clap-validator` on arm64. To do it by hand: `cargo build --release --target aarch64-apple-darwin` and `--target x86_64-apple-darwin`, then `lipo -create -output`. Do not pass `-arch arm64 -arch x86_64` to one clang++ invocation for `cxx-build`: Cargo needs a separate artefact per target for the Rust half anyway.

## Debugging across languages

lldb debugs Rust and C++ in one session; `cxx`'s generated shims appear in backtraces as `opbridge$cxxbridge1$...`, followed by your `opbridge::...` function.

```bash
lldb -- target/debug/deps/conformance-<hash>          # a test binary
(lldb) breakpoint set -n opbridge::OnePoleGain::process
(lldb) breakpoint set -n rust_panic                    # Rust panics, before unwinding
(lldb) breakpoint set -E c++                           # every C++ throw
(lldb) run
(lldb) bt
(lldb) frame variable
```

- Debug builds give both sides line information. `cxx-build` compiles C++ with `-g` in the dev profile, and with release plus debug info (`debug = 1`) in release.
- A host loading the plugin: `lldb -n "Host Name"` attaches, then `breakpoint set -n opbridge::process --shlib CppBridge` (breakpoints resolve when the bundle loads).
- To stop on the first C++ exception, `breakpoint set -E c++` (or `-n __cxa_throw`); to stop on an abort from a `noexcept` violation, `breakpoint set -n std::terminate`.
- To find a stray allocation on the audio thread, break on `malloc` with a condition on the thread: `breakpoint set -n malloc -c '(int)pthread_main_np() == 0'`, then `bt` shows which C++ line called it.
- Mixed stacks print with `bt`; `frame variable` shows C++ members and Rust locals.

## What to do next

Port the C++ class function by function behind the same `Plugin`, keeping the C++ as a reference. The example's tests are the pattern: a small `dc_settles_to_the_gain` style test per function, the conformance kit for the whole, and the SDK's format parity tests ([TESTING_PLUGINS.md](TESTING_PLUGINS.md)) when you compare the Rust version to the C++ one. When the C++ is gone, delete `cxx`, `build.rs` and `cpp/`; nothing else in the plugin changes.

## Limits

- The bridge costs one function call per channel per block, no more. It is not a way to run C++ with a GUI; editors are described in the SDK's editor docs.
- cxx supports a subset of C++ types across the boundary. Anything else (templates, overloaded names, references to standard containers) stays behind your shim.
- A `noexcept` function that throws terminates the host process. The kit cannot test that in process; review the C++ audio path for throwing calls, and use `-fno-exceptions` in a separate build of the DSP-only code if you want the compiler to prove it.
- `malloc_logger` is macOS only. On other platforms use that platform's allocation hooks.
