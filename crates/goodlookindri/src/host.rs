//! How a plugin uses this crate. The plugin owns the window and the parameters.
//!
//! - Audio thread: [`crate::level::LevelTracker::push`] and [`crate::spectrum::SampleRing::push`] only.
//! - GUI thread: read levels, step [`crate::design::Motion`], run the analyser, paint into a [`crate::scene::Scene`].
//! - Parameters stay in the host. `Item.parameter` is the host's id string. This crate never calls `set_parameter`.
//!
//! ```ignore
//! fn paint(scene: &mut Scene, item: &Item, norm: f64, plate: &Faceplate) {
//!     let dark = draw::use_dark_marks(item, plate.panel_top);
//!     draw::panel(scene, bounds, plate);
//!     draw::knob(scene, knob_bounds, norm, item, true, dark);
//! }
//! ```
//!
//! Replay `scene.cmds` with Cairo, Direct2D, tiny-skia, or an immediate-mode UI.
//! [`crate::scene::WaterCmd`] is the five-call subset for a host that already has that canvas:
//! use [`crate::water::brit_knob`] for the one style that subset can express, and the full [`crate::scene::Cmd`] list for the rest.

use crate::design::Item;

/// Read-only view of one normalised parameter. Implement this on the plugin's parameter table.
pub trait ParamView {
    fn id(&self) -> &str;
    fn normalized(&self) -> f64;
    fn name(&self) -> &str;
}

/// Bind a design row to a host parameter by id. Returns the normalised value, or `fallback` when the id is unknown.
pub fn value_for<'a, P: ParamView>(item: &Item, params: &'a [P], fallback: f64) -> f64 {
    params.iter().find(|p| p.id() == item.parameter).map(|p| p.normalized()).unwrap_or(fallback)
}

/// Positions in a design file are parent-relative. Scale them into the editor when the window is not 1:1 with the design.
pub fn map_rect(item: &Item, origin_x: f32, origin_y: f32, scale: f32) -> crate::scene::Rect {
    crate::scene::Rect::new(origin_x + item.x * scale, origin_y + item.y * scale, item.width * scale, item.height * scale)
}
