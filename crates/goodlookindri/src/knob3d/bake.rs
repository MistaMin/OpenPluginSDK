//! Angle-independent bake of a knob and the cheap per-angle frame on top of it.
//!
//! The bake is a cap-space buffer: height, gradient, material, texture bump and occlusion
//! on a regular grid, resolved once per (model, radius, accent). A frame only turns the
//! view ray and the light into cap space, samples the buffer and re-lights, so scalloped,
//! fluted, bar-shaped and wood-grained knobs cost the same per angle as round ones.

use std::sync::OnceLock;

use super::field::{Decal, Field, Mat, Surf, Surface, AO_DIRS, AO_ROT, CURV};
use super::model::{KnobModel, Rig, Sprite};
use super::render::{render_surface, Quality};
use super::shade::Lights;
use super::vec::v3;
use crate::color::Color;

/// Byte channels of a cell: diffuse rgb and f0 rgb (square-root coded), roughness,
/// anisotropy, coat, sheen, translucency, bump x and y, occlusion, curvature.
const CH: usize = 15;
/// Bump values are stored over +-BUMP.
const BUMP: f32 = 0.8;

/// One grid cell, 24 bytes.
#[derive(Clone, Copy)]
struct Cell {
    /// Height gradient.
    gx: f32,
    gy: f32,
    b: [u8; CH],
}

fn q8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

pub(crate) struct Grid {
    r: f32,
    zmax: f32,
    /// Cells per row and column.
    n: usize,
    /// Cells per pixel.
    k: f32,
    /// Half extent in pixels.
    half: f32,
    h: Vec<f32>,
    cells: Vec<Cell>,
    /// Coarse panel occlusion around the knob.
    fnn: usize,
    fk: f32,
    fhalf: f32,
    fao: Vec<f32>,
    decal: Option<Decal>,
}

fn par_rows<T: Send>(data: &mut [T], n: usize, f: impl Fn(usize, usize) -> T + Sync) {
    let threads = std::thread::available_parallelism().map(|t| t.get()).unwrap_or(4).min((n / 8).max(1));
    let rows_per = n.div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (ci, chunk) in data.chunks_mut(rows_per * n).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (i, out) in chunk.iter_mut().enumerate() {
                    *out = f(i % n, ci * rows_per + i / n);
                }
            });
        }
    });
}

impl Grid {
    fn new(field: &Field) -> Grid {
        let r = field.r;
        // denser for small knobs, where every edge is only a pixel or two wide
        let k = (120.0 / r).clamp(1.5, 4.0);
        let half = field.bound() + 1.0;
        let n = (2.0 * half * k).ceil() as usize + 2;
        let mut h = vec![0.0f32; n * n];
        par_rows(&mut h, n, |i, j| field.height(i as f32 / k - half, j as f32 / k - half));
        let blank = Cell { gx: 0.0, gy: 0.0, b: [0, 0, 0, 0, 0, 0, 128, 0, 0, 0, 0, 127, 127, 255, 127] };
        let mut cells = vec![blank; n * n];
        let lim = (half + 1.0) * (half + 1.0);
        par_rows(&mut cells, n, |i, j| {
            let (x, y) = (i as f32 / k - half, j as f32 / k - half);
            if x * x + y * y > lim {
                return blank;
            }
            let s = field.surf_n(x, y, AO_ROT, AO_DIRS);
            let m = s.mat;
            let sq = |v: f32| q8(v.max(0.0).sqrt());
            // 127 is exactly zero
            let bump = |v: f32| ((v / (2.0 * BUMP)).clamp(-0.5, 0.5) * 254.0 + 127.5) as u8;
            Cell {
                gx: s.gx,
                gy: s.gy,
                b: [sq(m.diff.x), sq(m.diff.y), sq(m.diff.z), sq(m.f0.x), sq(m.f0.y), sq(m.f0.z), q8(m.rough), q8(m.aniso), q8(m.coat), q8(m.sheen), q8(m.trans), bump(s.bx), bump(s.by), q8(s.ao), ((s.curv / (2.0 * CURV)).clamp(-0.5, 0.5) * 254.0 + 127.5) as u8],
            }
        });
        let fk = (36.0 / r).clamp(0.5, 2.0);
        let fhalf = r * 1.75 + 3.0;
        let fnn = (2.0 * fhalf * fk).ceil() as usize + 2;
        let mut fao = vec![1.0f32; fnn * fnn];
        par_rows(&mut fao, fnn, |i, j| field.floor_ao_n(i as f32 / fk - fhalf, j as f32 / fk - fhalf, AO_ROT, AO_DIRS));
        Grid { r, zmax: field.zmax, n, k, half, h, cells, fnn, fk, fhalf, fao, decal: field.decal() }
    }

    fn bytes(&self) -> usize {
        self.h.len() * 4 + self.cells.len() * std::mem::size_of::<Cell>() + self.fao.len() * 4
    }

    /// Cell index and bilinear weights, or `None` outside the grid.
    #[inline]
    fn locate(&self, x: f32, y: f32) -> Option<(usize, f32, f32)> {
        let (u, v) = ((x + self.half) * self.k, (y + self.half) * self.k);
        let m = (self.n - 1) as f32;
        if !(u >= 0.0 && v >= 0.0 && u < m && v < m) {
            return None;
        }
        let (i, j) = (u as usize, v as usize);
        Some((j * self.n + i, u - i as f32, v - j as f32))
    }
}

impl Surface for Grid {
    fn radius(&self) -> f32 {
        self.r
    }
    fn zmax(&self) -> f32 {
        self.zmax
    }
    #[inline]
    fn height(&self, x: f32, y: f32) -> f32 {
        let Some((i, tx, ty)) = self.locate(x, y) else { return 0.0 };
        let h = &self.h;
        let top = h[i] + (h[i + 1] - h[i]) * tx;
        let bot = h[i + self.n] + (h[i + self.n + 1] - h[i + self.n]) * tx;
        top + (bot - top) * ty
    }
    fn surf(&self, x: f32, y: f32, _rot: f32) -> Surf {
        let mut o = [0.0f32; CH];
        let (mut gx, mut gy) = (0.0f32, 0.0f32);
        if let Some((i, tx, ty)) = self.locate(x, y) {
            let idx = [i, i + 1, i + self.n, i + self.n + 1];
            // cells off the knob carry no material: weight them out so the silhouette
            // keeps the colour of the body
            let mut w = [(1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty];
            let mut sum = 0.0;
            for q in 0..4 {
                if self.h[idx[q]] <= 0.0 {
                    w[q] *= 0.02;
                }
                sum += w[q];
            }
            let inv = 1.0 / (sum.max(1e-9) * 255.0);
            for q in 0..4 {
                let c = &self.cells[idx[q]];
                let wq = w[q] * inv;
                gx += c.gx * wq * 255.0;
                gy += c.gy * wq * 255.0;
                for (acc, &b) in o.iter_mut().zip(&c.b) {
                    *acc += b as f32 * wq;
                }
            }
        } else {
            o[13] = 1.0;
            o[14] = 127.0 / 255.0;
        }
        let bump = |v: f32| (v * 255.0 - 127.0) / 254.0 * 2.0 * BUMP;
        Surf {
            gx,
            gy,
            bx: bump(o[11]),
            by: bump(o[12]),
            ao: o[13],
            curv: (o[14] * 255.0 - 127.0) / 254.0 * 2.0 * CURV,
            mat: Mat { diff: v3(o[0] * o[0], o[1] * o[1], o[2] * o[2]), f0: v3(o[3] * o[3], o[4] * o[4], o[5] * o[5]), rough: o[6], aniso: o[7], coat: o[8], sheen: o[9], trans: o[10] },
        }
    }
    fn floor_ao(&self, x: f32, y: f32, _rot: f32) -> f32 {
        let (u, v) = ((x + self.fhalf) * self.fk, (y + self.fhalf) * self.fk);
        let m = (self.fnn - 1) as f32;
        if !(u >= 0.0 && v >= 0.0 && u < m && v < m) {
            return 1.0;
        }
        let (i, j) = (u as usize, v as usize);
        let (tx, ty) = (u - i as f32, v - j as f32);
        let (q, n) = (j * self.fnn + i, self.fnn);
        let f = &self.fao;
        let top = f[q] + (f[q + 1] - f[q]) * tx;
        let bot = f[q + n] + (f[q + n + 1] - f[q + n]) * tx;
        top + (bot - top) * ty
    }
    fn decal(&self) -> Option<Decal> {
        self.decal
    }
}

/// Everything about a knob that does not depend on its angle. [`KnobBake::frame`] turns
/// it and re-lights it.
pub struct KnobBake {
    field: Field,
    lights: Lights,
    grid: OnceLock<Grid>,
}

impl KnobBake {
    /// Every model can be baked; the `Option` is kept for source compatibility and is
    /// always `Some`.
    pub fn new(model: &KnobModel, rig: &Rig, radius_px: f32, accent: Color) -> Option<KnobBake> {
        Some(Self::build(model, rig, radius_px, accent))
    }

    pub(crate) fn build(model: &KnobModel, rig: &Rig, radius_px: f32, accent: Color) -> KnobBake {
        KnobBake { field: Field::new(model, radius_px, accent), lights: Lights::new(rig), grid: OnceLock::new() }
    }

    /// The knob at `angle` under another rig (the bake itself holds no lighting).
    pub(crate) fn frame_lit(&self, lights: &Lights, angle: f32) -> Sprite {
        render_surface(self.grid(), lights, angle, Quality::FRAME)
    }

    fn grid(&self) -> &Grid {
        self.grid.get_or_init(|| Grid::new(&self.field))
    }

    /// Do the angle-independent work now (otherwise it happens on the first frame).
    pub fn warm(&self) {
        self.grid();
    }

    /// The knob at `angle`.
    pub fn frame(&self, angle: f32) -> Sprite {
        render_surface(self.grid(), &self.lights, angle, Quality::FRAME)
    }

    /// The same frame taken from the analytic field instead of the baked buffer, with the
    /// frame's sampling pattern: what the bake has to reproduce.
    #[cfg(test)]
    pub(crate) fn frame_unbaked(&self, angle: f32) -> Sprite {
        render_surface(&self.field, &self.lights, angle, Quality::FRAME)
    }

    /// Approximate heap size in bytes.
    pub(crate) fn bytes(&self) -> usize {
        self.grid.get().map_or(0, |g| g.bytes()) + 40_000
    }
}
