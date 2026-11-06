//! GoodLookin Direct Rendering Interface (DRI).
//!
//! Procedural 3D hardware control renderers, faceplates, ballistics, and native window embedding
//! without JUCE runtime dependencies.

pub mod canvas;
pub mod controls;
pub mod styles;
pub mod window;

pub use canvas::{DrawCmd, DrawList};
pub use controls::{draw_faceplate, draw_knob, draw_vu_meter};
pub use styles::{Color, FaceplateStyle, KnobStyle, MeterStyle};
pub use window::{DriHostWindow, DriView};
