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
    /// A filled axis-aligned rectangle.
    FillRect {
        /// Left edge, in points.
        x: f32,
        /// Top edge, in points.
        y: f32,
        /// Width, in points.
        w: f32,
        /// Height, in points.
        h: f32,
        /// Fill colour.
        color: Color,
    },
    /// The outline of an axis-aligned rectangle.
    StrokeRect {
        /// Left edge, in points.
        x: f32,
        /// Top edge, in points.
        y: f32,
        /// Width, in points.
        w: f32,
        /// Height, in points.
        h: f32,
        /// Line colour.
        color: Color,
        /// Line width, in points.
        stroke_width: f32,
    },
    /// A filled rectangle with rounded corners.
    FillRoundedRect {
        /// Left edge, in points.
        x: f32,
        /// Top edge, in points.
        y: f32,
        /// Width, in points.
        w: f32,
        /// Height, in points.
        h: f32,
        /// Corner radius, in points.
        radius: f32,
        /// Fill colour.
        color: Color,
    },
    /// The outline of a rectangle with rounded corners.
    StrokeRoundedRect {
        /// Left edge, in points.
        x: f32,
        /// Top edge, in points.
        y: f32,
        /// Width, in points.
        w: f32,
        /// Height, in points.
        h: f32,
        /// Corner radius, in points.
        radius: f32,
        /// Line colour.
        color: Color,
        /// Line width, in points.
        stroke_width: f32,
    },
    /// A filled circle. An ellipse of the scene is flattened to the circle that fits
    /// inside it.
    FillCircle {
        /// Centre x, in points.
        cx: f32,
        /// Centre y, in points.
        cy: f32,
        /// Radius, in points.
        radius: f32,
        /// Fill colour.
        color: Color,
    },
    /// The outline of a circle.
    StrokeCircle {
        /// Centre x, in points.
        cx: f32,
        /// Centre y, in points.
        cy: f32,
        /// Radius, in points.
        radius: f32,
        /// Line colour.
        color: Color,
        /// Line width, in points.
        stroke_width: f32,
    },
    /// A straight line. Polygons are flattened to their edges as lines.
    Line {
        /// Start x, in points.
        x1: f32,
        /// Start y, in points.
        y1: f32,
        /// End x, in points.
        x2: f32,
        /// End y, in points.
        y2: f32,
        /// Line colour.
        color: Color,
        /// Line width, in points.
        width: f32,
    },
    /// A circular arc from `start_angle` to `end_angle`.
    Arc {
        /// Centre x, in points.
        cx: f32,
        /// Centre y, in points.
        cy: f32,
        /// Radius, in points.
        radius: f32,
        /// Start angle in radians: 0 is 3 o'clock, positive runs clockwise on screen.
        start_angle: f32,
        /// End angle in radians, measured as `start_angle`.
        end_angle: f32,
        /// Line colour.
        color: Color,
        /// Line width, in points.
        width: f32,
    },
    /// A line of text.
    Text {
        /// The text.
        text: String,
        /// Anchor x, in points: the left edge, or the centre when `align_center` is set.
        x: f32,
        /// Anchor y, in points.
        y: f32,
        /// Font size, in points.
        size: f32,
        /// Text colour.
        color: Color,
        /// Centre the text on `x` instead of starting it there.
        align_center: bool,
    },
}

/// What one frame draws: a GoodLookin DRI [`Scene`] ([`Self::scene`]) and its flattened
/// copy ([`Self::commands`]).
///
/// The `draw_*` functions of [`crate::controls`] append to both. The shape methods on
/// `DrawList` itself ([`Self::fill_rect`], [`Self::text`], ...) append to
/// [`Self::commands`] only, so [`crate::SceneCanvas`], which rasterises the scene, does not
/// show them; to draw your own shapes on a canvas, build a [`Scene`] and [`Self::append`] it.
///
/// Everything here runs on the editor's (main) thread and allocates as the lists grow.
///
/// ```
/// use openplugin_dri::{draw_faceplate, draw_knob, Color, DrawList, KnobTier};
///
/// let mut dl = DrawList::new();
/// dl.set_knob_tier(KnobTier::Classic);
/// draw_faceplate(&mut dl, 240.0, 160.0, "Navy");
/// draw_knob(&mut dl, 60.0, 70.0, 28.0, 0.5, "GAIN", "0.0 dB", "Brit", Color::RED);
/// assert!(!dl.scene().cmds.is_empty());
/// assert!(!dl.commands().is_empty());
///
/// dl.clear();
/// assert!(dl.commands().is_empty());
/// assert_eq!(dl.knob_tier(), KnobTier::Classic); // the tier survives a clear
/// ```
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
    /// An empty list with the lit knob tier and no floor colour. Reserves room for 512
    /// flattened commands.
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

    /// Draw the knobs that follow in `tier`. Knobs already in the list keep theirs.
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

    /// Append a [`DrawCmd::FillRect`] to [`Self::commands`] (not to [`Self::scene`]).
    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        self.commands.push(DrawCmd::FillRect { x, y, w, h, color });
    }

    /// Append a [`DrawCmd::StrokeRect`] to [`Self::commands`] (not to [`Self::scene`]).
    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color, stroke_width: f32) {
        self.commands.push(DrawCmd::StrokeRect { x, y, w, h, color, stroke_width });
    }

    /// Append a [`DrawCmd::FillRoundedRect`] to [`Self::commands`] (not to [`Self::scene`]).
    pub fn fill_rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Color) {
        self.commands.push(DrawCmd::FillRoundedRect { x, y, w, h, radius, color });
    }

    /// Append a [`DrawCmd::StrokeRoundedRect`] to [`Self::commands`] (not to [`Self::scene`]).
    #[allow(clippy::too_many_arguments)] // public drawing API; grouping would break callers
    pub fn stroke_rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Color, stroke_width: f32) {
        self.commands.push(DrawCmd::StrokeRoundedRect { x, y, w, h, radius, color, stroke_width });
    }

    /// Append a [`DrawCmd::FillCircle`] to [`Self::commands`] (not to [`Self::scene`]).
    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Color) {
        self.commands.push(DrawCmd::FillCircle { cx, cy, radius, color });
    }

    /// Append a [`DrawCmd::StrokeCircle`] to [`Self::commands`] (not to [`Self::scene`]).
    pub fn stroke_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Color, stroke_width: f32) {
        self.commands.push(DrawCmd::StrokeCircle { cx, cy, radius, color, stroke_width });
    }

    /// Append a [`DrawCmd::Line`] to [`Self::commands`] (not to [`Self::scene`]).
    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, color: Color, width: f32) {
        self.commands.push(DrawCmd::Line { x1, y1, x2, y2, color, width });
    }

    /// Append a [`DrawCmd::Arc`] to [`Self::commands`] (not to [`Self::scene`]). Angles
    /// are in radians, 0 at 3 o'clock, clockwise positive.
    #[allow(clippy::too_many_arguments)] // public drawing API; grouping would break callers
    pub fn arc(&mut self, cx: f32, cy: f32, radius: f32, start_angle: f32, end_angle: f32, color: Color, width: f32) {
        self.commands.push(DrawCmd::Arc { cx, cy, radius, start_angle, end_angle, color, width });
    }

    /// Append a [`DrawCmd::Text`] to [`Self::commands`] (not to [`Self::scene`]).
    pub fn text(&mut self, text: impl Into<String>, x: f32, y: f32, size: f32, color: Color, align_center: bool) {
        self.commands.push(DrawCmd::Text { text: text.into(), x, y, size, color, align_center });
    }
}
