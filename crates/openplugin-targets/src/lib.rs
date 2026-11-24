//! OpenPluginSDK format entries.
//!
//! A plugin crate calls one macro per format:
//! - [`export_clap!`]: the CLAP entry (`export_clap!(MyPlugin)`, or
//!   `export_clap!(MyPlugin, editor = make_editor)`).
//! - [`export_lv2!`]: the LV2 descriptor, with the same two forms. [`lv2`] writes the
//!   bundle's `.ttl` files.
//! - [`export_shell!`]: the `op_shell_*` C ABI that the native VST3, Audio Unit and AAX
//!   shells in `native/` link, with the same two forms.
//!
//! The macros are the supported way in. What they expand to (`clap`, `clap_abi`,
//! `shell` and parts of `lv2`) is hidden and not part of the frozen API: the CLAP and
//! LV2 specifications and the C headers in `native/include` are the contracts there.
//!
//! # Example
//!
//! One plugin type, one line per format. The crate is built as a `cdylib` for CLAP and
//! LV2 and as a `staticlib` for the native shells.
//!
//! ```
//! use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
//!
//! static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", 0.0, 1.0, 1.0, Unit::None)];
//! static INFO: PluginInfo =
//!     PluginInfo::new("com.example.gain", "Gain", "Example", "1.0.0", Category::Utility, &PARAMS);
//!
//! #[derive(Default)]
//! struct Gain {
//!     gain: f32,
//! }
//!
//! impl Plugin for Gain {
//!     fn info(&self) -> &'static PluginInfo {
//!         &INFO
//!     }
//!     fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
//!     fn reset(&mut self) {}
//!     fn set_param(&mut self, id: &str, value: f32) -> bool {
//!         if id == "gain" {
//!             self.gain = value;
//!         }
//!         id == "gain"
//!     }
//!     fn param(&self, id: &str) -> Option<f32> {
//!         (id == "gain").then_some(self.gain)
//!     }
//!     fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
//!         for channel in io.iter_mut() {
//!             for sample in &mut channel[..frames] {
//!                 *sample *= self.gain;
//!             }
//!         }
//!     }
//! }
//!
//! openplugin_targets::export_clap!(Gain);
//! openplugin_targets::export_lv2!(Gain);
//! openplugin_targets::export_shell!(Gain);
//!
//! fn main() {
//!     // The host's first calls: the CLAP entry, LV2 plugin 0, and the shell's ABI number.
//!     assert!(clap_entry.init.is_some());
//!     assert!(!lv2_descriptor(0).is_null());
//!     assert!(lv2_descriptor(1).is_null());
//!     assert!(unsafe { op_shell_abi_version() } > 0);
//! }
//! ```
//!
//! # Threads and allocation
//!
//! The macros export what a host calls, so the host chooses the thread. In every format
//! the block call (CLAP `process`, LV2 `run`, `op_shell_process`) is the audio thread:
//! it does not allocate, lock or wait, and a panic inside the plugin is caught there and
//! the block is output as silence. Creating, activating, saving and loading state and
//! every editor call are main-thread calls and may allocate. The functions of [`lv2`]
//! that write description files are for a build step or a small program run after the
//! build, never for the audio thread.

#![deny(missing_docs)]

#[doc(hidden)]
pub mod clap;
#[doc(hidden)]
pub mod clap_abi;
pub mod lv2;
#[doc(hidden)]
pub mod shell;

#[doc(hidden)]
pub use clap::ClapAdapter;
#[doc(hidden)]
pub use lv2::Lv2Harness;
// The export macros name the core crate through this.
#[doc(hidden)]
pub use openplugin_core as __core;
