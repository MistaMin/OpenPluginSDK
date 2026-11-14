//! GoodLookin Design Rust Interface (DRI).
//!
//! Drawing comes from [`goodlookindri`]: 47 knob styles, 40 faceplates, meters, faders,
//! switches, and the spectrum. This crate does not reimplement them and it does not use JUCE.
//! [`DrawList`] keeps a [`goodlookindri::scene::Scene`] plus a flat command list for hosts
//! that only replay simple shapes.
//!
//! On macOS, [`cocoa::EditorView`] hosts an [`openplugin_core::Editor`] in an `NSView`;
//! every format bridge uses it.

pub mod canvas;
#[cfg(target_os = "macos")]
pub mod cocoa;
pub mod controls;
pub mod styles;
pub mod window;

pub use canvas::{DrawCmd, DrawList};
pub use controls::{
    draw_faceplate, draw_faceplate_code, draw_fader, draw_knob, draw_knob_code, draw_pan_knob, draw_slider,
    draw_vu_meter, fader_db_to_pos, fader_pos_to_db,
};
pub use goodlookindri;
pub use styles::{Color, FaceplateStyle, KnobStyle, MeterStyle};
pub use window::{DriHostWindow, DriView};
