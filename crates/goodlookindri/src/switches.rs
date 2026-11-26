// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Marcos Quirino Deida

//! Toggles, rockers, push buttons and key banks, lit by the one [`LightRig`]: drop shadows use
//! `shadow_offset` / `shadow_sigma` of each part's height, facets and curved faces are shaded
//! from their normals, and lit parts add bloom with [`Blend::Add`].

use crate::color::Color;
use crate::light::LightRig;
use crate::retro;
use crate::scene::{Blend, Cmd, Paint, Point, Rect, Scene, Shape, Stops};

const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;
const PI: f32 = std::f32::consts::PI;

fn solid_round(scene: &mut Scene, r: Rect, radius: f32, c: Color) { scene.fill_round(r, radius, c, c, true); }
fn hline(scene: &mut Scene, y: f32, x0: f32, x1: f32, c: Color, w: f32) { scene.line(Point::new(x0, y), Point::new(x1, y), c, w); }
fn text_c(scene: &mut Scene, r: Rect, text: &str, c: Color, size: f32) { scene.text(r.center(), text, c, size, true); }
fn box_at(w: f32, h: f32, c: Point) -> Rect { Rect::new(c.x - w * 0.5, c.y - h * 0.5, w, h) }

struct Rot { cx: f32, cy: f32, sin: f32, cos: f32, ang: f32 }
impl Rot {
    fn around(cx: f32, cy: f32, ang: f32) -> Self { Self { cx, cy, sin: ang.sin(), cos: ang.cos(), ang } }
    fn pt(&self, p: Point) -> Point {
        let x = p.x - self.cx;
        let y = p.y - self.cy;
        Point::new(self.cos * x - self.sin * y + self.cx, self.sin * x + self.cos * y + self.cy)
    }
    fn rect(&self, r: Rect) -> Rect {
        let p = [self.pt(Point::new(r.x, r.y)), self.pt(Point::new(r.right(), r.y)), self.pt(Point::new(r.x, r.bottom())), self.pt(Point::new(r.right(), r.bottom()))];
        let minx = p.iter().map(|q| q.x).fold(f32::MAX, f32::min);
        let miny = p.iter().map(|q| q.y).fold(f32::MAX, f32::min);
        let maxx = p.iter().map(|q| q.x).fold(f32::MIN, f32::max);
        let maxy = p.iter().map(|q| q.y).fold(f32::MIN, f32::max);
        Rect::new(minx, miny, maxx - minx, maxy - miny)
    }
    fn vec(&self, v: Point) -> Point { Point::new(self.cos * v.x - self.sin * v.y, self.sin * v.x + self.cos * v.y) }
    /// Shapes stay axis-aligned, so this is exact for quarter turns and a bounding box otherwise.
    fn shape(&self, s: Shape) -> Shape {
        match s { Shape::Ellipse(r) => Shape::Ellipse(self.rect(r)), Shape::Round(r, k) => Shape::Round(self.rect(r), k) }
    }
    fn paint(&self, p: Paint) -> Paint {
        match p {
            Paint::Solid(c) => Paint::Solid(c),
            Paint::Linear { stops, angle } => Paint::Linear { stops, angle: angle + self.ang },
            Paint::Radial { stops, center, rx, ry, focus } => {
                let (c, s) = (self.cos.abs(), self.sin.abs());
                Paint::Radial { stops, center: self.pt(center), rx: rx * c + ry * s, ry: rx * s + ry * c, focus: self.pt(focus) }
            }
            Paint::Conic { stops, center, start } => Paint::Conic { stops, center: self.pt(center), start: start + self.ang },
        }
    }
}
fn blit(scene: &mut Scene, tmp: Scene, rot: &Rot) {
    for c in tmp.cmds {
        scene.cmds.push(match c {
            Cmd::FillEllipse { rect, top, bottom, diagonal } => Cmd::FillEllipse { rect: rot.rect(rect), top, bottom, diagonal },
            Cmd::StrokeEllipse { rect, color, width } => Cmd::StrokeEllipse { rect: rot.rect(rect), color, width },
            Cmd::FillRoundRect { rect, radius, top, bottom, vertical } => Cmd::FillRoundRect { rect: rot.rect(rect), radius, top, bottom, vertical },
            Cmd::StrokeRoundRect { rect, radius, color, width } => Cmd::StrokeRoundRect { rect: rot.rect(rect), radius, color, width },
            Cmd::FillRect { rect, top, bottom, vertical } => Cmd::FillRect { rect: rot.rect(rect), top, bottom, vertical },
            Cmd::Line { a, b, color, width } => Cmd::Line { a: rot.pt(a), b: rot.pt(b), color, width },
            Cmd::FillPoly { pts, top, bottom, diagonal } => Cmd::FillPoly { pts: pts.into_iter().map(|p| rot.pt(p)).collect(), top, bottom, diagonal },
            Cmd::StrokePoly { pts, color, width, closed } => Cmd::StrokePoly { pts: pts.into_iter().map(|p| rot.pt(p)).collect(), color, width, closed },
            Cmd::StrokeArc { center, radius, from, to, color, width } => Cmd::StrokeArc { center: rot.pt(center), radius, from: from + rot.ang, to: to + rot.ang, color, width },
            Cmd::Text { at, text, color, size, center } => Cmd::Text { at: rot.pt(at), text, color, size, center },
            Cmd::ClipRound { rect, radius } => Cmd::ClipRound { rect: rot.rect(rect), radius },
            Cmd::PopClip => Cmd::PopClip,
            Cmd::Fill { shape, paint, feather, blend } => Cmd::Fill { shape: rot.shape(shape), paint: rot.paint(paint), feather, blend },
            Cmd::Shadow { shape, sigma, color } => Cmd::Shadow { shape: rot.shape(shape), sigma, color },
            Cmd::InnerShadow { shape, offset, sigma, color } => Cmd::InnerShadow { shape: rot.shape(shape), offset: rot.vec(offset), sigma, color },
            Cmd::Image { img, dest, opacity } => Cmd::Image { img, dest: rot.rect(dest), opacity },
        });
    }
}


const BLACK: Color = Color(0xff000000);
const WHITE: Color = Color(0xffffffff);

/// The studio rig seen from a frame that [`blit`] will later turn by `ang`, so light and shadows
/// still come from the panel's upper left after the turn.
fn rig_in(ang: f32) -> LightRig {
    let mut l = LightRig::studio();
    let [x, y, z] = l.key_dir;
    let (s, c) = ang.sin_cos();
    l.key_dir = [x * c + y * s, -x * s + y * c, z];
    l
}
/// Linear gradient angle whose first stop faces the key light.
fn lit_angle(rig: &LightRig) -> f32 {
    let d = rig.screen_dir();
    if d.x == 0.0 && d.y == 0.0 { HALF_PI } else { (-d.y).atan2(-d.x) }
}
fn unit(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l, v[2] / l]
}
/// `base` as lit on a surface with normal `n`, relative to a face that looks straight out of the
/// panel; `gloss` adds the key's specular highlight.
fn shade_col(rig: &LightRig, base: Color, n: [f32; 3], gloss: f32) -> Color {
    let n = unit(n);
    let (k, f) = (rig.shade(n), rig.shade([0.0, 0.0, 1.0]));
    let sp = rig.specular(n, 28.0) * gloss * 255.0;
    let ch = |v: u8, i: usize| (v as f32 * k[i] / f[i].max(1e-3) + sp * rig.key_color[i]).clamp(0.0, 255.0).round() as u32;
    Color::rgba(ch(base.r(), 0), ch(base.g(), 1), ch(base.b(), 2), base.a() as u32)
}
/// Soft shadow of `shape` floating `height` above what is under it.
fn drop_shadow(scene: &mut Scene, shape: Shape, rig: &LightRig, height: f32, alpha: f32) {
    scene.shadow(shape.translated(rig.shadow_offset(height)), rig.shadow_sigma(height), BLACK.with_alpha(alpha));
}
/// Seamless turn for polished rings: `hi` toward the light and opposite it, `lo` between.
fn chrome_conic(hi: Color, lo: Color) -> Stops {
    let mid = lo.mix(hi, 0.62);
    Stops::new(&[(0.0, hi), (0.22, lo), (0.46, mid), (0.56, mid), (0.80, lo), (1.0, hi)])
}
fn conic_start(rig: &LightRig) -> f32 {
    let d = rig.screen_dir();
    d.y.atan2(d.x)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RockerStyle { Lamp, Classic, Slim, Neon }
pub static ROCKER_NAMES: [&str; 4] = ["Lamp rocker", "Classic rocker", "Slim rocker", "Neon rocker"];

pub fn draw_rocker(scene: &mut Scene, r: Rect, on: bool, lamp: Color, style: RockerStyle, horizontal: bool, glow: bool) {
    let ang = if horizontal { HALF_PI } else { 0.0 };
    let rig = rig_in(ang);
    let la = lit_angle(&rig);
    let mut tmp = Scene::new();
    let g = &mut tmp;
    let b = if horizontal { box_at(r.h, r.w, r.center()) } else { r };
    let w = b.w;
    let h = b.h;
    let cx = b.center().x;

    // Bezel: a moulded frame standing just proud of the panel, chamfered into the well.
    let bezel = b.expanded(1.5);
    drop_shadow(g, Shape::Round(bezel, 4.5), &rig, 2.0, 0.55);
    g.fill_round_linear(bezel, 4.5, Stops::new(&[(0.0, Color(0xff4b4f54)), (0.35, Color(0xff1d1f22)), (1.0, Color(0xff060707))]), la);
    g.fill_round_linear(bezel.reduced(1.0, 1.0), 3.8, Stops::new(&[(0.0, Color(0xff232629)), (1.0, Color(0xff111214))]), la);
    let t = (w.min(h) * 0.085).max(2.0);
    let well = b.reduced(t, t * 0.8);
    g.fill_round_linear(well.expanded(0.8), 3.4, Stops::new(&[(0.0, Color(0xff000000)), (1.0, Color(0xff3a3d41))]), la);
    solid_round(g, well, 2.8, Color(0xff020303));
    g.inner_shadow(Shape::Round(well, 2.8), rig.shadow_offset(2.0), 1.5, BLACK.with_alpha(0.9));

    // Paddle: a shallow V seesaw. The raised half stands proud of the bezel, nearly flat, and
    // rolls over at its end, where its end wall shows; the pressed half slopes down into the
    // well, drawn smaller with the well's walls exposed round it, in the shade of the raised
    // half and of the bezel. Every surface is shaded from its normal, so which half is bright
    // follows the rig.
    let plate = well.reduced(1.3, 1.5);
    let split = plate.center().y;
    let top = Rect::new(plate.x, plate.y, plate.w, split - plate.y);
    let bottom = Rect::new(plate.x, split, plate.w, plate.bottom() - split);
    let body = match style {
        RockerStyle::Classic => Color(0xff2f3134),
        RockerStyle::Slim => Color(0xffc6c8c3),
        RockerStyle::Neon => Color(0xff0e151d),
        RockerStyle::Lamp => Color(0xff34363a),
    };
    let light = style == RockerStyle::Slim;
    let gloss = if light { 0.05 } else { 0.13 };
    // `dir`: from the pivot toward the raised end (−1 up the screen, +1 down).
    let dir = if on { -1.0f32 } else { 1.0 };
    let half_len = plate.h * 0.5;
    let rise = 1.1;
    let (raised, sunk) = if on {
        (Rect::new(plate.x - 0.5, plate.y - rise, plate.w + 1.0, half_len + rise), Rect::new(plate.x + 0.9, split, plate.w - 1.8, half_len - 1.7))
    } else {
        (Rect::new(plate.x - 0.5, split, plate.w + 1.0, half_len + rise), Rect::new(plate.x + 0.9, plate.y + 1.7, plate.w - 1.8, half_len - 1.7))
    };
    let lit_side = rig.screen_dir().y * dir > 0.0;
    // Gradient along a half: `slope(u)` is the rise of the surface per unit of distance from the
    // pivot, u = 0 at the pivot and 1 at the end.
    let along = |rect: Rect, toward: f32, col: Color, slope: &dyn Fn(f32) -> f32| -> Paint {
        let st: Vec<(f32, Color)> = (0..6).map(|i| {
            let p = i as f32 / 5.0;
            let u = if toward < 0.0 { 1.0 - p } else { p };
            let _ = rect;
            // Slopes are exaggerated so the curvature reads at plugin sizes.
            (p, shade_col(&rig, col, [0.0, -slope(u) * toward * 1.9, 1.0], gloss))
        }).collect();
        Paint::Linear { stops: Stops::new(&st), angle: HALF_PI }
    };
    // Pressed half first: it lies under everything else.
    let sunk_slope = |u: f32| -0.80 + 0.55 * u * u;
    g.fill_shape(Shape::Round(sunk, 1.6), along(sunk, -dir, body.darker(0.10), &sunk_slope), 0.0, Blend::Over);
    g.fill_round_linear(sunk, 1.6, Stops::new(&[(0.0, WHITE.with_alpha(0.08)), (0.08, WHITE.with_alpha(0.0)), (0.88, BLACK.with_alpha(0.0)), (1.0, BLACK.with_alpha(0.5))]), 0.0);
    // Shadow thrown across the pivot by the raised half, deeper when that half is on the lit side.
    let pv = if lit_side { 0.62 } else { 0.34 };
    let pv_stops = if on {
        Stops::new(&[(0.0, BLACK.with_alpha(pv)), (0.22, BLACK.with_alpha(pv * 0.55)), (0.55, BLACK.with_alpha(0.0))])
    } else {
        Stops::new(&[(0.45, BLACK.with_alpha(0.0)), (0.78, BLACK.with_alpha(pv * 0.55)), (1.0, BLACK.with_alpha(pv))])
    };
    g.fill_shape(Shape::Round(sunk, 1.6), Paint::Linear { stops: pv_stops, angle: HALF_PI }, 0.0, Blend::Over);
    // It sits below the bezel and takes the bezel's shadow.
    let sunk_zone = if on { Rect::new(well.x, split, well.w, well.bottom() - split) } else { Rect::new(well.x, well.y, well.w, split - well.y) };
    g.clip_round(sunk_zone, 0.0);
    g.inner_shadow(Shape::Round(well, 2.8), rig.shadow_offset(5.0), 2.6, BLACK.with_alpha(0.85));
    g.pop_clip();
    g.stroke_round(sunk, 1.6, BLACK.with_alpha(0.55), 0.6);

    // Raised half: shadow onto the bezel, the end wall, then the face.
    drop_shadow(g, Shape::Round(raised, 2.0), &rig, 3.5, 0.6);
    let wall_h = 2.6;
    let wall = if on { Rect::new(raised.x, raised.y - 0.2, raised.w, wall_h + 1.5) } else { Rect::new(raised.x, raised.bottom() - wall_h - 1.3, raised.w, wall_h + 1.5) };
    // Side walls: the block under the face, lit on the side toward the key.
    let wall_base = body.brighter(if light { 0.0 } else { 0.10 });
    let side_l = shade_col(&rig, wall_base, [-0.92, 0.0, 0.38], 0.5);
    let side_r = shade_col(&rig, wall_base, [0.92, 0.0, 0.38], 0.5);
    g.fill_round_linear(raised, 2.0, Stops::new(&[(0.0, side_l), (0.12, side_l.mix(body, 0.5)), (0.88, side_r.mix(body, 0.5)), (1.0, side_r)]), 0.0);
    let wall_c = shade_col(&rig, wall_base, [0.0, dir * 0.92, 0.38], 0.9);
    g.fill_round_linear(wall, 2.0, Stops::two(wall_c.brighter(0.10), wall_c.darker(0.25)), HALF_PI);
    let sw = 1.3;
    let face = if on { Rect::new(raised.x + sw, raised.y + wall_h * 0.55, raised.w - sw * 2.0, raised.h - wall_h * 0.55) } else { Rect::new(raised.x + sw, raised.y, raised.w - sw * 2.0, raised.h - wall_h * 0.55) };
    // Nearly flat, a small dip at the pivot, rolling over into the end wall.
    let raised_slope = |u: f32| if u < 0.30 { 0.46 - 1.3 * u } else if u < 0.66 { 0.07 } else { 0.07 - 2.6 * (u - 0.66) };
    g.fill_shape(Shape::Round(face, 1.8), along(face, dir, body, &raised_slope), 0.0, Blend::Over);
    // Rounded long edges: a cylinder-like falloff across the width, lit edge first.
    let edge = if light { 0.30 } else { 0.55 };
    g.fill_round_linear(face, 1.8, Stops::new(&[(0.0, WHITE.with_alpha(0.20)), (0.07, WHITE.with_alpha(0.0)), (0.88, BLACK.with_alpha(0.0)), (1.0, BLACK.with_alpha(edge))]), 0.0);
    // Sheen pooled on the flat of the face.
    let sheen_y = face.y + face.h * if on { 0.52 } else { 0.48 };
    g.glow_round(box_at(face.w - 3.0, face.h * 0.42, Point::new(cx, sheen_y)), face.h * 0.2, WHITE.with_alpha(if light { 0.20 } else { 0.085 }), face.h * 0.2, Blend::Screen);
    g.stroke_round(face, 1.8, BLACK.with_alpha(0.5), 0.6);
    // Crease at the pivot: a dark line, and the raised half's edge catching light beside it.
    let crease = split;
    hline(g, crease, plate.x - 0.3, plate.right() + 0.3, BLACK.with_alpha(if light { 0.5 } else { 0.8 }), 1.1);
    hline(g, crease + dir * 1.0, face.x + 1.0, face.right() - 1.0, WHITE.with_alpha(if light { 0.45 } else { 0.16 }), 0.6);
    // Edge where the face turns into the end wall.
    let turn = if on { face.y + 0.3 } else { face.bottom() - 0.3 };
    hline(g, turn, face.x + 1.2, face.right() - 1.2, WHITE.with_alpha(if lit_side { if light { 0.7 } else { 0.38 } } else { 0.10 }), 0.6);

    let ink = if light { Color(0xff222426) } else { Color(0xfff2f2ee) };
    let mark = w.min(h) * 0.15;
    let lamped = style == RockerStyle::Lamp || style == RockerStyle::Neon;
    let top_mark_y = top.center().y + if lamped { h * 0.05 } else { 0.0 };
    solid_round(g, box_at(mark * 0.45, mark * 1.7, Point::new(cx, top_mark_y)), 0.8, ink.with_alpha(if on { 1.0 } else { 0.8 }));
    g.stroke_ellipse(box_at(mark * 1.5, mark * 1.5, Point::new(cx, bottom.center().y)), ink.with_alpha(if on { 0.8 } else { 1.0 }), mark * 0.34);
    if lamped {
        let win = box_at(plate.w * 0.56, (h * 0.07).max(2.2), Point::new(cx, top.y + h * 0.085));
        solid_round(g, win.expanded(0.7), 1.8, Color(0xff020202));
        if on {
            let hot = lamp.brighter(0.75);
            g.fill_round_radial(win, 1.2, Stops::new(&[(0.0, hot), (0.7, lamp), (1.0, lamp.darker(0.25))]), win.center());
            if glow {
                g.glow_round(win.expanded(h * 0.12), h * 0.12, lamp.with_alpha(0.13), h * 0.14, Blend::Over);
                g.glow_round(win.expanded(h * 0.22), h * 0.22, lamp.with_alpha(0.55), h * 0.24, Blend::Add);
            } else {
                solid_round(g, win.expanded(2.0), 3.0, lamp.with_alpha(0.25));
            }
            solid_round(g, Rect::new(win.x + 1.0, win.y + win.h * 0.2, win.w - 2.0, win.h * 0.3), 0.6, WHITE.with_alpha(0.55));
        } else {
            g.fill_round_linear(win, 1.2, Stops::two(lamp.darker(0.88), lamp.darker(0.72)), HALF_PI);
            hline(g, win.y + 0.6, win.x + 1.0, win.right() - 1.0, WHITE.with_alpha(0.10), 0.5);
        }
    }
    if style == RockerStyle::Neon {
        if on && glow {
            g.glow_round(plate.expanded(4.0), 6.0, lamp.with_alpha(0.10), 7.0, Blend::Over);
            g.glow_round(plate.expanded(4.0), 6.0, lamp.with_alpha(0.16), 7.0, Blend::Add);
        }
        // Off, the tube is dark glass (the helper draws it switched off below 0.2 of full light).
        retro::glow_round(g, plate.reduced(0.2, 0.2), 2.2, if on { lamp } else { lamp.with_alpha(0.35) }, 1.0, if on { 1.0 } else { 0.2 }, glow);
    }
    if horizontal {
        blit(scene, tmp, &Rot::around(r.center().x, r.center().y, HALF_PI));
    } else {
        scene.cmds.append(&mut tmp.cmds);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleStyle { Bat, Lever, Mini }
pub static TOGGLE_NAMES: [&str; 3] = ["Bat toggle", "Lever toggle", "Mini toggle"];

pub fn draw_toggle(scene: &mut Scene, r: Rect, on: bool, style: ToggleStyle, accent: Color) {
    draw_toggle_at(scene, r, if on { 1.0 } else { -1.0 }, style, accent);
}

/// Toggle with the lever anywhere along its throw: `pos` +1 is fully up (on), −1 fully down, and
/// 0 points straight at the viewer, where the lever is foreshortened to its tip.
pub fn draw_toggle_at(scene: &mut Scene, r: Rect, pos: f32, style: ToggleStyle, accent: Color) {
    let rig = LightRig::studio();
    let ld = rig.screen_dir();
    let pos = if pos.is_finite() { pos.clamp(-1.0, 1.0) } else { 0.0 };
    let c = r.center();
    let s = r.w.min(r.h * 0.8);
    let nut_r = s * if style == ToggleStyle::Mini { 0.30 } else { 0.38 };
    let rr = nut_r * 1.12;

    // Washer on the panel.
    let washer = Scene::ellipse_at(c, rr * 1.13);
    drop_shadow(scene, Shape::Ellipse(washer), &rig, 1.2, 0.6);
    scene.fill_ellipse_conic(washer, chrome_conic(Color(0xffd9dde0), Color(0xff70787e)), conic_start(&rig));
    scene.stroke_ellipse(washer, Color(0xff1c2023).with_alpha(0.8), 0.6);
    scene.stroke_ellipse(washer.reduced(0.7, 0.7), WHITE.with_alpha(0.35), 0.5);

    // Hex nut: chamfer facets shaded from their normals, flat top with a soft reflection.
    let steel = Color(0xffc3c9cd);
    let corner = |i: usize, k: f32| { let a = i as f32 * PI / 3.0; Point::new(c.x + a.cos() * rr * k, c.y + a.sin() * rr * k) };
    let outer: Vec<Point> = (0..6).map(|i| corner(i, 1.0)).collect();
    let inner: Vec<Point> = (0..6).map(|i| corner(i, 0.80)).collect();
    drop_shadow(scene, Shape::Ellipse(Scene::ellipse_at(c, rr * 0.93)), &rig, 3.0, 0.6);
    scene.fill_poly(outer.clone(), Color(0xff8b9399), Color(0xff8b9399));
    for i in 0..6 {
        let am = (i as f32 + 0.5) * PI / 3.0;
        let col = shade_col(&rig, steel, [am.cos() * 0.9, am.sin() * 0.9, 0.45], 1.3);
        let j = (i + 1) % 6;
        scene.fill_poly(vec![outer[i], outer[j], inner[j], inner[i]], col, col);
    }
    for i in 0..6 { scene.line(outer[i], inner[i], Color(0xff14171a).with_alpha(0.35), 0.5); }
    scene.fill_poly(inner.clone(), Color(0xfff4f6f7), Color(0xff9aa2a8));
    scene.stroke_poly(inner, WHITE.with_alpha(0.45), 0.5, true);
    scene.stroke_poly(outer, Color(0xff14171a), 0.7, true);

    // Threaded bushing, seen end on: turns of thread around the socket.
    let rb = nut_r * 0.66;
    let bush = Scene::ellipse_at(c, rb);
    drop_shadow(scene, Shape::Ellipse(bush), &rig, 1.5, 0.5);
    scene.fill_ellipse_conic(bush, chrome_conic(Color(0xffffffff), Color(0xff596168)), conic_start(&rig));
    for k in [0.90f32, 0.78] {
        scene.stroke_ellipse(Scene::ellipse_at(c, rb * k), Color(0xff0c0e10).with_alpha(0.55), rb * 0.05);
        scene.stroke_ellipse(Scene::ellipse_at(c, rb * (k - 0.05)), WHITE.with_alpha(0.30), rb * 0.03);
    }
    scene.stroke_ellipse(bush, Color(0xff101214), 0.6);
    let socket = Scene::ellipse_at(c, rb * 0.64);
    scene.fill_ellipse_radial(socket, Stops::two(Color(0xff26292c), Color(0xff040505)), c);
    scene.inner_shadow(Shape::Ellipse(socket), rig.shadow_offset(2.5), 1.2, BLACK.with_alpha(0.9));

    // Lever: a rod leaning out of the panel. `sin_t` of its lean is what we see of its length.
    let (bat, mini) = (style == ToggleStyle::Bat, style == ToggleStyle::Mini);
    let len = r.h * if bat { 0.33 } else { 0.42 };
    let sin_max = 0.77f32;
    let cos_t = (1.0 - (pos * sin_max) * (pos * sin_max)).sqrt();
    let tip_h = len / sin_max * cos_t;
    let near = 0.86 + 0.22 * cos_t;
    let tip = Point::new(c.x, c.y - pos * len);
    let w0 = s * if bat { 0.105 } else if mini { 0.07 } else { 0.085 };
    let w1 = s * if bat { 0.20 } else if mini { 0.095 } else { 0.12 } * near;
    let black = bat;
    let ramp = if black {
        Stops::new(&[(0.0, Color(0xff040405)), (0.14, Color(0xff303438)), (0.27, Color(0xffaab0b5)), (0.40, Color(0xff2a2d30)), (0.78, Color(0xff060607)), (1.0, Color(0xff34383c))])
    } else {
        Stops::new(&[(0.0, Color(0xff4d555b)), (0.12, Color(0xffb4bbc0)), (0.28, Color(0xffffffff)), (0.46, Color(0xffb0b7bc)), (0.78, Color(0xff343a3f)), (1.0, Color(0xff8b9399))])
    };

    // Cast shadow: the rod's shadow runs from the socket to where the tip's lands.
    let n = 7;
    for i in 0..n {
        let t = (i as f32 + 0.5) / n as f32;
        let hgt = 2.0 + tip_h * t;
        let p = Point::new(c.x + (tip.x - c.x) * t, c.y + (tip.y - c.y) * t).add(rig.shadow_offset(hgt));
        let rad = (w0 + (w1 - w0) * t) * 0.95;
        scene.shadow_ellipse(Scene::ellipse_at(p, rad), rig.shadow_sigma(hgt) * 0.55, BLACK.with_alpha(0.24 + 0.14 * (1.0 - t)));
    }
    let tsh = rig.shadow_offset(2.0 + tip_h);
    scene.shadow_ellipse(Scene::ellipse_at(tip.add(tsh), w1), rig.shadow_sigma(tip_h) * 0.55, BLACK.with_alpha(0.38));

    let dx = tip.x - c.x;
    let dy = tip.y - c.y;
    let seen = (dx * dx + dy * dy).sqrt();
    if seen > 0.5 {
        let perp = Point::new(-dy / seen, dx / seen);
        let side = if perp.x * ld.x + perp.y * ld.y >= 0.0 { 1.0 } else { -1.0 };
        let quad = |u0: f32, u1: f32| vec![c.add(perp.mul(w0 * u0)), tip.add(perp.mul(w1 * u0)), tip.add(perp.mul(w1 * u1)), c.add(perp.mul(w0 * u1))];
        // The rod is a tapered cylinder: thin slices along its length, each a full multi-stop
        // gradient across its own width, so the highlight narrows with the taper.
        let _ = side;
        let ang = if ld.x <= 0.0 { 0.0 } else { PI };
        let slices = ((seen / 0.5).ceil() as usize).clamp(6, 120);
        for j in 0..slices {
            let (t0, t1) = (j as f32 / slices as f32, (j + 1) as f32 / slices as f32);
            let (ya, yb) = (c.y + dy * t0, c.y + dy * t1);
            let hw = w0 + (w1 - w0) * (t0 + t1) * 0.5;
            let rect = Rect::new(c.x - hw, ya.min(yb) - 0.06, hw * 2.0, (ya - yb).abs() + 0.12);
            scene.fill_shape(Shape::Round(rect, 0.0), Paint::Linear { stops: ramp, angle: ang }, 0.0, Blend::Over);
        }
        // Darker toward the socket, where the bushing and the rod's own lean shade it.
        let (near_c, far_c) = (BLACK.with_alpha(0.0), BLACK.with_alpha(if black { 0.45 } else { 0.38 }));
        let (g0, g1) = if tip.y < c.y { (near_c, far_c) } else { (far_c, near_c) };
        scene.fill_poly(quad(-1.0, 1.0), g0, g1);
        scene.stroke_poly(vec![c.add(perp.mul(-w0)), tip.add(perp.mul(-w1))], BLACK.with_alpha(0.55), 0.6, false);
        scene.stroke_poly(vec![c.add(perp.mul(w0)), tip.add(perp.mul(w1))], BLACK.with_alpha(0.55), 0.6, false);
        // Where the rod leaves the socket it is in the bushing's shade.
        scene.clip_round(socket, rb);
        scene.glow_ellipse(socket, BLACK.with_alpha(0.55), rb * 0.5, Blend::Over);
        scene.pop_clip();
    }

    // Rounded end of the bat, nearest the viewer.
    let ball = Scene::ellipse_at(tip, w1);
    let focus = tip.add(ld.mul(w1 * 0.42));
    if black {
        scene.fill_ellipse_radial(ball, Stops::new(&[(0.0, Color(0xffb9bfc4)), (0.16, Color(0xff4a4f54)), (0.5, Color(0xff17191b)), (1.0, Color(0xff030303))]), focus);
    } else {
        scene.fill_ellipse_radial(ball, Stops::new(&[(0.0, Color(0xffffffff)), (0.3, Color(0xffe3e7e9)), (0.7, Color(0xff8f979d)), (1.0, Color(0xff3c4247))]), focus);
    }
    scene.stroke_ellipse(ball, BLACK.with_alpha(0.6), 0.6);
    let lang = ld.y.atan2(ld.x);
    scene.stroke_arc(tip, w1 - 0.7, lang - 0.95, lang + 0.95, WHITE.with_alpha(if black { 0.38 } else { 0.85 }), 0.7);
    scene.stroke_arc(tip, w1 - 0.8, lang + PI - 0.7, lang + PI + 0.7, WHITE.with_alpha(if black { 0.14 } else { 0.30 }), 0.6);
    if style == ToggleStyle::Lever {
        let dot = Scene::ellipse_at(tip, w1 * 0.46);
        scene.fill_ellipse_solid(dot.expanded(0.6), Color(0xff0b0c0d));
        if pos > 0.5 {
            scene.fill_ellipse_radial(dot, Stops::new(&[(0.0, accent.brighter(0.8)), (0.6, accent), (1.0, accent.darker(0.3))]), tip);
            scene.glow_ellipse(Scene::ellipse_at(tip, w1 * 2.2), accent.with_alpha(0.45), w1 * 1.7, Blend::Add);
        } else {
            scene.fill_ellipse_radial(dot, Stops::two(accent.darker(0.6), accent.darker(0.85)), focus);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushStyle { Square, Round, Dome, Chamfer, Neon, Flat, Arcade, Pill }
pub static PUSH_NAMES: [&str; 8] = ["Square key", "Round chrome", "Illuminated dome", "Chamfer pad", "Neon outline", "Flat label", "Arcade", "Pill"];
pub const NUM_PUSH_STYLES: i32 = 8;

fn key_size(face: Rect) -> f32 { (face.h * 0.42).min(13.0) }

/// Recess a key sits in: dark at the lit side where the rim shades it, a lip of light opposite.
fn key_well(scene: &mut Scene, well: Rect, radius: f32, rig: &LightRig) {
    let la = lit_angle(rig);
    scene.fill_round_linear(well.expanded(0.8), radius + 0.8, Stops::new(&[(0.0, Color(0xff000000)), (0.6, Color(0xff16181a)), (1.0, Color(0xff4a4e53))]), la);
    solid_round(scene, well, radius, Color(0xff050606));
    scene.inner_shadow(Shape::Round(well, radius), rig.shadow_offset(2.0), 1.4, BLACK.with_alpha(0.9));
}

/// Light from inside a translucent cap: the face falls off toward its edges, a hot near-white
/// centre sits over the lamp, then bloom spills over whatever surrounds it. The spill is a
/// tinted wash under the additive light, so it also shows on pale panels where adding light
/// alone saturates to white.
fn lamp_glow(scene: &mut Scene, shape: Shape, colour: Color, glow: bool) {
    let r = shape.rect();
    let c = r.center();
    let fall = Paint::Radial { stops: Stops::new(&[(0.0, BLACK.with_alpha(0.0)), (0.5, BLACK.with_alpha(0.08)), (1.0, BLACK.with_alpha(0.48))]), center: c, rx: r.w * 0.60, ry: r.h * 0.95, focus: c };
    scene.fill_shape(shape, fall, 0.0, Blend::Over);
    let warm = colour.brighter(0.45);
    let hot = colour.mix(Color(0xfffff6e6), 0.80);
    let paint = Paint::Radial { stops: Stops::new(&[(0.0, hot.with_alpha(0.98)), (0.30, hot.mix(warm, 0.4).with_alpha(0.82)), (0.60, warm.with_alpha(0.40)), (0.85, warm.with_alpha(0.10)), (1.0, warm.with_alpha(0.0))]), center: c, rx: r.w * 0.50, ry: r.h * 0.72, focus: c };
    scene.fill_shape(shape, paint, 0.0, Blend::Over);
    if glow { spill(scene, shape, colour, 1.0); }
}

/// Bloom around a lit part: tinted wash, then additive light.
fn spill(scene: &mut Scene, shape: Shape, colour: Color, k: f32) {
    let r = shape.rect();
    let d = r.h.min(r.w) * 0.42;
    scene.fill_shape(shape.expanded(d * 0.9), Paint::Solid(colour.with_alpha(0.17 * k)), d * 1.15, Blend::Over);
    scene.fill_shape(shape.expanded(d), Paint::Solid(colour.with_alpha(0.38 * k)), d * 1.25, Blend::Add);
}

/// A soft neutral darkening on the panel round a lit part, drawn before the part: on a pale
/// panel it gives the bloom something to stand against; on a dark one it is all but invisible.
fn lit_surround(scene: &mut Scene, shape: Shape) {
    let d = shape.rect().h.min(shape.rect().w) * 0.22;
    scene.shadow(shape.expanded(d), d * 1.1, BLACK.with_alpha(0.20));
}

/// Centred text on a lit lens or tube: the run stamped a device pixel out at low alpha as a
/// halo, then the crisp glyphs.
fn halo_text(scene: &mut Scene, r: Rect, text: &str, ink: Color, halo: Color, size: f32) {
    let at = r.center();
    let d = (1.0 / crate::hardware::device_scale()).max(0.5);
    scene.glow_round(box_at(size * 0.55 * text.chars().count() as f32, size * 0.5, at).expanded(size * 0.6), size * 0.6, halo.with_alpha(0.16), size * 0.6, Blend::Add);
    for (dx, dy) in [(-d, 0.0), (d, 0.0), (0.0, -d), (0.0, d)] {
        scene.text(Point::new(at.x + dx, at.y + dy), text, halo.with_alpha(0.22), size, true);
    }
    scene.text(at, text, ink, size, true);
}

/// Octagon (see [`retro::chamfer`]) as a raised pad: eight bevel facets shaded from their
/// normals between `outer` and `inner`, lit edges picked out, and the flat top graded.
fn facet_pad(scene: &mut Scene, outer: &[Point], inner: &[Point], base: Color, top: (Color, Color), rig: &LightRig, gloss: f32) {
    let n = outer.len().min(inner.len());
    scene.fill_poly(outer.to_vec(), base.darker(0.4), base.darker(0.4));
    let mut face = Vec::with_capacity(n);
    for i in 0..n {
        let j = (i + 1) % n;
        let e = outer[j].sub(outer[i]);
        let len = (e.x * e.x + e.y * e.y).sqrt().max(1e-4);
        let (nx, ny) = (e.y / len, -e.x / len);
        let col = shade_col(rig, base, [nx * 0.95, ny * 0.95, 0.52], gloss);
        scene.fill_poly(vec![outer[i], outer[j], inner[j], inner[i]], col, col);
        face.push(nx * rig.screen_dir().x + ny * rig.screen_dir().y);
    }
    for i in 0..n { scene.line(outer[i], inner[i], BLACK.with_alpha(0.28), 0.5); }
    scene.fill_poly(inner.to_vec(), top.0, top.1);
    for i in 0..n {
        let j = (i + 1) % n;
        // Crease between facet and top: bright where the facet faces the light, dark opposite.
        if face[i] > 0.0 { scene.line(inner[i], inner[j], WHITE.with_alpha(0.34 * face[i]), 0.6); } else { scene.line(inner[i], inner[j], BLACK.with_alpha(0.30 * -face[i]), 0.6); }
    }
    scene.stroke_poly(outer.to_vec(), BLACK.with_alpha(0.7), 0.7, true);
}

/// Well for an octagonal pad: black floor, the lip away from the light catching it.
fn facet_well(scene: &mut Scene, path: &[Point], rig: &LightRig) {
    scene.fill_poly(path.to_vec(), Color(0xff030404), Color(0xff030404));
    let d = rig.screen_dir();
    for i in 0..path.len() {
        let (p, q) = (path[i], path[(i + 1) % path.len()]);
        let e = q.sub(p);
        let len = (e.x * e.x + e.y * e.y).sqrt().max(1e-4);
        let f = (e.y * d.x - e.x * d.y) / len;
        scene.line(p, q, if f < 0.0 { WHITE.with_alpha(0.26 * -f) } else { BLACK.with_alpha(0.6 * f) }, 0.8);
    }
}

/// Rectangular cap in a well: skirt, bevelled rim, dished or barrelled face.
fn cap_key(scene: &mut Scene, r: Rect, text: &str, radius: f32, base: Color, ink: Color, colour: Color, lit: bool, down: bool, glow: bool, travel: f32, concave: bool) {
    let rig = LightRig::studio();
    let la = lit_angle(&rig);
    if lit && glow { lit_surround(scene, Shape::Round(r.expanded(2.0), radius + 1.5)); }
    key_well(scene, r.expanded(2.0), radius + 1.5, &rig);
    // Light leaking past the cap into the well.
    if lit { scene.glow_round(r.expanded(2.0), radius + 1.5, colour.brighter(0.3).with_alpha(0.60), 2.6, Blend::Add); }
    let face = Rect::new(r.x, r.y + if down { travel * 0.75 } else { 0.0 }, r.w, r.h - travel);
    let cap = Rect::new(r.x, face.y, r.w, r.bottom() - face.y);
    drop_shadow(scene, Shape::Round(cap, radius), &rig, if down { 1.0 } else { 2.0 + travel }, if down { 0.5 } else { 0.7 });
    let body = if lit { colour.darker(0.34) } else { base };
    scene.fill_round_linear(cap, radius, Stops::new(&[(0.0, body.darker(0.45)), (1.0, body.darker(0.78))]), HALF_PI);
    // Rim bevel, then the face inside it.
    scene.fill_round_linear(face, radius, Stops::new(&[(0.0, body.brighter(0.55)), (0.45, body.brighter(0.05)), (1.0, body.darker(0.50))]), la);
    let bev = (face.h * 0.055).clamp(1.0, 2.2);
    let inner = face.reduced(bev, bev);
    let ir = (radius - bev).max(0.5);
    let stops = if concave { Stops::new(&[(0.0, body.darker(0.20)), (0.5, body.darker(0.04)), (1.0, body.brighter(0.16))]) } else { Stops::new(&[(0.0, body.brighter(0.22)), (0.5, body), (1.0, body.darker(0.26))]) };
    scene.fill_round_linear(inner, ir, stops, la);
    let c = inner.center();
    if concave {
        // Dish: the centre is deepest, so it is the darkest away from the lamp inside.
        let paint = Paint::Radial { stops: Stops::new(&[(0.0, BLACK.with_alpha(0.16)), (1.0, BLACK.with_alpha(0.0))]), center: c, rx: inner.w * 0.6, ry: inner.h * 0.75, focus: Point::new(c.x, c.y - inner.h * 0.1) };
        scene.fill_shape(Shape::Round(inner, ir), paint, 0.0, Blend::Over);
    }
    if lit { lamp_glow(scene, Shape::Round(inner, ir), colour, glow); }
    // Gloss: on a barrelled cap the side toward the light, on a dished one the far slope.
    let gh = inner.h * 0.36;
    let gy = if concave { inner.bottom() - gh - 0.5 } else { inner.y + 0.5 };
    scene.glow_round(Rect::new(inner.x + 1.0, gy, inner.w - 2.0, gh), ir.min(gh * 0.5), WHITE.with_alpha(if down { 0.06 } else if lit { 0.20 } else { 0.11 }), gh * 0.45, Blend::Screen);
    scene.stroke_round(face, radius, BLACK.with_alpha(0.45), 0.6);
    if down {
        scene.inner_shadow(Shape::Round(face, radius), rig.shadow_offset(travel + 2.0), 1.8, BLACK.with_alpha(if lit { 0.30 } else { 0.55 }));
        if !lit { scene.fill_round(face, radius, BLACK.with_alpha(0.10), BLACK.with_alpha(0.10), true); }
    }
    if lit {
        // The rim of the cap is thinnest: light leaks through it as a bright line.
        scene.stroke_round(face.reduced(0.35, 0.35), radius, colour.mix(WHITE, 0.62).with_alpha(0.80), 0.7);
        scene.stroke_round(inner, ir, colour.mix(WHITE, 0.4).with_alpha(0.28), 0.6);
    }
    text_c(scene, face, text, ink, key_size(face));
}

/// Black moulded ring with an inward chamfer, for round buttons.
fn dark_bezel(scene: &mut Scene, ring: Rect, hole: Rect, rig: &LightRig) {
    let la = lit_angle(rig);
    drop_shadow(scene, Shape::Ellipse(ring), rig, 2.0, 0.6);
    scene.fill_ellipse_linear(ring, Stops::new(&[(0.0, Color(0xff6a6f75)), (0.3, Color(0xff26292c)), (1.0, Color(0xff050606))]), la);
    let mid = Rect::new((ring.x + hole.x) * 0.5, (ring.y + hole.y) * 0.5, (ring.w + hole.w) * 0.5, (ring.h + hole.h) * 0.5);
    scene.fill_ellipse_linear(mid, Stops::new(&[(0.0, Color(0xff000000)), (0.6, Color(0xff141618)), (1.0, Color(0xff45494e))]), la);
    scene.fill_ellipse_solid(hole, Color(0xff030404));
    scene.inner_shadow(Shape::Ellipse(hole), rig.shadow_offset(2.0), 1.3, BLACK.with_alpha(0.9));
}

pub fn draw_push(scene: &mut Scene, r: Rect, text: &str, style: PushStyle, colour: Color, lit: bool, down: bool, glow: bool) {
    let rig = LightRig::studio();
    let la = lit_angle(&rig);
    let ld = rig.screen_dir();
    let off = if down { 2.0 } else { 0.0 };
    let face = r.translated(0.0, off);
    match style {
        PushStyle::Square => {
            let ink = if lit { Color(0xff0d0f11) } else { Color(0xff222a28) };
            cap_key(scene, r, text, 2.2, Color(0xffd0d3c3), ink, colour, lit, down, glow, 3.0, true);
        }
        PushStyle::Flat => {
            let ink = if lit { Color(0xff0a0c0e) } else { Color(0xffb8bfc2) };
            cap_key(scene, r, text, 1.8, Color(0xff32373b), ink, colour, lit, down, glow, 1.6, false);
        }
        PushStyle::Pill => {
            cap_key(scene, r, text, (r.h - 2.4) * 0.5, Color(0xffc8ccca), Color(0xff181c1e), colour, lit, down, glow, 2.4, false);
        }
        PushStyle::Round => {
            let d = r.w.min(r.h);
            let cir = box_at(d, d, r.center());
            let ring = cir.expanded(2.0);
            drop_shadow(scene, Shape::Ellipse(ring), &rig, 2.5, 0.6);
            scene.fill_ellipse_conic(ring, chrome_conic(Color(0xffffffff), Color(0xff5b6369)), conic_start(&rig));
            scene.stroke_ellipse(ring, Color(0xff101214), 0.6);
            scene.fill_ellipse_linear(cir.reduced(d * 0.035, d * 0.035), Stops::new(&[(0.0, Color(0xff3d4348)), (0.5, Color(0xff9aa2a8)), (1.0, Color(0xffffffff))]), la);
            let hole = cir.reduced(d * 0.085, d * 0.085);
            scene.fill_ellipse_solid(hole, Color(0xff040505));
            if lit {
                scene.stroke_ellipse(hole.reduced(d * 0.024, d * 0.024), colour, d * 0.05);
                scene.stroke_ellipse(hole.reduced(d * 0.024, d * 0.024), colour.brighter(0.6), d * 0.018);
                if glow {
                    scene.glow_ellipse(cir.expanded(d * 0.10), colour.with_alpha(0.16), d * 0.22, Blend::Over);
                    scene.glow_ellipse(cir.expanded(d * 0.10), colour.with_alpha(0.22), d * 0.24, Blend::Add);
                }
            } else {
                scene.inner_shadow(Shape::Ellipse(hole), rig.shadow_offset(2.0), 1.2, BLACK.with_alpha(0.9));
            }
            let cap = cir.reduced(d * 0.15, d * 0.15).translated(0.0, off * 0.6);
            let cc = cap.center();
            drop_shadow(scene, Shape::Ellipse(cap), &rig, if down { 0.8 } else { 3.0 }, if down { 0.45 } else { 0.65 });
            let dim = if down { 0.14 } else { 0.0 };
            scene.fill_ellipse_radial(cap, Stops::new(&[(0.0, WHITE.darker(dim)), (0.45, Color(0xffd0d5d8).darker(dim)), (0.85, Color(0xff838b91).darker(dim)), (1.0, Color(0xff4a5056).darker(dim))]), cc.add(ld.mul(cap.w * 0.24)));
            scene.stroke_ellipse(cap, BLACK.with_alpha(0.55), 0.6);
            scene.stroke_ellipse(cap.reduced(0.9, 0.9), WHITE.with_alpha(0.45), 0.6);
            // Finger dish.
            let dish = cap.reduced(cap.w * 0.15, cap.w * 0.15);
            scene.fill_ellipse_linear(dish, Stops::new(&[(0.0, Color(0xff8c949a).darker(dim)), (0.55, Color(0xffcfd4d7).darker(dim)), (1.0, Color(0xfff7f8f9).darker(dim))]), la);
            scene.inner_shadow(Shape::Ellipse(dish), rig.shadow_offset(1.2), 0.9, BLACK.with_alpha(0.35));
            if down { scene.inner_shadow(Shape::Ellipse(cap), rig.shadow_offset(3.0), 1.5, BLACK.with_alpha(0.5)); }
            text_c(scene, cap, text, Color(0xff1d2124), (cap.h * 0.40).min(13.0));
        }
        PushStyle::Dome => {
            let d = r.w.min(r.h);
            let cir = box_at(d, d, r.center()).translated(0.0, off * 0.5);
            if lit && glow { lit_surround(scene, Shape::Ellipse(box_at(d + 6.0, d + 6.0, r.center()))); }
            dark_bezel(scene, box_at(d + 6.0, d + 6.0, r.center()), box_at(d + 1.6, d + 1.6, r.center()), &rig);
            let c = cir.center();
            if lit {
                scene.fill_ellipse_radial(cir, Stops::new(&[(0.0, colour.brighter(0.92)), (0.30, colour.brighter(0.5)), (0.72, colour), (1.0, colour.darker(0.5))]), Point::new(c.x, c.y + d * 0.04));
            } else {
                let base = colour.darker(0.68);
                scene.fill_ellipse_radial(cir, Stops::new(&[(0.0, base.brighter(0.30)), (0.5, base), (1.0, base.darker(0.7))]), c.add(ld.mul(d * 0.2)));
            }
            scene.stroke_ellipse(cir, BLACK.with_alpha(0.6), 0.7);
            // Light through the lens gathers low on the side away from the key.
            scene.glow_ellipse(box_at(d * 0.62, d * 0.34, c.sub(ld.mul(d * 0.24))), colour.brighter(0.55).with_alpha(if lit { 0.40 } else { 0.16 }), d * 0.15, Blend::Screen);
            if down { scene.inner_shadow(Shape::Ellipse(cir), rig.shadow_offset(3.0), 1.6, BLACK.with_alpha(0.5)); }
            let gl = box_at(d * 0.50, d * 0.30, c.add(ld.mul(d * 0.25)));
            scene.glow_ellipse(gl, WHITE.with_alpha(if lit { 0.75 } else { 0.40 }), d * 0.11, Blend::Screen);
            if lit && glow {
                scene.glow_ellipse(cir.expanded(d * 0.34), colour.with_alpha(0.17), d * 0.44, Blend::Over);
                scene.glow_ellipse(cir.expanded(d * 0.38), colour.with_alpha(0.42), d * 0.50, Blend::Add);
            }
            text_c(scene, cir, text, if lit { Color(0xff0f1113) } else { Color(0xffeaeae4) }, (cir.h * 0.42).min(13.0));
        }
        PushStyle::Chamfer => {
            let ch = face.h * 0.28;
            let pad = face.reduced(1.0, 1.0);
            let approx = Shape::Round(pad, ch * 1.7);
            if lit && glow { lit_surround(scene, Shape::Round(r.expanded(2.0), ch * 1.7 + 2.0)); }
            facet_well(scene, &retro::chamfer(r.expanded(2.0), r.h * 0.28 + 1.0, false), &rig);
            if lit { scene.glow_round(r.expanded(1.5), ch * 1.7, colour.brighter(0.3).with_alpha(0.45), 2.4, Blend::Add); }
            drop_shadow(scene, approx, &rig, if down { 1.0 } else { 4.5 }, if down { 0.5 } else { 0.7 });
            drop_shadow(scene, approx, &rig, 0.8, 0.45);
            let base = if lit { colour.darker(0.22) } else { Color(0xff3a4047) };
            let bev = (face.h * 0.10).clamp(1.8, 3.4);
            let outer = retro::chamfer(pad, ch, false);
            let inner_r = pad.reduced(bev, bev);
            let ich = (ch - bev * 0.586).max(0.5);
            let inner = retro::chamfer(inner_r, ich, false);
            let k = if down { 0.02 } else { 0.14 };
            facet_pad(scene, &outer, &inner, base, (base.brighter(k + 0.06), base.darker(0.24)), &rig, 0.9);
            // The top is very slightly crowned: shade from the light across it.
            let top = Shape::Round(inner_r.reduced(0.4, 0.4), ich * 1.7);
            scene.fill_shape(top, Paint::Linear { stops: Stops::new(&[(0.0, WHITE.with_alpha(0.10)), (0.45, WHITE.with_alpha(0.0)), (1.0, BLACK.with_alpha(0.16))]), angle: la }, 0.0, Blend::Over);
            if lit { lamp_glow(scene, top, colour, glow); }
            let gh = inner_r.h * 0.34;
            scene.glow_round(Rect::new(inner_r.x + ich, inner_r.y + 0.6, inner_r.w - ich * 2.0, gh), gh * 0.5, WHITE.with_alpha(if down { 0.05 } else if lit { 0.20 } else { 0.11 }), gh * 0.45, Blend::Screen);
            // Coloured inlay line round the top.
            let inlay = retro::chamfer(inner_r.reduced(1.3, 1.3), (ich - 0.76).max(0.3), false);
            scene.stroke_poly(inlay, if lit { colour.mix(WHITE, 0.6).with_alpha(0.85) } else { colour.with_alpha(0.42) }, 0.8, true);
            if down { scene.inner_shadow(top, rig.shadow_offset(3.0), 1.6, BLACK.with_alpha(0.4)); }
            text_c(scene, face, text, if lit { Color(0xff090b0d) } else { Color(0xffdbe0e2) }, key_size(face));
        }
        PushStyle::Neon => {
            // A gloss-black pad with shallow bevels, and a neon tube laid round its top.
            let ch = face.h * 0.3;
            let pad = face.reduced(1.0, 1.0);
            let approx = Shape::Round(pad, ch * 1.7);
            if lit && glow { lit_surround(scene, Shape::Round(r.expanded(2.0), ch * 1.7 + 2.0)); }
            drop_shadow(scene, approx, &rig, if down { 0.8 } else { 3.5 }, if down { 0.5 } else { 0.7 });
            drop_shadow(scene, approx, &rig, 0.8, 0.5);
            let base = Color(0xff141b22);
            let bev = (face.h * 0.07).clamp(1.3, 2.4);
            let outer = retro::chamfer(pad, ch, false);
            let inner_r = pad.reduced(bev, bev);
            let ich = (ch - bev * 0.586).max(0.5);
            facet_pad(scene, &outer, &retro::chamfer(inner_r, ich, false), base, (Color(0xff0b1016), Color(0xff04070a)), &rig, 1.4);
            let top = Shape::Round(inner_r.reduced(0.4, 0.4), ich * 1.7);
            // Gloss: a reflection band across the upper part of the acrylic.
            scene.fill_shape(top, Paint::Linear { stops: Stops::new(&[(0.0, WHITE.with_alpha(0.10)), (0.30, WHITE.with_alpha(0.03)), (0.42, WHITE.with_alpha(0.0)), (1.0, WHITE.with_alpha(0.0))]), angle: la }, 0.0, Blend::Screen);
            let tube = retro::chamfer(inner_r.reduced(1.8, 1.8), (ich - 1.05).max(0.3), false);
            if lit {
                // The tube lights the acrylic inside it, strongest near the tube.
                let c = inner_r.center();
                let wash = Paint::Radial { stops: Stops::new(&[(0.0, colour.with_alpha(0.10)), (0.6, colour.with_alpha(0.20)), (1.0, colour.with_alpha(0.42))]), center: c, rx: inner_r.w * 0.55, ry: inner_r.h * 0.62, focus: c };
                scene.fill_shape(Shape::Round(inner_r.reduced(1.8, 1.8), ich * 1.7), wash, 0.0, Blend::Over);
                if glow { spill(scene, approx, colour, 0.75); }
            }
            retro::glow_stroke(scene, &tube, if lit { colour } else { colour.with_alpha(0.4) }, 1.4, if lit { 1.0 } else { 0.15 }, glow, true);
            if down { scene.inner_shadow(top, rig.shadow_offset(3.0), 1.6, BLACK.with_alpha(0.45)); }
            if lit {
                halo_text(scene, face, text, colour.brighter(0.85), colour, key_size(face));
            } else {
                text_c(scene, face, text, colour.darker(0.35).mix(Color(0xff8a949c), 0.35), key_size(face));
            }
        }
        PushStyle::Arcade => {
            let d = r.w.min(r.h);
            let cir = box_at(d, d, r.center());
            if lit && glow { lit_surround(scene, Shape::Ellipse(cir.expanded(2.0))); }
            dark_bezel(scene, cir.expanded(2.0), cir.reduced(d * 0.08, d * 0.08), &rig);
            let cap = cir.reduced(d * 0.12, d * 0.12).translated(0.0, off * 0.7);
            let cc = cap.center();
            let base = if lit { colour } else { colour.darker(0.55) };
            drop_shadow(scene, Shape::Ellipse(cap), &rig, if down { 0.8 } else { 3.5 }, if down { 0.5 } else { 0.7 });
            scene.fill_ellipse_radial(cap, Stops::new(&[(0.0, base.brighter(0.70)), (0.35, base.brighter(0.15)), (0.8, base.darker(0.25)), (1.0, base.darker(0.62))]), cc.add(ld.mul(cap.w * 0.30)));
            scene.stroke_ellipse(cap, BLACK.with_alpha(0.55), 0.6);
            // Concave plunger face inside the rolled rim.
            let dish = cap.reduced(cap.w * 0.13, cap.w * 0.13);
            scene.fill_ellipse_linear(dish, Stops::new(&[(0.0, base.darker(0.42)), (0.5, base.darker(0.08)), (1.0, base.brighter(0.30))]), la);
            if lit { lamp_glow(scene, Shape::Ellipse(dish), colour, glow); }
            scene.glow_ellipse(box_at(cap.w * 0.42, cap.h * 0.16, cc.add(ld.mul(cap.w * 0.40))), WHITE.with_alpha(0.55), cap.h * 0.07, Blend::Screen);
            if down { scene.inner_shadow(Shape::Ellipse(cap), rig.shadow_offset(3.0), 1.5, BLACK.with_alpha(0.5)); }
            text_c(scene, cap, text, if lit { Color(0xff0d0f11) } else { Color(0xffe6e2dc).with_alpha(0.85) }, (cap.h * 0.42).min(13.0));
        }
    }
}

/// Row of latching keys in one frame; the selected key is lit and stays down.
pub fn draw_key_bank(scene: &mut Scene, r: Rect, names: &[&str], selected: i32, style: PushStyle, colour: Color, glow: bool) {
    let n = names.len();
    if n == 0 { return; }
    let gap = 4.0;
    let w = (r.w - gap * (n - 1) as f32) / n as f32;
    let cell = |i: usize| Rect::new(r.x + i as f32 * (w + gap), r.y, w, r.h);
    let framed = matches!(style, PushStyle::Square | PushStyle::Flat);
    if framed {
        let rig = LightRig::studio();
        drop_shadow(scene, Shape::Round(r.expanded(3.5), 4.5), &rig, 1.5, 0.5);
        scene.fill_round_linear(r.expanded(3.5), 4.5, Stops::new(&[(0.0, Color(0xff4b4f54)), (0.3, Color(0xff1c1e21)), (1.0, Color(0xff060707))]), lit_angle(&rig));
        solid_round(scene, r.expanded(2.2), 3.5, Color(0xff050606));
    }
    // Unselected keys first, so the lit key's bloom lies over its neighbours.
    for (i, name) in names.iter().enumerate() {
        if i as i32 != selected { draw_push(scene, cell(i), name, style, colour, false, false, glow); }
    }
    for (i, name) in names.iter().enumerate() {
        if i as i32 == selected { draw_push(scene, cell(i), name, style, colour, true, true, glow); }
    }
}
