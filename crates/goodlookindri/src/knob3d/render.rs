//! Ray-marches a knob surface along the tilted view direction and assembles the sprite.
//! The surface lives in cap space; the knob's angle only turns the view ray, the light and
//! the resulting normal, so the same code draws the analytic field and the baked grid.

use std::f32::consts::PI;

use super::field::{Field, Surface, AO_ROT};
use super::model::{KnobModel, Rig, Sprite};
use super::shade::{shade, Lights};
use super::vec::{dot, grey, hash2, mixf, norm, ss, to_srgb, v3, Rng, V3};
use crate::color::Color;

/// Sampling pattern of one pixel.
#[derive(Clone, Copy)]
pub(crate) struct Quality {
    /// Supersampling grid per axis.
    pub grid: usize,
    /// Jitter the samples inside their cells.
    pub jitter: bool,
}

impl Quality {
    /// Reference quality for the analytic field.
    pub const DIRECT: Quality = Quality { grid: 4, jitter: true };
    /// Per-angle frames from a bake.
    pub const FRAME: Quality = Quality { grid: 2, jitter: false };
}

struct Ctx<'a, S: Surface> {
    f: &'a S,
    l: &'a Lights,
    q: Quality,
    tan_tilt: f32,
    /// Size scale relative to the 90 px knob the sampling patterns were tuned on.
    sc: f32,
    /// Knob angle.
    sa: f32,
    ca: f32,
    /// Cap-space shift of the view ray per unit height.
    td: (f32, f32),
    /// Cap-space horizontal direction towards the key light and its rise.
    ld: (f32, f32),
    rise: f32,
    tans: (f32, f32),
    bound: f32,
    zmax: f32,
    r: f32,
}

impl<S: Surface> Ctx<'_, S> {
    fn new<'a>(f: &'a S, l: &'a Lights, angle: f32, q: Quality) -> Ctx<'a, S> {
        let (sa, ca) = angle.sin_cos();
        let t = l.rig.tilt.tan();
        let k = l.key;
        let lh = (k.x * k.x + k.y * k.y).sqrt().max(1e-4);
        let (dx, dy) = (k.x / lh, k.y / lh);
        let rise = k.z / lh;
        Ctx {
            f,
            l,
            q,
            tan_tilt: t,
            sc: f.radius() / 90.0,
            sa,
            ca,
            td: (t * sa, -t * ca),
            ld: (dx * ca - dy * sa, dx * sa + dy * ca),
            rise,
            tans: wide_tans(&l.rig, rise),
            bound: f.bound(),
            zmax: f.zmax(),
            r: f.radius(),
        }
    }

    /// Panel point to cap space.
    #[inline]
    fn cap(&self, px: f32, py: f32) -> (f32, f32) {
        (px * self.ca - py * self.sa, px * self.sa + py * self.ca)
    }
    /// Cap-space vector back to panel space.
    #[inline]
    fn panel(&self, x: f32, y: f32) -> (f32, f32) {
        (x * self.ca + y * self.sa, -x * self.sa + y * self.ca)
    }

    /// First intersection of the view ray through panel point (px, py) with the knob:
    /// cap-space point and height.
    fn trace(&self, px: f32, py: f32) -> Option<(f32, f32, f32)> {
        let reach = self.zmax * self.tan_tilt;
        let b = self.bound;
        if px.abs() > b || py < -b || py > b + reach || px * px + (py - py.clamp(0.0, reach)).powi(2) > b * b {
            return None;
        }
        let (cx, cy) = self.cap(px, py);
        let f = self.f;
        let over = |z: f32| f.height(cx + self.td.0 * z, cy + self.td.1 * z) - z;
        let top = self.zmax + 0.5;
        let steps = ((reach / 0.7).ceil() as usize).max(1);
        let mut z_hi = top;
        for i in 1..=steps {
            let z = top * (1.0 - i as f32 / steps as f32);
            if over(z) > 0.02 {
                let (mut lo, mut hi) = (z, z_hi);
                for _ in 0..8 {
                    let mid = 0.5 * (lo + hi);
                    if over(mid) > 0.02 {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let z = 0.5 * (lo + hi);
                return Some((cx + self.td.0 * z, cy + self.td.1 * z, z));
            }
            z_hi = z;
        }
        None
    }

    /// Soft shadow of the key softbox at a cap-space point: 0 = fully shadowed, 1 = lit.
    /// A tight lobe and a wide one are tracked along the same ray; `min_w` is a floor on
    /// the penumbra width in pixels so the shadow has no hard core where it meets the base.
    fn shadow(&self, x: f32, y: f32, h0: f32, min_w: f32) -> f32 {
        let (dx, dy) = self.ld;
        let rise = self.rise;
        let (tan_a, tan_b) = self.tans;
        let (mut ra, mut rb) = (1.0f32, 1.0f32);
        let step = 0.8 * self.sc.max(0.5);
        let mut t = 0.6 * self.sc.max(0.5);
        // stop once even the wide penumbra cone has cleared the highest point
        let t_max = (self.zmax - h0) / (rise - tan_b) + 1.0;
        // and never march on once the ray has left the knob for good
        let b2 = self.bound * self.bound;
        while t < t_max {
            let (qx, qy) = (x + dx * t, y + dy * t);
            if qx * qx + qy * qy > b2 && qx * dx + qy * dy > 0.0 {
                break;
            }
            let hq = self.f.height(qx, qy);
            if hq > h0 + 0.01 {
                let clear = h0 + t * rise + 0.25 - hq;
                ra = ra.min(clear / (t * tan_a + min_w));
                rb = rb.min(clear / (t * tan_b + 2.0 * min_w));
                if rb < -1.0 {
                    break;
                }
            }
            t += (t * 0.16).max(step);
        }
        0.55 * ss(-0.6, 1.0, ra.max(-1.0)) + 0.45 * ss(-0.6, 1.0, rb.max(-1.0))
    }

    /// Linear colour of the knob where the view ray through a panel point hits it.
    fn sample(&self, px: f32, py: f32, rot: f32) -> Option<V3> {
        let (x, y, z) = self.trace(px, py)?;
        let s = self.f.surf(x, y, rot);
        let (nx, ny) = self.panel(-s.gx, -s.gy);
        let (bx, by) = self.panel(s.bx, s.by);
        let (hx, hy) = self.panel(x, y);
        let n = norm(norm(v3(nx, ny, 1.0)) + v3(bx, by, 0.0));
        // camera at a finite distance: flat faces get a gentle gradient
        let v = norm(self.l.view + v3(-hx, -hy, 0.0) * (0.16 / self.r));
        // never shade a facet that faces away from the eye (bump can tip it over)
        let n = if dot(n, v) < 0.02 { norm(n + v * (0.02 - dot(n, v))) } else { n };
        // the height-field horizon test over-darkens walls: there, use height above the panel
        let wall_ao = 0.72 + 0.28 * ss(0.0, 0.45 * self.r, z);
        let mut ao = mixf(wall_ao, s.ao, ss(0.3, 0.8, n.z));
        let mut m = s.mat;
        if let Some(d) = self.f.decal() {
            if n.z < 0.7 {
                let rr = (x * x + y * y).sqrt();
                let da = ((x.atan2(y) - d.ang + PI).rem_euclid(2.0 * PI) - PI) * rr;
                let q = (da * da + (z - d.z) * (z - d.z)).sqrt() / d.rad;
                if q < 1.4 {
                    let k = ss(1.0, 0.7, q);
                    let lip = ss(1.4, 1.05, q) * ss(0.7, 1.05, q) * if z < d.z { 1.0 } else { 0.25 };
                    m.diff = m.diff * (1.0 - 0.94 * k) + grey(0.06 * lip);
                    m.f0 = m.f0 * (1.0 - 0.92 * k);
                    m.coat *= 1.0 - k;
                    ao *= 1.0 - 0.85 * k;
                }
            }
        }
        let rr = (hx * hx + hy * hy).sqrt().max(1e-4);
        let sh = self.shadow(x, y, z, 0.0);
        let fv = 0.2 + 0.8 * ss(0.0, 0.9 * self.r, z);
        // the softboxes are close: the side of the knob nearer the key is a little brighter
        let near = 1.0 + 0.07 * ((hy - 0.6 * hx) / self.r).clamp(-1.2, 1.2);
        Some(shade(self.l, n, v, v3(hx / rr, hy / rr, 0.0), &m, ao, sh, fv, s.curv) * near)
    }

    /// Shadow alpha on the bare panel at a panel point.
    fn floor_dark(&self, px: f32, py: f32, rot: f32) -> f32 {
        let rig = &self.l.rig;
        let reach = self.r * 1.75 + 2.0;
        if px * px + py * py > (reach + self.zmax).powi(2) {
            return 0.0;
        }
        let (x, y) = self.cap(px, py);
        let sh = self.shadow(x, y, 0.0, 0.045 * self.r);
        let ao = self.f.floor_ao(x, y, rot);
        let keep = (1.0 - rig.shadow * (1.0 - sh)) * (1.0 - rig.occlusion * (1.0 - ao).powf(1.3));
        1.0 - keep
    }
}

/// Penumbra slopes of the tight and the wide shadow lobe.
fn wide_tans(rig: &Rig, rise: f32) -> (f32, f32) {
    let a = rig.shadow_softness.min(rise - 0.6).max(0.05);
    (a, (2.0 * a).min(rise - 0.6).max(a))
}

/// Sprite rectangle around the knob, in pixels.
struct Layout {
    w: usize,
    h: usize,
    ox: f32,
    oy: f32,
    fade: f32,
}

fn layout(r: f32, bound: f32, zmax: f32, l: &Lights) -> Layout {
    let b = bound;
    let k = l.key;
    let kh = (k.x * k.x + k.y * k.y).sqrt().max(1e-4);
    let cast = 0.8 * zmax / (k.z / kh - wide_tans(&l.rig, k.z / kh).1);
    let soft = 0.45 * r + 3.0;
    let left = (b + soft + (cast * k.x / kh).max(0.0)).ceil();
    let right = (b + soft + (-cast * k.x / kh).max(0.0)).ceil();
    let up = (b + (zmax * l.rig.tilt.tan()).max(soft + (-cast * k.y / kh).max(0.0))).ceil() + 1.0;
    let down = (b + soft + (cast * k.y / kh).max(0.0)).ceil();
    Layout { w: (left + right) as usize, h: (up + down) as usize, ox: left, oy: up, fade: (0.2 * r).max(3.0) }
}

/// Render one knob to a premultiplied sprite with its shadow, straight from the model.
pub fn render(model: &KnobModel, rig: &Rig, radius_px: f32, angle: f32, accent: Color) -> Sprite {
    let field = Field::new(model, radius_px, accent);
    render_surface(&field, &Lights::new(rig), angle, Quality::DIRECT)
}

pub(crate) fn render_surface<S: Surface>(surface: &S, lights: &Lights, angle: f32, q: Quality) -> Sprite {
    let ctx = Ctx::new(surface, lights, angle, q);
    let lay = layout(ctx.r, ctx.bound, ctx.zmax, lights);
    let (w, h) = (lay.w, lay.h);
    let mut px = vec![0u32; w * h];
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min((h / 6).max(1));
    let rows_per = h.div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (ci, chunk) in px.chunks_mut(rows_per * w).enumerate() {
            let (ctx, lay) = (&ctx, &lay);
            s.spawn(move || {
                for (i, out) in chunk.iter_mut().enumerate() {
                    let (ix, iy) = (i % w, ci * rows_per + i / w);
                    *out = pixel(ctx, ix, iy, lay.ox, lay.oy, w, h, lay.fade);
                }
            });
        }
    });
    Sprite { w: w as u32, h: h as u32, px, origin: (lay.ox, lay.oy) }
}

/// Rotated-grid offsets for the 2 x 2 pattern.
const RG: [(f32, f32); 4] = [(0.375, 0.125), (0.875, 0.375), (0.125, 0.625), (0.625, 0.875)];

#[allow(clippy::too_many_arguments)]
fn pixel<S: Surface>(ctx: &Ctx<S>, ix: usize, iy: usize, ox: f32, oy: f32, w: usize, h: usize, fade: f32) -> u32 {
    let mut rng = Rng((ix as u32).wrapping_mul(9781).wrapping_add((iy as u32).wrapping_mul(6271)) | 1);
    let mut col = grey(0.0);
    let mut hits = 0usize;
    let g = ctx.q.grid;
    let mut n = g * g;
    let (bx, by) = (ix as f32 - ox, oy - iy as f32);
    for j in 0..g {
        for i in 0..g {
            let (fx, fy) = if ctx.q.jitter {
                ((i as f32 + rng.next()) / g as f32, (j as f32 + rng.next()) / g as f32)
            } else if g == 2 {
                RG[j * 2 + i]
            } else {
                ((i as f32 + 0.5) / g as f32, (j as f32 + 0.5) / g as f32)
            };
            if let Some(c) = ctx.sample(bx + fx, by - fy, AO_ROT) {
                col += c;
                hits += 1;
            }
        }
    }
    let shaded = hits;
    if !ctx.q.jitter && hits > 0 && hits < n {
        // silhouette pixel of a coarse pattern: four samples give the colour, a finer grid
        // of plain ray hits gives the coverage
        const E: usize = 4;
        let mut fine = 0usize;
        for j in 0..E {
            for i in 0..E {
                fine += ctx.trace(bx + (i as f32 + 0.5) / E as f32, by - (j as f32 + 0.5) / E as f32).is_some() as usize;
            }
        }
        if fine > 0 {
            hits = fine;
            n = E * E;
        }
    }
    // the panel's shadow is smooth: one evaluation per pixel serves every sample that
    // looks past the knob
    let mut dark = 0.0;
    if hits < n {
        dark = ctx.floor_dark(bx + 0.5, by - 0.5, AO_ROT) * (n - hits) as f32 / n as f32;
    }
    if hits == 0 && dark < 1e-4 {
        return 0;
    }
    // the shadow must die out before the sprite border
    let edge = (ix as f32 + 0.5).min(w as f32 - ix as f32 - 0.5).min(iy as f32 + 0.5).min(h as f32 - iy as f32 - 0.5);
    let dark = dark * ss(0.5, fade, edge);
    let cov = hits as f32 / n as f32;
    let alpha = (cov + dark).min(1.0);
    let c = if shaded > 0 { col * (1.0 / shaded as f32) } else { col };
    let a8 = (alpha * 255.0 + 0.5).floor().min(255.0);
    let mut out = (a8 as u32) << 24;
    for (shift, lin) in [(16, c.x), (8, c.y), (0, c.z)] {
        // triangular dither, about one LSB, from a per-pixel hash
        let d = hash2(ix as i32 * 3 + shift, iy as i32) + hash2(ix as i32, iy as i32 * 3 + shift + 77) - 1.0;
        let v = (to_srgb(lin) * cov * 255.0 + 0.5 + d * 0.5 * cov).floor().clamp(0.0, a8);
        out |= (v as u32) << shift;
    }
    out
}
