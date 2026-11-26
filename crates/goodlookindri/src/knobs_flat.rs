// SPDX-License-Identifier: MIT
use crate::color::Color;
use crate::scene::{Point, Rect, Scene};
use std::f32::consts::PI;

pub struct KnobCtx {
    pub c: Point,
    pub r: f32,
    pub a: f32,
    pub accent: Color,
    pub dark_marks: bool,
}

impl KnobCtx {
    pub fn new(c: Point, r: f32, a: f32, accent: Color, dark_marks: bool) -> Self {
        Self { c, r, a, accent, dark_marks }
    }
}

fn dir(a: f32) -> Point { Point::new(a.sin(), -a.cos()) }
fn at(k: &KnobCtx, angle: f32, radius: f32) -> Point {
    let v = dir(angle);
    Point::new(k.c.x + v.x * radius, k.c.y + v.y * radius)
}
fn add(p: Point, o: Point) -> Point { Point::new(p.x + o.x, p.y + o.y) }
fn mul(p: Point, s: f32) -> Point { Point::new(p.x * s, p.y * s) }
fn ell(c: Point, radius: f32) -> Rect { Scene::ellipse_at(c, radius) }
fn rot(pivot: Point, p: Point, ang: f32) -> Point {
    let (s, co) = ang.sin_cos();
    let dx = p.x - pivot.x;
    let dy = p.y - pivot.y;
    Point::new(pivot.x + dx * co - dy * s, pivot.y + dx * s + dy * co)
}
fn shift(pts: &[Point], dx: f32, dy: f32) -> Vec<Point> {
    pts.iter().map(|p| Point::new(p.x + dx, p.y + dy)).collect()
}
fn white_a(a: f32) -> Color { Color::rgba(255, 255, 255, (a.clamp(0.0, 1.0) * 255.0).round() as u32) }
fn prop(a: f32) -> f32 { ((a / PI + 0.8) / 1.6).clamp(0.0, 1.0) }
fn arc_ang(a: f32) -> f32 { a - std::f32::consts::FRAC_PI_2 }

fn mark_colour(k: &KnobCtx, c: Color) -> Color {
    let b = c.perceived_brightness();
    if k.dark_marks && b > 0.45 { return Color(0xff252a2d).with_alpha(c.alpha_f()); }
    if !k.dark_marks && b < 0.30 { return Color(0xffe6eaec).with_alpha(c.alpha_f()); }
    c
}
fn disc(s: &mut Scene, k: &KnobCtx, radius: f32, top: Color, bottom: Color) {
    s.fill_ellipse(ell(k.c, radius), top, bottom);
}
fn rim(s: &mut Scene, k: &KnobCtx, radius: f32, col: Color, w: f32) {
    s.stroke_ellipse(ell(k.c, radius), col, w);
}
fn shadow(s: &mut Scene, k: &KnobCtx, radius: f32) {
    for n in (1..=5).rev() {
        let e = radius + n as f32 * 0.8;
        s.fill_ellipse_solid(ell(Point::new(k.c.x + 1.0, k.c.y + 3.0), e), Color(0x0b000000));
    }
}
fn dots(s: &mut Scene, k: &KnobCtx, radius_frac: f32, n: i32, size: f32, col: Color) {
    let col = mark_colour(k, col);
    for i in 0..n {
        let p = at(k, (-0.8 + 1.6 * i as f32 / (n - 1) as f32) * PI, k.r * radius_frac);
        s.fill_ellipse_solid(Rect::new(p.x - size * 0.5, p.y - size * 0.5, size, size), col);
    }
}
fn line(s: &mut Scene, k: &KnobCtx, from: f32, to: f32, w: f32, col: Color, angle: f32) {
    let p0 = at(k, angle, from);
    let p1 = at(k, angle, to);
    s.line(add(p0, Point::new(0.6, 0.8)), add(p1, Point::new(0.6, 0.8)), Color(0x66000000), w + 0.6);
    s.line(p0, p1, col, w);
}
fn flutes(s: &mut Scene, k: &KnobCtx, r0: f32, r1: f32, n: i32, dark: Color, light: Color, w: f32) {
    for i in 0..n {
        let t = k.a + i as f32 * std::f32::consts::TAU / n as f32;
        let v = dir(t);
        let l = (0.5 - 0.35 * (v.x + v.y)).clamp(0.0, 1.0);
        s.line(add(k.c, mul(v, r0)), add(k.c, mul(v, r1)), dark.mix(light, l), w);
    }
}
fn gloss(s: &mut Scene, k: &KnobCtx, radius: f32, strength: f32) {
    let w = radius * 1.2;
    let h = radius * 0.7;
    let cx = k.c.x - radius * 0.25;
    let cy = k.c.y - radius * 0.45;
    s.fill_ellipse(Rect::new(cx - w * 0.5, cy - h * 0.5, w, h), white_a(strength), white_a(0.0));
}
fn wedge_pts(k: &KnobCtx, base_half: f32, tip: f32, tip_half: f32, angle: f32) -> Vec<Point> {
    let v = dir(angle);
    let n = Point::new(-v.y, v.x);
    vec![
        add(k.c, mul(n, base_half)),
        add(add(k.c, mul(v, tip)), mul(n, tip_half)),
        add(add(k.c, mul(v, tip)), mul(n, -tip_half)),
        add(k.c, mul(n, -base_half)),
    ]
}
fn fill_wedge(s: &mut Scene, k: &KnobCtx, base_half: f32, tip: f32, tip_half: f32, angle: f32, top: Color, bottom: Color) {
    s.fill_poly(wedge_pts(k, base_half, tip, tip_half, angle), top, bottom);
}
fn stroke_wedge(s: &mut Scene, k: &KnobCtx, base_half: f32, tip: f32, tip_half: f32, angle: f32, col: Color, w: f32) {
    s.stroke_poly(wedge_pts(k, base_half, tip, tip_half, angle), col, w, true);
}
fn arc_stroke(s: &mut Scene, k: &KnobCtx, rad: f32, from: f32, to: f32, w: f32, col: Color) {
    if to <= from + 0.001 { return; }
    s.stroke_arc(k.c, rad, arc_ang(from), arc_ang(to), col, w);
}
fn poly_n(k: &KnobCtx, n: i32, radius: f32, a0: f32) -> Vec<Point> {
    (0..n).map(|i| at(k, a0 + i as f32 * std::f32::consts::TAU / n as f32, radius)).collect()
}
fn rounded_rect_pts(rect_c: Point, pivot: Point, w: f32, h: f32, rad: f32, ang: f32) -> Vec<Point> {
    let rad = rad.max(0.0).min(w * 0.5).min(h * 0.5);
    let hw = w * 0.5;
    let hh = h * 0.5;
    let steps = 5;
    let mut pts = Vec::new();
    let mut arc = |ox: f32, oy: f32, a0: f32, a1: f32| {
        for i in 0..=steps {
            let a = a0 + (a1 - a0) * (i as f32 / steps as f32);
            pts.push(Point::new(rect_c.x + ox + rad * a.cos(), rect_c.y + oy - rad * a.sin()));
        }
    };
    arc(hw - rad, -hh + rad, PI * 0.5, 0.0);
    arc(hw - rad, hh - rad, 0.0, -PI * 0.5);
    arc(-hw + rad, hh - rad, -PI * 0.5, -PI);
    arc(-hw + rad, -hh + rad, PI, PI * 0.5);
    pts.into_iter().map(|p| rot(pivot, p, ang)).collect()
}
fn fill_rr(s: &mut Scene, rect_c: Point, pivot: Point, w: f32, h: f32, rad: f32, ang: f32, top: Color, bottom: Color) {
    s.fill_poly(rounded_rect_pts(rect_c, pivot, w, h, rad, ang), top, bottom);
}
fn stroke_rr(s: &mut Scene, rect_c: Point, pivot: Point, w: f32, h: f32, rad: f32, ang: f32, col: Color, width: f32) {
    s.stroke_poly(rounded_rect_pts(rect_c, pivot, w, h, rad, ang), col, width, true);
}
fn grain(s: &mut Scene, c: Point, t: f32, ang: f32, col: Color, amp: f32, freq: f32, w: f32) {
    s.clip_round(ell(c, t), t);
    for i in -5..=5 {
        let mut pts = Vec::with_capacity(17);
        for step in 0..=16 {
            let x = c.x - t + t * 2.0 * step as f32 / 16.0;
            let y = c.y + i as f32 * t * 0.17 + (step as f32 * freq + i as f32).sin() * t * amp;
            pts.push(rot(c, Point::new(x, y), ang));
        }
        let width = if (w - 0.7).abs() < 0.001 { 0.7 + 0.2 * ((i & 1) as f32) } else { w };
        s.stroke_poly(pts, col, width, false);
    }
    s.pop_clip();
}
fn wood_knob(s: &mut Scene, k: &KnobCtx, side1: Color, side2: Color, top1: Color, top2: Color, grain_c: Color) {
    let r = k.r * 0.86;
    let t = r * 0.80;
    shadow(s, k, r);
    disc(s, k, r, side1, side2);
    rim(s, k, r, Color(0xff25150a), 0.8);
    disc(s, k, t, top1, top2);
    grain(s, k.c, t, k.a, grain_c, 0.045, 0.55, 0.7);
    rim(s, k, t, Color(0x55ffffff), 0.8);
    gloss(s, k, t, 0.20);
    line(s, k, t * 0.16, t * 0.92, k.r * 0.065, k.accent.brighter(0.3), k.a);
}
fn chamfer(c: Point, r: f32, ch: f32) -> Vec<Point> {
    vec![
        Point::new(c.x - r + ch, c.y - r), Point::new(c.x + r - ch, c.y - r),
        Point::new(c.x + r, c.y - r + ch), Point::new(c.x + r, c.y + r - ch),
        Point::new(c.x + r - ch, c.y + r), Point::new(c.x - r + ch, c.y + r),
        Point::new(c.x - r, c.y + r - ch), Point::new(c.x - r, c.y - r + ch),
    ]
}
fn contact(s: &mut Scene, k: &KnobCtx, r: f32, body: &[Point]) {
    for n in (1..=3).rev() {
        let dx = n as f32 * 0.6;
        let dy = n as f32 * 1.1;
        s.fill_poly(shift(body, dx, dy), Color(0x16000000), Color(0x16000000));
        s.fill_ellipse_solid(ell(Point::new(k.c.x + dx, k.c.y + dy), r), Color(0x16000000));
    }
}

pub fn draw_brit(s: &mut Scene, k: &KnobCtx) {
    let mark = if k.dark_marks { Color(0xff2a2f32) } else { Color(0xffc5cabf) };
    for n in 0..13 {
        let a = (-0.8 + 1.6 * n as f32 / 12.0) * PI;
        let v = dir(a);
        let outer = if n % 3 == 0 { 1.0 } else { 0.95 };
        let w = if n % 3 == 0 { 1.2 } else { 0.7 };
        s.line(add(k.c, mul(v, k.r * 0.88)), add(k.c, mul(v, k.r * outer)), mark, w);
    }
    let body_r = k.r * 0.74;
    for n in (1..=5).rev() {
        let e = body_r + n as f32 * 0.7;
        s.fill_ellipse_solid(ell(Point::new(k.c.x + 1.0, k.c.y + 3.0), e), Color(0x09000000));
    }
    disc(s, k, body_r, Color(0xff565c60), Color(0xff080a0c));
    let grip = body_r;
    for n in 0..28 {
        let t = k.a + n as f32 * std::f32::consts::TAU / 28.0;
        let v = dir(t);
        let light = (0.5 - 0.35 * (v.x + v.y)).clamp(0.0, 1.0);
        s.line(add(k.c, mul(v, grip * 0.73)), add(k.c, mul(v, grip * 0.92)), Color(0xff080a0b).mix(Color(0xff61686b), light), 2.2);
    }
    let cap_r = body_r - k.r * 0.11;
    disc(s, k, cap_r, k.accent.brighter(0.28), k.accent.darker(0.42));
    rim(s, k, cap_r, k.accent.darker(0.6), 0.8);
    let face = cap_r - k.r * 0.064;
    disc(s, k, face, k.accent.brighter(0.08), k.accent.darker(0.10));
    rim(s, k, face, Color(0x27ffffff), 0.7);
    let v = dir(k.a);
    let reach = face * 2.0;
    let tip = add(k.c, mul(v, reach * 0.43));
    let tail = add(k.c, mul(v, reach * 0.12));
    s.line(add(tail, Point::new(0.7, 0.7)), add(tip, Point::new(0.7, 0.7)), Color(0x77000000), 3.4);
    s.line(tail, tip, Color(0xfff5f2df), 2.5);
}

pub fn draw_n(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.94, 21, k.r * 0.05, Color(0xffe9eef2));
    let r = k.r * 0.80;
    let g = r * 0.78;
    shadow(s, k, r);
    disc(s, k, r, Color(0xfff6f8f8), Color(0xff656b6f));
    rim(s, k, r, Color(0xff2b3033), 0.8);
    disc(s, k, g, Color(0xff9a9ea1), Color(0xff45494c));
    flutes(s, k, g * 0.70, g * 0.97, 30, Color(0xff34383b), Color(0xffb4b8ba), 1.6);
    let c = r * 0.44;
    disc(s, k, c, k.accent.brighter(0.2), k.accent.darker(0.4));
    rim(s, k, c, k.accent.darker(0.7), 0.8);
    s.line(at(k, k.a, g * 1.01), at(k, k.a, r * 0.99), Color(0xff0a0b0c), k.r * 0.05);
}
pub fn draw_a(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.97, 21, k.r * 0.05, k.accent.brighter(0.55));
    let r = k.r * 0.74;
    let body = wedge_pts(k, r * 0.92, k.r * 0.97, k.r * 0.20, k.a);
    let lug_a = at(k, k.a + 1.72, r);
    let lug_b = at(k, k.a - 1.72, r);
    contact(s, k, r, &body);
    s.fill_poly(body, Color(0xfff1f3f3), Color(0xff8a9195));
    disc(s, k, r, Color(0xfff1f3f3), Color(0xff8a9195));
    for l in [lug_a, lug_b] { s.fill_ellipse(ell(l, r * 0.17), Color(0xfff1f3f3), Color(0xff8a9195)); }
    stroke_wedge(s, k, r * 0.92, k.r * 0.97, k.r * 0.20, k.a, Color(0xff2a2f32), 1.0);
    rim(s, k, r, Color(0xff2a2f32), 1.0);
    for l in [lug_a, lug_b] { s.stroke_ellipse(ell(l, r * 0.17), Color(0xff2a2f32), 0.8); }
    rim(s, k, r * 0.90, Color(0x55000000), 1.4);
    let c = r * 0.86;
    disc(s, k, c, k.accent.brighter(0.10), k.accent.darker(0.22));
    rim(s, k, c, k.accent.darker(0.6), 0.9);
    rim(s, k, c * 0.96, Color(0x1affffff), 0.7);
    s.line(at(k, k.a, r * 0.80), at(k, k.a, k.r * 0.93), Color(0x88000000), 1.2);
}
pub fn draw_mic(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.97, 21, k.r * 0.045, Color(0xffe9eef2));
    let r = k.r * 0.70;
    let body = wedge_pts(k, r * 0.90, k.r * 0.96, k.r * 0.24, k.a);
    contact(s, k, r, &body);
    s.fill_poly(body, k.accent.brighter(0.55), k.accent.darker(0.65));
    disc(s, k, r, k.accent.brighter(0.55), k.accent.darker(0.65));
    stroke_wedge(s, k, r * 0.90, k.r * 0.96, k.r * 0.24, k.a, k.accent.darker(0.8), 1.0);
    rim(s, k, r, k.accent.darker(0.8), 1.0);
    let c = r * 0.62;
    disc(s, k, c, k.accent.brighter(0.25), k.accent.darker(0.5));
    gloss(s, k, r, 0.22);
    line(s, k, r * 0.05, k.r * 0.93, k.r * 0.06, Color(0xfff8f4e6), k.a);
}
pub fn draw_fs(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.95, 11, k.r * 0.04, Color(0xffdfe3e2));
    let r = k.r * 0.84;
    shadow(s, k, r);
    disc(s, k, r, Color(0xffe9ebea), Color(0xff8d9492));
    rim(s, k, r, Color(0xff4a504f), 0.8);
    let c = r * 0.66;
    disc(s, k, c, k.accent.brighter(0.25), k.accent.darker(0.35));
    rim(s, k, c, k.accent.darker(0.7), 0.8);
    gloss(s, k, c, 0.16);
    line(s, k, r * 0.10, r * 0.98, k.r * 0.07, Color(0xff101213), k.a);
}
pub fn draw_mtl(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.95, 11, k.r * 0.035, Color(0xffc7ccce));
    let r = k.r * 0.86;
    let t = r * 0.76;
    shadow(s, k, r);
    disc(s, k, r, Color(0xffe4e7e9), Color(0xff6b7378));
    flutes(s, k, r * 0.84, r * 0.99, 72, Color(0xff4f575b), Color(0xffeff1f2), 0.9);
    rim(s, k, r, Color(0xff2f3538), 0.8);
    disc(s, k, t, Color(0xfffafbfb), Color(0xff9ea5a9));
    for i in 1..=6 { rim(s, k, t * i as f32 / 6.0, Color(0x14000000), 0.5); }
    rim(s, k, t, Color(0x66ffffff), 0.8);
    line(s, k, t * 0.08, t * 0.98, k.r * 0.07, k.accent.darker(0.2), k.a);
}
pub fn draw_wd(s: &mut Scene, k: &KnobCtx) {
    wood_knob(s, k, Color(0xffa97444), Color(0xff3e2412), Color(0xffc08855), Color(0xff6d4421), Color(0x40301808));
}
pub fn draw_mpl(s: &mut Scene, k: &KnobCtx) {
    wood_knob(s, k, Color(0xffe6c590), Color(0xff9a7442), Color(0xfff3dcb0), Color(0xffc29a64), Color(0x387a5428));
}
pub fn draw_bk(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.88;
    let t = k.r * 0.60;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff323336), Color(0xff09090a));
    flutes(s, k, r * 0.88, r * 0.99, 36, Color(0xff050506), Color(0xff4c4e52), 1.3);
    fill_wedge(s, k, t * 0.96, k.r * 0.97, k.r * 0.07, k.a, Color(0xff4a4b4f), Color(0xff060607));
    disc(s, k, t, Color(0xff4a4b4f), Color(0xff060607));
    rim(s, k, t, Color(0x44ffffff), 0.7);
    gloss(s, k, t, 0.26);
    line(s, k, t * 0.25, k.r * 0.90, k.r * 0.055, k.accent, k.a);
}
pub fn draw_pie(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.90;
    let inner = r * 0.88;
    shadow(s, k, r);
    let teeth = 24;
    let skirt: Vec<Point> = (0..teeth * 2).map(|i| {
        let t = k.a + i as f32 * PI / teeth as f32;
        at(k, t, if i % 2 == 1 { inner } else { r })
    }).collect();
    s.fill_poly(skirt.clone(), Color(0xfff6f6f4), Color(0xffaaaca9));
    s.stroke_poly(skirt, Color(0xff3a3c3b), 0.6, true);
    let c = r * 0.70;
    disc(s, k, c, Color(0xff2f3032), Color(0xff060607));
    rim(s, k, c, Color(0x55ffffff), 0.8);
    gloss(s, k, c, 0.12);
    line(s, k, c * 0.10, r * 0.98, k.r * 0.065, k.accent, k.a);
}
pub fn draw_snk(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.93, 11, k.r * 0.05, Color(0xffb4b8ba));
    let r = k.r * 0.78;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff34363a), Color(0xff0b0b0c));
    flutes(s, k, r * 0.90, r * 0.99, 48, Color(0xff0a0a0b), Color(0xff2f3134), 0.8);
    rim(s, k, r * 0.88, Color(0x22ffffff), 0.8);
    rim(s, k, r, Color(0xff000000), 0.8);
    gloss(s, k, r * 0.9, 0.08);
    line(s, k, r * 0.12, r * 0.84, k.r * 0.07, k.accent.brighter(0.4), k.a);
}
pub fn draw_slv(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.95;
    let t = k.r * 0.64;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff595d60), Color(0xff1b1e20));
    dots(s, k, 0.82, 21, k.r * 0.05, Color(0xffe6e9ea));
    shadow(s, k, t);
    disc(s, k, t, Color(0xfff4f6f6), Color(0xff8c9397));
    flutes(s, k, t * 0.86, t * 0.98, 60, Color(0xff6c7377), Color(0xffe9ecee), 0.8);
    rim(s, k, t, Color(0xff30363a), 0.8);
    disc(s, k, t * 0.78, Color(0xffffffff), Color(0xffaab1b5));
    line(s, k, t * 0.10, t * 0.76, k.r * 0.075, k.accent.darker(0.5), k.a);
}
pub fn draw_rd(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.95, 11, k.r * 0.045, k.accent.brighter(0.4));
    let r = k.r * 0.84;
    let c = r * 0.60;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff3d3f43), Color(0xff09090a));
    flutes(s, k, r * 0.74, r * 0.98, 36, Color(0xff060607), Color(0xff4b4d51), 1.4);
    rim(s, k, r, Color(0xff000000), 0.8);
    disc(s, k, c, k.accent.brighter(0.25), k.accent.darker(0.45));
    rim(s, k, c, k.accent.darker(0.75), 0.8);
    gloss(s, k, c, 0.2);
    line(s, k, r * 0.66, r * 0.96, k.r * 0.07, Color(0xfff1efe6), k.a);
}
pub fn draw_ptr(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.95, 11, k.r * 0.035, Color(0xffc7ccce));
    let r = k.r * 0.60;
    shadow(s, k, r);
    fill_wedge(s, k, r * 0.95, k.r * 0.90, k.r * 0.26, k.a, Color(0xfff4f6f6), Color(0xff7c8387));
    stroke_wedge(s, k, r * 0.95, k.r * 0.90, k.r * 0.26, k.a, Color(0xff2f3538), 0.7);
    disc(s, k, r, Color(0xfff4f6f6), Color(0xff7c8387));
    rim(s, k, r, Color(0xff2f3538), 0.7);
    gloss(s, k, r, 0.18);
    line(s, k, r * 0.05, k.r * 0.86, k.r * 0.07, k.accent.darker(0.6), k.a);
}
pub fn draw_dm(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.95, 11, k.r * 0.04, Color(0xff9aa3a6));
    let r = k.r * 0.80;
    shadow(s, k, r);
    disc(s, k, r, k.accent.brighter(0.7), k.accent.darker(0.75));
    rim(s, k, r, k.accent.darker(0.8), 1.0);
    gloss(s, k, r, 0.34);
    let p = at(k, k.a, r * 0.68);
    let d = k.r * 0.14;
    s.fill_ellipse_solid(Rect::new(p.x - d * 0.5 + 0.6, p.y - d * 0.5 + 0.8, d, d), Color(0x66000000));
    s.fill_ellipse_solid(Rect::new(p.x - d * 0.5, p.y - d * 0.5, d, d), Color(0xfffaf7ea));
}
pub fn draw_sq(s: &mut Scene, k: &KnobCtx) {
    let side = k.r * 0.78;
    shadow(s, k, side * 1.1);
    let outer = side * 2.0;
    fill_rr(s, k.c, k.c, outer, outer, side * 0.28, k.a, Color(0xff3b3d41), Color(0xff080809));
    stroke_rr(s, k.c, k.c, outer, outer, side * 0.28, k.a, Color(0xff000000), 1.0);
    let top = outer - side * 0.40;
    fill_rr(s, k.c, k.c, top, top, side * 0.2, k.a, Color(0xff0d0e10), Color(0xff2d2f33));
    stroke_rr(s, k.c, k.c, top, top, side * 0.2, k.a, Color(0x30ffffff), 0.8);
    let off = rot(Point::new(0.0, 0.0), Point::new(0.6, 0.8), k.a);
    let p0 = rot(k.c, Point::new(k.c.x, k.c.y - side * 0.1), k.a);
    let p1 = rot(k.c, Point::new(k.c.x, k.c.y - side * 0.84), k.a);
    s.line(add(p0, off), add(p1, off), Color(0x66000000), k.r * 0.09);
    s.line(p0, p1, k.accent, k.r * 0.09);
}
pub fn draw_seg(s: &mut Scene, k: &KnobCtx) {
    let n = 21;
    let p = prop(k.a);
    let w = k.r * 0.11;
    for i in 0..n {
        let f = (i as f32 + 0.5) / n as f32;
        let t = (-0.8 + 1.6 * f) * PI;
        let lit = f <= p + 0.5 / n as f32;
        let col = if f > 0.88 { Color(0xffe4412c) } else { k.accent };
        if lit { s.line(at(k, t, k.r * 0.70), at(k, t, k.r * 0.99), col.with_alpha(0.28), w * 1.7); }
        s.line(at(k, t, k.r * 0.74), at(k, t, k.r * 0.96), if lit { col } else { col.with_alpha(0.13) }, w);
    }
    let r = k.r * 0.60;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff2d2f33), Color(0xff08090a));
    rim(s, k, r, Color(0xff000000), 0.8);
    rim(s, k, r * 0.88, Color(0x1affffff), 0.8);
    gloss(s, k, r, 0.07);
    line(s, k, r * 0.14, r * 0.84, k.r * 0.07, k.accent.brighter(0.5), k.a);
}
pub fn draw_vfd(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.92;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff10221a), Color(0xff020504));
    rim(s, k, r, Color(0xff435248), 1.2);
    let p = prop(k.a);
    let n = 17;
    for i in 0..n {
        let f = i as f32 / (n - 1) as f32;
        let t = (-0.8 + 1.6 * f) * PI;
        let outer = k.r * (0.40 + 0.46 * f);
        let lit = f <= p + 0.5 / n as f32;
        if lit { s.line(at(k, t, k.r * 0.22), at(k, t, outer), k.accent.with_alpha(0.25), 3.0); }
        let col = if lit { k.accent.brighter(0.2) } else { k.accent.with_alpha(0.16) };
        s.line(at(k, t, k.r * 0.22), at(k, t, outer), col, 1.1);
    }
    rim(s, k, k.r * 0.16, k.accent.with_alpha(0.5), 1.0);
    gloss(s, k, r, 0.10);
    line(s, k, k.r * 0.10, k.r * 0.30, k.r * 0.07, k.accent.brighter(0.7), k.a);
}
pub fn draw_cyb(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.90;
    let frame = chamfer(k.c, r, r * 0.38);
    s.fill_poly(shift(&frame, 1.5, 3.0), Color(0x55000000), Color(0x55000000));
    s.fill_poly(frame.clone(), Color(0xff1b2024), Color(0xff050607));
    s.stroke_poly(frame.clone(), k.accent.with_alpha(0.22), 4.0, true);
    s.stroke_poly(frame, k.accent, 1.4, true);
    let ring = r * 0.62;
    rim(s, k, ring, k.accent.with_alpha(0.55), 1.0);
    for i in 0..12 {
        let t = k.a + i as f32 * std::f32::consts::TAU / 12.0;
        s.line(at(k, t, ring * 1.08), at(k, t, ring * 1.22), k.accent.with_alpha(0.7), 1.1);
    }
    line(s, k, ring * 0.18, ring * 0.86, k.r * 0.07, k.accent.brighter(0.4), k.a);
    let tip = at(k, k.a, ring * 0.86);
    s.fill_ellipse_solid(ell(tip, k.r * 0.06), k.accent.with_alpha(0.7));
}
pub fn draw_fat(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.90;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff3c3d40), Color(0xff050506));
    flutes(s, k, r * 0.80, r * 0.99, 40, Color(0xff050506), Color(0xff55575b), 1.4);
    disc(s, k, r * 0.74, Color(0xff2d2e30), Color(0xff09090a));
    rim(s, k, r * 0.74, Color(0x30ffffff), 0.8);
    line(s, k, r * 0.08, r * 0.97, k.r * 0.11, k.accent, k.a);
}
pub fn draw_pag(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.88;
    shadow(s, k, r);
    let oct = poly_n(k, 8, r, k.a + 0.5 * PI / 4.0);
    s.fill_poly(oct.clone(), Color(0xff414246), Color(0xff050506));
    s.stroke_poly(oct, Color(0x44ffffff), 0.8, true);
    flutes(s, k, r * 0.62, r * 0.84, 32, Color(0xff050506), Color(0xff4a4c50), 1.5);
    disc(s, k, r * 0.56, Color(0xff3a3b3e), Color(0xff060607));
    gloss(s, k, r * 0.56, 0.28);
    line(s, k, r * 0.30, r * 0.54, k.r * 0.07, k.accent, k.a);
}
pub fn draw_lvr(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.95, 11, k.r * 0.04, Color(0xffc7ccce));
    shadow(s, k, k.r * 0.5);
    let bar_c = Point::new(k.c.x, k.c.y - k.r * 0.28);
    fill_rr(s, bar_c, k.c, k.r * 0.34, k.r * 1.30, k.r * 0.08, k.a, Color(0xff4c4d51), Color(0xff050506));
    stroke_rr(s, bar_c, k.c, k.r * 0.34, k.r * 1.30, k.r * 0.08, k.a, Color(0x40ffffff), 0.7);
    let mark_c = Point::new(k.c.x, k.c.y - k.r * 0.74);
    let hw = k.r * 0.04;
    let hh = k.r * 0.16;
    let rect = [
        Point::new(mark_c.x - hw, mark_c.y - hh), Point::new(mark_c.x + hw, mark_c.y - hh),
        Point::new(mark_c.x + hw, mark_c.y + hh), Point::new(mark_c.x - hw, mark_c.y + hh),
    ];
    s.fill_poly(rect.into_iter().map(|p| rot(k.c, p, k.a)).collect(), k.accent, k.accent);
    disc(s, k, k.r * 0.30, Color(0xff3e3f43), Color(0xff060607));
    rim(s, k, k.r * 0.30, Color(0x44ffffff), 0.8);
}
pub fn draw_tab(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.95, 11, k.r * 0.04, Color(0xffc7ccce));
    let r = k.r * 0.50;
    shadow(s, k, r);
    fill_wedge(s, k, r * 0.5, k.r * 0.92, k.r * 0.10, k.a, Color(0xff4a4b4f), Color(0xff060607));
    disc(s, k, r, Color(0xff4a4b4f), Color(0xff060607));
    rim(s, k, r, Color(0x44ffffff), 0.8);
    line(s, k, r * 0.2, k.r * 0.88, k.r * 0.05, k.accent, k.a);
}
pub fn draw_wh(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.86;
    let c = r * 0.64;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff3a3b3e), Color(0xff060607));
    flutes(s, k, r * 0.74, r * 0.99, 44, Color(0xff060607), Color(0xff55575b), 1.3);
    disc(s, k, c, Color(0xfffbfbfa), Color(0xff9ea3a5));
    rim(s, k, c, Color(0xff2c3033), 0.8);
    gloss(s, k, c, 0.15);
    line(s, k, 0.0, c * 0.96, k.r * 0.07, k.accent.darker(0.6), k.a);
}
pub fn draw_knl(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.86;
    let c = r * 0.70;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff35363a), Color(0xff050506));
    flutes(s, k, r * 0.70, r * 0.99, 64, Color(0xff050506), Color(0xff5a5c60), 1.2);
    disc(s, k, c, Color(0xfffcfdfd), Color(0xff8f969a));
    for i in 1..=5 { rim(s, k, c * i as f32 / 5.0, Color(0x14000000), 0.5); }
    rim(s, k, c, Color(0xff2c3033), 0.8);
    line(s, k, c * 0.10, c * 0.96, k.r * 0.07, k.accent.darker(0.4), k.a);
}
pub fn draw_skt(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.95, 11, k.r * 0.04, Color(0xffc7ccce));
    let r = k.r * 0.86;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff3a3b3e), Color(0xff060607));
    flutes(s, k, r * 0.88, r * 0.99, 48, Color(0xff050506), Color(0xff4a4c50), 1.0);
    disc(s, k, r * 0.78, Color(0xff2a2b2d), Color(0xff0a0a0b));
    rim(s, k, r * 0.78, Color(0x2affffff), 0.8);
    line(s, k, r * 0.32, r * 0.99, k.r * 0.08, k.accent, k.a);
}
pub fn draw_rib(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.88;
    shadow(s, k, r);
    disc(s, k, r, k.accent.brighter(0.2), k.accent.darker(0.6));
    flutes(s, k, r * 0.62, r * 0.98, 34, k.accent.darker(0.7), k.accent.brighter(0.5), 1.8);
    disc(s, k, r * 0.58, k.accent.brighter(0.15), k.accent.darker(0.5));
    rim(s, k, r * 0.58, k.accent.darker(0.8), 0.8);
    gloss(s, k, r * 0.58, 0.18);
    line(s, k, r * 0.10, r * 0.54, k.r * 0.07, Color(0xfff7f4e8), k.a);
}
pub fn draw_chr(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.95, 21, k.r * 0.04, Color(0xffe9eef2));
    let r = k.r * 0.82;
    let c = r * 0.66;
    shadow(s, k, r);
    disc(s, k, r, Color(0xfffdfdfd), Color(0xff52585c));
    rim(s, k, r * 0.9, Color(0x55000000), 1.2);
    rim(s, k, r, Color(0xff25292b), 0.8);
    disc(s, k, c, Color(0xff4a4c50), Color(0xff030304));
    rim(s, k, c, Color(0xff000000), 0.8);
    gloss(s, k, c, 0.45);
    line(s, k, c * 0.10, r * 0.96, k.r * 0.065, Color(0xfff5f2e6), k.a);
}
pub fn draw_arc(s: &mut Scene, k: &KnobCtx) {
    let lo = -0.8 * PI;
    let hi = 0.8 * PI;
    let w = k.r * 0.17;
    let rad = k.r * 0.84;
    let split = lo + (hi - lo) * 0.86;
    arc_stroke(s, k, rad, lo, hi, w, k.accent.with_alpha(0.14));
    arc_stroke(s, k, rad, lo, k.a.min(split), w * 1.6, k.accent.with_alpha(0.22));
    arc_stroke(s, k, rad, lo, k.a.min(split), w, k.accent);
    arc_stroke(s, k, rad, split, k.a, w, Color(0xffe4412c));
    let r = k.r * 0.60;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff2a2c30), Color(0xff08090a));
    rim(s, k, r, Color(0xff000000), 0.8);
    gloss(s, k, r, 0.07);
    line(s, k, r * 0.14, r * 0.84, k.r * 0.07, k.accent.brighter(0.5), k.a);
}
pub fn draw_brg(s: &mut Scene, k: &KnobCtx) {
    let n = 14;
    let p = prop(k.a);
    for i in 0..n {
        let f = (i as f32 + 0.5) / n as f32;
        let t = (-0.8 + 1.6 * f) * PI;
        let len = k.r * (0.10 + 0.26 * f);
        let lit = f <= p + 0.5 / n as f32;
        let col = if f > 0.88 { Color(0xffe4412c) } else { k.accent };
        s.line(at(k, t, k.r * 0.64), at(k, t, k.r * 0.64 + len), if lit { col } else { col.with_alpha(0.14) }, k.r * 0.10);
    }
    let r = k.r * 0.54;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff2a2c30), Color(0xff08090a));
    rim(s, k, r, Color(0xff000000), 0.8);
    line(s, k, r * 0.14, r * 0.84, k.r * 0.07, k.accent.brighter(0.5), k.a);
}
pub fn draw_neo(s: &mut Scene, k: &KnobCtx) {
    let lo = -0.8 * PI;
    let rad = k.r * 0.80;
    disc(s, k, rad, Color(0xff0a0f14), Color(0xff020305));
    arc_stroke(s, k, rad, lo, 0.8 * PI, k.r * 0.04, k.accent.with_alpha(0.25));
    arc_stroke(s, k, rad, lo, k.a, k.r * 0.16, k.accent.with_alpha(0.20));
    arc_stroke(s, k, rad, lo, k.a, k.r * 0.05, k.accent);
    rim(s, k, rad * 0.62, k.accent.with_alpha(0.35), 0.8);
    let p = at(k, k.a, rad);
    s.fill_ellipse_solid(ell(p, k.r * 0.13), k.accent.with_alpha(0.35));
    s.fill_ellipse_solid(ell(p, k.r * 0.07), k.accent.brighter(0.7));
    line(s, k, rad * 0.20, rad * 0.58, k.r * 0.04, k.accent.with_alpha(0.8), k.a);
}
pub fn draw_hex(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.92;
    let hx = poly_n(k, 6, r, 0.0);
    s.fill_poly(shift(&hx, 1.5, 3.0), Color(0x55000000), Color(0x55000000));
    s.fill_poly(hx.clone(), k.accent.darker(0.8), Color(0xff050607));
    s.stroke_poly(hx.clone(), k.accent.with_alpha(0.25), 4.0, true);
    s.stroke_poly(hx, k.accent, 1.3, true);
    s.stroke_poly(poly_n(k, 6, r * 0.62, 0.0), k.accent.with_alpha(0.45), 0.9, true);
    line(s, k, r * 0.12, r * 0.60, k.r * 0.07, k.accent.brighter(0.5), k.a);
}
pub fn draw_yel(s: &mut Scene, k: &KnobCtx) {
    let side = k.r * 0.90;
    let outer = Rect::new(k.c.x - side, k.c.y - side, side * 2.0, side * 2.0);
    s.fill_round(outer.translated(1.5, 3.0), side * 0.30, Color(0x55000000), Color(0x55000000), false);
    s.fill_round(outer, side * 0.30, k.accent.brighter(0.2), k.accent.darker(0.5), false);
    s.stroke_round(outer, side * 0.30, k.accent.darker(0.8), 1.0);
    let screen = outer.reduced(side * 0.20, side * 0.20);
    s.fill_round(screen, side * 0.18, Color(0xff06090b), Color(0xff06090b), false);
    s.stroke_round(screen, side * 0.18, Color(0xff3ae6ee).with_alpha(0.35), 1.0);
    let ring = screen.w * 0.34;
    rim(s, k, ring, Color(0xff3ae6ee).with_alpha(0.45), 0.9);
    line(s, k, ring * 0.2, ring, k.r * 0.07, Color(0xff6cf2f8), k.a);
}
pub fn draw_key(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.80;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff3a3c40), Color(0xff070708));
    rim(s, k, r, Color(0xff000000), 0.8);
    disc(s, k, r * 0.76, Color(0xff0b0c0d), Color(0xff2b2d31));
    rim(s, k, r * 0.76, Color(0x2affffff), 0.8);
    line(s, k, r * 0.10, r * 0.66, k.r * 0.09, k.accent, k.a);
}
pub fn draw_brz(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.84;
    let t = r * 0.76;
    shadow(s, k, r);
    disc(s, k, r, Color(0xfff3d684), Color(0xff6f4d14));
    flutes(s, k, r * 0.82, r * 0.99, 56, Color(0xff5a3d0e), Color(0xfffbe9a8), 1.1);
    rim(s, k, r, Color(0xff3d2907), 0.8);
    disc(s, k, t, Color(0xfffff0b4), Color(0xff9a7122));
    rim(s, k, t, Color(0x66ffffff), 0.8);
    gloss(s, k, t, 0.25);
    line(s, k, t * 0.1, t * 0.96, k.r * 0.07, k.accent.darker(0.7), k.a);
}
pub fn draw_chk(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.95, 11, k.r * 0.04, Color(0xffc7ccce));
    let r = k.r * 0.58;
    shadow(s, k, r);
    fill_wedge(s, k, r * 0.96, k.r * 0.92, k.r * 0.22, k.a, Color(0xff4c4d51), Color(0xff050506));
    stroke_wedge(s, k, r * 0.96, k.r * 0.92, k.r * 0.22, k.a, Color(0x44ffffff), 0.7);
    disc(s, k, r, Color(0xff4c4d51), Color(0xff050506));
    rim(s, k, r, Color(0x33ffffff), 0.8);
    gloss(s, k, r, 0.22);
    line(s, k, r * 0.05, k.r * 0.88, k.r * 0.06, k.accent, k.a);
}
pub fn draw_ivr(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.86;
    let t = r * 0.74;
    shadow(s, k, r);
    disc(s, k, r, Color(0xfff6efd8), Color(0xffa89c76));
    flutes(s, k, r * 0.76, r * 0.99, 36, Color(0xff857a58), Color(0xfffffbea), 1.4);
    rim(s, k, r, Color(0xff4d4630), 0.8);
    disc(s, k, t, Color(0xfffaf4de), Color(0xffc2b78f));
    rim(s, k, t, Color(0x66ffffff), 0.8);
    gloss(s, k, t, 0.2);
    line(s, k, t * 0.1, t * 0.96, k.r * 0.07, k.accent.darker(0.7), k.a);
}
pub fn draw_led(s: &mut Scene, k: &KnobCtx) {
    let n = 15;
    let p = prop(k.a);
    for i in 0..n {
        let f = i as f32 / (n - 1) as f32;
        let lit = f <= p + 0.5 / n as f32;
        let pt = at(k, (-0.8 + 1.6 * f) * PI, k.r * 0.86);
        let d = k.r * 0.10;
        if lit { s.fill_ellipse_solid(ell(pt, d), k.accent.with_alpha(0.3)); }
        s.fill_ellipse_solid(ell(pt, d * 0.5), if lit { k.accent.brighter(0.3) } else { k.accent.with_alpha(0.16) });
    }
    let r = k.r * 0.66;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff35373b), Color(0xff060607));
    flutes(s, k, r * 0.88, r * 0.99, 40, Color(0xff060607), Color(0xff2f3134), 0.8);
    rim(s, k, r, Color(0xff000000), 0.8);
    line(s, k, r * 0.14, r * 0.84, k.r * 0.07, Color(0xfff1efe6), k.a);
}
pub fn draw_dbl(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.90;
    let i = r * 0.52;
    shadow(s, k, r);
    disc(s, k, r, Color(0xfff2f4f4), Color(0xff5d6367));
    flutes(s, k, r * 0.80, r * 0.99, 48, Color(0xff4b5155), Color(0xffe9ecee), 1.1);
    rim(s, k, r, Color(0xff2a2f32), 0.8);
    line(s, k, i * 1.05, r * 0.96, k.r * 0.06, Color(0xff15181a), k.a);
    shadow(s, k, i);
    disc(s, k, i, Color(0xff3e4044), Color(0xff050506));
    flutes(s, k, i * 0.7, i * 0.98, 28, Color(0xff050506), Color(0xff5a5c60), 1.0);
    disc(s, k, i * 0.62, k.accent.brighter(0.2), k.accent.darker(0.5));
    rim(s, k, i * 0.62, k.accent.darker(0.8), 0.8);
    line(s, k, 0.0, i * 0.58, k.r * 0.05, Color(0xfff7f4e8), k.a);
}
pub fn draw_mini(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.92, 11, k.r * 0.05, Color(0xffc7ccce));
    let r = k.r * 0.46;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff414246), Color(0xff060607));
    rim(s, k, r, Color(0xff000000), 0.8);
    gloss(s, k, r, 0.12);
    line(s, k, r * 0.1, r * 0.88, k.r * 0.09, k.accent.brighter(0.6), k.a);
}
pub fn draw_tl(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.80;
    shadow(s, k, r);
    disc(s, k, r, Color(0xff3a3b3e), Color(0xff060607));
    flutes(s, k, r * 0.74, r * 0.99, 60, Color(0xff060607), Color(0xff55575b), 1.1);
    disc(s, k, r * 0.70, Color(0xff2c2d30), Color(0xff09090a));
    rim(s, k, r * 0.70, Color(0x2affffff), 0.8);
    line(s, k, 0.0, r * 0.96, k.r * 0.08, k.accent.brighter(0.6), k.a);
}
pub fn draw_glw(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.70;
    disc(s, k, r, Color(0xff12181d), Color(0xff020304));
    rim(s, k, r, k.accent.with_alpha(0.55), 1.2);
    let lo = -0.8 * PI;
    arc_stroke(s, k, k.r * 0.88, lo, 0.8 * PI, k.r * 0.07, k.accent.with_alpha(0.14));
    if k.a > lo + 0.01 {
        for n in (1..=3).rev() {
            arc_stroke(s, k, k.r * 0.88, lo, k.a, k.r * (0.07 + 0.07 * n as f32), k.accent.with_alpha(0.07 * (4 - n) as f32));
        }
        arc_stroke(s, k, k.r * 0.88, lo, k.a, k.r * 0.07, k.accent);
        arc_stroke(s, k, k.r * 0.88, lo, k.a, k.r * 0.025, k.accent.brighter(0.8));
    }
    let p = at(k, k.a, r * 0.86);
    s.fill_ellipse_solid(ell(p, k.r * 0.12), k.accent.with_alpha(0.35));
    line(s, k, r * 0.30, r * 0.88, k.r * 0.06, k.accent.brighter(0.7), k.a);
    gloss(s, k, r, 0.07);
}
pub fn draw_grd(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.95, 21, k.r * 0.04, k.accent.with_alpha(0.8));
    let r = k.r * 0.80;
    disc(s, k, r, Color(0xff0a0f16), Color(0xff020306));
    s.clip_round(ell(k.c, r), r);
    let col = k.accent.with_alpha(0.28);
    for i in -4..=4 {
        s.line(Point::new(k.c.x + i as f32 * r * 0.09, k.c.y + r * 0.05), Point::new(k.c.x + i as f32 * r * 0.5, k.c.y + r), col, 0.8);
    }
    for j in 1..=5 {
        let y = k.c.y + r * 0.05 + (j as f32 / 5.0).powi(2) * r * 0.95;
        s.line(Point::new(k.c.x - r, y), Point::new(k.c.x + r, y), col, 1.0);
    }
    s.fill_rect(Rect::new(k.c.x - r, k.c.y - r * 0.6, r * 2.0, r * 0.65), Color(0x00000000), k.accent.with_alpha(0.35), true);
    s.pop_clip();
    rim(s, k, r, k.accent, 1.3);
    line(s, k, r * 0.10, r * 0.96, k.r * 0.07, k.accent.brighter(0.8), k.a);
}
pub fn draw_dsh(s: &mut Scene, k: &KnobCtx) {
    let p = prop(k.a);
    disc(s, k, k.r * 0.98, Color(0xff15110c), Color(0xff050404));
    let n = 16;
    for i in 0..n {
        let f = i as f32 / (n - 1) as f32;
        let t = (-0.8 + 1.6 * f) * PI;
        let len = k.r * (0.10 + 0.28 * f);
        let lit = f <= p + 0.5 / n as f32;
        let col = if f > 0.84 { Color(0xffff3b2a) } else { k.accent };
        s.line(at(k, t, k.r * 0.60), at(k, t, k.r * 0.60 + len), if lit { col } else { col.with_alpha(0.12) }, k.r * 0.075);
    }
    let r = k.r * 0.50;
    disc(s, k, r, Color(0xff2a2c30), Color(0xff070809));
    rim(s, k, r, Color(0xff000000), 0.8);
    line(s, k, r * 0.1, r * 0.95, k.r * 0.06, Color(0xffff8a26), k.a);
    disc(s, k, k.r * 0.10, Color(0xff9a9fa3), Color(0xff2a2d30));
}
pub fn draw_ebn(s: &mut Scene, k: &KnobCtx) {
    wood_knob(s, k, Color(0xff3a2c22), Color(0xff0b0806), Color(0xff2b2018), Color(0xff0e0a07), Color(0x30c8a070));
    let c = k.r * 0.86 * 0.80 * 0.46;
    disc(s, k, c, Color(0xfff1d98a), Color(0xff8a6218));
    rim(s, k, c, Color(0xff4a3208), 1.0);
    for i in 1..=3 { rim(s, k, c * i as f32 / 3.2, Color(0x44000000), 0.5); }
    gloss(s, k, c, 0.25);
}
pub fn draw_wnt(s: &mut Scene, k: &KnobCtx) {
    dots(s, k, 0.96, 21, k.r * 0.04, Color(0xffe9eef2));
    let r = k.r * 0.82;
    let w = r * 0.78;
    shadow(s, k, r);
    disc(s, k, r, Color(0xfffcfdfd), Color(0xff535a5e));
    rim(s, k, r, Color(0xff25292b), 0.8);
    rim(s, k, r * 0.90, Color(0x55000000), 1.0);
    disc(s, k, w, Color(0xff9a6a3e), Color(0xff3a2210));
    grain(s, k.c, w, k.a, Color(0x40201008), 0.05, 0.6, 0.8);
    rim(s, k, w, Color(0x55ffffff), 0.8);
    gloss(s, k, w, 0.22);
    line(s, k, w * 0.12, w * 0.94, k.r * 0.065, k.accent.brighter(0.4), k.a);
}
pub fn draw_rck(s: &mut Scene, k: &KnobCtx) {
    let r = k.r * 0.86;
    for n in (1..=6).rev() {
        let e = r + n as f32 * 0.9;
        s.fill_ellipse_solid(ell(Point::new(k.c.x + 0.8, k.c.y + 2.6), e), Color(0x12000000));
    }
    disc(s, k, r, Color(0xff2b2c2e), Color(0xff050506));
    flutes(s, k, r * 0.80, r * 0.995, 72, Color(0xff030304), Color(0xff4a4c50), 0.9);
    rim(s, k, r, Color(0xff000000), 1.0);
    let t = r * 0.78;
    disc(s, k, t, Color(0xff4a4b4f), Color(0xff0d0d0f));
    for i in 1..=7 {
        let col = if i % 2 == 1 { Color(0x0cffffff) } else { Color(0x0c000000) };
        rim(s, k, t * i as f32 / 7.5, col, 0.6);
    }
    rim(s, k, t, Color(0x55000000), 1.0);
    s.stroke_ellipse(ell(Point::new(k.c.x + 0.8, k.c.y + 0.8), (t - 0.8).max(0.0)), Color(0x22ffffff), 0.7);
    let col = if k.accent.perceived_brightness() > 0.2 { k.accent } else { Color(0xfff2f2f0) };
    s.line(add(at(k, k.a, r * 0.16), Point::new(0.7, 0.9)), add(at(k, k.a, r * 0.96), Point::new(0.7, 0.9)), Color(0x99000000), k.r * 0.115);
    s.line(at(k, k.a, r * 0.16), at(k, k.a, r * 0.96), col, k.r * 0.085);
}

pub fn draw_style_flat(scene: &mut Scene, style_code: &str, center: Point, radius: f32, angle: f32, accent: Color, dark_marks: bool) {
    let k = KnobCtx { c: center, r: radius, a: angle, accent, dark_marks };
    match style_code {
        "Brit" | "console" => draw_brit(scene, &k),
        "N" => draw_n(scene, &k),
        "A" => draw_a(scene, &k),
        "FS" => draw_fs(scene, &k),
        "Mtl" | "metal" => draw_mtl(scene, &k),
        "Wd" => draw_wd(scene, &k),
        "Bk" | "bakelite" => draw_bk(scene, &k),
        "Pie" => draw_pie(scene, &k),
        "Snk" => draw_snk(scene, &k),
        "Slv" | "ivory" => draw_slv(scene, &k),
        "Rd" => draw_rd(scene, &k),
        "Ptr" => draw_ptr(scene, &k),
        "Dm" => draw_dm(scene, &k),
        "Sq" => draw_sq(scene, &k),
        "Seg" => draw_seg(scene, &k),
        "Vfd" => draw_vfd(scene, &k),
        "Cyb" => draw_cyb(scene, &k),
        "Fat" => draw_fat(scene, &k),
        "Pag" => draw_pag(scene, &k),
        "Lvr" => draw_lvr(scene, &k),
        "Tab" => draw_tab(scene, &k),
        "Wh" => draw_wh(scene, &k),
        "Knl" => draw_knl(scene, &k),
        "Skt" => draw_skt(scene, &k),
        "Rib" => draw_rib(scene, &k),
        "Chr" => draw_chr(scene, &k),
        "Arc" => draw_arc(scene, &k),
        "Brg" => draw_brg(scene, &k),
        "Neo" => draw_neo(scene, &k),
        "Hex" => draw_hex(scene, &k),
        "Yel" => draw_yel(scene, &k),
        "Key" => draw_key(scene, &k),
        "Brz" => draw_brz(scene, &k),
        "Chk" => draw_chk(scene, &k),
        "Ivr" => draw_ivr(scene, &k),
        "Mpl" => draw_mpl(scene, &k),
        "Led" => draw_led(scene, &k),
        "Dbl" => draw_dbl(scene, &k),
        "Mini" => draw_mini(scene, &k),
        "Tl" => draw_tl(scene, &k),
        "Glw" => draw_glw(scene, &k),
        "Grd" => draw_grd(scene, &k),
        "Dsh" => draw_dsh(scene, &k),
        "Mic" => draw_mic(scene, &k),
        "Ebn" => draw_ebn(scene, &k),
        "Wnt" => draw_wnt(scene, &k),
        "Rck" => draw_rck(scene, &k),
        _ => {}
    }
}
