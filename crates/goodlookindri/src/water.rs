//! Five-primitive canvas: gradient ellipse, stroke ellipse, line, gradient panel, stroke panel.
//! `brit_knob` and `panel` target hosts that already draw with only those calls.
//! Every other style returns [`WaterResult::UnsupportedStyle`]. Paint those with [`crate::scene::Scene`].

use crate::color::Color;
use crate::design::{find_style, Item, KnobStyle};
use crate::scene::{Point, Rect, WaterCmd};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaterResult { Ok, InvalidBounds, InvalidValue, InvalidColor, UnsupportedStyle }

fn circle(c: Point, r: f32) -> Rect { Rect::new(c.x - r, c.y - r, r * 2.0, r * 2.0) }
fn radial(c: Point, r: f32, a: f64) -> Point {
    Point::new(c.x + (a.sin() as f32) * r, c.y - (a.cos() as f32) * r)
}
fn valid(b: Rect) -> bool {
    b.x.is_finite() && b.y.is_finite() && b.w.is_finite() && b.h.is_finite()
        && b.x.abs() <= 8192.0 && b.y.abs() <= 8192.0 && b.w >= 20.0 && b.h >= 20.0 && b.w <= 4096.0 && b.h <= 4096.0
}
fn mix(a: Color, b: Color, t: f64) -> Color { a.mix(b, t as f32) }

pub fn brit_knob(out: &mut Vec<WaterCmd>, bounds: Rect, proportion: f64, item: &Item, enabled: bool) -> WaterResult {
    if !valid(bounds) { return WaterResult::InvalidBounds; }
    if !proportion.is_finite() { return WaterResult::InvalidValue; }
    let Some(st) = find_style(&item.style) else { return WaterResult::UnsupportedStyle; };
    if st.style != KnobStyle::Brit { return WaterResult::UnsupportedStyle; }
    let Some(mut base) = Color::from_hex(&item.colour) else { return WaterResult::InvalidColor; };
    if !enabled { base = Color(0xff697170); }
    let pi = std::f64::consts::PI;
    let d = bounds.w.min(bounds.h);
    let r = d / 2.0;
    let a = (proportion.clamp(0.0, 1.0) * 1.6 - 0.8) * pi;
    let c = Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0);
    let marks = if enabled { Color(0xffc5cabf) } else { Color(0xff707a7b) };
    for n in 0..13 {
        let t = (-0.8 + 1.6 * n as f64 / 12.0) * pi;
        let w = if n % 3 == 0 { 1.3 } else { 0.8 };
        let outer = if n % 3 == 0 { 1.0 } else { 0.95 };
        out.push(WaterCmd::Line { a: radial(c, r * 0.88, t), b: radial(c, r * outer, t), color: marks, width: w });
    }
    let skirt = d * 0.38;
    let grip = skirt * 0.9;
    let collar = grip * 0.76;
    let cap = collar * 0.88;
    let face = cap * 0.82;
    for n in (1..=6).rev() {
        out.push(WaterCmd::GradientEllipse { rect: circle(Point::new(c.x + 1.0, c.y + 2.0), skirt + n as f32 * 0.8), top: Color(0x10000000), bottom: Color(0x10000000) });
    }
    out.push(WaterCmd::GradientEllipse { rect: circle(c, skirt), top: Color(0xff555b5f), bottom: Color(0xff0c0e10) });
    out.push(WaterCmd::StrokeEllipse { rect: circle(c, skirt), color: Color(0x75ffffff), width: 1.2 });
    out.push(WaterCmd::GradientEllipse { rect: circle(c, grip), top: Color(0xff2d3134), bottom: Color(0xff090b0d) });
    for n in 0..32 {
        let t = a + n as f64 * 2.0 * pi / 32.0;
        let light = (0.5 - 0.38 * (t.sin() - t.cos())).clamp(0.0, 1.0);
        out.push(WaterCmd::Line { a: radial(c, grip * 0.72, t), b: radial(c, grip * 0.98, t), color: mix(Color(0xff08090b), Color(0xff7a8286), light), width: 1.8 });
    }
    out.push(WaterCmd::GradientEllipse { rect: circle(c, collar), top: Color(0xff181b1d), bottom: Color(0xff3a4044) });
    out.push(WaterCmd::StrokeEllipse { rect: circle(c, collar * 0.92), color: Color(0x22ffffff), width: 0.5 });
    out.push(WaterCmd::GradientEllipse { rect: circle(c, cap), top: mix(base, Color(0xffffffff), 0.4), bottom: mix(base, Color(0xff000000), 0.6) });
    out.push(WaterCmd::StrokeEllipse { rect: circle(c, cap), color: Color(0x70ffffff), width: 1.0 });
    out.push(WaterCmd::GradientEllipse { rect: circle(c, face), top: mix(base, Color(0xffffffff), 0.18), bottom: mix(base, Color(0xff000000), 0.35) });
    out.push(WaterCmd::StrokeEllipse { rect: circle(c, face), color: Color(0x40ffffff), width: 0.7 });
    let shadow_c = Point::new(c.x + 0.4, c.y + 0.6);
    out.push(WaterCmd::Line { a: radial(shadow_c, face * 0.15, a), b: radial(shadow_c, face * 0.94, a), color: Color(0x66000000), width: 3.0 });
    out.push(WaterCmd::Line { a: radial(c, face * 0.15, a), b: radial(c, face * 0.94, a), color: Color(0xfff8f6ed), width: 2.4 });
    WaterResult::Ok
}

pub fn panel(out: &mut Vec<WaterCmd>, bounds: Rect, plate: &crate::design::Faceplate) -> WaterResult {
    if !valid(bounds) { return WaterResult::InvalidBounds; }
    out.push(WaterCmd::GradientPanel { rect: bounds, radius: 3.0, top: Color(plate.panel_top), bottom: Color(plate.panel_bottom) });
    out.push(WaterCmd::StrokePanel { rect: bounds, radius: 3.0, color: Color(plate.border), width: 1.0 });
    WaterResult::Ok
}
