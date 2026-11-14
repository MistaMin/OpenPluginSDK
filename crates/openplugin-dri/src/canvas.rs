//! 2D canvas. [`DrawList::scene`] is the GoodLookin DRI command list. `commands` is a
//! flattened copy for hosts that only draw rectangles, ellipses, lines, arcs, and text.

use goodlookindri::scene::Scene;

use crate::styles::Color;

#[derive(Debug, Clone)]
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

#[derive(Debug, Default, Clone)]
pub struct DrawList {
    pub commands: Vec<DrawCmd>,
    /// Full GoodLookin DRI scene. This is the drawing the knob and plate styles actually produce.
    pub scene: Scene,
}

impl DrawList {
    pub fn new() -> Self {
        Self { commands: Vec::with_capacity(512), scene: Scene::new() }
    }

    pub fn clear(&mut self) {
        self.commands.clear();
        self.scene.clear();
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

    pub fn arc(&mut self, cx: f32, cy: f32, radius: f32, start_angle: f32, end_angle: f32, color: Color, width: f32) {
        self.commands.push(DrawCmd::Arc { cx, cy, radius, start_angle, end_angle, color, width });
    }

    pub fn text(&mut self, text: impl Into<String>, x: f32, y: f32, size: f32, color: Color, align_center: bool) {
        self.commands.push(DrawCmd::Text { text: text.into(), x, y, size, color, align_center });
    }
}
