//! The few system calls the probes need, declared by hand so the crate needs
//! no binding generator.
//!
//! Each platform has its own file, and this switch is the only place that
//! picks one. Work for one platform edits only that platform's file: a Linux
//! change never touches `macos.rs`, a Windows change never touches `linux.rs`.
//! Every platform file provides the same items, which is all the rest of the
//! crate uses:
//!
//! - `open_library(path)` and `symbol(handle, name)`: load a shared library
//!   and look up an exported function (CLAP);
//! - `ModuleRef`, `MODULE_ENTRY`, `load_bundle(path)` and
//!   `bundle_function(module, name)`: load a VST3 module and find its
//!   functions, including the module entry point that takes the `ModuleRef`;
//! - `exit_now(code)`: leave the process without running any teardown.
//!
//! `macos.rs` also provides the CoreFoundation and AudioToolbox items that
//! `au.rs` uses; Audio Units exist only on macOS.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
compile_error!("openplugin-port supports macOS, Linux and Windows");
