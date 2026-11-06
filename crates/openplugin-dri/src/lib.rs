//! GoodLookin Direct Rendering Interface (DRI).
//!
//! Procedural 3D hardware control renderers, faceplates, ballistics, and native window embedding
//! without JUCE runtime dependencies.

pub mod canvas;
pub mod controls;
pub mod styles;
pub mod window;

pub use canvas::{DrawCmd, DrawList};
pub use controls::{draw_faceplate, draw_fader, draw_knob, draw_pan_knob, draw_slider, draw_vu_meter, fader_db_to_pos, fader_pos_to_db};
pub use styles::{Color, FaceplateStyle, KnobStyle, MeterStyle};
pub use window::{DriHostWindow, DriView};
