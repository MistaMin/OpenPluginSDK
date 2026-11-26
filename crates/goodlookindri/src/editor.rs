//! Development inspector state. No widgets: the host paints and forwards pointer events.
//! Release builds can omit this module. It does not own DSP attachments.

use crate::color::Color;
use crate::design::{self, Item};
use crate::scene::{Point, Rect, Scene};

#[derive(Clone, Debug)]
pub struct Editor {
    items: Vec<Item>,
    undo: Vec<Vec<Item>>,
    selected: Option<usize>,
    wheel_h: f32,
    wheel_s: f32,
    wheel_v: f32,
    drag: i32,
}

impl Editor {
    pub fn new(items: Vec<Item>) -> Self {
        Self { items, undo: Vec::new(), selected: None, wheel_h: 0.0, wheel_s: 1.0, wheel_v: 1.0, drag: 0 }
    }
    pub fn items(&self) -> &[Item] { &self.items }
    pub fn selected(&self) -> Option<&Item> { self.selected.and_then(|i| self.items.get(i)) }
    pub fn select(&mut self, index: Option<usize>) { self.selected = index.filter(|i| *i < self.items.len()); self.sync_wheel(); }
    pub fn select_id(&mut self, id: &str) { self.selected = self.items.iter().position(|i| i.id == id); self.sync_wheel(); }

    fn push_undo(&mut self) { self.undo.push(self.items.clone()); if self.undo.len() > 64 { self.undo.remove(0); } }
    pub fn undo(&mut self) -> bool {
        if let Some(prev) = self.undo.pop() { self.items = prev; true } else { false }
    }
    fn edit<F: FnOnce(&mut Item)>(&mut self, f: F) -> bool {
        let Some(i) = self.selected else { return false; };
        self.push_undo();
        f(&mut self.items[i]);
        design::validate(&self.items[i]).is_ok() || {
            // roll back invalid
            if let Some(prev) = self.undo.pop() { self.items = prev; }
            false
        }
    }
    pub fn set_style(&mut self, style: &str) -> bool { self.edit(|i| i.style = style.into()) }
    pub fn set_label(&mut self, label: &str) -> bool { self.edit(|i| i.label = label.into()) }
    pub fn set_colour_hex(&mut self, hex: &str) -> bool { self.edit(|i| i.colour = hex.into()) }
    pub fn set_font(&mut self, size: f32) -> bool { self.edit(|i| i.font_size = size) }
    pub fn set_bounds(&mut self, x: f32, y: f32, w: f32, h: f32) -> bool {
        self.edit(|i| { i.x = x; i.y = y; i.width = w; i.height = h; })
    }
    pub fn set_marks(&mut self, marks: &str) -> bool { self.edit(|i| i.marks = marks.into()) }
    pub fn to_csv(&self) -> Result<String, design::DesignError> { design::write_design(&self.items) }
    pub fn load_csv(&mut self, text: &str) -> bool {
        match design::read_design(text) {
            Ok(items) => { self.push_undo(); self.items = items; self.selected = None; true }
            Err(_) => false,
        }
    }

    fn sync_wheel(&mut self) {
        if let Some(item) = self.selected() {
            if let Some(c) = Color::from_hex(&item.colour) {
                let (h, s, v) = c.to_hsv();
                if s > 0.002 && v > 0.002 { self.wheel_h = h; }
                self.wheel_s = s; self.wheel_v = v;
            }
        }
    }
    pub fn colour(&self) -> Color { Color::from_hsv(self.wheel_h, self.wheel_s, self.wheel_v) }

    /// Hit-test the colour wheel drawn by [`paint_wheel`] inside `bounds`.
    pub fn wheel_mouse(&mut self, bounds: Rect, p: Point, down: bool) -> bool {
        let c = bounds.center();
        let outer = bounds.w.min(bounds.h) * 0.5 - 2.0;
        let ring = outer * 0.20;
        let side = (outer - ring) * 1.30;
        let sq = Rect::new(c.x - side * 0.5, c.y - side * 0.5, side, side);
        let dx = p.x - c.x; let dy = p.y - c.y;
        let d = (dx * dx + dy * dy).sqrt();
        if down {
            self.drag = if d >= outer - ring - 2.0 && d <= outer + 2.0 { 1 } else if sq.expanded(2.0).contains(p) { 2 } else { 0 };
        }
        if self.drag == 1 {
            let mut a = dx.atan2(-dy) / std::f32::consts::TAU;
            if a < 0.0 { a += 1.0; }
            self.wheel_h = a;
        } else if self.drag == 2 {
            self.wheel_s = ((p.x - sq.x) / sq.w).clamp(0.0, 1.0);
            self.wheel_v = 1.0 - ((p.y - sq.y) / sq.h).clamp(0.0, 1.0);
        } else { return false; }
        let hex = self.colour().to_hex();
        if let Some(i) = self.selected {
            // live colour: one undo step per gesture is the host's job; here we write through
            let mut trial = self.items[i].clone();
            trial.colour = hex.clone();
            if design::validate(&trial).is_ok() {
                self.items[i].colour = hex;
            }
        }
        true
    }
    pub fn wheel_mouse_up(&mut self) { self.drag = 0; }

    pub fn paint_wheel(&self, scene: &mut Scene, bounds: Rect) {
        let c = bounds.center();
        let outer = bounds.w.min(bounds.h) * 0.5 - 2.0;
        let ring_w = outer * 0.20;
        let mid = outer - ring_w * 0.5;
        for i in 0..180 {
            let a0 = i as f32 / 180.0 * std::f32::consts::TAU;
            scene.stroke_arc(c, mid, a0, a0 + 0.045, Color::from_hsv(i as f32 / 180.0, 1.0, 1.0), ring_w);
        }
        let side = (outer - ring_w) * 1.30;
        let sq = Rect::new(c.x - side * 0.5, c.y - side * 0.5, side, side);
        scene.fill_rect(sq, Color(0xffffffff), Color::from_hsv(self.wheel_h, 1.0, 1.0), false);
        scene.fill_rect(sq, Color(0x00000000), Color(0xff000000), true);
        scene.stroke_round(sq, 0.0, Color(0xff000000), 1.0);
    }
}
