//! Host-owned drawing list. GoodLookin DRI never opens a window or touches audio parameters.
//! A plugin (Water Channel, an EQ, nih-plug, iced, egui, Cairo) replays [`Cmd`] on its GUI thread.
//!
//! Water Channel's C++ canvas only had five primitives and rejected every style except Brit.
//! This list is the same idea with the operations the full toolkit actually paints:
//! ellipses, round rects, lines, arcs, filled polygons, stroked polylines, and text runs.
//! Text is a run of glyphs the host draws with its own font; the toolkit does not bundle one.
//!
//! 3D 2.0 adds [`Cmd::Fill`] (multi-stop linear, radial with focus, conic; feathered edges;
//! light blends), [`Cmd::Shadow`], [`Cmd::InnerShadow`] and [`Cmd::Image`]. A host that only
//! knows the original commands replays [`Scene::lower_basic`] instead, which rewrites them:
//!
//! | new | old approximation |
//! |---|---|
//! | `Fill` solid / linear | `FillEllipse` / `FillRoundRect`, first and last stop, vertical or diagonal |
//! | `Fill` radial | same, first stop toward the focus, last stop away from it |
//! | `Fill` conic | same, stops at 0 and 0.5 |
//! | `Fill` with `feather` | three stacked inset fills of a third of the opacity each; blends become source-over |
//! | `Shadow` | four stacked translucent fills spread over ±1.5 sigma |
//! | `InnerShadow` | one translucent stroke just inside the edge |
//! | `Image` | dropped |
//!
//! The Water Channel five-call canvas ([`WaterCmd`]) is fed by [`crate::water`], never from a
//! `Scene`; lowering keeps its role: hosts on that canvas still get only old-style geometry.

use crate::color::Color;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point { pub x: f32, pub y: f32 }
impl Point {
    pub fn new(x: f32, y: f32) -> Self { Self { x, y } }
    pub fn add(self, o: Point) -> Self { Self { x: self.x + o.x, y: self.y + o.y } }
    pub fn mul(self, s: f32) -> Self { Self { x: self.x * s, y: self.y * s } }
    pub fn sub(self, o: Point) -> Self { Self { x: self.x - o.x, y: self.y - o.y } }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }
impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self { Self { x, y, w, h } }
    pub fn right(self) -> f32 { self.x + self.w }
    pub fn bottom(self) -> f32 { self.y + self.h }
    pub fn center(self) -> Point { Point::new(self.x + self.w * 0.5, self.y + self.h * 0.5) }
    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.right() && p.y < self.bottom()
    }
    pub fn translated(self, dx: f32, dy: f32) -> Self { Self { x: self.x + dx, y: self.y + dy, ..self } }
    pub fn reduced(self, dx: f32, dy: f32) -> Self {
        Self { x: self.x + dx, y: self.y + dy, w: (self.w - dx * 2.0).max(0.0), h: (self.h - dy * 2.0).max(0.0) }
    }
    pub fn expanded(self, d: f32) -> Self { self.reduced(-d, -d) }
}

pub const MAX_STOPS: usize = 6;

/// Gradient stops: up to [`MAX_STOPS`] `(position, colour)` pairs, positions 0..1 ascending.
/// Colours mix in sRGB with straight alpha, like [`Color::mix`]. Outside the first and last stop
/// the end colour is held.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stops { len: u8, pos: [f32; MAX_STOPS], col: [Color; MAX_STOPS] }
impl Stops {
    /// Stops past [`MAX_STOPS`] are dropped, positions clamped to 0..1 and sorted. Empty is transparent.
    pub fn new(stops: &[(f32, Color)]) -> Self {
        let mut s = Stops { len: 0, pos: [0.0; MAX_STOPS], col: [Color(0); MAX_STOPS] };
        for &(p, c) in stops.iter().take(MAX_STOPS) {
            let mut i = s.len as usize;
            let p = if p.is_finite() { p.clamp(0.0, 1.0) } else { 0.0 };
            while i > 0 && s.pos[i - 1] > p { s.pos[i] = s.pos[i - 1]; s.col[i] = s.col[i - 1]; i -= 1; }
            s.pos[i] = p;
            s.col[i] = c;
            s.len += 1;
        }
        if s.len == 0 { s.len = 1; }
        s
    }
    pub fn two(a: Color, b: Color) -> Self { Self::new(&[(0.0, a), (1.0, b)]) }
    /// Evenly spaced from 0 to 1.
    pub fn even(cols: &[Color]) -> Self {
        let n = cols.len().clamp(1, MAX_STOPS);
        let v: Vec<(f32, Color)> = cols.iter().take(n).enumerate().map(|(i, c)| (if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 }, *c)).collect();
        Self::new(&v)
    }
    pub fn len(&self) -> usize { self.len as usize }
    pub fn get(&self, i: usize) -> (f32, Color) { (self.pos[i], self.col[i]) }
    pub fn first(&self) -> Color { self.col[0] }
    pub fn last(&self) -> Color { self.col[self.len as usize - 1] }
    pub fn at(&self, t: f32) -> Color {
        let n = self.len as usize;
        if !(t > self.pos[0]) { return self.col[0]; }
        for i in 1..n {
            if t <= self.pos[i] {
                let span = self.pos[i] - self.pos[i - 1];
                return self.col[i - 1].mix(self.col[i], if span > 0.0 { (t - self.pos[i - 1]) / span } else { 1.0 });
            }
        }
        self.col[n - 1]
    }
}

/// Outline for the 3D 2.0 commands. `Round`'s radius is clamped to half the shorter side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape { Ellipse(Rect), Round(Rect, f32) }
impl Shape {
    pub fn rect(self) -> Rect { match self { Shape::Ellipse(r) | Shape::Round(r, _) => r } }
    pub fn translated(self, d: Point) -> Self {
        match self { Shape::Ellipse(r) => Shape::Ellipse(r.translated(d.x, d.y)), Shape::Round(r, k) => Shape::Round(r.translated(d.x, d.y), k) }
    }
    /// Grows (or with negative `d` shrinks) every side; a round rect's radius follows.
    pub fn expanded(self, d: f32) -> Self {
        match self { Shape::Ellipse(r) => Shape::Ellipse(r.expanded(d)), Shape::Round(r, k) => Shape::Round(r.expanded(d), (k + d).max(0.0)) }
    }
}

/// How [`Cmd::Fill`] is coloured. Points and radii are in scene units, like the shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    Solid(Color),
    /// `angle` in radians, screen space: 0 runs left → right, π/2 top → bottom. t = 0 and t = 1
    /// fall on the shape's bounds along that direction (CSS `linear-gradient`).
    Linear { stops: Stops, angle: f32 },
    /// t = 0 at `focus`, t = 1 on the ellipse `center`, `rx`, `ry` (SVG focal radial). A focus
    /// outside 98% of that ellipse is pulled in.
    Radial { stops: Stops, center: Point, rx: f32, ry: f32, focus: Point },
    /// t runs once around `center`, starting at `start` radians (0 = 3 o'clock) and going
    /// clockwise on screen. Repeat the first stop last for a seamless turn.
    Conic { stops: Stops, center: Point, start: f32 },
}

/// Compositing for [`Cmd::Fill`]. `Add` and `Screen` brighten only (lights, bloom, glints).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Blend {
    #[default]
    Over,
    /// dst + src·α, clamped.
    Add,
    /// dst + (1 − dst)·src·α: like `Add` but never clips to white.
    Screen,
}

static NEXT_IMAGE: AtomicU64 = AtomicU64::new(1);

/// Premultiplied sRGB `0xAARRGGBB` pixels, top row first. Each constructed image gets a new
/// process-wide `id`; equality is by `id` only, so hosts can key texture caches on it.
#[derive(Clone)]
pub struct Image { pub id: u64, pub w: u32, pub h: u32, pub px: Arc<[u32]> }
impl Image {
    /// `px` must hold `w * h` premultiplied pixels.
    pub fn new(w: u32, h: u32, px: impl Into<Arc<[u32]>>) -> Self {
        let px = px.into();
        assert_eq!(px.len(), w as usize * h as usize, "Image: pixel count is not w * h");
        Self { id: NEXT_IMAGE.fetch_add(1, Ordering::Relaxed), w, h, px }
    }
    /// From straight-alpha pixels (the [`Color`] layout).
    pub fn from_straight(w: u32, h: u32, px: &[u32]) -> Self {
        let pm = |p: u32| {
            let a = p >> 24;
            let ch = |c: u32| ((c & 255) * a + 127) / 255;
            (a << 24) | (ch(p >> 16) << 16) | (ch(p >> 8) << 8) | ch(p)
        };
        Self::new(w, h, px.iter().map(|p| pm(*p)).collect::<Vec<u32>>())
    }
}
impl PartialEq for Image { fn eq(&self, o: &Self) -> bool { self.id == o.id } }
impl Eq for Image {}
impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "Image {{ id: {}, w: {}, h: {} }}", self.id, self.w, self.h) }
}

/// Linear gradient stops are the two colours. `vertical` paints top→bottom; otherwise the
/// gradient runs from the top-left of the shape toward the bottom-right (JUCE knob discs).
///
/// Non-exhaustive: hosts outside this crate need a `_` arm. Either ignore unknown commands or
/// replay [`Scene::lower_basic`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Cmd {
    FillEllipse { rect: Rect, top: Color, bottom: Color, diagonal: bool },
    StrokeEllipse { rect: Rect, color: Color, width: f32 },
    FillRoundRect { rect: Rect, radius: f32, top: Color, bottom: Color, vertical: bool },
    StrokeRoundRect { rect: Rect, radius: f32, color: Color, width: f32 },
    FillRect { rect: Rect, top: Color, bottom: Color, vertical: bool },
    Line { a: Point, b: Point, color: Color, width: f32 },
    /// Closed fill. `diagonal` uses a top-left → bottom-right gradient across the bounds of `pts`.
    FillPoly { pts: Vec<Point>, top: Color, bottom: Color, diagonal: bool },
    StrokePoly { pts: Vec<Point>, color: Color, width: f32, closed: bool },
    /// Angles in radians, 0 = +x in standard math, matching JUCE `addCentredArc` (0 = 3 o'clock, clockwise positive in screen space).
    StrokeArc { center: Point, radius: f32, from: f32, to: f32, color: Color, width: f32 },
    Text { at: Point, text: String, color: Color, size: f32, center: bool },
    /// Push/pop a clip rounded-rect. Hosts that cannot clip may ignore these.
    ClipRound { rect: Rect, radius: f32 },
    PopClip,
    /// Shape filled with `paint`. `feather` (scene units) > 0 fades the edge inward: fully
    /// painted `feather` inside the outline, zero on it, smoothstep between. 0 is the usual
    /// one-pixel anti-aliased edge.
    Fill { shape: Shape, paint: Paint, feather: f32, blend: Blend },
    /// Drop shadow under `shape` (offset it yourself, see [`crate::light::LightRig::shadow_offset`]).
    /// Coverage is ½·erfc(d / (σ√2)) of the shape's signed distance, so it is ~1 well inside and
    /// half on the outline. Composited as a multiply toward `color` scaled by its alpha, which keeps
    /// the hue of what is underneath.
    Shadow { shape: Shape, sigma: f32, color: Color },
    /// Shadow inside `shape`, as if cast by its rim onto a recess: the outline moved by `offset`
    /// is blurred by `sigma`, and the darkening is where that moved shape does not cover.
    InnerShadow { shape: Shape, offset: Point, sigma: f32, color: Color },
    /// Premultiplied image stretched into `dest` (bilinear, clamped at the edges). An exact
    /// device-pixel fit is copied 1:1.
    Image { img: Arc<Image>, dest: Rect, opacity: f32 },
}

#[derive(Clone, Debug, Default)]
pub struct Scene { pub cmds: Vec<Cmd> }

impl Scene {
    pub fn new() -> Self { Self { cmds: Vec::new() } }
    pub fn clear(&mut self) { self.cmds.clear(); }

    pub fn fill_ellipse(&mut self, rect: Rect, top: Color, bottom: Color) {
        self.cmds.push(Cmd::FillEllipse { rect, top, bottom, diagonal: top != bottom });
    }
    pub fn fill_ellipse_solid(&mut self, rect: Rect, color: Color) {
        self.cmds.push(Cmd::FillEllipse { rect, top: color, bottom: color, diagonal: false });
    }
    pub fn stroke_ellipse(&mut self, rect: Rect, color: Color, width: f32) {
        self.cmds.push(Cmd::StrokeEllipse { rect, color, width });
    }
    pub fn fill_round(&mut self, rect: Rect, radius: f32, top: Color, bottom: Color, vertical: bool) {
        self.cmds.push(Cmd::FillRoundRect { rect, radius, top, bottom, vertical });
    }
    pub fn stroke_round(&mut self, rect: Rect, radius: f32, color: Color, width: f32) {
        self.cmds.push(Cmd::StrokeRoundRect { rect, radius, color, width });
    }
    pub fn fill_rect(&mut self, rect: Rect, top: Color, bottom: Color, vertical: bool) {
        self.cmds.push(Cmd::FillRect { rect, top, bottom, vertical });
    }
    pub fn line(&mut self, a: Point, b: Point, color: Color, width: f32) {
        self.cmds.push(Cmd::Line { a, b, color, width });
    }
    pub fn fill_poly(&mut self, pts: Vec<Point>, top: Color, bottom: Color) {
        self.cmds.push(Cmd::FillPoly { pts, top, bottom, diagonal: top != bottom });
    }
    pub fn stroke_poly(&mut self, pts: Vec<Point>, color: Color, width: f32, closed: bool) {
        self.cmds.push(Cmd::StrokePoly { pts, color, width, closed });
    }
    pub fn stroke_arc(&mut self, center: Point, radius: f32, from: f32, to: f32, color: Color, width: f32) {
        self.cmds.push(Cmd::StrokeArc { center, radius, from, to, color, width });
    }
    pub fn text(&mut self, at: Point, text: impl Into<String>, color: Color, size: f32, center: bool) {
        self.cmds.push(Cmd::Text { at, text: text.into(), color, size, center });
    }
    pub fn clip_round(&mut self, rect: Rect, radius: f32) { self.cmds.push(Cmd::ClipRound { rect, radius }); }
    pub fn pop_clip(&mut self) { self.cmds.push(Cmd::PopClip); }

    pub fn ellipse_at(c: Point, radius: f32) -> Rect {
        Rect::new(c.x - radius, c.y - radius, radius * 2.0, radius * 2.0)
    }

    pub fn fill_shape(&mut self, shape: Shape, paint: Paint, feather: f32, blend: Blend) {
        self.cmds.push(Cmd::Fill { shape, paint, feather, blend });
    }
    /// Multi-stop linear gradient, `angle` as in [`Paint::Linear`].
    pub fn fill_ellipse_linear(&mut self, rect: Rect, stops: Stops, angle: f32) {
        self.fill_shape(Shape::Ellipse(rect), Paint::Linear { stops, angle }, 0.0, Blend::Over);
    }
    pub fn fill_round_linear(&mut self, rect: Rect, radius: f32, stops: Stops, angle: f32) {
        self.fill_shape(Shape::Round(rect, radius), Paint::Linear { stops, angle }, 0.0, Blend::Over);
    }
    /// Radial gradient filling the ellipse inscribed in `rect`, t = 0 at `focus` (scene units).
    pub fn fill_ellipse_radial(&mut self, rect: Rect, stops: Stops, focus: Point) {
        let paint = Paint::Radial { stops, center: rect.center(), rx: rect.w * 0.5, ry: rect.h * 0.5, focus };
        self.fill_shape(Shape::Ellipse(rect), paint, 0.0, Blend::Over);
    }
    /// Radial gradient over a round rect; t = 1 on the ellipse inscribed in `rect`, so corners hold the last stop.
    pub fn fill_round_radial(&mut self, rect: Rect, radius: f32, stops: Stops, focus: Point) {
        let paint = Paint::Radial { stops, center: rect.center(), rx: rect.w * 0.5, ry: rect.h * 0.5, focus };
        self.fill_shape(Shape::Round(rect, radius), paint, 0.0, Blend::Over);
    }
    /// Conic sweep around the ellipse centre (brushed metal, anisotropic highlights).
    pub fn fill_ellipse_conic(&mut self, rect: Rect, stops: Stops, start: f32) {
        self.fill_shape(Shape::Ellipse(rect), Paint::Conic { stops, center: rect.center(), start }, 0.0, Blend::Over);
    }
    /// Soft-edged solid ellipse for glows and bloom; `feather` as in [`Cmd::Fill`].
    pub fn glow_ellipse(&mut self, rect: Rect, color: Color, feather: f32, blend: Blend) {
        self.fill_shape(Shape::Ellipse(rect), Paint::Solid(color), feather, blend);
    }
    pub fn glow_round(&mut self, rect: Rect, radius: f32, color: Color, feather: f32, blend: Blend) {
        self.fill_shape(Shape::Round(rect, radius), Paint::Solid(color), feather, blend);
    }
    pub fn shadow(&mut self, shape: Shape, sigma: f32, color: Color) { self.cmds.push(Cmd::Shadow { shape, sigma, color }); }
    pub fn shadow_ellipse(&mut self, rect: Rect, sigma: f32, color: Color) { self.shadow(Shape::Ellipse(rect), sigma, color); }
    pub fn shadow_round(&mut self, rect: Rect, radius: f32, sigma: f32, color: Color) { self.shadow(Shape::Round(rect, radius), sigma, color); }
    pub fn inner_shadow(&mut self, shape: Shape, offset: Point, sigma: f32, color: Color) {
        self.cmds.push(Cmd::InnerShadow { shape, offset, sigma, color });
    }
    pub fn image(&mut self, img: &Arc<Image>, dest: Rect, opacity: f32) {
        self.cmds.push(Cmd::Image { img: img.clone(), dest, opacity });
    }

    /// Copy with every 3D 2.0 command rewritten into the original ones (table in the module docs).
    pub fn lower_basic(&self) -> Scene {
        let mut out = Scene::new();
        for c in &self.cmds {
            match c {
                Cmd::Fill { shape, paint, feather, .. } => {
                    let (a, b, vertical) = two_stop(*shape, paint);
                    if *feather > 0.0 {
                        let k = |c: Color| c.with_alpha(1.0 - (1.0 - c.alpha_f()).powf(1.0 / 3.0));
                        for i in 0..3 { out.basic_fill(shape.expanded(-feather * (1.0 + 2.0 * i as f32) / 6.0), k(a), k(b), vertical); }
                    } else {
                        out.basic_fill(*shape, a, b, vertical);
                    }
                }
                Cmd::Shadow { shape, sigma, color } => {
                    let c = color.with_alpha(color.alpha_f() * 0.3);
                    for i in 0..4 { out.basic_fill(shape.expanded(sigma * (1.5 - 0.75 * i as f32)), c, c, true); }
                }
                Cmd::InnerShadow { shape, sigma, color, .. } => {
                    let (c, w) = (color.with_alpha(color.alpha_f() * 0.5), sigma.max(0.5));
                    match shape.expanded(-w * 0.5) {
                        Shape::Ellipse(rect) => out.stroke_ellipse(rect, c, w),
                        Shape::Round(rect, radius) => out.stroke_round(rect, radius, c, w),
                    }
                }
                Cmd::Image { .. } => {}
                other => out.cmds.push(other.clone()),
            }
        }
        out
    }
    fn basic_fill(&mut self, shape: Shape, top: Color, bottom: Color, vertical: bool) {
        match shape {
            Shape::Ellipse(rect) => self.cmds.push(Cmd::FillEllipse { rect, top, bottom, diagonal: !vertical && top != bottom }),
            Shape::Round(rect, radius) => self.fill_round(rect, radius, top, bottom, vertical),
        }
    }
}

/// (top colour, bottom colour, vertical) for the old two-colour fills.
fn two_stop(shape: Shape, paint: &Paint) -> (Color, Color, bool) {
    match *paint {
        Paint::Solid(c) => (c, c, true),
        Paint::Linear { stops, angle } => {
            let (dx, dy) = (angle.cos(), angle.sin());
            let vertical = dx.abs() < 0.38;
            let flip = if vertical { dy < 0.0 } else { dx < 0.0 };
            if flip { (stops.last(), stops.first(), vertical) } else { (stops.first(), stops.last(), vertical) }
        }
        Paint::Radial { stops, center, focus, .. } => {
            let flip = focus.x + focus.y > center.x + center.y + 1e-3 * shape.rect().w.max(1.0);
            if flip { (stops.last(), stops.first(), false) } else { (stops.first(), stops.last(), false) }
        }
        Paint::Conic { stops, .. } => (stops.first(), stops.at(0.5), false),
    }
}

/// Five-primitive subset used by the Water Channel canvas. Hosts that only implement those
/// can still paint panels and the Brit/console knob through [`crate::draw::water_knob`].
#[derive(Clone, Debug, PartialEq)]
pub enum WaterCmd {
    GradientEllipse { rect: Rect, top: Color, bottom: Color },
    StrokeEllipse { rect: Rect, color: Color, width: f32 },
    Line { a: Point, b: Point, color: Color, width: f32 },
    GradientPanel { rect: Rect, radius: f32, top: Color, bottom: Color },
    StrokePanel { rect: Rect, radius: f32, color: Color, width: f32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_sort_and_sample() {
        let s = Stops::new(&[(1.0, Color(0xff0000ff)), (0.0, Color(0xffff0000)), (0.5, Color(0xff00ff00))]);
        assert_eq!(s.len(), 3);
        assert_eq!(s.at(-1.0), Color(0xffff0000));
        assert_eq!(s.at(0.5), Color(0xff00ff00));
        assert_eq!(s.at(2.0), Color(0xff0000ff));
        assert_eq!(s.at(0.25), Color(0xffff0000).mix(Color(0xff00ff00), 0.5));
        assert_eq!(Stops::new(&[(0.0, Color(1)); 9]).len(), MAX_STOPS);
        assert_eq!(Stops::even(&[Color(1), Color(2), Color(3)]).get(1), (0.5, Color(2)));
    }

    #[test]
    fn images_have_unique_ids() {
        let a = Image::new(1, 1, vec![0xffffffff]);
        let b = Image::new(1, 1, vec![0xffffffff]);
        assert_ne!(a, b);
        assert_eq!(a, a.clone());
        assert_eq!(Image::from_straight(1, 1, &[0x80ff4000]).px[0], 0x80802000);
    }

    #[test]
    fn lower_basic_uses_only_old_commands() {
        let r = Rect::new(10.0, 10.0, 40.0, 30.0);
        let img = Arc::new(Image::new(2, 2, vec![0; 4]));
        let mut sc = Scene::new();
        sc.fill_rect(r, Color(0xff101010), Color(0xff202020), true);
        sc.fill_ellipse_radial(r, Stops::two(Color(0xffffffff), Color(0xff000000)), Point::new(20.0, 18.0));
        sc.fill_round_linear(r, 4.0, Stops::even(&[Color(0xff111111), Color(0xff999999), Color(0xff333333)]), 1.0);
        sc.fill_ellipse_conic(r, Stops::two(Color(0xff444444), Color(0xffcccccc)), 0.0);
        sc.glow_round(r, 6.0, Color(0x80ffcc66), 8.0, Blend::Screen);
        sc.shadow_ellipse(r, 4.0, Color(0xa0000000));
        sc.inner_shadow(Shape::Round(r, 3.0), Point::new(0.0, 2.0), 2.0, Color(0x80000000));
        sc.image(&img, r, 1.0);
        let lo = sc.lower_basic();
        assert_eq!(lo.cmds.len(), 1 + 1 + 1 + 1 + 3 + 4 + 1);
        assert!(lo.cmds.iter().all(|c| !matches!(c, Cmd::Fill { .. } | Cmd::Shadow { .. } | Cmd::InnerShadow { .. } | Cmd::Image { .. })));
        assert_eq!(lo.cmds[0], sc.cmds[0]);
        assert_eq!(lo.cmds[1], Cmd::FillEllipse { rect: r, top: Color(0xffffffff), bottom: Color(0xff000000), diagonal: true });
    }
}
