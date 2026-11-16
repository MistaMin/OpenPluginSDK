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
