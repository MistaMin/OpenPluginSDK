//! Procedural studio environment and the surface shader. Everything is linear light.
//!
//! The environment (softboxes, room, rim ring) is evaluated once per rig into a small
//! pyramid of pre-blurred octahedral maps, one per roughness, so that shading a sample is
//! a few table lookups.

use std::sync::{Arc, Mutex};

use super::field::Mat;
use super::model::Rig;
use super::vec::{clamp01, cross, dot, grey, lerp3, mixf, norm, reflect, srgb, ss, to_linear, v3, V3};

/// Colour of the key softbox.
const KEY: V3 = v3(3.6, 3.5, 3.35);
/// Roughness of each map level.
const LEVELS: [f32; 10] = [0.0, 0.05, 0.10, 0.16, 0.24, 0.34, 0.48, 0.65, 0.85, 1.0];
const SIZES: [usize; 10] = [224, 192, 160, 128, 96, 64, 48, 32, 24, 24];

struct Level {
    n: usize,
    /// r, g, b of the room and lights, weight of the panel colour, key softbox.
    px: Vec<[f32; 5]>,
}

pub(crate) struct EnvMaps {
    levels: Vec<Level>,
}

static ENVS: Mutex<Vec<(u64, Arc<EnvMaps>)>> = Mutex::new(Vec::new());

/// Rig with derived vectors.
pub(crate) struct Lights {
    pub rig: Rig,
    /// Unit vector towards the key light (x right, y up, z out of the panel).
    pub key: V3,
    /// Unit vector towards the camera.
    pub view: V3,
    /// Linear panel colour.
    pub floor: V3,
    /// What a mirror sees when it looks at the panel.
    floor_refl: V3,
    /// Strength of the grazing rim light, already scaled for the panel brightness.
    rim_gain: f32,
    /// Strength of the two low front kickers that draw the satin band on walls.
    kick_gain: f32,
    env: Arc<EnvMaps>,
}

fn view_of(rig: &Rig) -> V3 {
    let (st, ct) = rig.tilt.sin_cos();
    v3(0.0, -st, ct)
}

impl Lights {
    pub fn new(rig: &Rig) -> Lights {
        let (se, ce) = rig.key_elevation.sin_cos();
        let (sa, ca) = rig.key_azimuth.sin_cos();
        let f = rig.floor;
        let floor = v3(to_linear(f.r() as f32 / 255.0), to_linear(f.g() as f32 / 255.0), to_linear(f.b() as f32 / 255.0));
        let lum = 0.2126 * floor.x + 0.7152 * floor.y + 0.0722 * floor.z;
        // the panel is itself lit by the key and keeps a little room light when dark; a dark
        // panel also throws back the rim lights that graze it, which is what outlines black
        // walls on a black panel
        let floor_refl = floor * 0.60 + grey(0.035) + grey(0.035 * rig.rim) * (1.0 - lum).powi(3);
        let rim_gain = 0.17 * rig.rim * (0.22 + 0.78 * (1.0 - lum.sqrt()).max(0.0).powi(2));
        let kick_gain = rig.rim * (0.38 + 0.62 * (1.0 - lum.sqrt()).max(0.0).powi(2));
        let fp = rig.env_fingerprint();
        let env = {
            let mut g = ENVS.lock().unwrap();
            match g.iter().find(|e| e.0 == fp) {
                Some(e) => e.1.clone(),
                None => {
                    let e = Arc::new(EnvMaps::build(rig));
                    if g.len() >= 6 {
                        g.remove(0);
                    }
                    g.push((fp, e.clone()));
                    e
                }
            }
        };
        Lights { rig: *rig, key: v3(-ce * sa, ce * ca, se), view: view_of(rig), floor, floor_refl, rim_gain, kick_gain, env }
    }

    /// Environment in direction `d` blurred for `rough`: (key softbox, room, panel).
    #[inline]
    fn look(&self, d: V3, rough: f32) -> (f32, V3, V3) {
        let s = self.env.sample(d, rough);
        (s[4], v3(s[0], s[1], s[2]), self.floor_refl * s[3])
    }
}

/// Soft rectangular light in gnomonic coordinates around `c`.
fn softbox(d: V3, c: V3, hint: V3, hu: f32, hv: f32, blur: f32) -> f32 {
    let cd = dot(d, c);
    if cd <= 0.02 {
        return 0.0;
    }
    let bx = norm(hint - c * dot(hint, c));
    let by = cross(c, bx);
    let u = dot(d, bx) / cd;
    let v = dot(d, by) / cd;
    let a = ss(hu + blur, hu - blur, u.abs()) * ss(hv + blur, hv - blur, v.abs());
    let fall = 1.0 - 0.35 * ((u / hu).powi(2) + (v / hv).powi(2)).min(2.0) * 0.5;
    // a blurred box spreads its energy: keep the integral roughly constant
    let spread = (hu * hv) / ((hu.max(blur)) * (hv.max(blur)));
    a * fall * spread
}

/// Analytic environment: [room r, g, b, panel weight, key].
fn env(rig: &Rig, key_dir: V3, view: V3, d: V3, rough: f32) -> [f32; 5] {
    let blur = 0.015 + 1.4 * rough * rough;
    let e = rig.environment;
    // studio room: warm-grey walls, brighter towards the top of the screen, panel below
    let horizon = ss(-0.40 - blur, 0.0 + blur, d.z);
    let wall = lerp3(srgb(0.20, 0.195, 0.19), srgb(0.50, 0.49, 0.48), clamp01(0.5 + 0.5 * d.y));
    let mut other = wall * (e * horizon);
    let mut floor_w = (1.0 - horizon) * (0.60 + 0.40 * clamp01(0.5 - 0.5 * d.y));
    // big overhead diffuser near the camera, with a sweep so flat faces get a gradient
    let zen = ss(0.50 - blur, 0.96, d.z);
    let sweep = ss(-0.45 - blur, 0.45 + blur, d.y - 0.35 * d.x);
    other += srgb(0.66, 0.66, 0.67) * (zen * (0.22 + 0.78 * sweep) * e);
    // tall strip light at the right, slightly cool
    let strip = softbox(d, norm(v3(0.86, 0.1, 0.48)), v3(0.0, 1.0, 0.0), 0.8, 0.07, blur);
    other += v3(1.5, 1.6, 1.75) * (strip * e);
    // second, thinner strip left of the key: gives glossy caps a double streak
    let strip2 = softbox(d, norm(v3(-0.80, 0.25, 0.54)), v3(0.0, 1.0, 0.0), 0.55, 0.035, blur);
    other += v3(1.25, 1.2, 1.1) * (strip2 * e);
    // broad fill panel behind the key: the big soft shape glossy domes mirror
    let fill = softbox(d, norm(v3(-0.55, 0.62, 0.56)), v3(1.0, 0.0, 0.0), 0.55, 0.26, blur);
    other += v3(1.25, 1.22, 1.16) * (fill * e);
    // low warm kicker from the bottom, grazing
    let kick = softbox(d, norm(v3(-0.35, -0.85, 0.3)), v3(1.0, 0.0, 0.0), 0.5, 0.10, blur);
    other += v3(1.0, 0.90, 0.78) * (kick * e);
    // two tall edge lights, left and right, standing on the table: they draw the long
    // streaks down cone skirts and the bright lines on shoulders and lobes
    let edge_l = softbox(d, norm(v3(-0.90, 0.30, 0.30)), v3(0.0, 0.0, 1.0), 0.50, 0.055, blur);
    let edge_r = softbox(d, norm(v3(0.93, -0.12, 0.34)), v3(0.0, 0.0, 1.0), 0.50, 0.045, blur);
    other += v3(1.25, 1.27, 1.35) * (edge_l * e) + v3(1.0, 1.0, 1.05) * (edge_r * e);
    // rim ring: a low band of light all round the set, strongest at the sides and the back.
    // Rounded edges and chamfers pick it up as the thin bright line that outlines a knob.
    let band = ss(-0.02 - blur, 0.07 + 0.5 * blur, d.z) * ss(0.40 + blur, 0.16, d.z);
    let h = (d.x * d.x + d.y * d.y).sqrt().max(1e-4);
    let side = 0.30 + 0.55 * (d.x / h).powi(2) + 0.35 * clamp01(d.y / h);
    other += v3(0.55, 0.58, 0.64) * (band * side * rig.rim * e);
    // dark flag in front of the camera keeps mirror metal from going flat grey
    let rd = ((d.x - view.x).powi(2) + (d.y - view.y).powi(2)).sqrt() / d.z.max(0.05);
    let gobo = 1.0 - 0.25 * ss(0.20 + blur + 0.06, 0.04, rd);
    other = other * gobo;
    floor_w *= gobo;
    // key softbox
    let k = softbox(d, key_dir, v3(1.0, 0.0, 0.0), rig.key_size * 1.25, rig.key_size, blur);
    [other.x, other.y, other.z, floor_w, k * rig.key_intensity]
}

fn oct_inv(u: f32, v: f32) -> V3 {
    let (px, py) = (2.0 * u - 1.0, 2.0 * v - 1.0);
    let z = 1.0 - px.abs() - py.abs();
    if z >= 0.0 {
        norm(v3(px, py, z))
    } else {
        norm(v3((1.0 - py.abs()) * px.signum(), (1.0 - px.abs()) * py.signum(), z))
    }
}

impl EnvMaps {
    fn build(rig: &Rig) -> EnvMaps {
        let (se, ce) = rig.key_elevation.sin_cos();
        let (sa, ca) = rig.key_azimuth.sin_cos();
        let key = v3(-ce * sa, ce * ca, se);
        let view = view_of(rig);
        let mut levels: Vec<Level> = SIZES.iter().map(|&n| Level { n, px: vec![[0.0; 5]; n * n] }).collect();
        std::thread::scope(|s| {
            for (li, lv) in levels.iter_mut().enumerate() {
                s.spawn(move || {
                    let n = lv.n;
                    for (i, p) in lv.px.iter_mut().enumerate() {
                        let d = oct_inv((i % n) as f32 / (n - 1) as f32, (i / n) as f32 / (n - 1) as f32);
                        *p = env(rig, key, view, d, LEVELS[li]);
                    }
                });
            }
        });
        EnvMaps { levels }
    }

    #[inline]
    fn sample(&self, d: V3, rough: f32) -> [f32; 5] {
        let s = 1.0 / (d.x.abs() + d.y.abs() + d.z.abs()).max(1e-9);
        let (mut px, mut py) = (d.x * s, d.y * s);
        if d.z < 0.0 {
            let (ax, ay) = (px.abs(), py.abs());
            px = (1.0 - ay) * if d.x < 0.0 { -1.0 } else { 1.0 };
            py = (1.0 - ax) * if d.y < 0.0 { -1.0 } else { 1.0 };
        }
        let (u, v) = (px * 0.5 + 0.5, py * 0.5 + 0.5);
        let rough = rough.clamp(0.0, 1.0);
        let mut i = 0;
        while i + 2 < LEVELS.len() && LEVELS[i + 1] <= rough {
            i += 1;
        }
        let t = (rough - LEVELS[i]) / (LEVELS[i + 1] - LEVELS[i]);
        let a = self.levels[i].at(u, v);
        if t <= 0.001 {
            return a;
        }
        let b = self.levels[i + 1].at(u, v);
        let mut o = [0.0; 5];
        for c in 0..5 {
            o[c] = a[c] + (b[c] - a[c]) * t;
        }
        o
    }
}

impl Level {
    #[inline]
    fn at(&self, u: f32, v: f32) -> [f32; 5] {
        let m = (self.n - 1) as f32;
        let (fx, fy) = ((u * m).clamp(0.0, m - 0.001), (v * m).clamp(0.0, m - 0.001));
        let (ix, iy) = (fx as usize, fy as usize);
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let i = iy * self.n + ix;
        let (a, b, c, d) = (&self.px[i], &self.px[i + 1], &self.px[i + self.n], &self.px[i + self.n + 1]);
        let mut o = [0.0; 5];
        for k in 0..5 {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bot = c[k] + (d[k] - c[k]) * tx;
            o[k] = top + (bot - top) * ty;
        }
        o
    }
}

/// Fresnel with the grazing peak damped for rough surfaces.
fn schlick_r(f0: V3, c: f32, rough: f32) -> V3 {
    let m = (1.0 - c.clamp(0.0, 1.0)).powi(5);
    let g = (1.0 - rough).max(0.0).powi(2);
    let top = v3(g.max(f0.x), g.max(f0.y), g.max(f0.z));
    f0 + (top - f0) * m
}

/// Shade one surface point. `v` points to the camera, `t` runs across the lathe grooves,
/// `fv` says how much of the lit panel a mirror at this point can see (little near the foot),
/// `curv` is the convexity of the surface (positive on edges and ridges).
#[allow(clippy::too_many_arguments)]
pub(crate) fn shade(l: &Lights, n: V3, v: V3, t: V3, m: &Mat, ao: f32, sh: f32, fv: f32, curv: f32) -> V3 {
    let nv = dot(n, v).max(1e-3);
    // the softbox maps hold the key at one exposure; rough paint needs its broad highlight
    // lifted to read at all, rough metal needs it held back or every satin face clips
    let metal = ss(0.2, 0.6, (m.f0.x + m.f0.y + m.f0.z) / 3.0);
    let key_gain = mixf((1.0 + 2.0 * ss(0.15, 0.6, m.rough)) * (1.0 - 0.6 * m.coat), 1.0 - 0.45 * ss(0.2, 0.5, m.rough), metal);
    // satin metal scatters part of what it reflects out of the picture
    let spec_gain = 1.0 - 0.24 * metal * ss(0.2, 0.45, m.rough);
    let spec_ao = ao.powf(1.3) * mixf(1.0, sh, 0.15);
    // base specular; spun metal spreads the reflection across its grooves
    let taps = if m.aniso > 0.01 { 9 } else { 1 };
    // each tap must be at least as wide as the gap to the next, or the fan shows as bands
    let rough = if taps > 1 { m.rough.max((2.2 * m.aniso / taps as f32 / 1.4).sqrt()) } else { m.rough };
    let mut spec = grey(0.0);
    for j in 0..taps {
        let s = if taps > 1 { ((j as f32 + 0.5) / taps as f32 * 2.0 - 1.0) * m.aniso } else { 0.0 };
        let nj = if taps > 1 { norm(n + t * s) } else { n };
        let r = reflect(v * -1.0, nj);
        let f = schlick_r(m.f0, dot(nj, v).max(0.0), m.rough);
        let (k, o, fl) = l.look(r, rough);
        spec += f * (KEY * (k * sh * key_gain) + (o + fl * fv) * spec_ao);
    }
    spec = spec * (spec_gain / taps as f32);
    // diffuse: wrapped key (it is a big softbox), sky from the top of the screen, a frontal
    // fill and the light the panel bounces back at the walls
    let rig = &l.rig;
    let ndl = clamp01((dot(n, l.key) + 0.25) / 1.25);
    // (the low frontal fill keeps the walls that face the viewer alive)
    let amb = (grey(0.20) * (0.5 + 0.5 * n.y) + grey(0.13) * n.z.max(0.0) + grey(0.30) * (0.25 * n.z - 0.9 * n.y).max(0.0) + grey(0.05) + (l.floor * 0.55 + grey(0.02)) * (0.5 - 0.5 * n.z)) * rig.ambient;
    let fd = 1.0 - schlick_r(grey(0.04), nv, 0.0).x;
    let light = grey(0.78 * rig.key_intensity * ndl * sh) + amb * (ao * mixf(1.0, sh, 0.2));
    let mut diffuse = m.diff * light * fd;
    if m.trans > 0.0 {
        // light that went in comes back out further round and more saturated
        let wrap = clamp01((dot(n, l.key) + 0.95) / 1.95);
        let deep = v3(m.diff.x.powf(0.6), m.diff.y.powf(0.6), m.diff.z.powf(0.6)) * m.diff * 2.2;
        let through = deep * ((0.50 * wrap * wrap + 0.22 * (1.0 - nv).powi(2)) * mixf(0.6, 1.0, sh) * ao);
        diffuse = diffuse * (1.0 - 0.45 * m.trans) + through * m.trans;
    }
    if m.sheen > 0.0 {
        let g = (1.0 - nv).powi(3);
        diffuse += (m.diff * 0.5 + grey(0.5)) * light * (0.22 * m.sheen * g);
        diffuse += (m.diff + grey(0.03)) * (0.9 * m.sheen * ndl * sh * (1.0 - nv * nv));
    }
    let mut col = diffuse + spec;
    if m.coat > 0.0 {
        let fc = m.coat * schlick_r(grey(0.04), nv, 0.0).x;
        let r = reflect(v * -1.0, n);
        // a thinner coat is a softer, dimmer mirror
        let (k, o, fl) = l.look(r, 0.03 + 0.15 * (1.0 - m.coat));
        // lacquer is a mirror: it shows the lights at their real (far brighter) level and
        // the dim room hardly at all, which is what makes black gloss read as black
        col = col * (1.0 - fc) + (KEY * (k * 5.0 * sh) + (o * (grey(0.22) + o * 2.1) + fl * (0.5 * fv)) * spec_ao) * fc;
    }
    // rim light: grazes whatever turns away from the eye towards the sides and the back, so
    // rounded edges and dome flanks get the thin light line that outlines dark knobs
    if l.rim_gain > 0.0 {
        let w = (1.0 - nv).powi(3) * clamp01(0.3 + 0.9 * n.y + 0.5 * n.x.abs());
        let gloss = m.coat.max(1.0 - m.rough);
        col += v3(0.86, 0.90, 1.0) * (l.rim_gain * w * (0.55 + 0.45 * gloss) * ao);
    }
    // kickers: two low strip lights in front, left and right of the camera. A vertical wall
    // mirrors only the panel, so on a dark panel these are what give it a satin band and
    // make flutes and knurling glint.
    let gloss = m.coat.max(1.0 - m.rough).clamp(0.0, 1.0);
    if l.kick_gain > 0.0 {
        let p = mixf(5.0, 30.0, gloss * gloss);
        let dl = dot(n, KICK_L).max(0.0);
        let dr = dot(n, KICK_R).max(0.0);
        let lobe = v3(1.0, 0.98, 0.94) * dl.powf(p) + v3(0.86, 0.92, 1.0) * (dr.powf(p) * 0.8);
        // paint and plastic take the band at a fixed strength, bare metal by its own colour
        let fk = lerp3(grey(0.05 + 0.10 * gloss + 0.05 * m.sheen), m.f0 * 0.16, metal);
        let wide = (m.diff + grey(0.004)) * ((dl * dl + dr * dr * 0.8) * 0.7);
        col += (lobe * fk + wide) * (l.kick_gain * (0.35 + 0.65 * ao) * mixf(0.55, 1.0, sh));
    }
    // a convex edge or ridge is a tiny mirror that fans out over half the room, so it always
    // finds a light: the crisp line along a top edge, the glint on each knurl ridge and each
    // lobe. Grooves get the opposite, a little extra darkness.
    if curv > 0.0 {
        let k = ss(0.05, 0.55, curv);
        let facing = 0.42 + 0.58 * clamp01(0.5 + 0.5 * dot(n, l.key)) * mixf(0.5, 1.0, sh);
        let fe = lerp3(grey(0.13 + 0.20 * gloss), m.f0 * 0.55, metal) + m.diff * 0.35;
        col += v3(1.0, 0.99, 0.97) * fe * (k * facing * (0.4 + 0.6 * ao) * (0.45 + 0.55 * l.kick_gain.min(1.0)));
    } else {
        col = col * (1.0 - 0.4 * ss(0.05, 0.6, -curv));
    }
    col
}

/// Directions towards the two front kickers.
const KICK_L: V3 = v3(-0.800, -0.555, 0.228);
const KICK_R: V3 = v3(0.853, -0.452, 0.261);
