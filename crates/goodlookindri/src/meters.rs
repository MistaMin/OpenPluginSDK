// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Marcos Quirino Deida

//! Meters, lamps, lit keys and faders.
//!
//! Everything here takes its light from [`LightRig::studio`]: drop shadows fall down-right,
//! bevels are lit on the upper-left, recesses are shadowed on their upper-left inner wall.
//! Moving-coil meters (VU, PPM) sit in a recessed bezel behind two-layer glass, with the
//! needle pivot at the true arc centre below the window under a moulded cover.

use crate::color::Color;
use crate::light::LightRig;
use crate::scene::{Blend, Paint, Point, Rect, Scene, Shape, Stops};

fn lim(v: f32, lo: f32, hi: f32) -> f32 { v.max(lo).min(hi) }

fn box_at(w: f32, h: f32, c: Point) -> Rect {
    Rect::new(c.x - w * 0.5, c.y - h * 0.5, w, h)
}
fn ellipse_c(c: Point, rx: f32, ry: f32) -> Rect {
    Rect::new(c.x - rx, c.y - ry, rx * 2.0, ry * 2.0)
}
fn with_y(r: Rect, y: f32) -> Rect { Rect::new(r.x, y, r.w, r.h) }
fn with_h(r: Rect, h: f32) -> Rect { Rect::new(r.x, r.y, r.w, h) }
fn text_c(scene: &mut Scene, r: Rect, text: &str, color: Color, size: f32) {
    scene.text(r.center(), text, color, size, true);
}
fn hline(scene: &mut Scene, y: f32, x0: f32, x1: f32, color: Color, w: f32) {
    scene.line(Point::new(x0, y), Point::new(x1, y), color, w);
}
fn vline(scene: &mut Scene, x: f32, y0: f32, y1: f32, color: Color, w: f32) {
    scene.line(Point::new(x, y0), Point::new(x, y1), color, w);
}

const WHITE: Color = Color(0xffffffff);
const BLACK: Color = Color(0xff000000);
fn white(a: f32) -> Color { WHITE.with_alpha(a) }
fn black(a: f32) -> Color { BLACK.with_alpha(a) }
fn clear(c: Color) -> Color { c.with_alpha(0.0) }

fn rig() -> LightRig { LightRig::studio() }

/// Gradient angle for [`Paint::Linear`] that starts (t = 0) on the lit side and runs away from the key.
fn light_angle() -> f32 {
    let d = rig().screen_dir();
    (-d.y).atan2(-d.x)
}

/// `c` lit by the rig on a facet with normal `n`, relative to a flat facet (flat returns `c`).
/// `gain` exaggerates the difference so small bevels read at plugin sizes.
fn facet(c: Color, n: [f32; 3], gain: f32) -> Color {
    let rig = rig();
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
    let n = [n[0] / l, n[1] / l, n[2] / l];
    let s = rig.shade(n);
    let f = rig.shade([0.0, 0.0, 1.0]);
    let spec = rig.specular(n, 24.0) * 0.35 * gain;
    let ch = |v: u8, i: usize| {
        let k = (s[i] / f[i]).max(0.0).powf(gain);
        (v as f32 * k + spec * 255.0).clamp(0.0, 255.0).round() as u32
    };
    Color::rgba(ch(c.r(), 0), ch(c.g(), 1), ch(c.b(), 2), c.a() as u32)
}

/// Soft drop shadow of `shape` for something `height` above the panel.
fn drop_shadow(scene: &mut Scene, shape: Shape, height: f32, strength: f32) {
    let rig = rig();
    scene.shadow(shape.translated(rig.shadow_offset(height)), rig.shadow_sigma(height), black(strength));
}

/// Shadow cast by the rim of a recess `depth` deep onto its floor.
fn recess_shadow(scene: &mut Scene, shape: Shape, depth: f32, strength: f32) {
    let rig = rig();
    scene.inner_shadow(shape, rig.shadow_offset(depth), rig.shadow_sigma(depth), black(strength));
}

pub fn vu_position(amplitude_over_reference: f32) -> f32 {
    lim(amplitude_over_reference * 0.7079, 0.0, 1.0)
}
pub fn vu_position_for_db(vu: f32) -> f32 {
    vu_position(10f32.powf(vu / 20.0))
}
pub fn db_position(db: f32, floor_db: f32, ceil_db: f32) -> f32 {
    lim((db - floor_db) / (ceil_db - floor_db), 0.0, 1.0)
}
pub fn ladder_colour(db: f32) -> Color {
    if db > -3.0 { Color(0xffe5382a) } else if db > -12.0 { Color(0xfff0b323) } else { Color(0xff3ccf5a) }
}

// ---------------------------------------------------------------------------------------------
// Moving-coil meters
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VuFace { Cream, Black }

fn juce_arc(scene: &mut Scene, c: Point, rad: f32, from: f32, to: f32, color: Color, width: f32) {
    let k = -std::f32::consts::FRAC_PI_2;
    scene.stroke_arc(c, rad, from + k, to + k, color, width);
}

/// Deep black bezel raised off the panel with a chamfer down into the window.
/// Returns the window and its corner radius.
fn meter_bezel(scene: &mut Scene, r: Rect) -> (Rect, f32) {
    let la = light_angle();
    let m = r.h.min(r.w);
    let rad = (m * 0.035).clamp(2.5, 6.0);
    let b = (m * 0.055).clamp(4.0, 10.0);
    drop_shadow(scene, Shape::Round(r, rad), 3.0, 0.6);
    // Outer bevel: a lit upper-left edge, falling into shade bottom-right.
    scene.fill_round_linear(r, rad, Stops::new(&[(0.0, Color(0xff5c5f62)), (0.3, Color(0xff1d1e20)), (1.0, Color(0xff020202))]), la);
    let top = r.reduced(1.1, 1.1);
    scene.fill_round_linear(top, rad - 1.1, Stops::new(&[(0.0, Color(0xff27282b)), (0.5, Color(0xff131415)), (1.0, Color(0xff0a0a0b))]), la);
    let win = r.reduced(b, b);
    let wr = (rad - b * 0.5).max(1.5);
    // Chamfer into the window: the lower-right wall faces the key and catches it.
    let e = b * 0.45;
    scene.fill_round_linear(win.expanded(e), wr + e, Stops::new(&[(0.0, Color(0xff010101)), (0.55, Color(0xff0b0b0c)), (1.0, Color(0xff3a3c3f))]), la);
    scene.fill_round(win.expanded(0.7), wr + 0.7, BLACK, BLACK, true);
    (win, wr)
}

struct Mark<'a> { pos: f32, label: &'a str, red: bool }

struct DialSpec<'a> {
    /// Face radial stops when fully backlit, centre to corners.
    lit: [(f32, Color); 4],
    /// Face colour with the backlight off.
    paper: Color,
    glow: Color,
    ink: Color,
    red: Color,
    needle: Color,
    majors: &'a [Mark<'a>],
    minors: &'a [(f32, bool)],
    /// Position where the red zone band begins (it runs to 1).
    red_from: Option<f32>,
    percent: bool,
    signs: bool,
    title: &'a str,
    title_left: bool,
    num_size: f32,
    backlight: f32,
}

fn arc_band(pivot: Point, a0: f32, a1: f32, outer: f32, inner: impl Fn(f32) -> f32, n: usize) -> Vec<Point> {
    let pt = |a: f32, rad: f32| Point::new(pivot.x + a.sin() * rad, pivot.y - a.cos() * rad);
    let mut pts = Vec::with_capacity(n * 2 + 2);
    for i in 0..=n {
        let a = a0 + (a1 - a0) * i as f32 / n as f32;
        pts.push(pt(a, outer));
    }
    for i in (0..=n).rev() {
        let t = i as f32 / n as f32;
        pts.push(pt(a0 + (a1 - a0) * t, inner(t)));
    }
    pts
}

fn quad(p0: Point, ctrl: Point, p1: Point, n: i32) -> Vec<Point> {
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            Point::new(u * u * p0.x + 2.0 * u * t * ctrl.x + t * t * p1.x, u * u * p0.y + 2.0 * u * t * ctrl.y + t * t * p1.y)
        })
        .collect()
}

fn draw_dial(scene: &mut Scene, r: Rect, needle: f32, s: &DialSpec) {
    let rig = rig();
    let (f, fr) = meter_bezel(scene, r);
    let (w, h) = (f.w, f.h);
    let cx = f.center().x;
    let bl = lim(s.backlight, 0.0, 1.0);
    let shape = Shape::Round(f, fr);

    scene.clip_round(f, fr);
    // Face: paper lit from behind, a warm hotspot fading toward the corners.
    let st: Vec<(f32, Color)> = s.lit.iter().map(|(p, c)| (*p, s.paper.mix(*c, bl))).collect();
    let hot = Point::new(cx, f.y + h * 0.56);
    scene.fill_shape(shape, Paint::Radial { stops: Stops::new(&st), center: hot, rx: w * 0.66, ry: h * 0.98, focus: Point::new(cx, f.y + h * 0.48) }, 0.0, Blend::Over);
    if bl > 0.0 {
        scene.glow_ellipse(box_at(w * 0.62, h * 0.78, hot), s.glow.with_alpha(0.20 * bl), h * 0.36, Blend::Add);
    }

    // Arc geometry: the scale is an arc about a pivot below the window.
    let half = w * 0.40;
    let sag = h * 0.22;
    let mut radius = (half * half + sag * sag) / (2.0 * sag);
    let mut span = (half / radius).min(1.0).asin();
    if span > 0.72 {
        span = 0.72;
        radius = half / span.sin();
    }
    let arc_top = f.y + h * 0.36;
    let pivot = Point::new(cx, arc_top + radius);
    let ang = |p: f32| -span + 2.0 * span * p;
    let pt = |a: f32, rad: f32| Point::new(pivot.x + a.sin() * rad, pivot.y - a.cos() * rad);
    let line_w = (h * 0.008).max(0.8);
    let red_from = s.red_from.unwrap_or(1.0);

    // Scale: main arc, red zone band thickening toward full scale.
    juce_arc(scene, pivot, radius, ang(0.0), ang(red_from), s.ink, line_w);
    if s.red_from.is_some() {
        let thick = h * 0.05;
        let band = arc_band(pivot, ang(red_from), ang(1.0), radius + line_w * 0.5, |t| radius - line_w * 0.5 - thick * t.powf(0.8), 18);
        scene.fill_poly(band, s.red, s.red);
    }
    let tick_major = h * 0.075;
    let tick_minor = h * 0.042;
    for &(p, red) in s.minors {
        let a = ang(p);
        scene.line(pt(a, radius), pt(a, radius + tick_minor), if red { s.red } else { s.ink }, line_w * 0.9);
    }
    let ns = h * s.num_size;
    for m in s.majors {
        let a = ang(m.pos);
        let c = if m.red { s.red } else { s.ink };
        scene.line(pt(a, radius - line_w), pt(a, radius + tick_major), c, line_w * 1.35);
        scene.text(pt(a, radius + tick_major + ns * 0.72), m.label, c, ns, true);
    }
    // Signs just past the arc ends.
    if s.signs {
        let sign = ns * 0.95;
        scene.text(pt(ang(0.0) - 0.07, radius + h * 0.01), "-", s.ink, sign * 1.3, true);
        scene.text(pt(ang(1.0) + 0.07, radius + h * 0.01), "+", s.red, sign, true);
    }

    if s.percent {
        // Secondary scale: percent of reference voltage, dots on a smaller arc with tiny numerals.
        let r2 = radius - h * 0.10;
        for (i, lab) in ["0", "20", "40", "60", "80", "100"].iter().enumerate() {
            let p = vu_position(i as f32 * 0.2);
            let a = ang(p.max(0.012));
            let dot = h * 0.011;
            scene.fill_ellipse_solid(Scene::ellipse_at(pt(a, r2), dot), s.ink);
            scene.text(pt(a, r2 - h * 0.065), *lab, s.ink.with_alpha(0.85), h * 0.058, true);
        }
    }

    // Moulded cover over the movement; its top edge pokes into the window.
    let below = pivot.y - f.bottom();
    let cover_r = below + h * 0.13;
    let title_y = f.bottom() - h * 0.22;
    let title_x = if s.title_left { f.x + w * 0.15 } else { f.right() - w * 0.14 };
    scene.text(Point::new(title_x, title_y), s.title, s.ink, h * 0.125, true);

    // Needle: tapered, with a soft shadow whose offset grows toward the tip (parallax).
    let na = ang(lim(needle, 0.0, 1.0));
    let d = Point::new(na.sin(), -na.cos());
    let n = Point::new(na.cos(), na.sin());
    let r0 = cover_r - 2.0;
    let r1 = radius + tick_major + h * 0.02;
    let base = pivot.add(d.mul(r0));
    let tip = pivot.add(d.mul(r1));
    let (wb, wt) = ((h * 0.018).max(1.3), (h * 0.0045).max(0.45));
    let needle_poly = |b: Point, t: Point, kb: f32, kt: f32| vec![b.add(n.mul(wb * kb)), t.add(n.mul(wt * kt)), t.sub(n.mul(wt * kt)), b.sub(n.mul(wb * kb))];
    let (o0, o1) = (rig.shadow_offset(2.0), rig.shadow_offset(h * 0.07));
    let (o0s, o1s) = (o0.mul(1.4), o1.mul(1.5));
    scene.fill_poly(needle_poly(base.add(o0s), tip.add(o1s), 2.6, 5.5), black(0.035), black(0.035));
    scene.fill_poly(needle_poly(base.add(o0s.mul(0.85)), tip.add(o1s.mul(0.85)), 1.5, 3.0), black(0.05), black(0.05));
    scene.fill_poly(needle_poly(base.add(o0), tip.add(o1), 0.7, 1.3), black(0.18), black(0.18));
    scene.fill_poly(needle_poly(base, tip, 0.5, 0.5), s.needle, s.needle);

    // Cover: shadow onto the face, then the black dome with a lit rim.
    let cr = ellipse_c(pivot, cover_r, cover_r);
    drop_shadow(scene, Shape::Ellipse(cr), 3.5, 0.55);
    let foc = Point::new(pivot.x - cover_r * 0.18, pivot.y - cover_r * 0.9);
    scene.fill_ellipse_radial(cr, Stops::new(&[(0.0, Color(0xff3a3c3f)), (0.1, Color(0xff26272a)), (0.5, Color(0xff141516)), (1.0, Color(0xff0a0a0b))]), foc);
    let rim_a = (f.w * 0.5 / cover_r).min(1.0).asin();
    juce_arc(scene, pivot, cover_r - 0.6, -rim_a, rim_a * 0.4, white(0.22), 0.7);

    // The bezel's shadow on everything behind the glass.
    recess_shadow(scene, shape, 5.0, 0.85);
    recess_shadow(scene, shape, 11.0, 0.35);

    // Glass, two layers: a soft diagonal streak, a sharp lit edge; and a faint inner reflection.
    let streak = Stops::new(&[(0.0, white(0.0)), (0.16, white(0.0)), (0.25, white(0.13)), (0.33, white(0.04)), (0.42, white(0.0)), (1.0, white(0.0))]);
    scene.fill_shape(shape, Paint::Linear { stops: streak, angle: 0.62 }, 0.0, Blend::Screen);
    let sheen = Stops::new(&[(0.0, white(0.0)), (0.58, white(0.0)), (0.66, white(0.06)), (0.74, white(0.0)), (1.0, white(0.0))]);
    scene.fill_shape(shape, Paint::Linear { stops: sheen, angle: 0.62 }, 0.0, Blend::Screen);
    let mut refl = quad(Point::new(f.x + 2.0, f.y + h * 0.30), Point::new(cx, f.y + h * 0.05), Point::new(f.right() - 2.0, f.y + h * 0.30), 10);
    refl.push(Point::new(f.right() - 2.0, f.y + 2.0));
    refl.push(Point::new(f.x + 2.0, f.y + 2.0));
    scene.fill_poly(refl, white(0.05), white(0.0));
    let inset = (h * 0.035).max(2.0);
    let ra = if s.paper.perceived_brightness() > 0.5 { 1.0 } else { 0.45 };
    scene.stroke_poly(
        vec![Point::new(f.x + inset, f.bottom() - h * 0.14), Point::new(f.x + inset, f.bottom() - inset), Point::new(f.x + w * 0.22, f.bottom() - inset)],
        white(0.12 * ra),
        0.6,
        false,
    );
    scene.stroke_poly(
        vec![Point::new(f.right() - w * 0.22, f.bottom() - inset), Point::new(f.right() - inset, f.bottom() - inset), Point::new(f.right() - inset, f.bottom() - h * 0.14)],
        white(0.08 * ra),
        0.6,
        false,
    );
    scene.pop_clip();
    // Sharp glass edge catching the key along the top and left.
    hline(scene, f.y + 0.6, f.x + fr + 1.0, f.right() - fr - 1.0, white(0.30), 0.6);
    vline(scene, f.x + 0.6, f.y + fr + 1.0, f.bottom() - fr - 1.0, white(0.12), 0.6);
}

const VU_MAJORS: [Mark<'static>; 11] = [
    Mark { pos: 0.0708, label: "20", red: false },
    Mark { pos: 0.2239, label: "10", red: false },
    Mark { pos: 0.3162, label: "7", red: false },
    Mark { pos: 0.3981, label: "5", red: false },
    Mark { pos: 0.5012, label: "3", red: false },
    Mark { pos: 0.5623, label: "2", red: false },
    Mark { pos: 0.6310, label: "1", red: false },
    Mark { pos: 0.7079, label: "0", red: false },
    Mark { pos: 0.7943, label: "1", red: true },
    Mark { pos: 0.8913, label: "2", red: true },
    Mark { pos: 1.0, label: "3", red: true },
];
const VU_MINORS_DB: [f32; 9] = [-15.0, -6.0, -4.0, -2.5, -1.5, -0.5, 0.5, 1.5, 2.5];

/// Classic VU meter. `needle` is the 0..1 movement position (see [`vu_position`]).
pub fn draw_vu_meter(scene: &mut Scene, r: Rect, needle: f32, face: VuFace) {
    draw_vu_meter_lit(scene, r, needle, face, 1.0);
}

/// [`draw_vu_meter`] with the lamp behind the face at `backlight` 0 (off) ..1 (full).
pub fn draw_vu_meter_lit(scene: &mut Scene, r: Rect, needle: f32, face: VuFace, backlight: f32) {
    let minors: Vec<(f32, bool)> = VU_MINORS_DB.iter().map(|d| (vu_position_for_db(*d), *d > 0.0)).collect();
    let spec = match face {
        VuFace::Cream => DialSpec {
            lit: [(0.0, Color(0xfffff6dc)), (0.42, Color(0xfff8e2ac)), (0.78, Color(0xffe2b470)), (1.0, Color(0xffa8692e))],
            paper: Color(0xffcfc3a4),
            glow: Color(0xffffd8a0),
            ink: Color(0xff1d1914),
            red: Color(0xffd0261c),
            needle: Color(0xff15130f),
            majors: &VU_MAJORS,
            minors: &minors,
            red_from: Some(vu_position_for_db(0.0)),
            percent: true,
            signs: true,
            title: "VU",
            title_left: false,
            num_size: 0.10,
            backlight,
        },
        VuFace::Black => DialSpec {
            lit: [(0.0, Color(0xff3b2d1c)), (0.45, Color(0xff261d12)), (0.8, Color(0xff140f0a)), (1.0, Color(0xff070605))],
            paper: Color(0xff121110),
            glow: Color(0xffff9a3a),
            ink: Color(0xffffc27a),
            red: Color(0xffff5a32),
            needle: Color(0xffffb04e),
            majors: &VU_MAJORS,
            minors: &minors,
            red_from: Some(vu_position_for_db(0.0)),
            percent: true,
            signs: true,
            title: "VU",
            title_left: false,
            num_size: 0.10,
            backlight,
        },
    };
    draw_dial(scene, r, needle, &spec);
}

const PPM_MAJORS: [Mark<'static>; 7] = [
    Mark { pos: 0.0, label: "1", red: false },
    Mark { pos: 1.0 / 6.0, label: "2", red: false },
    Mark { pos: 2.0 / 6.0, label: "3", red: false },
    Mark { pos: 3.0 / 6.0, label: "4", red: false },
    Mark { pos: 4.0 / 6.0, label: "5", red: false },
    Mark { pos: 5.0 / 6.0, label: "6", red: false },
    Mark { pos: 1.0, label: "7", red: true },
];

/// Peak programme meter, marks 1..7. `level01` is the needle position.
pub fn draw_ppm(scene: &mut Scene, r: Rect, level01: f32) {
    draw_ppm_lit(scene, r, level01, 1.0);
}

/// [`draw_ppm`] with the face backlight at `backlight` 0..1.
pub fn draw_ppm_lit(scene: &mut Scene, r: Rect, level01: f32, backlight: f32) {
    let minors: Vec<(f32, bool)> = (0..6).map(|i| ((i as f32 + 0.5) / 6.0, i == 5)).collect();
    let spec = DialSpec {
        lit: [(0.0, Color(0xfffffffb)), (0.5, Color(0xfff1f1ea)), (0.82, Color(0xffd9dad0)), (1.0, Color(0xffa9aba2))],
        paper: Color(0xffc4c5bd),
        glow: Color(0xfffff0d8),
        ink: Color(0xff1b1d1c),
        red: Color(0xffc8261c),
        needle: Color(0xff121211),
        majors: &PPM_MAJORS,
        minors: &minors,
        red_from: Some(5.0 / 6.0),
        percent: false,
        signs: false,
        title: "PPM",
        title_left: true,
        num_size: 0.15,
        backlight,
    };
    draw_dial(scene, r, level01, &spec);
}

// ---------------------------------------------------------------------------------------------
// LED ladders
// ---------------------------------------------------------------------------------------------

/// One rectangular LED lens at `brightness` 0..1, moulded like a low barrel: a dark pocket
/// around it, shading toward its ends and lower edge, a specular line along the top shoulder.
/// Bloom is not drawn; see [`draw_led_bloom`].
pub fn draw_led(scene: &mut Scene, s: Rect, colour: Color, brightness: f32) {
    let k = lim(brightness, 0.0, 1.0);
    let c = colour.with_alpha(1.0);
    let thin = s.w.min(s.h);
    let rr = (thin * 0.22).clamp(0.6, 2.2);
    let down = std::f32::consts::FRAC_PI_2;
    let shape = Shape::Round(s, rr);
    // Pocket: a black gap all round (one stroke straddling the edge, drawn first).
    let g = (thin * 0.10).clamp(0.45, 0.9);
    scene.stroke_round(s.expanded(g * 0.5), rr + g * 0.5, Color(0xff010202), g + 0.6);
    // Off: dull translucent plastic, a little room light refracted toward the bottom.
    let off = Stops::new(&[(0.0, c.darker(0.60)), (0.30, c.darker(0.74)), (0.70, c.darker(0.82)), (0.92, c.darker(0.66)), (1.0, c.darker(0.86))]);
    scene.fill_round_linear(s, rr, off, down);
    if k > 0.0 {
        let on = Stops::new(&[(0.0, c.darker(0.22).with_alpha(k)), (0.20, c.mix(WHITE, 0.30).with_alpha(k)), (0.46, c.mix(WHITE, 0.06).with_alpha(k)), (0.80, c.darker(0.22).with_alpha(k)), (1.0, c.darker(0.58).with_alpha(k))]);
        scene.fill_round_linear(s, rr, on, down);
        // The die behind the lens: a hot core that falls off toward the ends.
        let core = s.reduced(s.w * 0.14, s.h * 0.16);
        scene.glow_round(core, rr, c.mix(Color(0xfffff6e0), 0.55).with_alpha(0.42 * k), core.w.min(core.h) * 0.48, Blend::Add);
    }
    // Barrel ends: the lens curves away along its long axis.
    let (e0, e1) = (if k > 0.0 { 0.30 } else { 0.42 }, if k > 0.0 { 0.38 } else { 0.50 });
    let ends = Stops::new(&[(0.0, black(e0)), (0.14, black(0.0)), (0.86, black(0.0)), (1.0, black(e1))]);
    scene.fill_shape(shape, Paint::Linear { stops: ends, angle: if s.w >= s.h { 0.0 } else { down } }, 0.0, Blend::Over);
    // Specular line on the top shoulder, and the dark lower edge of the moulding.
    let sh = (s.h * 0.17).clamp(0.5, 1.6);
    let spec = Rect::new(s.x + rr * 0.9 + s.w * 0.05, s.y + s.h * 0.13, (s.w - rr * 1.8 - s.w * 0.10).max(0.5), sh);
    scene.glow_round(spec, sh * 0.5, white(if k > 0.0 { 0.62 } else { 0.24 }), sh * 0.42, Blend::Screen);
}

/// Additive bloom around a lit LED, light spilling onto what surrounds it.
pub fn draw_led_bloom(scene: &mut Scene, s: Rect, colour: Color, brightness: f32) {
    let k = lim(brightness, 0.0, 1.0);
    if k <= 0.0 { return; }
    let e = (s.w.min(s.h) * 0.35).clamp(2.0, 5.0);
    // A tinted spill that also shows on pale panels, then the additive light.
    scene.glow_round(s.expanded(e), 2.0 + e, colour.with_alpha(0.10 * k), e * 1.6, Blend::Over);
    scene.glow_round(s.expanded(e), 2.0 + e, colour.with_alpha(0.20 * k), e * 1.6, Blend::Add);
}

pub fn draw_led_ladder(scene: &mut Scene, r: Rect, horizontal: bool, level_db: f32, peak_db: f32, segments: i32) {
    let segments = segments.max(1);
    let span = if horizontal { r.w } else { r.h };
    let gap = (span / segments as f32 * 0.17).clamp(1.5, 3.0);
    let seg_len = (span - gap * (segments - 1) as f32) / segments as f32;
    let peak_idx = (db_position(peak_db, -48.0, 0.0) * (segments - 1) as f32 + 0.5).floor();
    // Recessed channel the lenses sit in.
    let ch = Shape::Round(r.expanded(1.0), 2.0);
    scene.fill_shape(ch, Paint::Solid(Color(0xff040505)), 0.0, Blend::Over);
    let mut blooms = Vec::new();
    for i in 0..segments {
        let db = -48.0 + 48.0 * (i + 1) as f32 / segments as f32;
        let lit = level_db >= db - 48.0 / segments as f32 * 0.5 || i as f32 == peak_idx;
        let c = ladder_colour(db);
        let s = if horizontal {
            Rect::new(r.x + i as f32 * (seg_len + gap), r.y, seg_len, r.h)
        } else {
            Rect::new(r.x, r.bottom() - (i + 1) as f32 * seg_len - i as f32 * gap, r.w, seg_len)
        };
        let k = if !lit { 0.0 } else if level_db >= db - 1.5 || i as f32 == peak_idx { 1.0 } else { 0.72 };
        draw_led(scene, s, c, k);
        if k > 0.0 { blooms.push((s, c, k)); }
    }
    recess_shadow(scene, ch, 1.8, 0.8);
    for (s, c, k) in blooms { draw_led_bloom(scene, s, c, k); }
}

pub fn draw_stereo_ladder(scene: &mut Scene, r: Rect, db_l: f32, db_r: f32, peak_db: f32) {
    let la = light_angle();
    drop_shadow(scene, Shape::Round(r, 4.0), 2.0, 0.5);
    scene.fill_round_linear(r, 4.0, Stops::new(&[(0.0, Color(0xff3a3f42)), (0.35, Color(0xff15181a)), (1.0, Color(0xff050606))]), la);
    let face = r.reduced(1.2, 1.2);
    scene.fill_round_linear(face, 3.0, Stops::two(Color(0xff131617), Color(0xff0a0b0c)), std::f32::consts::FRAC_PI_2);
    recess_shadow(scene, Shape::Round(face, 3.0), 2.0, 0.6);
    let b = r.reduced(10.0, 8.0);
    let bar_w = 30.0f32.min(b.w * 0.26);
    let lb = Rect::new(b.x + b.w * 0.5 - bar_w - 16.0, b.y, bar_w, b.h);
    let rb = Rect::new(b.x + b.w * 0.5 + 16.0, b.y, bar_w, b.h);
    draw_led_ladder(scene, lb, false, db_l, peak_db, 24);
    draw_led_ladder(scene, rb, false, db_r, peak_db, 24);
    let ink = Color(0xffa3abaf);
    for d in [0, -6, -12, -24, -36, -48] {
        let y = b.bottom() - db_position(d as f32, -48.0, 0.0) * b.h;
        let yc = lim(y, b.y + 5.0, b.bottom() - 5.0);
        text_c(scene, box_at(30.0, 9.0, Point::new(b.center().x, yc)), &d.to_string(), ink, 7.0);
    }
    text_c(scene, with_h(with_y(lb, r.bottom() - 11.0), 10.0), "L", ink, 8.0);
    text_c(scene, with_h(with_y(rb, r.bottom() - 11.0), 10.0), "R", ink, 8.0);
}

/// Dark display window let into the panel: lip shaded by the rig, a graded floor.
fn screen_window(scene: &mut Scene, r: Rect, radius: f32, top: Color, bottom: Color) {
    let lip = Stops::new(&[(0.0, black(0.80)), (0.5, black(0.32)), (1.0, white(0.20))]);
    scene.fill_round_linear(r.expanded(1.1), radius + 1.1, lip, light_angle());
    scene.fill_round_linear(r, radius, Stops::two(top, bottom), std::f32::consts::FRAC_PI_2);
}
/// Glass over a [`screen_window`]: the panel's shadow on the contents, a reflection, a dark edge.
fn screen_glass(scene: &mut Scene, r: Rect, radius: f32) {
    let shape = Shape::Round(r, radius);
    recess_shadow(scene, shape, 3.2, 0.85);
    let g = Rect::new(r.x + 1.0, r.y + 1.0, (r.w - 2.0).max(0.0), r.h * 0.55);
    scene.fill_shape(Shape::Round(g, (radius - 1.0).max(0.0)), Paint::Linear { stops: Stops::new(&[(0.0, white(0.11)), (0.4, white(0.035)), (1.0, white(0.0))]), angle: std::f32::consts::FRAC_PI_2 }, 0.0, Blend::Screen);
    scene.stroke_round(r, radius, black(0.9), 1.0);
}

/// Arc of LED wedges that grow toward full scale, in a channel cut to their outline.
pub fn draw_segment_arc(scene: &mut Scene, r: Rect, level01: f32, lit: Color) {
    let n = 32;
    let rad = r.h * 1.05;
    let half = ((r.w * 0.46) / rad).min(0.98).asin();
    let centre = Point::new(r.center().x, r.bottom() + rad * 0.18);
    let at = |a: f32, d: f32| Point::new(centre.x + a.sin() * d, centre.y - a.cos() * d);
    let (r_in, r_out) = (rad * 0.70, |f: f32| rad * (0.82 + 0.16 * f));
    let da = half / n as f32;
    let red = Color(0xffe4412c);

    // Channel: the outline of the whole fan, grown a little, dark with a lit lower lip.
    let band = |grow: f32| -> Vec<Point> {
        let m = 24;
        let (a0, a1) = (-half - grow / rad, half + grow / rad);
        let mut pts: Vec<Point> = (0..=m).map(|i| { let t = i as f32 / m as f32; at(a0 + (a1 - a0) * t, r_out(t) + grow) }).collect();
        pts.extend((0..=m).rev().map(|i| at(a0 + (a1 - a0) * i as f32 / m as f32, r_in - grow)));
        pts
    };
    let o = rig().shadow_offset(1.2);
    scene.fill_poly(band(3.6).iter().map(|p| p.add(o)).collect(), white(0.13), white(0.13));
    scene.fill_poly(band(3.4), Color(0xff020303), Color(0xff0b0d0e));
    scene.stroke_poly(band(3.4), black(0.85), 0.8, true);

    let wedge = |i: usize| -> (Vec<Point>, f32) {
        let f = (i as f32 + 0.5) / n as f32;
        let a = (-1.0 + 2.0 * f) * half;
        let w = da * 0.62;
        (vec![at(a - w, r_in), at(a - w, r_out(f)), at(a + w, r_out(f)), at(a + w, r_in)], f)
    };
    let col_at = |f: f32| if f > 0.86 { red } else if f > 0.72 { lit.mix(Color(0xffffc530), 0.75) } else { lit };
    let on = |f: f32| f <= level01 + 0.5 / n as f32;
    // Unlit lenses.
    for i in 0..n {
        let (pts, f) = wedge(i);
        if on(f) { continue; }
        let c = col_at(f);
        scene.fill_poly(pts, c.darker(0.70), c.darker(0.84));
    }
    // Bloom under the lit run, then the lenses.
    for i in 0..n {
        let (_, f) = wedge(i);
        if !on(f) { continue; }
        let a = (-1.0 + 2.0 * f) * half;
        let mid = at(a, (r_in + r_out(f)) * 0.5);
        let len = r_out(f) - r_in;
        scene.glow_ellipse(box_at(len * 0.9 + 8.0, len * 0.9 + 8.0, mid), col_at(f).with_alpha(0.085), len * 0.45 + 4.0, Blend::Add);
    }
    for i in 0..n {
        let (pts, f) = wedge(i);
        if !on(f) { continue; }
        let c = col_at(f);
        let a = (-1.0 + 2.0 * f) * half;
        scene.fill_poly(pts, c.mix(WHITE, 0.22), c.darker(0.20));
        // Hot die line down the middle and a specular tick near the outer end.
        scene.line(at(a, r_in + 1.2), at(a, r_out(f) - 1.2), c.mix(Color(0xfffff6e0), 0.6).with_alpha(0.75), (rad * da * 0.34).max(0.5));
        scene.line(at(a - da * 0.3, r_out(f) - 1.0), at(a + da * 0.3, r_out(f) - 1.0), white(0.5), 0.6);
    }
    for i in 0..n {
        let (pts, _) = wedge(i);
        scene.stroke_poly(pts, black(0.55), 0.5, true);
    }
}

/// Fan of vector strokes on a phosphor screen behind glass.
pub fn draw_vector_fan(scene: &mut Scene, r: Rect, level01: f32, lit: Color) {
    let c = lit.with_alpha(1.0);
    screen_window(scene, r, 4.0, Color(0xff04120b).mix(c, 0.03), Color(0xff010503));
    scene.clip_round(r, 4.0);
    let b = r.reduced(8.0, 8.0);
    // Phosphor haze low on the screen, a faint graticule.
    scene.glow_ellipse(Rect::new(b.x - b.w * 0.1, b.y + b.h * 0.35, b.w * 1.2, b.h), c.with_alpha(0.07), b.h * 0.4, Blend::Add);
    for i in 1..4 {
        let y = (b.bottom() - b.h * i as f32 / 4.0).round() + 0.25;
        hline(scene, y, r.x, r.right(), c.with_alpha(0.07), 0.5);
    }
    let n = 26;
    let line = |i: usize| {
        let f = i as f32 / (n - 1) as f32;
        (b.x + f * b.w, b.h * (0.28 + 0.72 * f), f)
    };
    let on = |f: f32| f <= level01 + 0.5 / n as f32;
    for i in 0..n {
        let (x, hgt, f) = line(i);
        if !on(f) { vline(scene, x, b.bottom(), b.bottom() - hgt, c.with_alpha(0.13), 1.1); }
    }
    for i in 0..n {
        let (x, hgt, f) = line(i);
        if !on(f) { continue; }
        let bar = Rect::new(x - 0.6, b.bottom() - hgt, 1.2, hgt);
        scene.glow_round(bar.expanded(4.5), 5.0, c.with_alpha(0.085), 4.5, Blend::Add);
        scene.glow_round(bar.expanded(1.6), 2.2, c.with_alpha(0.16), 1.6, Blend::Add);
    }
    for i in 0..n {
        let (x, hgt, f) = line(i);
        if !on(f) { continue; }
        let bar = Rect::new(x - 0.65, b.bottom() - hgt, 1.3, hgt);
        scene.fill_round_linear(bar, 0.65, Stops::new(&[(0.0, c.mix(WHITE, 0.75)), (0.12, c.mix(WHITE, 0.35)), (1.0, c.darker(0.12))]), std::f32::consts::FRAC_PI_2);
        scene.glow_ellipse(box_at(4.4, 4.4, Point::new(x, b.bottom() - hgt)), c.mix(WHITE, 0.6).with_alpha(0.55), 1.8, Blend::Add);
    }
    hline(scene, b.bottom() + 1.5, b.x - 2.0, b.right() + 2.0, c.with_alpha(0.45), 0.7);
    scene.pop_clip();
    screen_glass(scene, r, 4.0);
    scene.stroke_round(r.reduced(0.5, 0.5), 3.5, c.with_alpha(0.16), 1.0);
}

/// One segment of a seven-segment digit as a pointed bar (`a`..`g`), slanted like a real display.
fn seg_poly(s: char, x0: f32, x1: f32, y0: f32, y1: f32, th: f32) -> (Vec<Point>, Point, Point) {
    let ym = (y0 + y1) * 0.5;
    let g = th * 0.16;
    let h = th * 0.5;
    let skew = |p: Point| Point::new(p.x + (ym - p.y) * 0.09, p.y);
    let bar = |a: Point, b: Point| -> (Vec<Point>, Point, Point) {
        let horiz = (a.y - b.y).abs() < 1e-3;
        let pts = if horiz {
            vec![Point::new(a.x, a.y), Point::new(a.x + h, a.y - h), Point::new(b.x - h, b.y - h), Point::new(b.x, b.y), Point::new(b.x - h, b.y + h), Point::new(a.x + h, a.y + h)]
        } else {
            vec![Point::new(a.x, a.y), Point::new(a.x + h, a.y + h), Point::new(b.x + h, b.y - h), Point::new(b.x, b.y), Point::new(b.x - h, b.y - h), Point::new(a.x - h, a.y + h)]
        };
        (pts.into_iter().map(skew).collect(), skew(a), skew(b))
    };
    let (xl, xr) = (x0 + h, x1 - h);
    let (yt, yb) = (y0 + h, y1 - h);
    match s {
        'a' => bar(Point::new(xl + g, yt), Point::new(xr - g, yt)),
        'b' => bar(Point::new(xr, yt + g), Point::new(xr, ym - g)),
        'c' => bar(Point::new(xr, ym + g), Point::new(xr, yb - g)),
        'd' => bar(Point::new(xl + g, yb), Point::new(xr - g, yb)),
        'e' => bar(Point::new(xl, ym + g), Point::new(xl, yb - g)),
        'f' => bar(Point::new(xl, yt + g), Point::new(xl, ym - g)),
        _ => bar(Point::new(xl + g, ym), Point::new(xr - g, ym)),
    }
}

/// Seven-segment readout: pointed, slanted segments with a hot core and additive bloom; with
/// `background` it sits in a recessed window behind glass and shows the unlit segments as ghosts.
/// `lit`'s alpha dims the whole display.
pub fn draw_seven_segment(scene: &mut Scene, r: Rect, text: &str, lit: Color, background: bool) {
    const SEGS: [&str; 10] = ["abcdef", "bc", "abged", "abgcd", "fgbc", "afgcd", "afgedc", "abc", "abcdefg", "abcdfg"];
    let mut cells = String::new();
    let mut dots = Vec::new();
    for ch in text.chars() {
        if ch == '.' && !dots.is_empty() { *dots.last_mut().unwrap() = true; continue; }
        cells.push(ch);
        dots.push(false);
    }
    let n = cells.chars().count();
    if n == 0 { return; }
    let dim = lit.alpha_f();
    let c = lit.with_alpha(1.0);
    if background {
        screen_window(scene, r, 3.0, Color(0xff0c0a08).mix(c, 0.035), Color(0xff040303));
        scene.glow_ellipse(r.reduced(r.w * 0.06, r.h * 0.12), c.with_alpha(0.05 * dim), r.h * 0.35, Blend::Add);
    }
    let b = r.reduced(r.h * 0.16, r.h * 0.16);
    // Digits keep a display's proportions (pitch no more than 0.8 of the height) and the group
    // is centred.
    let ch = b.h;
    let cw = (b.w / n as f32).min(ch * 0.88);
    let b = Rect::new(b.center().x - cw * n as f32 * 0.5, b.y, cw * n as f32, b.h);
    let th = (cw * 0.17).min(ch * 0.115).max(0.8);
    let mut on_segs: Vec<(Vec<Point>, Point, Point)> = Vec::new();
    let mut dot_at = Vec::new();
    for (i, cc) in cells.chars().enumerate() {
        let dw = cw * 0.76;
        let xc = b.x + (i as f32 + 0.5) * cw;
        let (x0, x1) = (xc - dw * 0.5, xc + dw * 0.5);
        let on = if cc.is_ascii_digit() { SEGS[cc as usize - '0' as usize] } else if cc == '-' || cc == '+' { "g" } else { "" };
        for s in ['a', 'b', 'c', 'd', 'e', 'f', 'g'] {
            let seg = seg_poly(s, x0, x1, b.y, b.bottom(), th);
            if on.contains(s) { on_segs.push(seg); } else if background { scene.fill_poly(seg.0, c.with_alpha(0.07 * dim), c.with_alpha(0.07 * dim)); }
        }
        let dp = Point::new(x1 + th * 0.75 - ch * 0.045, b.bottom() - th * 0.5);
        if dots[i] { dot_at.push(dp); } else if background { scene.fill_ellipse_solid(box_at(th, th, dp), c.with_alpha(0.07 * dim)); }
    }
    // Bloom for every lit segment first, so no halo lies over a neighbour's core.
    let halo = (th * 2.2).clamp(1.6, 7.0);
    for (_, a, e) in &on_segs {
        let bb = Rect::new(a.x.min(e.x), a.y.min(e.y), (a.x - e.x).abs(), (a.y - e.y).abs());
        scene.glow_round(bb.expanded(halo + th * 0.3), halo + th * 0.3, c.with_alpha(0.15 * dim), halo, Blend::Add);
        scene.glow_round(bb.expanded(th * 1.1), th * 1.1, c.with_alpha(0.20 * dim), th * 0.9, Blend::Add);
    }
    for p in &dot_at { scene.glow_ellipse(box_at(th * 4.6, th * 4.6, *p), c.with_alpha(0.30 * dim), th * 1.8, Blend::Add); }
    let core = c.mix(Color(0xfffff4e0), 0.62);
    for (pts, a, e) in on_segs {
        scene.fill_poly(pts, c.mix(WHITE, 0.16).with_alpha(dim), c.darker(0.10).with_alpha(dim));
        let d = e.sub(a);
        let inset = th * 0.45 / (d.x * d.x + d.y * d.y).sqrt().max(1e-3);
        scene.line(a.add(d.mul(inset)), e.sub(d.mul(inset)), core.with_alpha(0.80 * dim), (th * 0.32).max(0.45));
    }
    for p in dot_at {
        scene.fill_ellipse_solid(box_at(th * 1.15, th * 1.15, p), c.mix(WHITE, 0.16).with_alpha(dim));
        scene.fill_ellipse_solid(box_at(th * 0.5, th * 0.5, p), core.with_alpha(0.8 * dim));
    }
    if background { screen_glass(scene, r, 3.0); }
}

// ---------------------------------------------------------------------------------------------
// Lamps and lit keys
// ---------------------------------------------------------------------------------------------

/// Ring around a jewel lamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LampBezel { Chrome, Black, Flush }

/// Jewel lamp in a chrome bezel; see [`draw_jewel`].
pub fn draw_lamp(scene: &mut Scene, c: Point, radius: f32, colour: Color, on: bool) {
    draw_jewel(scene, c, radius, colour, if on { 1.0 } else { 0.0 }, bezel_for(radius));
}

fn bezel_for(_radius: f32) -> LampBezel { LampBezel::Chrome }

/// Jewel lamp: a lens of `radius` at `c` with radial shading and a rim, a hot core and
/// additive bloom spilling onto the panel when lit (`brightness` 0..1), a dull translucent
/// lens when off. The bezel adds about 0.36 × `radius` around the lens.
pub fn draw_jewel(scene: &mut Scene, c: Point, radius: f32, colour: Color, brightness: f32, bezel: LampBezel) {
    let la = light_angle();
    let k = lim(brightness, 0.0, 1.0);
    let col = colour.with_alpha(1.0);
    let r = radius.max(1.0);
    // Light spilling onto the panel around the fitting.
    if k > 0.0 {
        // The tinted spill shows on pale panels too, where added light alone washes out.
        scene.glow_ellipse(ellipse_c(c, r * 3.2, r * 3.2), col.with_alpha(0.16 * k), r * 2.3, Blend::Over);
        scene.glow_ellipse(ellipse_c(c, r * 3.6, r * 3.6), col.with_alpha(0.16 * k), r * 2.8, Blend::Screen);
    }
    let ring = if bezel == LampBezel::Flush { r * 1.08 } else { r * 1.38 };
    drop_shadow(scene, Shape::Ellipse(ellipse_c(c, ring, ring)), (r * 0.3).clamp(1.0, 4.0), 0.65);
    match bezel {
        LampBezel::Chrome => {
            let rr = ellipse_c(c, ring, ring);
            let chrome = Stops::new(&[
                (0.0, Color(0xffe2e6e8)),
                (0.18, Color(0xff6c7276)),
                (0.40, Color(0xfff6f8f9)),
                (0.62, Color(0xff474c50)),
                (0.84, Color(0xffc6cbce)),
                (1.0, Color(0xffe2e6e8)),
            ]);
            scene.fill_ellipse_conic(rr, chrome, -2.3);
            scene.fill_ellipse_linear(rr, Stops::new(&[(0.0, white(0.30)), (0.5, white(0.0)), (1.0, black(0.45))]), la);
            scene.stroke_ellipse(rr, black(0.45), 0.6);
            // Concave seat down to the lens: dark where it faces away from the key.
            let seat = ellipse_c(c, r * 1.14, r * 1.14);
            scene.fill_ellipse_linear(seat, Stops::new(&[(0.0, Color(0xff2a2d30)), (0.6, Color(0xff6a7074)), (1.0, Color(0xffd5d9db))]), la);
        }
        LampBezel::Black => {
            let rr = ellipse_c(c, ring, ring);
            scene.fill_ellipse_linear(rr, Stops::new(&[(0.0, Color(0xff4a4d50)), (0.35, Color(0xff1a1b1d)), (1.0, Color(0xff060606))]), la);
            let seat = ellipse_c(c, r * 1.14, r * 1.14);
            scene.fill_ellipse_linear(seat, Stops::new(&[(0.0, Color(0xff050505)), (1.0, Color(0xff2e3033))]), la);
        }
        LampBezel::Flush => {}
    }
    let lens = ellipse_c(c, r, r);
    scene.fill_ellipse_solid(ellipse_c(c, r * 1.04, r * 1.04), Color(0xff060606));
    // Off: dark translucent glass, the light it gathers focused low and right inside it.
    if k < 1.0 {
        let off = Stops::new(&[(0.0, col.darker(0.45).mix(Color(0xff3a3a3a), 0.15)), (0.45, col.darker(0.68)), (1.0, col.darker(0.88))]);
        scene.fill_ellipse_radial(lens, off, Point::new(c.x + r * 0.28, c.y + r * 0.34));
    }
    if k > 0.0 {
        let a = k;
        let hot = col.mix(Color(0xfffff2dc), 0.82);
        let on = Stops::new(&[
            (0.0, hot.with_alpha(a)),
            (0.2, col.mix(hot, 0.4).with_alpha(a)),
            (0.48, col.with_alpha(a)),
            (0.8, col.darker(0.32).with_alpha(a)),
            (1.0, col.darker(0.6).with_alpha(a)),
        ]);
        scene.fill_ellipse_radial(lens, on, Point::new(c.x + r * 0.04, c.y + r * 0.06));
    }
    // Faceted ring inside the jewel and a dark rim at the edge of the glass.
    scene.stroke_ellipse(ellipse_c(c, r * 0.68, r * 0.68), if k > 0.0 { col.mix(WHITE, 0.5).with_alpha(0.22 * k) } else { white(0.06) }, (r * 0.05).max(0.5));
    scene.stroke_ellipse(ellipse_c(c, r * 0.96, r * 0.96), col.darker(0.75).with_alpha(0.7), (r * 0.08).max(0.6));
    if k > 0.0 {
        scene.glow_ellipse(ellipse_c(c, r * 0.42, r * 0.42), col.mix(Color(0xfffff2dc), 0.7).with_alpha(0.55 * k), r * 0.4, Blend::Add);
    }
    // Specular: sharp window of the key at the upper-left, a broad soft sheen, a bounce at the lower-right.
    scene.glow_ellipse(ellipse_c(Point::new(c.x - r * 0.12, c.y - r * 0.38), r * 0.62, r * 0.40), white(0.16), r * 0.35, Blend::Screen);
    scene.glow_ellipse(ellipse_c(Point::new(c.x - r * 0.33, c.y - r * 0.44), r * 0.30, r * 0.17), white(0.85), r * 0.12, Blend::Over);
    scene.glow_ellipse(ellipse_c(Point::new(c.x + r * 0.42, c.y + r * 0.48), r * 0.22, r * 0.13), white(0.18), r * 0.1, Blend::Screen);
    if k > 0.0 {
        scene.glow_ellipse(ellipse_c(c, r * 2.0, r * 2.0), col.with_alpha(0.30 * k), r * 1.3, Blend::Add);
    }
}

/// Lit key: a translucent cap lit from inside, light spilling out at its edges and into the
/// well around it. Unlit, a milky cap tinted by the lamp colour.
pub fn draw_lit_key(scene: &mut Scene, r: Rect, text: &str, lit: bool, lamp: Color, down: bool) {
    let la = light_angle();
    let down_ang = std::f32::consts::FRAC_PI_2;
    let lamp = lamp.with_alpha(1.0);
    // Well.
    let well = r.expanded(1.5);
    let wr = 3.2;
    scene.fill_round_linear(well.expanded(0.8), wr + 0.8, Stops::new(&[(0.0, Color(0xff050606)), (1.0, Color(0xff3a3f42))]), la);
    scene.fill_round(well, wr, Color(0xff0b0d0e), Color(0xff0b0d0e), true);
    if lit {
        scene.glow_round(well, wr, lamp.with_alpha(0.55), 3.0, Blend::Add);
    }
    recess_shadow(scene, Shape::Round(well, wr), 2.5, 0.8);
    let height = if down { 0.8 } else { 2.6 };
    let face = r.reduced(0.6, 0.6).translated(0.0, if down { 0.8 } else { 0.0 });
    let fr = 2.2;
    let cap = Shape::Round(face, fr);
    drop_shadow(scene, cap, height, if lit { 0.35 } else { 0.7 });
    if lit {
        // Edges glow hottest: light runs through the side walls of the cap.
        scene.fill_shape(cap, Paint::Solid(lamp.darker(0.38)), 0.0, Blend::Over);
        let hot = lamp.mix(Color(0xfffff6e6), 0.7);
        let inner = Stops::new(&[(0.0, hot), (0.35, lamp.mix(hot, 0.35)), (0.75, lamp), (1.0, lamp.darker(0.1))]);
        let ip = Paint::Radial { stops: inner, center: face.center(), rx: face.w * 0.6, ry: face.h * 0.75, focus: Point::new(face.center().x, face.y + face.h * 0.42) };
        scene.fill_shape(cap.expanded(-1.0), ip, (face.h * 0.28).max(2.0), Blend::Over);
        scene.stroke_round(face.reduced(0.4, 0.4), fr, lamp.mix(WHITE, 0.75).with_alpha(0.9), 0.8);
    } else {
        let base = lamp.darker(0.5).mix(Color(0xff767a78), 0.5);
        let st = Stops::new(&[(0.0, base.brighter(0.18)), (0.5, base), (1.0, base.darker(0.35))]);
        scene.fill_round_linear(face, fr, st, down_ang);
        // Light from the room collects in the lower half of the translucent body.
        let low = Rect::new(face.x + 1.5, face.y + face.h * 0.5, face.w - 3.0, face.h * 0.45);
        scene.glow_round(low, fr, lamp.with_alpha(0.14), low.h * 0.45, Blend::Screen);
        scene.stroke_round(face.reduced(0.4, 0.4), fr, white(0.10), 0.7);
    }
    // Gloss on the upper half of the cap.
    let gloss = Rect::new(face.x + 1.0, face.y + 0.8, face.w - 2.0, face.h * 0.45);
    scene.fill_round_linear(gloss, fr, Stops::new(&[(0.0, white(if lit { 0.38 } else { 0.22 })), (1.0, white(0.0))]), down_ang);
    hline(scene, face.y + 0.7, face.x + fr, face.right() - fr, white(0.55), 0.6);
    scene.stroke_round(face, fr, black(0.45), 0.6);
    let ink = if lit { lamp.darker(0.78) } else { Color(0xffe2e6e3).with_alpha(0.85) };
    text_c(scene, face, text, ink, face.h * 0.40);
    if lit {
        scene.glow_round(face.expanded(5.0), fr + 5.0, lamp.with_alpha(0.10), 7.0, Blend::Over);
        scene.glow_round(face.expanded(5.0), fr + 5.0, lamp.with_alpha(0.22), 7.0, Blend::Add);
    }
}

// ---------------------------------------------------------------------------------------------
// Faders
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaderCap { Oval, Console }

/// Fader in a slot; vertical when `r` is taller than wide, with a printed 0..10 scale.
pub fn draw_slot_fader(scene: &mut Scene, r: Rect, prop01: f32, cap: FaderCap, cap_colour: Color) {
    draw_fader(scene, r, prop01, cap, cap_colour, r.h > r.w, true);
}

/// Fader with an explicit orientation. `prop01` 1 is the top (vertical) or the right end.
/// `scale` prints ticks on both sides of the slot with numerals on the left (or above).
pub fn draw_fader(scene: &mut Scene, r: Rect, prop01: f32, cap: FaderCap, cap_colour: Color, vertical: bool, scale: bool) {
    draw_fader_on(scene, r, prop01, cap, cap_colour, vertical, scale, Color(0xff14181b));
}

/// [`draw_fader`] on a panel of colour `panel`: the scale is printed in dark ink on a pale
/// panel and pale ink on a dark one, and the slot's lit lip is toned to match.
pub fn draw_fader_on(scene: &mut Scene, r: Rect, prop01: f32, cap: FaderCap, cap_colour: Color, vertical: bool, scale: bool, panel: Color) {
    let pale = panel.perceived_brightness() > 0.5;
    let (len, wid) = if vertical { (r.h, r.w) } else { (r.w, r.h) };
    // Along / across → screen rect.
    let rect = |a0: f32, c0: f32, la: f32, lc: f32| if vertical { Rect::new(r.x + c0, r.y + a0, lc, la) } else { Rect::new(r.x + a0, r.y + c0, la, lc) };
    let pnt = |a: f32, c: f32| if vertical { Point::new(r.x + c, r.y + a) } else { Point::new(r.x + a, r.y + c) };
    let (cap_along, cap_across) = match cap {
        FaderCap::Console => ((wid * 0.62).max(8.0), wid * 0.86),
        FaderCap::Oval => ((wid * 0.52).max(7.0), wid * 0.74),
    };
    let t0 = cap_along * 0.5 + 1.0;
    let t1 = (len - cap_along * 0.5 - 1.0).max(t0);
    let along = |p: f32| { let p = lim(p, 0.0, 1.0); if vertical { t1 - (t1 - t0) * p } else { t0 + (t1 - t0) * p } };
    let mid = wid * 0.5;

    // Slot: a lower-lip highlight peeking out below / right, the black slot over it, inner shadow.
    let sw = (wid * 0.16).clamp(3.0, 6.0);
    let slot = rect(t0 - sw * 0.5, mid - sw * 0.5, (t1 - t0) + sw, sw);
    let sr = sw * 0.5;
    scene.stroke_round(slot.translated(0.4, 0.8), sr, white(if pale { 0.75 } else { 0.26 }), 0.9);
    if pale { scene.stroke_round(slot.translated(-0.3, -0.6), sr, black(0.30), 0.8); }
    scene.fill_round(slot, sr, Color(0xff030404), Color(0xff030404), true);
    recess_shadow(scene, Shape::Round(slot, sr), 2.5, 0.9);
    let rail_a = if vertical { (slot.x + slot.w * 0.5, slot.y + sr, slot.x + slot.w * 0.5, slot.bottom() - sr) } else { (slot.x + sr, slot.y + slot.h * 0.5, slot.right() - sr, slot.y + slot.h * 0.5) };
    scene.line(Point::new(rail_a.0, rail_a.1), Point::new(rail_a.2, rail_a.3), white(0.06), 0.7);

    if scale {
        let ink = if pale { Color(0xff16191b).with_alpha(0.88) } else { Color(0xffd3dade).with_alpha(0.86) };
        let tw = if pale { 0.8 } else { 0.75 };
        let g = sw * 0.5 + wid * 0.06;
        let (maj, min) = (wid * 0.15, wid * 0.08);
        let ns = (wid * 0.19).clamp(5.0, 8.0);
        for i in 0..=10 {
            let a = along(i as f32 / 10.0);
            let l = if i % 2 == 0 { maj } else { min };
            scene.line(pnt(a, mid + g), pnt(a, mid + g + l), ink, tw);
            scene.line(pnt(a, mid - g), pnt(a, mid - g - l), ink, tw);
            if i % 2 == 0 && wid >= 20.0 {
                scene.text(pnt(a, mid - g - maj - ns * 0.62), &i.to_string(), ink, ns, true);
            }
        }
    }

    let pos = along(prop01);
    let cr = rect(pos - cap_along * 0.5, mid - cap_across * 0.5, cap_along, cap_across);
    let base = cap_colour.with_alpha(1.0);
    // Normals: "front" faces the low end of travel, "back" the high end.
    let (n_lo, n_hi, n_l, n_r) = if vertical {
        ([0.0, -0.75, 0.66], [0.0, 0.75, 0.66], [-0.75, 0.0, 0.66], [0.75, 0.0, 0.66])
    } else {
        ([-0.75, 0.0, 0.66], [0.75, 0.0, 0.66], [0.0, -0.75, 0.66], [0.0, 0.75, 0.66])
    };
    let along_ang = if vertical { std::f32::consts::FRAC_PI_2 } else { 0.0 };
    let across_ang = if vertical { 0.0 } else { std::f32::consts::FRAC_PI_2 };
    match cap {
        FaderCap::Console => {
            let rad = (cap_across.min(cap_along) * 0.12).clamp(1.2, 3.0);
            drop_shadow(scene, Shape::Round(cr, rad), 7.0, 0.55);
            drop_shadow(scene, Shape::Round(cr, rad), 1.8, 0.45);
            // Side facets along the travel axis, a flat top between them.
            let lo = facet(base, n_lo, 2.2);
            let hi = facet(base, n_hi, 2.2);
            let top = base;
            let st = Stops::new(&[(0.0, lo), (0.22, lo.mix(top, 0.3)), (0.25, top.brighter(0.04)), (0.75, top.darker(0.08)), (0.78, hi.mix(top, 0.3)), (1.0, hi)]);
            scene.fill_round_linear(cr, rad, st, along_ang);
            let sl = facet(base, n_l, 2.2);
            let sr2 = facet(base, n_r, 2.2);
            let side = Stops::new(&[(0.0, sl), (0.07, clear(sl)), (0.93, clear(sr2)), (1.0, sr2)]);
            scene.fill_round_linear(cr, rad, side, across_ang);
            // Grip ridges on the flat top, either side of the index line.
            let span_a = cap_along * 0.5;
            let ridge_from = cap_across * 0.14;
            let ridge_to = cap_across * 0.86;
            for j in [-3, -2, -1, 1, 2, 3] {
                let a = pos + span_a * 0.15 * j as f32;
                let p0 = pnt(a, mid - cap_across * 0.5 + ridge_from);
                let p1 = pnt(a, mid - cap_across * 0.5 + ridge_to);
                scene.line(p0, p1, black(0.28), 0.6);
                let o = if vertical { Point::new(0.0, 0.6) } else { Point::new(0.6, 0.0) };
                scene.line(p0.add(o), p1.add(o), white(0.30), 0.5);
            }
            // Index line.
            let p0 = pnt(pos, mid - cap_across * 0.5 + cap_across * 0.06);
            let p1 = pnt(pos, mid + cap_across * 0.5 - cap_across * 0.06);
            let o = if vertical { Point::new(0.0, 0.7) } else { Point::new(0.7, 0.0) };
            scene.line(p0.add(o), p1.add(o), black(0.45), 1.2);
            scene.line(p0, p1, if base.perceived_brightness() > 0.6 { Color(0xff1a1c1d) } else { WHITE }, 1.1);
            scene.stroke_round(cr.reduced(0.5, 0.5), rad, white(0.18), 0.6);
            scene.stroke_round(cr, rad, black(0.55), 0.6);
        }
        FaderCap::Oval => {
            let rad = cap_along.min(cap_across) * 0.5;
            drop_shadow(scene, Shape::Round(cr, rad), 6.0, 0.55);
            drop_shadow(scene, Shape::Round(cr, rad), 1.6, 0.45);
            let foc = Point::new(cr.x + cr.w * 0.38, cr.y + cr.h * 0.32);
            let st = Stops::new(&[(0.0, base.mix(WHITE, 0.55)), (0.35, base.brighter(0.08)), (0.8, base.darker(0.28)), (1.0, base.darker(0.5))]);
            scene.fill_round_radial(cr, rad, st, foc);
            let p0 = pnt(pos, mid - cap_across * 0.5 + rad * 0.7);
            let p1 = pnt(pos, mid + cap_across * 0.5 - rad * 0.7);
            let o = if vertical { Point::new(0.0, 0.6) } else { Point::new(0.6, 0.0) };
            scene.line(p0.add(o), p1.add(o), white(0.4), 0.8);
            scene.line(p0, p1, if base.perceived_brightness() > 0.6 { Color(0xff2a2c2d) } else { WHITE }, 1.0);
            let hl = Rect::new(cr.x + cr.w * 0.18, cr.y + cr.h * 0.12, cr.w * 0.38, cr.h * 0.26);
            scene.glow_round(hl, hl.h * 0.5, white(0.55), hl.h * 0.45, Blend::Screen);
            scene.stroke_round(cr, rad, black(0.45), 0.6);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_counts_stay_sane() {
        let mut s = Scene::new();
        draw_vu_meter(&mut s, Rect::new(0.0, 0.0, 220.0, 140.0), 0.6, VuFace::Cream);
        assert!(s.cmds.len() < 200, "vu: {}", s.cmds.len());
        s.clear();
        draw_lamp(&mut s, Point::new(20.0, 20.0), 10.0, Color(0xffff3b4a), true);
        assert!(s.cmds.len() < 30, "lamp: {}", s.cmds.len());
        s.clear();
        draw_stereo_ladder(&mut s, Rect::new(0.0, 0.0, 70.0, 220.0), -8.0, -14.0, -3.0);
        assert!(s.cmds.len() < 400, "ladder: {}", s.cmds.len());
    }
}
