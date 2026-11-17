//! 2D canvas. [`DrawList::scene`] is the GoodLookin DRI command list: every command,
//! including the gradients, shadows and lit-knob images of GoodLookin DRI 0.2. Replay it
//! with [`crate::SceneCanvas`]. [`DrawList::commands`] is a flattened copy for hosts that only draw
//! rectangles, ellipses, lines, arcs, and text: GoodLookin DRI's `Scene::lower_basic`,
//! with each lit knob drawn again in the classic tier, since a basic host has no images.

use goodlookindri::color::Color as GlColor;
use goodlookindri::draw::KnobMode;
use goodlookindri::scene::{Cmd, Scene};

use crate::styles::Color;

/// One shape of the flattened list ([`DrawList::commands`]).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum DrawCmd {
    FillRect { x: f32, y: f32, w: f32, h: f32, color: Color },
    StrokeRect { x: f32, y: f32, w: f32, h: f32, color: Color, stroke_width: f32 },
    FillRoundedRect { x: f32, y: f32, w: f32, h: f32, radius: f32, color: Color },
    StrokeRoundedRect { x: f32, y: f32, w: f32, h: f32, radius: f32, color: Color, stroke_width: f32 },
    FillCircle { cx: f32, cy: f32, radius: f32, color: Color },
    StrokeCircle { cx: f32, cy: f32, radius: f32, color: Color, stroke_width: f32 },
    Line { x1: f32, y1: f32, x2: f32, y2: f32, color: Color, width: f32 },
    Arc { cx: f32, cy: f32, radius: f32, start_angle: f32, end_angle: f32, color: Color, width: f32 },
    Text { text: String, x: f32, y: f32, size: f32, color: Color, align_center: bool },
}

#[derive(Debug, Clone)]
pub struct DrawList {
    pub(crate) commands: Vec<DrawCmd>,
    pub(crate) scene: Scene,
    /// The tier knobs are drawn in: [`KnobMode::Lit`] (the default, per-pixel shaded),
    /// [`KnobMode::Classic`] (the vector drawing of GoodLookin DRI 0.1) or [`KnobMode::Flat`].
    /// A `lit_`, `flat_` style prefix still overrides it for one knob.
    pub(crate) knob_tier: KnobMode,
    /// The colour the knobs stand on, for the lit tier's reflections. A faceplate sets it.
    pub(crate) floor: Option<u32>,
}

impl Default for DrawList {
    fn default() -> Self {
        Self::new()
    }
}

fn sdk_color(color: GlColor) -> Color {
    Color::rgba(
        color.r() as f32 / 255.0,
        color.g() as f32 / 255.0,
        color.b() as f32 / 255.0,
        color.a() as f32 / 255.0,
    )
}

impl DrawList {
    pub fn new() -> Self {
        Self { commands: Vec::with_capacity(512), scene: Scene::new(), knob_tier: KnobMode::Lit, floor: None }
    }

    /// The flattened drawing, for a host that only replays simple shapes.
    pub fn commands(&self) -> &[DrawCmd] {
        &self.commands
    }

    /// The full GoodLookin DRI scene: the drawing the knob and plate styles actually produce.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// The tier knobs are drawn in ([`crate::KnobTier`]; lit by default).
    pub fn knob_tier(&self) -> KnobMode {
        self.knob_tier
    }

    pub fn set_knob_tier(&mut self, tier: KnobMode) {
        self.knob_tier = tier;
    }

    /// Empty the lists. The tier and the floor colour stay.
    pub fn clear(&mut self) {
        self.commands.clear();
        self.scene.clear();
    }

    /// Append a GoodLookin DRI scene: all of it to [`Self::scene`], and its basic
    /// lowering to [`Self::commands`].
    pub fn append(&mut self, scene: Scene) {
        self.flatten(&scene.lower_basic());
        self.scene.cmds.extend(scene.cmds);
    }

    /// [`Self::append`], with `basic` standing in for `scene` in [`Self::commands`]
    /// (a lit knob, whose image a basic host cannot draw, as its classic drawing).
    #[doc(hidden)]
    pub fn append_with_basic(&mut self, scene: Scene, basic: &Scene) {
        let has_image = scene.cmds.iter().any(|c| matches!(c, Cmd::Image { .. }));
        if has_image {
            self.flatten(&basic.lower_basic());
        } else {
            self.flatten(&scene.lower_basic());
        }
        self.scene.cmds.extend(scene.cmds);
    }

    fn flatten(&mut self, scene: &Scene) {
        for cmd in &scene.cmds {
            match cmd {
                Cmd::FillEllipse { rect, top, .. } => {
                    let center = rect.center();
                    self.fill_circle(center.x, center.y, rect.w.min(rect.h) * 0.5, sdk_color(*top));
                }
                Cmd::StrokeEllipse { rect, color, width } => {
                    let center = rect.center();
                    self.stroke_circle(center.x, center.y, rect.w.min(rect.h) * 0.5, sdk_color(*color), *width);
                }
                Cmd::FillRoundRect { rect, radius, top, .. } => {
                    self.fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, *radius, sdk_color(*top));
                }
                Cmd::FillRect { rect, top, .. } => self.fill_rect(rect.x, rect.y, rect.w, rect.h, sdk_color(*top)),
                Cmd::StrokeRoundRect { rect, radius, color, width } => {
                    self.stroke_rounded_rect(rect.x, rect.y, rect.w, rect.h, *radius, sdk_color(*color), *width);
                }
                Cmd::Line { a, b, color, width } => self.line(a.x, a.y, b.x, b.y, sdk_color(*color), *width),
                Cmd::StrokeArc { center, radius, from, to, color, width } => {
                    self.arc(center.x, center.y, *radius, *from, *to, sdk_color(*color), *width);
                }
                Cmd::Text { at, text, color, size, center } => {
                    self.text(text.clone(), at.x, at.y, *size, sdk_color(*color), *center);
                }
                Cmd::StrokePoly { pts, color, width, closed } => {
                    for pair in pts.windows(2) {
                        self.line(pair[0].x, pair[0].y, pair[1].x, pair[1].y, sdk_color(*color), *width);
                    }
                    if *closed && pts.len() > 2 {
                        let (a, b) = (pts[pts.len() - 1], pts[0]);
                        self.line(a.x, a.y, b.x, b.y, sdk_color(*color), *width);
                    }
                }
                Cmd::FillPoly { pts, top, .. } if pts.len() >= 3 => {
                    // No polygon fill in the basic set: its outline.
                    for i in 0..pts.len() {
                        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                        self.line(a.x, a.y, b.x, b.y, sdk_color(*top), 1.0);
                    }
                }
                _ => {}
            }
        }
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        self.commands.push(DrawCmd::FillRect { x, y, w, h, color });
    }

    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color, stroke_width: f32) {
        self.commands.push(DrawCmd::StrokeRect { x, y, w, h, color, stroke_width });
    }

    pub fn fill_rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Color) {
        self.commands.push(DrawCmd::FillRoundedRect { x, y, w, h, radius, color });
    }

    #[allow(clippy::too_many_arguments)] // public drawing API; grouping would break callers
    pub fn stroke_rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Color, stroke_width: f32) {
        self.commands.push(DrawCmd::StrokeRoundedRect { x, y, w, h, radius, color, stroke_width });
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Color) {
        self.commands.push(DrawCmd::FillCircle { cx, cy, radius, color });
    }

    pub fn stroke_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Color, stroke_width: f32) {
        self.commands.push(DrawCmd::StrokeCircle { cx, cy, radius, color, stroke_width });
    }

    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, color: Color, width: f32) {
        self.commands.push(DrawCmd::Line { x1, y1, x2, y2, color, width });
    }

    #[allow(clippy::too_many_arguments)] // public drawing API; grouping would break callers
    pub fn arc(&mut self, cx: f32, cy: f32, radius: f32, start_angle: f32, end_angle: f32, color: Color, width: f32) {
        self.commands.push(DrawCmd::Arc { cx, cy, radius, start_angle, end_angle, color, width });
    }

    pub fn text(&mut self, text: impl Into<String>, x: f32, y: f32, size: f32, color: Color, align_center: bool) {
        self.commands.push(DrawCmd::Text { text: text.into(), x, y, size, color, align_center });
    }
}
