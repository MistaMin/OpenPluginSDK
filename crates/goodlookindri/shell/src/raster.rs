//! Software replay of [`goodlookindri::scene::Cmd`].
//!
//! Every shape is a signed-distance function; coverage is `0.5 - d` clamped to 0..1, which gives
//! one pixel of anti-aliasing with no supersampling. Slow for very large scenes, but it has no
//! dependencies and no requirement beyond the standard library.
//!
//! The 3D 2.0 commands reuse the same distance functions: shadows map distance through
//! ½·erfc(d / (σ√2)), feathered fills through a smoothstep. All blending is in sRGB bytes, like
//! the original commands, so old and new layers stack predictably.

use crate::text::{self, Glyph, GlyphCache};
use goodlookindri::color::Color;
use goodlookindri::scene::{Blend, Cmd, Image, Paint, Point, Rect, Scene, Shape, Stops};

/// Opaque `0x00RRGGBB` pixels, top row first.
pub struct Framebuffer {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
    glyphs: GlyphCache,
}

#[derive(Clone, Copy)]
struct Clip { cx: f32, cy: f32, hx: f32, hy: f32, r: f32 }

enum Fill {
    Solid(Color),
    Grad { top: Color, bottom: Color, vertical: bool, bx: f32, by: f32, inv_v: f32, inv_d: f32 },
}

impl Fill {
    fn grad(top: Color, bottom: Color, vertical: bool, bx: f32, by: f32, bw: f32, bh: f32) -> Fill {
        if top == bottom { return Fill::Solid(top); }
        Fill::Grad { top, bottom, vertical, bx, by, inv_v: 1.0 / bh.max(1e-3), inv_d: 1.0 / (bw + bh).max(1e-3) }
    }
    fn at(&self, x: f32, y: f32) -> Color {
        match self {
            Fill::Solid(c) => *c,
            Fill::Grad { top, bottom, vertical, bx, by, inv_v, inv_d } => {
                let t = if *vertical { (y - by) * inv_v } else { ((x - bx) + (y - by)) * inv_d };
                top.mix(*bottom, t)
            }
        }
    }
}

fn sd_round(px: f32, py: f32, cx: f32, cy: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let r = r.min(hx).min(hy).max(0.0);
    let qx = (px - cx).abs() - (hx - r);
    let qy = (py - cy).abs() - (hy - r);
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

fn sd_ellipse(px: f32, py: f32, cx: f32, cy: f32, a: f32, b: f32) -> f32 {
    let (a, b) = (a.max(1e-3), b.max(1e-3));
    let (dx, dy) = ((px - cx) / a, (py - cy) / b);
    ((dx * dx + dy * dy).sqrt() - 1.0) * a.min(b)
}

fn seg_dist(px: f32, py: f32, a: Point, b: Point) -> f32 {
    let (vx, vy) = (b.x - a.x, b.y - a.y);
    let (wx, wy) = (px - a.x, py - a.y);
    let l2 = vx * vx + vy * vy;
    let t = if l2 > 0.0 { ((wx * vx + wy * vy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (dx, dy) = (wx - vx * t, wy - vy * t);
    (dx * dx + dy * dy).sqrt()
}

fn poly_dist(pts: &[Point], closed: bool, px: f32, py: f32) -> f32 {
    let n = pts.len();
    let mut d = f32::MAX;
    let last = if closed { n } else { n.saturating_sub(1) };
    for i in 0..last { d = d.min(seg_dist(px, py, pts[i], pts[(i + 1) % n])); }
    d
}

fn poly_signed(pts: &[Point], px: f32, py: f32) -> f32 {
    let n = pts.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (pts[i], pts[j]);
        if (a.y > py) != (b.y > py) && px < (b.x - a.x) * (py - a.y) / (b.y - a.y) + a.x { inside = !inside; }
        j = i;
    }
    let d = poly_dist(pts, true, px, py);
    if inside { -d } else { d }
}

/// A [`Shape`] in device pixels.
#[derive(Clone, Copy)]
struct Sdf { ellipse: bool, cx: f32, cy: f32, hx: f32, hy: f32, r: f32 }

impl Sdf {
    fn new(shape: Shape, s: f32) -> Sdf {
        let (rect, ellipse, r) = match shape { Shape::Ellipse(r) => (r, true, 0.0), Shape::Round(r, k) => (r, false, k) };
        Sdf { ellipse, cx: (rect.x + rect.w * 0.5) * s, cy: (rect.y + rect.h * 0.5) * s, hx: rect.w * 0.5 * s, hy: rect.h * 0.5 * s, r: r * s }
    }
    fn dist(&self, px: f32, py: f32) -> f32 {
        if self.ellipse { sd_ellipse(px, py, self.cx, self.cy, self.hx, self.hy) } else { sd_round(px, py, self.cx, self.cy, self.hx, self.hy, self.r) }
    }
    fn bb(&self, pad: f32) -> [f32; 4] { [self.cx - self.hx - pad, self.cy - self.hy - pad, self.cx + self.hx + pad, self.cy + self.hy + pad] }
}

/// A [`Paint`] resolved to device pixels.
enum Shader {
    Solid(Color),
    Linear { stops: Stops, ox: f32, oy: f32, dx: f32, dy: f32 },
    /// Focal radial in the unit space of the ellipse: `e` is centre − focus, `a` = |e|² − 1.
    Radial { stops: Stops, fx: f32, fy: f32, irx: f32, iry: f32, ex: f32, ey: f32, a: f32 },
    Conic { stops: Stops, cx: f32, cy: f32, start: f32 },
}

impl Shader {
    fn new(paint: &Paint, sh: &Sdf, s: f32) -> Shader {
        match *paint {
            Paint::Solid(c) => Shader::Solid(c),
            Paint::Linear { stops, angle } => {
                let (c, n) = (angle.cos(), angle.sin());
                let he = (c.abs() * sh.hx + n.abs() * sh.hy).max(1e-3);
                Shader::Linear { stops, ox: sh.cx - c * he, oy: sh.cy - n * he, dx: c / (2.0 * he), dy: n / (2.0 * he) }
            }
            Paint::Radial { stops, center, rx, ry, focus } => {
                let (irx, iry) = (1.0 / (rx * s).max(1e-3), 1.0 / (ry * s).max(1e-3));
                let (mut ex, mut ey) = ((center.x - focus.x) * s * irx, (center.y - focus.y) * s * iry);
                let l = (ex * ex + ey * ey).sqrt();
                if l > 0.98 { ex *= 0.98 / l; ey *= 0.98 / l; }
                let (fx, fy) = (center.x * s - ex / irx, center.y * s - ey / iry);
                Shader::Radial { stops, fx, fy, irx, iry, ex, ey, a: ex * ex + ey * ey - 1.0 }
            }
            Paint::Conic { stops, center, start } => Shader::Conic { stops, cx: center.x * s, cy: center.y * s, start },
        }
    }
    fn at(&self, px: f32, py: f32) -> Color {
        match self {
            Shader::Solid(c) => *c,
            Shader::Linear { stops, ox, oy, dx, dy } => stops.at((px - ox) * dx + (py - oy) * dy),
            Shader::Radial { stops, fx, fy, irx, iry, ex, ey, a } => {
                let (qx, qy) = ((px - fx) * irx, (py - fy) * iry);
                let de = qx * ex + qy * ey;
                stops.at((de - (de * de - a * (qx * qx + qy * qy)).max(0.0).sqrt()) / a)
            }
            Shader::Conic { stops, cx, cy, start } => stops.at(((py - cy).atan2(px - cx) - start).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU),
        }
    }
}

/// erfc for the shadow falloff (Abramowitz and Stegun 7.1.27, error under 5e-4, no `exp`).
fn erfc(x: f32) -> f32 {
    let z = x.abs();
    let p = 1.0 + z * (0.278393 + z * (0.230389 + z * (0.000972 + z * 0.078108)));
    let p2 = p * p;
    let r = 1.0 / (p2 * p2);
    if x >= 0.0 { r } else { 2.0 - r }
}

fn smooth(t: f32) -> f32 { let t = t.clamp(0.0, 1.0); t * t * (3.0 - 2.0 * t) }

fn bounds(pts: &[Point]) -> [f32; 4] {
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for p in pts { b = [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)]; }
    b
}

impl Framebuffer {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, pixels: vec![0; width * height], glyphs: GlyphCache::default() }
    }

    pub fn clear(&mut self, c: Color) {
        let v = c.0 & 0x00ff_ffff;
        for p in self.pixels.iter_mut() { *p = v; }
    }

    /// PPM (P6) bytes, for tests and CI artefacts.
    pub fn to_ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        for p in &self.pixels { out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]); }
        out
    }

    /// Paints `scene`. Scene units are multiplied by `scale` (the backing scale of the display).
    pub fn replay(&mut self, scene: &Scene, scale: f32) {
        let s = scale;
        let mut clips: Vec<Clip> = Vec::new();
        for cmd in &scene.cmds {
            match cmd {
                Cmd::FillEllipse { rect, top, bottom, diagonal } => {
                    let (x, y, w, h) = (rect.x * s, rect.y * s, rect.w * s, rect.h * s);
                    let fill = Fill::grad(*top, *bottom, !*diagonal, x, y, w, h);
                    let (cx, cy, a, b) = (x + w * 0.5, y + h * 0.5, w * 0.5, h * 0.5);
                    self.draw(&clips, [x, y, x + w, y + h], |px, py| sd_ellipse(px, py, cx, cy, a, b), &fill);
                }
                Cmd::StrokeEllipse { rect, color, width } => {
                    let (x, y, w, h) = (rect.x * s, rect.y * s, rect.w * s, rect.h * s);
                    let hw = (width * s).max(1.0) * 0.5;
                    let (cx, cy, a, b) = (x + w * 0.5, y + h * 0.5, w * 0.5, h * 0.5);
                    self.draw(&clips, [x - hw, y - hw, x + w + hw, y + h + hw], |px, py| sd_ellipse(px, py, cx, cy, a, b).abs() - hw, &Fill::Solid(*color));
                }
                Cmd::FillRoundRect { rect, radius, top, bottom, vertical } => {
                    let (x, y, w, h) = (rect.x * s, rect.y * s, rect.w * s, rect.h * s);
                    let fill = Fill::grad(*top, *bottom, *vertical, x, y, w, h);
                    let (cx, cy, hx, hy, r) = (x + w * 0.5, y + h * 0.5, w * 0.5, h * 0.5, radius * s);
                    self.draw(&clips, [x, y, x + w, y + h], |px, py| sd_round(px, py, cx, cy, hx, hy, r), &fill);
                }
                Cmd::FillRect { rect, top, bottom, vertical } => {
                    let (x, y, w, h) = (rect.x * s, rect.y * s, rect.w * s, rect.h * s);
                    let fill = Fill::grad(*top, *bottom, *vertical, x, y, w, h);
                    let (cx, cy, hx, hy) = (x + w * 0.5, y + h * 0.5, w * 0.5, h * 0.5);
                    self.draw(&clips, [x, y, x + w, y + h], |px, py| sd_round(px, py, cx, cy, hx, hy, 0.0), &fill);
                }
                Cmd::StrokeRoundRect { rect, radius, color, width } => {
                    let (x, y, w, h) = (rect.x * s, rect.y * s, rect.w * s, rect.h * s);
                    let hw = (width * s).max(1.0) * 0.5;
                    let (cx, cy, hx, hy, r) = (x + w * 0.5, y + h * 0.5, w * 0.5, h * 0.5, radius * s);
                    self.draw(&clips, [x - hw, y - hw, x + w + hw, y + h + hw], |px, py| sd_round(px, py, cx, cy, hx, hy, r).abs() - hw, &Fill::Solid(*color));
                }
                Cmd::Line { a, b, color, width } => {
                    let (a, b) = (a.mul(s), b.mul(s));
                    let hw = (width * s).max(1.0) * 0.5;
                    let bb = bounds(&[a, b]);
                    self.draw(&clips, [bb[0] - hw, bb[1] - hw, bb[2] + hw, bb[3] + hw], |px, py| seg_dist(px, py, a, b) - hw, &Fill::Solid(*color));
                }
                Cmd::FillPoly { pts, top, bottom, diagonal } => {
                    if pts.len() < 3 { continue; }
                    let pts: Vec<Point> = pts.iter().map(|p| p.mul(s)).collect();
                    let bb = bounds(&pts);
                    let fill = Fill::grad(*top, *bottom, !*diagonal, bb[0], bb[1], bb[2] - bb[0], bb[3] - bb[1]);
                    self.draw(&clips, bb, |px, py| poly_signed(&pts, px, py), &fill);
                }
                Cmd::StrokePoly { pts, color, width, closed } => {
                    if pts.len() < 2 { continue; }
                    let pts: Vec<Point> = pts.iter().map(|p| p.mul(s)).collect();
                    let hw = (width * s).max(1.0) * 0.5;
                    let bb = bounds(&pts);
                    self.draw(&clips, [bb[0] - hw, bb[1] - hw, bb[2] + hw, bb[3] + hw], |px, py| poly_dist(&pts, *closed, px, py) - hw, &Fill::Solid(*color));
                }
                Cmd::StrokeArc { center, radius, from, to, color, width } => {
                    let (c, r) = (center.mul(s), radius * s);
                    let steps = (((to - from).abs() * r / 2.0).ceil() as usize).clamp(4, 512);
                    let pts: Vec<Point> = (0..=steps)
                        .map(|i| {
                            let a = from + (to - from) * i as f32 / steps as f32;
                            Point::new(c.x + r * a.cos(), c.y + r * a.sin())
                        })
                        .collect();
                    let hw = (width * s).max(1.0) * 0.5;
                    let bb = bounds(&pts);
                    self.draw(&clips, [bb[0] - hw, bb[1] - hw, bb[2] + hw, bb[3] + hw], |px, py| poly_dist(&pts, false, px, py) - hw, &Fill::Solid(*color));
                }
                Cmd::Text { at, text, color, size, center } => {
                    let px = text::quantise(size * s);
                    let (pens, total) = text::pen_positions(text, px);
                    let x0 = if *center { at.x * s - total * 0.5 } else { at.x * s };
                    let baseline = at.y * s + px * text::MIDDLE;
                    for (c, pen) in pens {
                        let g = self.glyphs.get(c, px);
                        let ox = (x0 + pen).round() as i64 + g.left as i64;
                        let oy = (baseline - g.top as f32).round() as i64;
                        blit(&mut self.pixels, self.width, self.height, &clips, g, ox, oy, *color);
                    }
                }
                Cmd::ClipRound { rect, radius } => {
                    clips.push(Clip { cx: (rect.x + rect.w * 0.5) * s, cy: (rect.y + rect.h * 0.5) * s, hx: rect.w * 0.5 * s, hy: rect.h * 0.5 * s, r: radius * s });
                }
                Cmd::PopClip => { clips.pop(); }
                Cmd::Fill { shape, paint, feather, blend } => {
                    let sh = Sdf::new(*shape, s);
                    let shader = Shader::new(paint, &sh, s);
                    let (f, mode) = (feather * s, *blend);
                    self.shade(&clips, sh.bb(0.0), |dst, px, py, cc| {
                        let d = sh.dist(px, py);
                        let cov = if f > 1.0 { smooth(-d / f) } else { (0.5 - d).clamp(0.0, 1.0) } * cc;
                        if cov > 0.0 { compose(dst, shader.at(px, py), cov, mode); }
                    });
                }
                Cmd::Shadow { shape, sigma, color } => {
                    let sh = Sdf::new(*shape, s);
                    let sg = (sigma * s).max(0.3);
                    let k = std::f32::consts::FRAC_1_SQRT_2 / sg;
                    self.shade(&clips, sh.bb(sg * 3.5), |dst, px, py, cc| darken(dst, *color, 0.5 * erfc(sh.dist(px, py) * k) * cc));
                }
                Cmd::InnerShadow { shape, offset, sigma, color } => {
                    let sh = Sdf::new(*shape, s);
                    let moved = Sdf::new(shape.translated(*offset), s);
                    let k = std::f32::consts::FRAC_1_SQRT_2 / (sigma * s).max(0.3);
                    self.shade(&clips, sh.bb(0.0), |dst, px, py, cc| {
                        let mask = (0.5 - sh.dist(px, py)).clamp(0.0, 1.0) * cc;
                        if mask > 0.0 { darken(dst, *color, mask * 0.5 * erfc(-moved.dist(px, py) * k)); }
                    });
                }
                Cmd::Image { img, dest, opacity } => self.image(&clips, img, *dest, *opacity, s),
                _ => {}
            }
        }
    }

    /// Visits every pixel centre in `bb` (plus one pixel) that the clips do not hide entirely,
    /// with the clip coverage there.
    fn shade<F: FnMut(&mut u32, f32, f32, f32)>(&mut self, clips: &[Clip], bb: [f32; 4], mut f: F) {
        let x0 = ((bb[0].floor() as i64) - 1).max(0) as usize;
        let y0 = ((bb[1].floor() as i64) - 1).max(0) as usize;
        let x1 = (((bb[2].ceil() as i64) + 1).max(0) as usize).min(self.width);
        let y1 = (((bb[3].ceil() as i64) + 1).max(0) as usize).min(self.height);
        for y in y0..y1 {
            let py = y as f32 + 0.5;
            for x in x0..x1 {
                let px = x as f32 + 0.5;
                let cc = clip_coverage(clips, px, py);
                if cc > 0.0 { f(&mut self.pixels[y * self.width + x], px, py, cc); }
            }
        }
    }

    fn image(&mut self, clips: &[Clip], img: &Image, dest: Rect, opacity: f32, s: f32) {
        let op = opacity.clamp(0.0, 1.0);
        let (x, y, w, h) = (dest.x * s, dest.y * s, dest.w * s, dest.h * s);
        let (iw, ih) = (img.w as usize, img.h as usize);
        if op <= 0.0 || iw == 0 || ih == 0 || !(w > 0.0 && h > 0.0) { return; }
        let whole = |v: f32| (v - v.round()).abs() < 1e-3;
        if whole(x) && whole(y) && (w - iw as f32).abs() < 1e-3 && (h - ih as f32).abs() < 1e-3 {
            let (ox, oy) = (x.round() as i64, y.round() as i64);
            for j in 0..ih {
                let py = oy + j as i64;
                if py < 0 || py >= self.height as i64 { continue; }
                for i in 0..iw {
                    let px = ox + i as i64;
                    if px < 0 || px >= self.width as i64 { continue; }
                    let k = op * clip_coverage(clips, px as f32 + 0.5, py as f32 + 0.5);
                    if k > 0.0 { over_pre(&mut self.pixels[py as usize * self.width + px as usize], unpack(img.px[j * iw + i]), k); }
                }
            }
            return;
        }
        let (su, sv) = (iw as f32 / w, ih as f32 / h);
        let (cx, cy, hx, hy) = (x + w * 0.5, y + h * 0.5, w * 0.5, h * 0.5);
        self.shade(clips, [x, y, x + w, y + h], |dst, px, py, cc| {
            let k = (0.5 - sd_round(px, py, cx, cy, hx, hy, 0.0)).clamp(0.0, 1.0) * cc * op;
            if k > 0.0 { over_pre(dst, bilinear(img, (px - x) * su - 0.5, (py - y) * sv - 0.5), k); }
        });
    }

    fn draw<D: Fn(f32, f32) -> f32>(&mut self, clips: &[Clip], bb: [f32; 4], dist: D, fill: &Fill) {
        let x0 = ((bb[0].floor() as i64) - 1).max(0) as usize;
        let y0 = ((bb[1].floor() as i64) - 1).max(0) as usize;
        let x1 = (((bb[2].ceil() as i64) + 1).max(0) as usize).min(self.width);
        let y1 = (((bb[3].ceil() as i64) + 1).max(0) as usize).min(self.height);
        for y in y0..y1 {
            let py = y as f32 + 0.5;
            for x in x0..x1 {
                let px = x as f32 + 0.5;
                let cov = (0.5 - dist(px, py)).clamp(0.0, 1.0) * clip_coverage(clips, px, py);
                if cov <= 0.0 { continue; }
                blend(&mut self.pixels[y * self.width + x], fill.at(px, py), cov);
            }
        }
    }
}

/// Product of the coverage of every active clip at a point.
fn clip_coverage(clips: &[Clip], px: f32, py: f32) -> f32 {
    let mut cov = 1.0;
    for c in clips {
        cov *= (0.5 - sd_round(px, py, c.cx, c.cy, c.hx, c.hy, c.r)).clamp(0.0, 1.0);
        if cov <= 0.0 { break; }
    }
    cov
}

/// Source-over blend of `col` at `cov` into one opaque pixel.
fn blend(dst: &mut u32, col: Color, cov: f32) {
    let a = col.alpha_f() * cov;
    if a <= 0.0 { return; }
    let mix = |s: u8, d: u32| (d as f32 + (s as f32 - d as f32) * a).round() as u32;
    let (dr, dg, db) = ((*dst >> 16) & 255, (*dst >> 8) & 255, *dst & 255);
    *dst = (mix(col.r(), dr) << 16) | (mix(col.g(), dg) << 8) | mix(col.b(), db);
}

/// [`blend`] for [`Blend::Over`]; `Add` and `Screen` only brighten.
fn compose(dst: &mut u32, col: Color, cov: f32, mode: Blend) {
    let a = col.alpha_f() * cov;
    let ch = |s: u8, d: u32| -> u32 {
        let (s, d) = (s as f32, d as f32);
        let v = match mode { Blend::Add => d + s * a, _ => d + (255.0 - d) * s / 255.0 * a };
        v.round().min(255.0) as u32
    };
    match mode {
        Blend::Over => blend(dst, col, cov),
        _ => {
            if a <= 0.0 { return; }
            let (dr, dg, db) = ((*dst >> 16) & 255, (*dst >> 8) & 255, *dst & 255);
            *dst = (ch(col.r(), dr) << 16) | (ch(col.g(), dg) << 8) | ch(col.b(), db);
        }
    }
}

/// Multiplies the pixel toward `col` by `cov` times its alpha: black darkens evenly and keeps
/// the hue underneath; a tinted shadow filters like coloured glass.
fn darken(dst: &mut u32, col: Color, cov: f32) {
    let a = col.alpha_f() * cov;
    if a <= 0.0 { return; }
    let ch = |s: u8, d: u32| (d as f32 * (1.0 - a * (1.0 - s as f32 / 255.0))).round() as u32;
    let (dr, dg, db) = ((*dst >> 16) & 255, (*dst >> 8) & 255, *dst & 255);
    *dst = (ch(col.r(), dr) << 16) | (ch(col.g(), dg) << 8) | ch(col.b(), db);
}

/// Premultiplied `0xAARRGGBB` as `[a, r, g, b]` in 0..255.
fn unpack(p: u32) -> [f32; 4] { [(p >> 24) as f32, ((p >> 16) & 255) as f32, ((p >> 8) & 255) as f32, (p & 255) as f32] }

/// Bilinear sample at texel coordinates (texel centres on integers), clamped at the edges.
fn bilinear(img: &Image, u: f32, v: f32) -> [f32; 4] {
    let (w, h) = (img.w as i64, img.h as i64);
    let (fu, fv) = (u.floor(), v.floor());
    let (tx, ty) = (u - fu, v - fv);
    let at = |x: i64, y: i64| unpack(img.px[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize]);
    let (x, y) = (fu as i64, fv as i64);
    let (p00, p10, p01, p11) = (at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1));
    let mut o = [0.0; 4];
    for i in 0..4 {
        let top = p00[i] + (p10[i] - p00[i]) * tx;
        let bot = p01[i] + (p11[i] - p01[i]) * tx;
        o[i] = top + (bot - top) * ty;
    }
    o
}

/// Premultiplied source-over of `p` scaled by `k` into one opaque pixel.
fn over_pre(dst: &mut u32, p: [f32; 4], k: f32) {
    let inv = 1.0 - p[0] / 255.0 * k;
    let ch = |s: f32, d: u32| (s * k + d as f32 * inv).round().clamp(0.0, 255.0) as u32;
    let (dr, dg, db) = ((*dst >> 16) & 255, (*dst >> 8) & 255, *dst & 255);
    *dst = (ch(p[1], dr) << 16) | (ch(p[2], dg) << 8) | ch(p[3], db);
}

/// Blends one glyph's coverage with its top-left corner at (`ox`, `oy`), skipping pixels off the buffer.
fn blit(pixels: &mut [u32], width: usize, height: usize, clips: &[Clip], g: &Glyph, ox: i64, oy: i64, col: Color) {
    for row in 0..g.height {
        let py = oy + row as i64;
        if py < 0 || py >= height as i64 { continue; }
        for column in 0..g.width {
            let px = ox + column as i64;
            if px < 0 || px >= width as i64 { continue; }
            let cov = g.coverage[row * g.width + column] as f32 / 255.0;
            if cov <= 0.0 { continue; }
            let cov = cov * clip_coverage(clips, px as f32 + 0.5, py as f32 + 0.5);
            blend(&mut pixels[py as usize * width + px as usize], col, cov);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const W: usize = 64;
    fn rgb(p: u32) -> [i32; 3] { [(p >> 16 & 255) as i32, (p >> 8 & 255) as i32, (p & 255) as i32] }
    fn render(sc: &Scene, bg: u32) -> Framebuffer {
        let mut fb = Framebuffer::new(W, W);
        fb.clear(Color(bg));
        fb.replay(sc, 1.0);
        fb
    }
    fn px(fb: &Framebuffer, x: usize, y: usize) -> u32 { fb.pixels[y * fb.width + x] }

    #[test]
    fn shadow_is_darkest_in_the_middle_and_falls_off() {
        let mut sc = Scene::new();
        sc.shadow_ellipse(Rect::new(22.0, 22.0, 20.0, 20.0), 4.0, Color(0xff000000));
        let fb = render(&sc, 0xffffffff);
        let row: Vec<i32> = (32..W).map(|x| rgb(px(&fb, x, 32))[0]).collect();
        assert!(row[0] < 8, "centre {}", row[0]);
        assert!(row.windows(2).all(|w| w[0] <= w[1]), "{:?}", row);
        assert!((100..160).contains(&rgb(px(&fb, 42, 32))[0]), "edge {}", rgb(px(&fb, 42, 32))[0]);
        assert_eq!(px(&fb, 63, 32), 0x00ffffff);
        assert_eq!(px(&fb, 0, 0), 0x00ffffff);
        // Multiply, not overlay: a black shadow at half alpha halves every channel and keeps the hue.
        let mut sc = Scene::new();
        sc.shadow_round(Rect::new(0.0, 0.0, 64.0, 64.0), 4.0, 2.0, Color(0x80000000));
        let c = rgb(px(&render(&sc, 0xff6480c8), 32, 32));
        assert!((c[0] - 50).abs() <= 1 && (c[1] - 64).abs() <= 1 && (c[2] - 100).abs() <= 1, "{:?}", c);
        // An orange shadow filters a blue backdrop to black instead of painting orange on it.
        let mut sc = Scene::new();
        sc.shadow_round(Rect::new(0.0, 0.0, 64.0, 64.0), 4.0, 2.0, Color(0xffff8000));
        assert_eq!(rgb(px(&render(&sc, 0xff0000ff), 32, 32)), [0, 0, 0]);
    }

    #[test]
    fn inner_shadow_darkens_the_rim_only() {
        let mut sc = Scene::new();
        sc.inner_shadow(Shape::Ellipse(Rect::new(8.0, 8.0, 48.0, 48.0)), Point::new(0.0, 3.0), 3.0, Color(0xff000000));
        let fb = render(&sc, 0xffffffff);
        let (top, mid, bottom) = (rgb(px(&fb, 32, 9))[0], rgb(px(&fb, 32, 32))[0], rgb(px(&fb, 32, 54))[0]);
        assert!(top < 120 && mid > 250 && bottom > top, "{} {} {}", top, mid, bottom);
        assert_eq!(px(&fb, 2, 2), 0x00ffffff);
    }

    #[test]
    fn radial_gradient_centre_edge_and_focus() {
        let r = Rect::new(8.0, 8.0, 48.0, 48.0);
        let stops = Stops::two(Color(0xffffffff), Color(0xff000000));
        let mut sc = Scene::new();
        sc.fill_ellipse_radial(r, stops, r.center());
        let fb = render(&sc, 0xff808080);
        assert!(rgb(px(&fb, 32, 32))[0] > 245);
        assert!(rgb(px(&fb, 32, 9))[0] < 20 && rgb(px(&fb, 54, 32))[0] < 20);
        assert!((rgb(px(&fb, 44, 32))[0] - 128).abs() < 12);
        assert_eq!(px(&fb, 2, 2), 0x00808080);
        let mut sc = Scene::new();
        sc.fill_ellipse_radial(r, stops, Point::new(22.0, 22.0));
        let fb = render(&sc, 0xff808080);
        assert!(rgb(px(&fb, 22, 22))[0] > 245);
        assert!(rgb(px(&fb, 22, 22))[0] > rgb(px(&fb, 42, 42))[0] + 100);
        // Multi-stop: the middle stop shows between centre and rim.
        let mut sc = Scene::new();
        sc.fill_round_radial(r, 6.0, Stops::new(&[(0.0, Color(0xffff0000)), (0.5, Color(0xff00ff00)), (1.0, Color(0xff0000ff))]), r.center());
        let fb = render(&sc, 0xff000000);
        let (mid, rim) = (rgb(px(&fb, 44, 32)), rgb(px(&fb, 32, 55)));
        assert!(mid[1] > 235 && mid[0] < 20 && mid[2] < 20, "{:?}", mid);
        assert!(rim[2] > 200 && rim[0] == 0, "{:?}", rim);
    }

    #[test]
    fn linear_and_conic_multi_stop() {
        let r = Rect::new(0.0, 0.0, 64.0, 64.0);
        let stops = Stops::even(&[Color(0xffff0000), Color(0xff00ff00), Color(0xff0000ff)]);
        let mut sc = Scene::new();
        sc.fill_round_linear(r, 0.0, stops, 0.0);
        let fb = render(&sc, 0xff000000);
        assert!(rgb(px(&fb, 0, 30))[0] > 245 && rgb(px(&fb, 63, 30))[2] > 245);
        assert!(rgb(px(&fb, 32, 5))[1] > 245);
        let mut sc = Scene::new();
        sc.fill_round_linear(r, 0.0, stops, std::f32::consts::FRAC_PI_2);
        let fb = render(&sc, 0xff000000);
        assert!(rgb(px(&fb, 30, 0))[0] > 245 && rgb(px(&fb, 30, 63))[2] > 245);
        let mut sc = Scene::new();
        sc.fill_ellipse_conic(r, Stops::even(&[Color(0xffff0000), Color(0xff00ff00), Color(0xff0000ff), Color(0xffff0000)]), 0.0);
        let fb = render(&sc, 0xff000000);
        let (right, below, left) = (rgb(px(&fb, 60, 32)), rgb(px(&fb, 31, 60)), rgb(px(&fb, 4, 31)));
        assert!(right[0] > 200 && right[1] < 60, "{:?}", right);
        assert!(below[1] > below[0] && below[1] > below[2], "{:?}", below);
        assert!(left[1] > 60 && left[2] > 60, "{:?}", left);
    }

    #[test]
    fn feathered_glow_and_light_blends() {
        let mut sc = Scene::new();
        sc.glow_ellipse(Rect::new(8.0, 8.0, 48.0, 48.0), Color(0xffffffff), 16.0, Blend::Over);
        let fb = render(&sc, 0xff000000);
        assert_eq!(px(&fb, 32, 32), 0x00ffffff);
        let row: Vec<i32> = (32..W).map(|x| rgb(px(&fb, x, 32))[0]).collect();
        assert!(row.windows(2).all(|w| w[0] >= w[1]), "{:?}", row);
        assert!(row[18] > 10 && row[18] < 245, "{}", row[18]);
        assert_eq!(px(&fb, 57, 32), 0);
        let mut sc = Scene::new();
        sc.glow_round(Rect::new(0.0, 0.0, 64.0, 64.0), 0.0, Color(0xffff0000), 0.0, Blend::Add);
        assert_eq!(px(&render(&sc, 0xff00ff00), 32, 32), 0x00ffff00);
        let mut sc = Scene::new();
        sc.glow_round(Rect::new(0.0, 0.0, 64.0, 64.0), 0.0, Color(0xff808080), 0.0, Blend::Screen);
        let c = rgb(px(&render(&sc, 0xff808080), 32, 32));
        assert!((c[0] - 192).abs() <= 1, "{:?}", c);
        let mut sc = Scene::new();
        sc.glow_round(Rect::new(0.0, 0.0, 64.0, 64.0), 0.0, Color(0xffffffff), 0.0, Blend::Add);
        assert_eq!(px(&render(&sc, 0xffc0c0c0), 32, 32), 0x00ffffff);
    }

    #[test]
    fn image_blits_one_to_one_and_blends_premultiplied() {
        let pix: Vec<u32> = (0..12u32).map(|i| 0xff000000 | (i * 20) << 16 | (255 - i * 20) << 8 | (i * 7)).collect();
        let img = Arc::new(Image::new(4, 3, pix.clone()));
        let mut sc = Scene::new();
        sc.image(&img, Rect::new(5.0, 7.0, 4.0, 3.0), 1.0);
        let fb = render(&sc, 0xff000000);
        for j in 0..3 { for i in 0..4 { assert_eq!(px(&fb, 5 + i, 7 + j), pix[j * 4 + i] & 0xffffff); } }
        assert_eq!(px(&fb, 4, 7), 0);
        assert_eq!(px(&fb, 9, 7), 0);
        // Half-transparent premultiplied red over white: 64 + 255 * (1 - 128/255).
        let half = Arc::new(Image::new(1, 1, vec![0x80400000]));
        let mut sc = Scene::new();
        sc.image(&half, Rect::new(1.0, 1.0, 1.0, 1.0), 1.0);
        sc.image(&half, Rect::new(2.0, 1.0, 1.0, 1.0), 0.5);
        let fb = render(&sc, 0xffffffff);
        assert_eq!(rgb(px(&fb, 1, 1)), [191, 127, 127]);
        assert_eq!(rgb(px(&fb, 2, 1)), [223, 191, 191]);
        // Scaled path: a solid 2x2 image doubled at scale 2 covers 8x8 device pixels exactly.
        let solid = Arc::new(Image::new(2, 2, vec![0xff3366cc; 4]));
        let mut sc = Scene::new();
        sc.image(&solid, Rect::new(1.0, 1.0, 4.0, 4.0), 1.0);
        let mut fb = Framebuffer::new(16, 16);
        fb.replay(&sc, 2.0);
        assert_eq!(fb.pixels[2 * 16 + 2], 0x003366cc);
        assert_eq!(fb.pixels[9 * 16 + 9], 0x003366cc);
        assert_eq!(fb.pixels[10 * 16 + 10], 0);
        // Bilinear between a black and a white texel lands mid-grey; clips still apply.
        let ramp = Arc::new(Image::new(2, 1, vec![0xff000000, 0xffffffff]));
        let mut sc = Scene::new();
        sc.clip_round(Rect::new(0.0, 0.0, 8.0, 16.0), 0.0);
        sc.image(&ramp, Rect::new(0.0, 0.0, 16.0, 1.0), 1.0);
        sc.pop_clip();
        let mut fb = Framebuffer::new(16, 1);
        fb.clear(Color(0xff00ff00));
        fb.replay(&sc, 1.0);
        assert_eq!(fb.pixels[2], 0);
        assert_eq!(rgb(fb.pixels[7]), [112, 112, 112]); // u = 7.5 / 8 - 0.5
        assert_eq!(fb.pixels[8], 0x0000ff00);
    }

    #[test]
    fn lowered_scene_still_paints() {
        let mut sc = Scene::new();
        sc.shadow_ellipse(Rect::new(20.0, 20.0, 24.0, 24.0), 4.0, Color(0xff000000));
        sc.fill_ellipse_radial(Rect::new(20.0, 20.0, 24.0, 24.0), Stops::two(Color(0xffffffff), Color(0xff202020)), Point::new(28.0, 28.0));
        let fb = render(&sc.lower_basic(), 0xff808080);
        assert!(rgb(px(&fb, 32, 32))[0] > 128 && rgb(px(&fb, 32, 46))[0] < 128);
    }

    #[test]
    fn fills_and_clips() {
        let mut sc = Scene::new();
        sc.fill_rect(Rect::new(2.0, 2.0, 4.0, 4.0), Color(0xffff0000), Color(0xffff0000), true);
        let mut fb = Framebuffer::new(8, 8);
        fb.replay(&sc, 1.0);
        assert_eq!(fb.pixels[3 * 8 + 3], 0x00ff0000);
        assert_eq!(fb.pixels[0], 0);
        let mut sc = Scene::new();
        sc.clip_round(Rect::new(0.0, 0.0, 3.0, 8.0), 0.0);
        sc.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), Color(0xff00ff00), Color(0xff00ff00), true);
        sc.pop_clip();
        let mut fb = Framebuffer::new(8, 8);
        fb.replay(&sc, 1.0);
        assert_eq!(fb.pixels[4], 0);
        assert_eq!(fb.pixels[1], 0x0000ff00);
    }

    #[test]
    fn scale_doubles_geometry() {
        let mut sc = Scene::new();
        sc.fill_rect(Rect::new(1.0, 1.0, 2.0, 2.0), Color(0xffffffff), Color(0xffffffff), true);
        let mut fb = Framebuffer::new(8, 8);
        fb.replay(&sc, 2.0);
        assert_eq!(fb.pixels[2 * 8 + 2], 0x00ffffff);
        assert_eq!(fb.pixels[5 * 8 + 5], 0x00ffffff);
        assert_eq!(fb.pixels[6 * 8 + 6], 0);
    }

    #[test]
    fn every_knob_style_draws_pixels() {
        for flat in [false, true] {
            let mut sc = Scene::new();
            goodlookindri::gallery::paint_page(&mut sc, 0, flat, false);
            let mut fb = Framebuffer::new(700, 500);
            fb.clear(Color(0xff202020));
            fb.replay(&sc, 1.0);
            assert!(fb.pixels.iter().any(|p| *p != 0x00202020));
        }
    }
}
