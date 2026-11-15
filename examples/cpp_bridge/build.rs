//! Compiles the C++ side with the system clang++ (C++17) through cxx-build.
//!
//! `cc` (inside cxx-build) passes the Cargo target's architecture and
//! `MACOSX_DEPLOYMENT_TARGET` to clang++, so `cargo build --target
//! x86_64-apple-darwin` and `--target aarch64-apple-darwin` each give a slice
//! that `lipo` joins (scripts/build_plugin.sh does this for `--archs universal`).

use std::env;

fn main() {
    let mut build = cxx_build::bridge("src/lib.rs");
    if env::var_os("CXX").is_none() && env::var_os("CXX_aarch64_apple_darwin").is_none() && cfg!(target_os = "macos") {
        build.compiler("clang++");
    }
    build.std("c++17").include("cpp").file("cpp/dsp.cpp").file("cpp/bridge.cpp").warnings(true);
    build.flag_if_supported("-Wextra").flag_if_supported("-fvisibility=hidden");

    if env::var_os("MACOSX_DEPLOYMENT_TARGET").is_none() && cfg!(target_os = "macos") {
        build.flag("-mmacosx-version-min=11.0"); // what rustc links with by default
    }

    // No build path in the shipped binary (build_plugin.sh checks for one).
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let out = env::var("OUT_DIR").unwrap_or_default();
    for (from, to) in [(out.as_str(), "build"), (manifest.as_str(), "plugin")] {
        if !from.is_empty() {
            build.flag_if_supported(format!("-ffile-prefix-map={from}={to}"));
        }
    }

    if env::var_os("CARGO_FEATURE_TEST_FAULTS").is_some() {
        build.define("OPB_TEST_FAULTS", "1").file("cpp/count_new.cpp");
    }
    build.compile("opbridge");

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cpp");
}
