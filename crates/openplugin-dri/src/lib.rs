//! GoodLookin Design Rust Interface (DRI).
//!
//! Drawing comes from [`goodlookindri`]: 47 knob styles, 40 faceplates, meters, faders,
//! switches, and the spectrum. This crate does not reimplement them and it does not use JUCE.
//! [`DrawList`] keeps a [`goodlookindri::scene::Scene`] plus a flat command list for hosts
//! that only replay simple shapes.
//!
//! GoodLookin DRI 0.2 adds a lit tier: knobs shaded per pixel and blitted as images, plus
//! gradient, shadow and image commands. [`DrawList::set_knob_tier`] picks the tier (lit by
//! default; classic and flat remain), and [`SceneCanvas`] rasterises a whole scene into the
//! pixels an [`openplugin_core::Editor`] returns, at the display's backing scale.
//!
//! On macOS, [`cocoa::EditorView`] hosts an [`openplugin_core::Editor`] in an `NSView`;
//! every format bridge uses it. On Windows, [`win32::EditorView`] hosts it in a child `HWND`.

pub mod canvas;
#[cfg(target_os = "macos")]
pub mod cocoa;
pub mod controls;
pub mod design_editor;
pub mod hit;
pub mod render;
pub mod styles;
#[cfg(windows)]
pub mod win32;
#[cfg(target_os = "linux")]
pub mod x11;

pub use canvas::{DrawCmd, DrawList};
/// GoodLookin DRI's `goodlookindri::draw::KnobMode` under the SDK's name: `Lit`, `Classic`
/// or `Flat`. A tier GoodLookin adds shows up here.
pub use goodlookindri::draw::KnobMode as KnobTier;
pub use render::SceneCanvas;
pub use controls::{
    draw_faceplate, draw_fader, draw_knob, draw_pan_knob, draw_slider, draw_vu_meter, fader_db_to_pos,
    fader_pos_to_db, knob_angle,
};
/// The drawing library, for building scenes of your own. Its version is part of the
/// SDK's API: a breaking GoodLookin DRI release is a breaking SDK release.
pub use goodlookindri;
pub use design_editor::{DesignEditor, DesignEditorError, DesignOptions};
pub use hit::{Area, Control, ControlId, ControlKind, ControlSet, DoubleClick, Key, Modifiers};
pub use openplugin_core::editor::{DamageRect, DamageTracker};
pub use styles::{Color, MeterStyle};
