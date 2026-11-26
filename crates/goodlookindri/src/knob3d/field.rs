//! A knob resolved for one size, in cap space: the pointer lies along +y and the knob's
//! turning angle is applied by the renderer. Blurred profile tables, plan shapes, angular
//! detail, indicators and procedural material textures.

use std::f32::consts::PI;

use super::model::{Angular, AngularKind, KnobModel, Material, Plan, Pointer, PointerKind, Segment};
use super::vec::{grey, lerp3, mixf, ss, v3, vnoise, V3};
use crate::color::Color;

const LUT_N: usize = 2048;
const LUT_SPAN: f32 = 1.12;
/// Directions of the occlusion estimate, and the fixed turn of the pattern. The bake and
/// the direct render use the same estimator, so a bake only adds interpolation error.
pub(crate) const AO_DIRS: usize = 10;
pub(crate) const AO_ROT: f32 = 0.3;
/// Largest convexity that is kept.
pub(crate) const CURV: f32 = 2.0;

/// Material in the form the shader wants.
#[derive(Clone, Copy)]
pub(crate) struct Mat {
    pub diff: V3,
    pub f0: V3,
    pub rough: f32,
    pub aniso: f32,
    pub coat: f32,
    pub sheen: f32,
    pub trans: f32,
}

/// Everything the shader needs at a surface point besides its position.
#[derive(Clone, Copy)]
pub(crate) struct Surf {
    /// Height gradient, cap space.
    pub gx: f32,
    pub gy: f32,
    /// Small normal perturbation, cap space.
    pub bx: f32,
    pub by: f32,
    pub ao: f32,
    /// Convexity: height above the mean of the neighbours a short step away, per unit
    /// step. Positive on top edges and ridges (which catch light), negative in grooves.
    pub curv: f32,
    pub mat: Mat,
}

/// Set-screw hole on the wall, pixels and radians.
#[derive(Clone, Copy)]
pub(crate) struct Decal {
    pub ang: f32,
    pub z: f32,
    pub rad: f32,
}

/// What the renderer needs from a knob, in cap space (pixels, y towards the pointer).
pub(crate) trait Surface: Sync {
    fn radius(&self) -> f32;
    fn zmax(&self) -> f32;
    fn bound(&self) -> f32 {
        self.radius() * 1.04 + 1.5
    }
    fn height(&self, x: f32, y: f32) -> f32;
    /// `rot` turns the occlusion sampling pattern (ignored when occlusion is baked).
    fn surf(&self, x: f32, y: f32, rot: f32) -> Surf;
    /// Openness of the panel at a point next to the knob, 0..1.
    fn floor_ao(&self, x: f32, y: f32, rot: f32) -> f32;
    fn decal(&self) -> Option<Decal>;
}

#[derive(Clone)]
struct Mod {
    a: Angular,
    amp: f32,
    /// Scallop cutter: centre distance and radius (radius units).
    cut: (f32, f32),
    /// Radial width over which the detail fades in.
    fade: f32,
}

#[derive(Clone)]
struct Ptr {
    kind: PointerKind,
    r0: f32,
    r1: f32,
    hw: f32,
    depth: f32,
    mat: Material,
}

/// One blurred radial height table with its materials.
#[derive(Clone)]
struct Table {
    lut: Vec<f32>,
    segs: Vec<(f32, Material)>,
    zmax: f32,
    k: f32,
}

impl Table {
    fn new(profile: &[Segment], r: f32, hs: f32, edge: f32, accent: Color) -> Table {
        let raw_at = |u: f32| -> f32 {
            for s in profile {
                if u < s.r1 {
                    return if u >= s.r0 { s.height(u) * hs } else { 0.0 };
                }
            }
            0.0
        };
        // tabulate, then blur to round every edge (never sharper than about half a pixel)
        let raw: Vec<f32> = (0..LUT_N).map(|i| raw_at(i as f32 / (LUT_N - 1) as f32 * LUT_SPAN)).collect();
        let bins_per_px = (LUT_N - 1) as f32 / (LUT_SPAN * r);
        let sigma = (edge * r).max(0.55) * bins_per_px;
        let rad = (sigma * 3.0).ceil() as i32;
        let wts: Vec<f32> = (-rad..=rad).map(|k| (-(k as f32).powi(2) / (2.0 * sigma * sigma)).exp()).collect();
        let wsum: f32 = wts.iter().sum();
        let mut lut = vec![0.0f32; LUT_N];
        let mut zmax = 0.0f32;
        for i in 0..LUT_N as i32 {
            let mut acc = 0.0;
            for k in -rad..=rad {
                let j = (i + k).abs().min(LUT_N as i32 - 1);
                acc += raw[j as usize] * wts[(k + rad) as usize];
            }
            lut[i as usize] = acc / wsum;
            zmax = zmax.max(lut[i as usize]);
        }
        Table { lut, segs: profile.iter().map(|s| (s.r1, s.material.tinted(accent))).collect(), zmax, k: bins_per_px }
    }
    /// Height at a pseudo-radius in pixels.
    fn at(&self, rho: f32) -> f32 {
        let f = rho.max(0.0) * self.k;
        if f >= (LUT_N - 1) as f32 {
            return 0.0;
        }
        let i = f as usize;
        mixf(self.lut[i], self.lut[i + 1], f - i as f32)
    }
    fn mat(&self, u: f32) -> Material {
        for (r1, m) in &self.segs {
            if u < *r1 {
                return *m;
            }
        }
        self.segs.last().map(|s| s.1).unwrap_or(Material::plastic(0.5, 0.5, 0.5, 0.5))
    }
}

#[derive(Clone)]
struct Layer {
    table: Table,
    plan: Plan,
    mods: Vec<Mod>,
    mod_from: f32,
}

impl Layer {
    fn new(profile: &[Segment], plan: Plan, angular: &[Angular], r: f32, hs: f32, edge: f32, accent: Color) -> Layer {
        let mods: Vec<Mod> = angular
            .iter()
            .filter(|a| matches!(a.kind, AngularKind::Bar { .. }) || (a.count > 0 && a.depth > 0.0))
            .map(|a| {
                let mut amp = a.depth;
                let mut cut = (0.0, 0.0);
                let mut fade = 0.22;
                match a.kind {
                    AngularKind::Knurl => {
                        // ridge pitch in pixels; fade before it can alias
                        let pitch = 2.0 * PI * r * a.r_in.max(0.3) / a.count as f32;
                        amp *= ss(1.8, 3.6, pitch);
                        fade = 0.04;
                    }
                    AngularKind::Scallops => {
                        let e = 1.0 - a.depth;
                        let ca = (0.3 * 2.0 * PI / a.count as f32).cos();
                        let rc = if e < ca - 1e-3 { (2.0 * e * ca - 1.0 - e * e) / (2.0 * (e - ca)) } else { 0.2 };
                        cut = (e + rc, rc);
                        fade = 0.03;
                    }
                    AngularKind::Flutes => fade = 0.08,
                    AngularKind::Teeth { .. } => fade = 0.03,
                    AngularKind::Bar { .. } => {}
                }
                Mod { a: *a, amp, cut, fade }
            })
            .collect();
        let mod_from = mods.iter().map(|m| m.a.r_in - m.fade).fold(f32::MAX, f32::min);
        Layer { table: Table::new(profile, r, hs, edge, accent), plan, mods, mod_from }
    }

    /// Pseudo-radius in pixels: the radius at which the round profile is read.
    fn rho(&self, x: f32, y: f32, rr: f32, r: f32) -> f32 {
        let mut out = rr;
        let mut phi = f32::NAN;
        match self.plan {
            Plan::Round => {}
            Plan::Polygon { sides, rounding } => {
                let n = sides.max(3) as f32;
                phi = x.atan2(y);
                let a = PI / n;
                let th = ((phi / (2.0 * a) + 0.5).rem_euclid(1.0) - 0.5) * 2.0 * a;
                let eps = rounding.clamp(0.0, 1.0) * a * 0.9;
                let ths = a - ((a - th.abs()).powi(2) + eps * eps).sqrt();
                out = rr * ths.cos() / (a - eps).cos();
            }
            Plan::Bar { tip, tail, tip_width, tail_width, rounding } => {
                let (xn, yn) = (x.abs() / r, y / r);
                let len = (tip + tail).max(1e-3);
                let w = tail_width + (tip_width - tail_width) * ((yn + tail) / len);
                let d_side = (w - xn) * len / (len * len + (tip_width - tail_width).powi(2)).sqrt();
                let d = smin(smin(tip - yn, yn + tail, rounding), d_side, rounding);
                return (1.0 - d) * r;
            }
        }
        let u = rr / r;
        if self.mods.is_empty() || u < self.mod_from {
            return out;
        }
        if phi.is_nan() {
            phi = x.atan2(y);
        }
        for m in &self.mods {
            let w = ss(m.a.r_in - m.fade, m.a.r_in, u) * if m.a.r_out >= 1.0 { 1.0 } else { ss(m.a.r_out + 0.08, m.a.r_out, u) };
            if w <= 0.0 {
                continue;
            }
            let n = m.a.count as f32;
            let scale = match m.a.kind {
                AngularKind::Flutes => 1.0 - m.amp * (0.5 + 0.5 * (n * phi).cos()),
                AngularKind::Knurl => {
                    let c = (0.5 * n * phi).sin().abs();
                    1.0 - m.amp * (1.0 - c) * (1.0 - c)
                }
                AngularKind::Scallops => {
                    // cuts sit between lobes; a lobe is centred on the pointer
                    let pitch = 2.0 * PI / n;
                    let d = (phi / pitch).rem_euclid(1.0) * pitch - 0.5 * pitch;
                    let (dc, rc) = m.cut;
                    let disc = rc * rc - dc * dc * d.sin() * d.sin();
                    if disc > 0.0 {
                        (dc * d.cos() - disc.sqrt()).min(1.0)
                    } else {
                        1.0
                    }
                }
                AngularKind::Teeth { duty } => {
                    let d = ((phi * n / (2.0 * PI)).rem_euclid(1.0) - 0.5).abs();
                    // d is 0.5 on a lug centre and 0 in the middle of a gap
                    let half = 0.5 * duty.clamp(0.02, 0.98);
                    1.0 - m.amp * ss(0.5 - half + 0.03, 0.5 - half - 0.03, d)
                }
                AngularKind::Bar { tip_width, tail_width, tail } => bar_radius(phi, tip_width, tail_width, tail),
            };
            out /= mixf(1.0, scale, w).max(0.04);
        }
        out
    }
}

fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 1e-5 {
        return a.min(b);
    }
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * k * 0.25
}

#[derive(Clone)]
pub(crate) struct Field {
    /// Base radius in pixels.
    pub r: f32,
    /// Highest point in pixels.
    pub zmax: f32,
    layers: Vec<Layer>,
    ptrs: Vec<Ptr>,
    /// Insert disc: radius, depth (pixels), material.
    insert: Option<(f32, f32, Material)>,
    screw: Option<Decal>,
}

impl Field {
    pub fn new(model: &KnobModel, radius_px: f32, accent: Color) -> Field {
        let r = radius_px.max(2.0);
        let hs = model.height * r;
        let mut layers = vec![Layer::new(&model.profile, model.plan, &model.angular, r, hs, model.edge, accent)];
        if !model.under.is_empty() {
            layers.push(Layer::new(&model.under, Plan::Round, &[], r, hs, model.edge, accent));
        }
        for p in &model.parts {
            layers.push(Layer::new(&p.profile, p.plan, &p.angular, r, hs, model.edge, accent));
        }
        let mut zmax = layers.iter().map(|l| l.table.zmax).fold(0.0, f32::max);
        let ptr = |p: &Pointer| Ptr {
            kind: p.kind,
            r0: p.r0 * r,
            r1: p.r1 * r,
            // never thinner than about two and a half pixels, so small knobs keep a readable
            // pointer
            hw: (p.width * r * 0.5).max(if p.kind == PointerKind::Dot { 1.8 } else { 1.2 }),
            depth: p.depth * r,
            mat: p.material.tinted(accent),
        };
        let ptrs: Vec<Ptr> = model.pointer.iter().chain(model.marks.iter()).map(ptr).collect();
        for p in &ptrs {
            if p.kind == PointerKind::Ridge {
                zmax += p.depth;
            }
        }
        let insert = model.insert.map(|i| (i.r * r, i.depth * hs, i.material.tinted(accent)));
        let screw = model.screw.map(|s| Decal { ang: s.angle, z: s.height * hs, rad: (s.radius * r).max(1.0) });
        Field { r, zmax, layers, ptrs, insert, screw }
    }

    /// Pointer coverage 0..1 at a point.
    fn ptr_mask(&self, p: &Ptr, x: f32, y: f32) -> f32 {
        if p.kind == PointerKind::Dot {
            let d = (x * x + (y - p.r1) * (y - p.r1)).sqrt();
            return ss(p.hw + 0.5, p.hw - 0.5, d);
        }
        if y < p.r0 - 1.0 || y > p.r1 + 1.0 {
            return 0.0;
        }
        ss(p.hw + 0.5, p.hw - 0.5, x.abs()) * ss(p.r0 - 0.7, p.r0 + 0.7, y) * ss(p.r1 + 0.7, p.r1 - 0.7, y)
    }

    fn body_height(&self, x: f32, y: f32, rr: f32) -> f32 {
        let mut h = 0.0f32;
        for l in &self.layers {
            h = h.max(l.table.at(l.rho(x, y, rr, self.r)));
        }
        h
    }

    /// Material and a small normal perturbation at a surface point.
    fn material(&self, x: f32, y: f32) -> (Mat, f32, f32) {
        let rr = (x * x + y * y).sqrt().max(1e-4);
        let mut src = Material::plastic(0.5, 0.5, 0.5, 0.5);
        let mut best = -1.0f32;
        for l in &self.layers {
            let rho = l.rho(x, y, rr, self.r);
            let h = l.table.at(rho);
            if h > best {
                best = h;
                src = l.table.mat(rho / self.r);
            }
        }
        let (mut mat, mut bx, mut by) = self.convert(&src, x, y, rr);
        if let Some((ir, _, im)) = &self.insert {
            let k = ss(ir + 0.5, ir - 0.5, rr);
            if k > 0.0 {
                let (pm, pbx, pby) = self.convert(im, x, y, rr);
                mat = blend(&mat, &pm, k);
                bx = mixf(bx, pbx, k);
                by = mixf(by, pby, k);
            }
        }
        for p in &self.ptrs {
            let k = self.ptr_mask(p, x, y);
            if k > 0.0 {
                let (pm, _, _) = self.convert(&p.mat, x, y, rr);
                mat = blend(&mat, &pm, k);
                bx *= 1.0 - k;
                by *= 1.0 - k;
            }
        }
        (mat, bx, by)
    }

    fn convert(&self, m: &Material, x: f32, y: f32, rr: f32) -> (Mat, f32, f32) {
        let base = v3(m.base[0], m.base[1], m.base[2]);
        let mut tint = grey(1.0);
        let mut rough = m.roughness;
        let (mut bx, mut by) = (0.0f32, 0.0f32);
        let (rx, ry) = (x / rr, y / rr);
        let rings = m.rings + if m.anisotropy > 0.0 { m.grain } else { 0.0 };
        if rings > 0.0 {
            // lathe rings: concentric grooves of varying depth
            let ring = vnoise(rr * 1.3, 3.0) * 0.6 + vnoise(rr * 0.33, 9.0) * 0.4;
            tint = tint * (1.0 + 0.45 * rings * (ring - 0.5));
            let b = (vnoise(rr * 2.3, 1.0) - 0.5) * 0.25 * rings;
            bx += rx * b;
            by += ry * b;
        }
        if m.wood > 0.0 {
            let (sx, sy) = (x / self.r, y / self.r);
            // straight figure along y, bent a little by slow noise
            let g = sx + 0.035 * (vnoise(sy * 0.9 + 2.0, sx * 0.7) - 0.5) + 0.006 * (vnoise(sy * 4.0, sx * 3.0 + 4.0) - 0.5);
            let oct = |f: f32, fy: f32, seed: f32| (vnoise(g * f + seed, sy * fy + seed * 1.7) - 0.5) * ss(1.4, 3.5, self.r / f);
            let band = oct(4.0, 0.12, 1.0) * 0.7 + oct(11.0, 0.18, 5.0) * 0.7 + oct(30.0, 0.3, 9.0) * 0.6 + oct(75.0, 0.6, 13.0) * 0.45;
            // pores: short dark dashes along the grain
            let pore = (vnoise(g * 110.0, sy * 9.0) - 0.62).max(0.0) * 2.6 * ss(1.4, 3.0, self.r / 110.0);
            let t = (1.0 + m.wood * (1.25 * band - 0.6 * pore)).max(0.25);
            // darker figure is also redder: steeper curve for green and blue
            tint = tint * v3(t, t.powf(1.25), t.powf(1.6));
            rough = (rough + 0.2 * m.wood * band).clamp(0.05, 1.0);
        }
        if m.grain > 0.0 && m.anisotropy <= 0.0 {
            if m.clear_coat > 0.0 {
                // slow waviness of the moulding plus a little orange peel in the lacquer
                let (sx, sy) = (x / self.r, y / self.r);
                bx += (vnoise(sx * 2.3 + 4.0, sy * 2.3) - 0.5) * 0.6 * m.grain + (vnoise(x * 0.45, y * 0.45) - 0.5) * 0.05 * m.grain;
                by += (vnoise(sx * 2.3, sy * 2.3 + 7.0) - 0.5) * 0.6 * m.grain + (vnoise(x * 0.45 + 9.0, y * 0.45) - 0.5) * 0.05 * m.grain;
            } else {
                // blasted / satin speckle
                let s = vnoise(x * 1.1, y * 1.1) * 0.6 + vnoise(x * 0.31 + 5.0, y * 0.31) * 0.4;
                tint = tint * (1.0 + m.grain * (s - 0.5));
                rough = (rough + 0.3 * m.grain * (s - 0.5)).max(0.02);
                bx += (vnoise(x * 0.9 + 3.0, y * 0.9) - 0.5) * 0.12 * m.grain;
                by += (vnoise(x * 0.9, y * 0.9 + 3.0) - 0.5) * 0.12 * m.grain;
            }
        }
        let mat = Mat {
            diff: base * tint * (1.0 - m.metallic),
            f0: lerp3(grey(m.f0), base * tint, m.metallic),
            rough,
            aniso: m.anisotropy,
            coat: m.clear_coat,
            sheen: m.sheen,
            trans: m.translucency,
        };
        (mat, bx, by)
    }
}

fn blend(a: &Mat, b: &Mat, k: f32) -> Mat {
    Mat {
        diff: lerp3(a.diff, b.diff, k),
        f0: lerp3(a.f0, b.f0, k),
        rough: mixf(a.rough, b.rough, k),
        aniso: mixf(a.aniso, b.aniso, k),
        coat: mixf(a.coat, b.coat, k),
        sheen: mixf(a.sheen, b.sheen, k),
        trans: mixf(a.trans, b.trans, k),
    }
}

/// Horizon-based openness of a height field at a point of height `h0`, 0..1.
pub(crate) fn horizon_ao(height: &impl Fn(f32, f32) -> f32, x: f32, y: f32, h0: f32, rot: f32, sc: f32, dirs: usize) -> f32 {
    const DIST: [f32; 6] = [1.5, 4.5, 10.0, 19.0, 34.0, 56.0];
    let mut occ = 0.0;
    for d in 0..dirs {
        let a = rot + d as f32 * (2.0 * PI / dirs as f32);
        let (s, c) = a.sin_cos();
        let mut maxs = 0.0f32;
        for &t in DIST.iter() {
            let t = t * sc;
            maxs = maxs.max((height(x + c * t, y + s * t) - h0) / t);
        }
        occ += maxs / (1.0 + maxs * maxs).sqrt();
    }
    1.0 - occ / dirs as f32
}

impl Field {
    /// Gradient, material and occlusion with `dirs` occlusion directions.
    pub fn surf_n(&self, x: f32, y: f32, rot: f32, dirs: usize) -> Surf {
        let e = 0.35;
        let gx = (self.height(x + e, y) - self.height(x - e, y)) / (2.0 * e);
        let gy = (self.height(x, y + e) - self.height(x, y - e)) / (2.0 * e);
        let (mat, bx, by) = self.material(x, y);
        let h = self.height(x, y);
        let ao = horizon_ao(&|a, b| self.height(a, b), x, y, h, rot, self.r / 90.0, dirs);
        // how far the surface stands above its neighbours a short step away, per unit step:
        // zero on any plane, positive on top edges and ridges, negative in grooves
        let d = (0.022 * self.r).max(0.8);
        let around = self.height(x - d, y) + self.height(x + d, y) + self.height(x, y - d) + self.height(x, y + d);
        let curv = ((4.0 * h - around) / (4.0 * d)).clamp(-CURV, CURV);
        Surf { gx, gy, bx, by, ao, curv, mat }
    }
    pub fn floor_ao_n(&self, x: f32, y: f32, rot: f32, dirs: usize) -> f32 {
        horizon_ao(&|a, b| self.height(a, b), x, y, 0.0, rot, self.r / 90.0, dirs)
    }
}

impl Surface for Field {
    fn radius(&self) -> f32 {
        self.r
    }
    fn zmax(&self) -> f32 {
        self.zmax
    }
    /// Height in pixels at a point in cap space (origin at the base centre).
    fn height(&self, x: f32, y: f32) -> f32 {
        let rr = (x * x + y * y).sqrt();
        if rr > self.bound() {
            return 0.0;
        }
        let mut h = self.body_height(x, y, rr);
        if h <= 0.0 {
            return 0.0;
        }
        if let Some((ir, depth, _)) = &self.insert {
            h -= depth * ss(ir + 0.6, ir - 0.6, rr);
        }
        for p in &self.ptrs {
            match p.kind {
                PointerKind::Line => {}
                PointerKind::Groove => h -= p.depth * self.ptr_mask(p, x, y),
                PointerKind::Dot => {
                    if p.depth > 0.0 {
                        let d2 = (x * x + (y - p.r1) * (y - p.r1)) / (p.hw * p.hw);
                        h -= p.depth * (1.0 - d2).max(0.0);
                    }
                }
                PointerKind::Ridge => {
                    if h > 0.3 && y > p.r0 - 1.0 && y < p.r1 + 1.0 {
                        let across = (1.0 - (x / (p.hw + 0.5)).powi(2)).max(0.0).sqrt();
                        h += p.depth * across * ss(p.r0 - 0.7, p.r0 + 0.7, y) * ss(p.r1 + 0.7, p.r1 - 0.7, y);
                    }
                }
            }
        }
        h.max(0.0)
    }
    fn surf(&self, x: f32, y: f32, rot: f32) -> Surf {
        self.surf_n(x, y, rot, AO_DIRS)
    }
    fn floor_ao(&self, x: f32, y: f32, rot: f32) -> f32 {
        self.floor_ao_n(x, y, rot, AO_DIRS)
    }
    fn decal(&self) -> Option<Decal> {
        self.screw
    }
}

/// Polar radius of a tapered bar (tip at 1 ahead, tail behind), corners rounded.
fn bar_radius(phi: f32, tip_w: f32, tail_w: f32, tail: f32) -> f32 {
    let (s, c) = phi.sin_cos();
    let s = s.abs();
    let mut acc = 0.0f32;
    if c > 1e-3 {
        acc += c.powi(8);
    }
    if c < -1e-3 {
        acc += (-c / tail.max(0.05)).powi(8);
    }
    let b = (tip_w - tail_w) / (1.0 + tail);
    let a = (tail_w + b * tail).max(0.02);
    let den = s - b * c;
    if den > 1e-4 {
        acc += (den / a).powi(8);
    }
    acc.max(1e-6).powf(-0.125).min(1.0)
}
