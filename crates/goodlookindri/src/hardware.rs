// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Marcos Quirino Deida

//! Procedural hardware: faceplates and their finishes, screws, rack ears, keys and the printed
//! or engraved legends on a plate.
//!
//! Everything is lit by one [`LightRig`] (the `_with` variants take one; the short forms use
//! [`LightRig::studio`]), so bevels, sheens, screw domes and cast shadows agree with the knobs.
//!
//! Surface grain (brushed aluminium, bead-blasted anodising, powder coat, hammertone, wood) is a
//! small seamless texture built from a deterministic hash, once per backing scale (see
//! [`set_device_scale`]), and tiled with [`Scene::image`] one texel per device pixel. Hosts that replay
//! [`Scene::lower_basic`] drop images and still get the gradients, bevels and brushing streaks.

use crate::color::Color;
use crate::light::LightRig;
use crate::scene::{Blend, Image, Paint, Point, Rect, Scene, Shape, Stops};
use std::f32::consts::PI;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Plain painted plate (very fine paint grain).
pub const FINISH_FLAT: i32 = 0;
/// Brushed aluminium: anisotropic grain, short streaks, soft vertical sheen bands.
pub const FINISH_BRUSHED: i32 = 1;
pub const FINISH_WOOD: i32 = 2;
pub const FINISH_SCANLINES: i32 = 3;
pub const FINISH_HAZARD: i32 = 4;
pub const FINISH_GRID: i32 = 5;
pub const FINISH_NEON_EDGE: i32 = 6;
pub const FINISH_HORIZON: i32 = 7;
/// Bead-blasted anodised aluminium: fine isotropic grain and a broad soft sheen.
pub const FINISH_ANODISED: i32 = 8;
/// Powder coat / stove enamel: sparse speckle and a faint orange peel.
pub const FINISH_POWDER: i32 = 9;
/// Hammertone paint: hammered dimples with a metallic mottle.
pub const FINISH_HAMMERTONE: i32 = 10;
/// Satin black: the plate colours are pulled toward black, fine grain, wide soft highlight.
pub const FINISH_SATIN_BLACK: i32 = 11;
/// One past the last finish number.
pub const FINISH_COUNT: i32 = 12;

const WHITE: Color = Color(0xffffffff);
const BLACK: Color = Color(0xff000000);

fn hline(scene: &mut Scene, y: f32, x0: f32, x1: f32, color: Color, w: f32) {
    scene.line(Point::new(x0, y), Point::new(x1, y), color, w);
}
fn vline(scene: &mut Scene, x: f32, y0: f32, y1: f32, color: Color, w: f32) {
    scene.line(Point::new(x, y0), Point::new(x, y1), color, w);
}

// ---------------------------------------------------------------------------------------------
// Hashing, noise, textures
// ---------------------------------------------------------------------------------------------

fn hash_u(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^= x >> 16;
    x
}
fn hash2(x: u32, y: u32, seed: u32) -> u32 {
    hash_u(x.wrapping_mul(0x9e37_79b1) ^ hash_u(y.wrapping_add(seed.wrapping_mul(0x85eb_ca77))))
}
/// 0..1
fn unit(h: u32) -> f32 { (h >> 8) as f32 / 16_777_216.0 }
/// -1..1
fn snorm(h: u32) -> f32 { unit(h) * 2.0 - 1.0 }
/// Triangular -1..1 noise (sum of two uniforms), softer than white noise.
fn tri(x: u32, y: u32, seed: u32) -> f32 { unit(hash2(x, y, seed)) + unit(hash2(x, y, seed ^ 0x5bd1_e995)) - 1.0 }
/// Stable seed from a position, so the same plate always gets the same streaks.
fn pos_seed(x: f32, y: f32) -> u32 { hash_u((x * 8.0).round() as i32 as u32 ^ hash_u((y * 8.0).round() as i32 as u32).wrapping_add(0x27d4_eb2f)) }

/// Value noise, -1..1, periodic every `period` lattice cells.
fn value_noise(x: f32, y: f32, period: i64, seed: u32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (s(fx), s(fy));
    let p = |i: i64, j: i64| snorm(hash2(i.rem_euclid(period) as u32, j.rem_euclid(period) as u32, seed));
    let (i, j) = (xi as i64, yi as i64);
    let a = p(i, j) + (p(i + 1, j) - p(i, j)) * sx;
    let b = p(i, j + 1) + (p(i + 1, j + 1) - p(i, j + 1)) * sx;
    a + (b - a) * sy
}

/// Signed texel: positive lightens (white at that alpha), negative darkens (black).
fn signed_px(v: f32) -> u32 {
    let a = (v.clamp(-1.0, 1.0).abs() * 255.0).round() as u32;
    if v >= 0.0 { (a << 24) | (a << 16) | (a << 8) | a } else { a << 24 }
}

/// Box blur of a periodic signal, normalised to unit standard deviation.
fn blur_wrap_norm(src: &[f32], len: usize) -> Vec<f32> {
    let n = src.len();
    let mut out = vec![0.0; n];
    let mut acc: f32 = (0..len).map(|k| src[k % n]).sum();
    for x in 0..n {
        out[(x + len / 2) % n] = acc / len as f32;
        acc += src[(x + len) % n] - src[x];
    }
    let var = out.iter().map(|v| v * v).sum::<f32>() / n as f32;
    let k = 1.0 / var.sqrt().max(1e-6);
    out.iter_mut().for_each(|v| *v *= k);
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Tex { Grain = 0, Brushed = 1, Speckle = 2, Hammer = 3, Wood = 4 }

static DEVICE_SCALE: AtomicU32 = AtomicU32::new(0x4000_0000); // 2.0

/// Tells the finishes which backing scale the host rasterises at (device pixels per scene
/// unit), so surface grain is generated one texel per device pixel and stays sharp at 1x, 1.5x,
/// 2x and 3x alike. Clamped to 1..4 and rounded to quarter steps. The default is 2.
pub fn set_device_scale(scale: f32) {
    let s = if scale.is_finite() { (scale.clamp(1.0, 4.0) * 4.0).round() / 4.0 } else { 2.0 };
    DEVICE_SCALE.store(s.to_bits(), Ordering::Relaxed);
}
/// The scale last given to [`set_device_scale`].
pub fn device_scale() -> f32 { f32::from_bits(DEVICE_SCALE.load(Ordering::Relaxed)) }

fn texture(t: Tex) -> Arc<Image> { texture_at(t, device_scale()) }

fn texture_at(t: Tex, scale: f32) -> Arc<Image> {
    static CACHE: OnceLock<Mutex<HashMap<(Tex, u32), Arc<Image>>>> = OnceLock::new();
    let key = (t, (scale * 4.0).round() as u32);
    let mut map = CACHE.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner());
    map.entry(key).or_insert_with(|| Arc::new(build_texture(t, key.1 as f32 / 4.0))).clone()
}

/// Builds a seamless tile at `scale` texels per scene unit. Structure (dimples, peel, rings,
/// streak lengths) is laid out in scene units so it keeps its physical size; the finest grain is
/// one texel, so it is as sharp as the display allows.
fn build_texture(t: Tex, scale: f32) -> Image {
    let rig = LightRig::studio();
    let l = rig.screen_dir();
    let k = scale / 2.0;
    let dim = |units: f32| ((units * scale).round() as u32).max(8);
    // Single-texel noise covers more of the plate at low scales; ease it there.
    let fine = (0.72 + 0.28 * k).min(1.12);
    match t {
        Tex::Grain => {
            let (w, h) = (dim(128.0), dim(128.0));
            let (ux, uy) = (256.0 / w as f32, 256.0 / h as f32);
            let px: Vec<u32> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| {
                let v = 0.8 * fine * tri(x, y, 11) + 0.35 * value_noise(x as f32 * ux / 4.0, y as f32 * uy / 4.0, 64, 12);
                signed_px(v * 0.9)
            }).collect();
            Image::new(w, h, px)
        }
        Tex::Brushed => {
            let (w, h) = (dim(256.0), dim(128.0));
            let (long_n, short_n) = (((72.0 * k).round() as usize).max(3), ((9.0 * k).round() as usize).max(2));
            let mut px = Vec::with_capacity((w * h) as usize);
            for y in 0..h {
                let white: Vec<f32> = (0..w).map(|x| snorm(hash2(x, y, 21))).collect();
                let long = blur_wrap_norm(&blur_wrap_norm(&white, long_n), long_n);
                let short = blur_wrap_norm(&white, short_n);
                let band = (y as f32 / (3.0 * k).max(1.0)) as u32;
                let row = snorm(hash_u(y.wrapping_add(0x3c6e_f372))) * 0.9 * fine + 0.35 * snorm(hash_u(band.wrapping_add(77)));
                for x in 0..w as usize {
                    let v = 0.30 * row + 0.40 * long[x] + 0.22 * short[x] + 0.12 * fine * white[x];
                    px.push(signed_px(v * 0.75));
                }
            }
            Image::new(w, h, px)
        }
        Tex::Speckle => {
            let (w, h) = (dim(128.0), dim(128.0));
            let (ux, uy) = (256.0 / w as f32, 256.0 / h as f32);
            let height = |x: f32, y: f32| value_noise(x / 4.0, y / 4.0, 64, 31) + 0.5 * value_noise(x / 2.0, y / 2.0, 128, 32);
            // The same number of specks per unit of plate at every scale.
            let p = (0.0070 / (k * k)).min(0.03);
            let px: Vec<u32> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| {
                let (fx, fy) = (x as f32 * ux, y as f32 * uy);
                // Lit where the surface falls toward the light: -grad(h) . L.
                let peel = (height(fx - l.x, fy - l.y) - height(fx + l.x, fy + l.y)) * 0.55;
                let s = unit(hash2(x, y, 33));
                let speck = if s > 1.0 - p { 0.85 * fine } else if s < p * 1.15 { -0.9 * fine } else { 0.0 };
                signed_px(0.6 * peel + speck + 0.18 * fine * tri(x, y, 34))
            }).collect();
            Image::new(w, h, px)
        }
        Tex::Hammer => {
            let (w, h) = (dim(128.0), dim(128.0));
            let g: i64 = 14;
            let cell = w as f32 / g as f32;
            let feat = |i: i64, j: i64| {
                let (wi, wj) = (i.rem_euclid(g) as u32, j.rem_euclid(g) as u32);
                (i as f32 + 0.15 + 0.7 * unit(hash2(wi, wj, 41)), j as f32 + 0.15 + 0.7 * unit(hash2(wi, wj, 42)))
            };
            let mut px = Vec::with_capacity((w * h) as usize);
            for y in 0..h {
                for x in 0..w {
                    let (u, v) = ((x as f32 + 0.5) / cell, (y as f32 + 0.5) / cell);
                    let (ci, cj) = (u.floor() as i64, v.floor() as i64);
                    let (mut d1, mut d2, mut best) = (f32::MAX, f32::MAX, (0.0, 0.0));
                    for dj in -1..=1 {
                        for di in -1..=1 {
                            let (fx, fy) = feat(ci + di, cj + dj);
                            let d = ((u - fx).powi(2) + (v - fy).powi(2)).sqrt();
                            if d < d1 { d2 = d1; d1 = d; best = (u - fx, v - fy); } else if d < d2 { d2 = d; }
                        }
                    }
                    // Round cup of radius R around each strike; the wall away from the light faces it.
                    let rr = 0.62;
                    let slope = if d1 < rr { 2.0 * (1.0 - d1 / rr) * (d1 / rr).sqrt() } else { 0.0 };
                    let lit = -slope * (best.0 * l.x + best.1 * l.y) / d1.max(1e-4);
                    let ridge = (1.0 - ((d2 - d1) / 0.08).min(1.0)) * 0.12;
                    let mottle = value_noise(x as f32 * 16.0 / w as f32, y as f32 * 16.0 / h as f32, 16, 43);
                    px.push(signed_px(0.7 * lit + ridge + 0.22 * mottle + 0.16 * fine * tri(x, y, 44)));
                }
            }
            Image::new(w, h, px)
        }
        Tex::Wood => {
            // Design space 512 x 256, seamless both ways: every noise term has a whole number of
            // lattice periods across the tile and the ring count is whole.
            let (w, h) = (dim(256.0), dim(128.0));
            let (ux, uy) = (512.0 / w as f32, 256.0 / h as f32);
            let pore_n = ((14.0 * k).round() as usize).max(2);
            let mut px = Vec::with_capacity((w * h) as usize);
            for y in 0..h {
                let white: Vec<f32> = (0..w).map(|x| snorm(hash2(x, y, 51))).collect();
                let pores = blur_wrap_norm(&white, pore_n);
                for x in 0..w as usize {
                    let (fx, fy) = (x as f32 * ux, y as f32 * uy);
                    // Mostly straight grain: rings of uneven spacing, gently drifting along x.
                    let drift = 0.55 * value_noise(fx / 256.0, fy / 128.0, 2, 52) + 0.08 * value_noise(fx / 64.0, fy / 32.0, 8, 53);
                    let t = fy / 256.0 * 16.0 + 1.6 * value_noise(0.0, fy / 16.0, 16, 55) + drift;
                    let f = t - t.floor();
                    let strength = 0.5 + 0.5 * unit(hash_u((t.floor() as i64).rem_euclid(16) as u32 ^ 0x5a5a));
                    let late = ((f - 0.62) / 0.22).clamp(0.0, 1.0);
                    let ring = (late * late * (3.0 - 2.0 * late) - ((f - 0.9) / 0.1).clamp(0.0, 1.0)) * strength;
                    let figure = 0.18 * value_noise(fx / 64.0, fy / 4.0, 8, 54);
                    let v = -0.5 * ring - 0.28 * pores[x].max(0.0) + figure + 0.1 + 0.05 * fine * tri(x as u32, y, 56);
                    px.push(signed_px(v));
                }
            }
            Image::new(w, h, px)
        }
    }
}

/// Tiles `img` over `area` one texel per device pixel (see [`set_device_scale`]), with the tile
/// origin snapped to the device grid so the blit is exact. Rows are staggered by a hash of
/// `seed` unless `stagger` is false (for grain that must line up across rows).
fn tile_texture_opt(scene: &mut Scene, img: &Arc<Image>, area: Rect, opacity: f32, seed: u32, stagger: bool) {
    if opacity <= 0.0 || area.w <= 0.0 || area.h <= 0.0 { return; }
    let s = device_scale();
    let (tw, th) = (img.w as f32 / s, img.h as f32 / s);
    let (ox, oy) = ((area.x * s).floor(), (area.y * s).floor());
    let mut row = 0u32;
    loop {
        let y = (oy + row as f32 * img.h as f32) / s;
        if y >= area.bottom() { break; }
        let off = if stagger { (hash_u(seed ^ row.wrapping_mul(0x9e37_79b9)) % img.w) as f32 } else { 0.0 };
        let mut i = 0u32;
        loop {
            let x = (ox - off + i as f32 * img.w as f32) / s;
            if x >= area.right() { break; }
            if x + tw > area.x { scene.image(img, Rect::new(x, y, tw, th), opacity); }
            i += 1;
        }
        row += 1;
    }
}
fn tile_texture(scene: &mut Scene, img: &Arc<Image>, area: Rect, opacity: f32, seed: u32) {
    tile_texture_opt(scene, img, area, opacity, seed, true);
}

// ---------------------------------------------------------------------------------------------
// Light helpers
// ---------------------------------------------------------------------------------------------

/// Gradient angle (for [`Paint::Linear`]) that runs from the lit side to the shaded side.
fn away_angle(rig: &LightRig) -> f32 {
    let d = rig.screen_dir();
    if d.x == 0.0 && d.y == 0.0 { PI * 0.5 } else { (-d.y).atan2(-d.x) }
}

/// Colour of a surface whose on-screen facing is `f` (+1 straight at the light, -1 away).
fn edge_shade(face: Color, f: f32) -> Color {
    if f >= 0.0 { face.mix(WHITE, 0.06 + 0.30 * f) } else { face.darker(0.08 + 0.50 * -f) }
}

fn facing(rig: &LightRig, nx: f32, ny: f32) -> f32 {
    let d = rig.screen_dir();
    nx * d.x + ny * d.y
}

/// Four-sided chamfer between `r` and `r` inset by `b`, each side shaded by its facing.
/// Gradients run across each side (outer roll-off to inner crease).
fn chamfer(scene: &mut Scene, r: Rect, radius: f32, b: f32, top: Color, bottom: Color, rig: &LightRig) {
    let mid = top.mix(bottom, 0.5);
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    let (t, bt, l, rt) = (
        edge_shade(top, facing(rig, 0.0, -1.0)),
        edge_shade(bottom, facing(rig, 0.0, 1.0)),
        edge_shade(mid, facing(rig, -1.0, 0.0)),
        edge_shade(mid, facing(rig, 1.0, 0.0)),
    );
    scene.fill_round_linear(r, radius, Stops::new(&[(0.0, t), (0.5, l.mix(rt, 0.5)), (1.0, bt)]), PI * 0.5);
    scene.clip_round(r, radius);
    let p = Point::new;
    // Top and bottom: gradient across the bevel (outer edge to crease).
    scene.fill_poly(vec![p(x0, y0), p(x1, y0), p(x1 - b, y0 + b), p(x0 + b, y0 + b)], t.mix(top, 0.25), t);
    scene.fill_poly(vec![p(x0 + b, y1 - b), p(x1 - b, y1 - b), p(x1, y1), p(x0, y1)], bt, bt.darker(0.25));
    // Sides: gradient along the length (light falls off downward).
    scene.cmds.push(crate::scene::Cmd::FillPoly { pts: vec![p(x0, y0), p(x0 + b, y0 + b), p(x0 + b, y1 - b), p(x0, y1)], top: l.mix(t, 0.35), bottom: l.darker(0.18), diagonal: false });
    scene.cmds.push(crate::scene::Cmd::FillPoly { pts: vec![p(x1, y0), p(x1, y1), p(x1 - b, y1 - b), p(x1 - b, y0 + b)], top: rt.mix(t, 0.2), bottom: rt.darker(0.2), diagonal: false });
    scene.pop_clip();
}

// ---------------------------------------------------------------------------------------------
// Panels and finishes
// ---------------------------------------------------------------------------------------------

/// A faceplate with its finish, lit by the studio rig. `finish` is one of the `FINISH_*` numbers.
pub fn draw_panel(scene: &mut Scene, r: Rect, top: Color, bottom: Color, finish: i32) {
    draw_panel_with(scene, r, 2.5, top, bottom, finish, &LightRig::studio());
}

/// [`draw_panel`] with a corner radius and a light rig: soft drop shadow, chamfered edge shaded
/// per side, a sheen gradient at the rig's angle, the finish texture, falloff and vignette.
pub fn draw_panel_with(scene: &mut Scene, r: Rect, radius: f32, top: Color, bottom: Color, finish: i32, rig: &LightRig) {
    let (top, bottom) = if finish == FINISH_SATIN_BLACK {
        (top.mix(Color(0xff232427), 0.85), bottom.mix(Color(0xff131415), 0.85))
    } else {
        (top, bottom)
    };
    let radius = radius.max(0.0);
    // Shadows: a wide soft one for the stand-off and a tight contact one.
    let soft = rig.shadow_offset(5.0);
    scene.shadow_round(r.translated(soft.x, soft.y), radius + 2.0, rig.shadow_sigma(5.0) * 2.4, Color(0x8c000000));
    let near = rig.shadow_offset(1.2);
    scene.shadow_round(r.translated(near.x, near.y), radius, rig.shadow_sigma(1.2) + 0.4, Color(0x99000000));

    let b = (r.w.min(r.h) * 0.06).clamp(0.6, 2.0);
    chamfer(scene, r, radius, b, top, bottom, rig);
    let face = r.reduced(b, b);
    let fr = (radius - b * 0.6).max(0.5);

    // Base: the plate's own top/bottom colours, tilted to the light and lifted on the lit side.
    let ang = away_angle(rig);
    scene.fill_round_linear(face, fr, Stops::new(&[(0.0, top.mix(WHITE, 0.04)), (0.35, top), (1.0, bottom)]), ang);

    scene.clip_round(face, fr);
    let seed = pos_seed(r.x, r.y);
    finish_layer(scene, face, finish, top.mix(bottom, 0.5), seed, rig);
    plate_light(scene, face, fr, finish, seed, rig);
    scene.pop_clip();

    // Crease between chamfer and face, and the dark outer edge against the background.
    scene.stroke_round(face, fr, Color(0x1a000000), 0.6);
    scene.stroke_round(r.expanded(0.25), radius + 0.25, Color(0x8c06080a), 0.6);
}

/// The finish texture alone over `r` (clipped to it), lit by the studio rig.
pub fn draw_finish(scene: &mut Scene, r: Rect, finish: i32) {
    if finish <= 0 { return; }
    scene.clip_round(r, 2.0);
    finish_layer(scene, r, finish, Color(0xff808080), pos_seed(r.x, r.y), &LightRig::studio());
    scene.pop_clip();
}

fn is_metal(finish: i32) -> bool { matches!(finish, FINISH_BRUSHED | FINISH_ANODISED | FINISH_HAMMERTONE) }
fn is_screen(finish: i32) -> bool { matches!(finish, FINISH_SCANLINES | FINISH_GRID | FINISH_NEON_EDGE | FINISH_HORIZON) }

/// Broad sheen, top-to-bottom falloff and vignette on top of the finish (inside the face clip).
fn plate_light(scene: &mut Scene, face: Rect, fr: f32, finish: i32, seed: u32, rig: &LightRig) {
    let d = rig.screen_dir();
    let c = face.center();
    let shape = Shape::Round(face, fr);
    let sheen = if is_screen(finish) { 0.05 } else if finish == FINISH_SATIN_BLACK { 0.16 } else if is_metal(finish) { 0.13 } else { 0.10 };
    // Key sheen: a wide soft pool on the lit side.
    let focus = Point::new(c.x + d.x * face.w * 0.30, c.y + d.y * face.h * 0.45);
    let (rx, ry) = (face.w * 0.85, face.h * 1.15);
    scene.fill_shape(shape, Paint::Radial {
        stops: Stops::new(&[(0.0, WHITE.with_alpha(sheen)), (0.45, WHITE.with_alpha(sheen * 0.35)), (1.0, WHITE.with_alpha(0.0))]),
        center: focus, rx, ry, focus,
    }, 0.0, Blend::Screen);
    if matches!(finish, FINISH_BRUSHED | FINISH_ANODISED) {
        // Anisotropic bands: brushed along x, so the highlight smears into vertical bands.
        let j = |k: u32| (unit(hash_u(seed.wrapping_add(k))) - 0.5) * 0.12;
        let k = if finish == FINISH_BRUSHED { 1.0 } else { 0.55 };
        let hi = |a: f32| WHITE.with_alpha(a * k);
        scene.fill_shape(shape, Paint::Linear { stops: Stops::new(&[
            (0.0, hi(0.0)), (0.18 + j(1), hi(0.10)), (0.34 + j(2), hi(0.0)), (0.58 + j(3), hi(0.16)), (0.78 + j(4), hi(0.02)), (1.0, hi(0.07)),
        ]), angle: 0.04 }, 0.0, Blend::Screen);
        let lo = |a: f32| BLACK.with_alpha(a * k);
        scene.fill_shape(shape, Paint::Linear { stops: Stops::new(&[
            (0.0, lo(0.10)), (0.12, lo(0.0)), (0.42 + j(5), lo(0.07)), (0.62, lo(0.0)), (0.88 + j(6), lo(0.08)), (1.0, lo(0.13)),
        ]), angle: 0.04 }, 0.0, Blend::Over);
    }
    if finish == FINISH_SATIN_BLACK {
        // Satin: a second, very wide and soft highlight along the top.
        scene.fill_shape(shape, Paint::Linear { stops: Stops::new(&[(0.0, WHITE.with_alpha(0.07)), (0.3, WHITE.with_alpha(0.02)), (0.6, WHITE.with_alpha(0.0))]), angle: PI * 0.5 }, 0.0, Blend::Screen);
    }
    // Falloff away from the light, then a vignette toward the corners.
    scene.fill_shape(shape, Paint::Linear { stops: Stops::new(&[(0.0, BLACK.with_alpha(0.0)), (0.5, BLACK.with_alpha(0.05)), (1.0, BLACK.with_alpha(0.17))]), angle: away_angle(rig) }, 0.0, Blend::Over);
    scene.fill_shape(shape, Paint::Radial {
        stops: Stops::new(&[(0.0, BLACK.with_alpha(0.0)), (0.72, BLACK.with_alpha(0.0)), (1.0, BLACK.with_alpha(0.13))]),
        center: c, rx: face.w * 0.56, ry: face.h * 0.62, focus: c,
    }, 0.0, Blend::Over);
}

fn brushed_streaks(scene: &mut Scene, r: Rect, seed: u32, strength: f32) {
    let n = ((r.w * r.h) / 70.0).clamp(16.0, 900.0) as u32;
    for i in 0..n {
        let h = hash2(i, seed, 61);
        let y = r.y + unit(h) * r.h;
        let x = r.x - 6.0 + unit(hash_u(h ^ 0x68e3_1da4)) * (r.w + 12.0);
        let lr = unit(hash_u(h ^ 0x1b56_c4e9));
        let len = 4.0 + lr * lr * (r.w * 0.35).min(160.0);
        let a = (0.02 + 0.05 * unit(hash_u(h ^ 0x2545_f491))) * strength;
        let c = if h & 1 == 0 { WHITE.with_alpha(a * 1.2) } else { BLACK.with_alpha(a * 0.6) };
        let w = 0.45 + 0.4 * unit(hash_u(h ^ 0x9e37_79b9));
        hline(scene, y, x, x + len, c, w);
    }
}

/// Draws the finish over `face`. The caller has clipped to the face.
fn finish_layer(scene: &mut Scene, face: Rect, finish: i32, base: Color, seed: u32, rig: &LightRig) {
    let (x0, x1, y0, y1) = (face.x, face.right(), face.y, face.bottom());
    let light_base = base.perceived_brightness() > 0.55;
    match finish {
        FINISH_FLAT => tile_texture(scene, &texture(Tex::Grain), face, if light_base { 0.05 } else { 0.07 }, seed),
        FINISH_BRUSHED => {
            tile_texture(scene, &texture(Tex::Brushed), face, if light_base { 0.20 } else { 0.13 }, seed);
            brushed_streaks(scene, face, seed, if light_base { 1.0 } else { 0.8 });
        }
        FINISH_WOOD => {
            tile_texture_opt(scene, &texture(Tex::Wood), face, 0.62, seed, false);
            let mut y = y0 + 4.0;
            let mut k = 0u32;
            while y < y1 {
                let ph = unit(hash2(k, seed, 71)) * 6.0;
                let pts: Vec<Point> = (0..=((x1 - x0) / 6.0).ceil() as i32)
                    .map(|i| {
                        let x = x0 + i as f32 * 6.0;
                        Point::new(x, y + (x * 0.031 + ph).sin() * 1.6 + (x * 0.011 + y * 0.2).sin() * 1.2)
                    })
                    .collect();
                scene.stroke_poly(pts, BLACK.with_alpha(0.06 + 0.06 * unit(hash2(k, seed, 72))), 0.7, false);
                y += 4.0 + 5.0 * unit(hash2(k, seed, 73));
                k += 1;
            }
            tile_texture(scene, &texture(Tex::Grain), face, 0.05, seed);
        }
        FINISH_SCANLINES => {
            // A phosphor screen behind glass: a lit pool, raster lines on the device grid with a
            // bright phosphor row under each dark gap, one hot beam row, dark tube edges.
            let (w, h) = (x1 - x0, y1 - y0);
            let c = face.center();
            let shape = Shape::Round(face, 0.0);
            let (hue, sat, _) = base.to_hsv();
            let tint = Color::from_hsv(hue, (sat * 0.9).min(0.85), 1.0);
            scene.fill_shape(shape, Paint::Radial {
                stops: Stops::new(&[(0.0, tint.with_alpha(0.20)), (0.5, tint.with_alpha(0.08)), (1.0, tint.with_alpha(0.0))]),
                center: Point::new(c.x, y0 + h * 0.46), rx: w * 0.62, ry: h * 0.70, focus: Point::new(c.x, y0 + h * 0.46),
            }, 0.0, Blend::Screen);
            tile_texture(scene, &texture(Tex::Grain), face, 0.05, seed);
            let s = device_scale();
            let pitch = (3.0 * s).round().max(2.0);
            let gap = (pitch / 3.0).round().max(1.0);
            let mut py = (y0 * s).floor();
            while py / s < y1 {
                let y = py / s;
                scene.fill_rect(Rect::new(x0, y, w, gap / s), BLACK.with_alpha(0.34), BLACK.with_alpha(0.34), true);
                scene.fill_rect(Rect::new(x0, y + gap / s, w, 1.0 / s), tint.with_alpha(0.085), tint.with_alpha(0.085), true);
                py += pitch;
            }
            // The row being drawn, with a short afterglow above it.
            let beam = y0 + h * (0.30 + 0.4 * unit(hash_u(seed ^ 0x51ed)));
            scene.fill_shape(Shape::Round(Rect::new(x0, beam - 9.0, w, 9.0), 0.0), Paint::Linear { stops: Stops::new(&[(0.0, tint.with_alpha(0.0)), (0.7, tint.with_alpha(0.02)), (1.0, tint.with_alpha(0.08))]), angle: PI * 0.5 }, 0.0, Blend::Add);
            scene.glow_round(Rect::new(x0 - 4.0, beam - 2.2, w + 8.0, 3.2), 1.6, tint.with_alpha(0.11), 1.5, Blend::Add);
            // Glass: a curved reflection from the upper left and the tube darkening at its edges.
            scene.fill_rect(Rect::new(x0, y0, w, h * 0.45), Color(0x14ffffff), Color(0x00ffffff), true);
            scene.glow_ellipse(Rect::new(x0 - w * 0.25, y0 - h * 0.55, w * 0.95, h * 0.95), WHITE.with_alpha(0.07), h * 0.30, Blend::Screen);
            scene.inner_shadow(Shape::Round(face, 3.0), Point::new(0.0, 0.0), (w.min(h) * 0.10).clamp(3.0, 14.0), BLACK.with_alpha(0.75));
        }
        FINISH_HAZARD => {
            tile_texture(scene, &texture(Tex::Grain), face, 0.06, seed);
            let bh = 6.0;
            for y_band in [y0 + 1.0, y1 - 1.0 - bh] {
                let band = Rect::new(x0, y_band, x1 - x0, bh);
                scene.fill_rect(band, Color(0xff1a1a18), Color(0xff121210), true);
                let mut x = x0 - 8.0;
                while x < x1 {
                    scene.cmds.push(crate::scene::Cmd::FillPoly {
                        pts: vec![Point::new(x, y_band + bh), Point::new(x + 6.0, y_band + bh), Point::new(x + 12.0, y_band), Point::new(x + 6.0, y_band)],
                        top: Color(0xfff0d438), bottom: Color(0xffcfae14), diagonal: false,
                    });
                    x += 12.0;
                }
                scene.clip_round(band, 0.0);
                tile_texture(scene, &texture(Tex::Speckle), band, 0.25, seed ^ 5);
                scene.pop_clip();
                let o = rig.shadow_offset(0.8);
                scene.inner_shadow(Shape::Round(band, 0.0), o, 0.6, Color(0x99000000));
            }
        }
        FINISH_NEON_EDGE => {
            // Gloss-black acrylic with a neon tube set in from the edge: light spilling onto the
            // plate on both sides of the tube, additive bloom, a hot core, and two short magenta
            // tubes breaking the run at opposite corners.
            let (cyan, mag) = (Color(0xff39f2ff), Color(0xffff4fd8));
            let (w, h) = (x1 - x0, y1 - y0);
            let inset = (w.min(h) * 0.035).clamp(2.5, 5.0);
            let tube = face.reduced(inset, inset);
            let tr = inset.min(3.5);
            let tw = (inset * 0.42).clamp(1.1, 1.8);
            scene.fill_shape(Shape::Round(face, 0.0), Paint::Linear { stops: Stops::new(&[(0.0, WHITE.with_alpha(0.07)), (0.22, WHITE.with_alpha(0.015)), (0.48, WHITE.with_alpha(0.05)), (0.56, WHITE.with_alpha(0.0)), (1.0, WHITE.with_alpha(0.0))]), angle: away_angle(rig) }, 0.0, Blend::Screen);
            // Spill on the plate, falling off away from the tube.
            for k in 1..=11 {
                let d = k as f32 * 0.95;
                let a = 0.115 * (-(k as f32) / 3.4).exp();
                scene.stroke_round(tube.reduced(d, d), (tr - d).max(0.5), cyan.with_alpha(a), 1.3);
                if d < inset + 1.0 { scene.stroke_round(tube.expanded(d), tr + d, cyan.with_alpha(a), 1.3); }
            }
            // Bloom, one feathered band per side.
            let f = (inset * 1.9).clamp(4.0, 9.0);
            for band in [
                Rect::new(tube.x, tube.y - 0.5, tube.w, 1.0), Rect::new(tube.x, tube.bottom() - 0.5, tube.w, 1.0),
                Rect::new(tube.x - 0.5, tube.y, 1.0, tube.h), Rect::new(tube.right() - 0.5, tube.y, 1.0, tube.h),
            ] {
                scene.glow_round(band.expanded(f), f, cyan.with_alpha(0.20), f, Blend::Add);
            }
            // Reflection of the tube in the gloss, a little way in.
            scene.stroke_round(tube.reduced(inset * 1.9, inset * 1.9), 1.0, cyan.with_alpha(0.035), 0.8);
            // The glass tube and its core.
            scene.stroke_round(tube, tr, cyan.darker(0.25).with_alpha(0.55), tw + 1.4);
            scene.stroke_round(tube, tr, cyan, tw);
            scene.stroke_round(tube, tr, cyan.brighter(0.8), tw * 0.42);
            // Magenta tubes: a gap in the cyan run, then the short tube with its own bloom.
            let ml = (w * 0.13).clamp(12.0, 60.0);
            for (mx, my) in [(tube.x + tr + 3.0, tube.y), (tube.right() - tr - 3.0 - ml, tube.bottom())] {
                let seg = Rect::new(mx, my - tw * 0.5, ml, tw);
                scene.fill_round(seg.expanded(1.6), 1.6, Color(0xff04060c), Color(0xff04060c), true);
                scene.glow_round(seg.expanded(f * 1.1), f * 1.1, mag.with_alpha(0.30), f * 1.1, Blend::Add);
                scene.glow_round(seg.expanded(2.0), 2.4, mag.with_alpha(0.45), 2.0, Blend::Add);
                scene.fill_round(seg.expanded(0.4), tw, mag, mag, true);
                scene.fill_round(seg.reduced(1.0, tw * 0.29), tw * 0.3, mag.brighter(0.8), mag.brighter(0.8), true);
            }
        }
        FINISH_HORIZON => {
            let hy = y0 + (y1 - y0) * 0.58;
            let (w, cx) = (x1 - x0, x0 + (x1 - x0) * 0.5);
            let (mag, cyan) = (Color(0xffff4fd8), Color(0xff35f2ff));
            scene.fill_rect(Rect::new(x0, y0, w, hy - y0), Color(0x00ff4fd8), Color(0x55ff4fd8), true);
            // Haze on the horizon and its light on the floor.
            scene.glow_ellipse(Rect::new(cx - w * 0.50, hy - 14.0, w, 28.0), mag.with_alpha(0.36), 14.0, Blend::Add);
            scene.glow_ellipse(Rect::new(cx - w * 0.22, hy - 7.0, w * 0.44, 14.0), Color(0xffffc9f0).with_alpha(0.40), 7.0, Blend::Add);
            scene.fill_rect(Rect::new(x0, hy, w, (y1 - hy) * 0.6), mag.with_alpha(0.16), mag.with_alpha(0.0), true);
            // Floor grid: each line is a soft sleeve under a sharp core, fading toward the viewer's feet.
            for i in -8..=8 {
                let (a, b) = (Point::new(cx + i as f32 * 3.0, hy), Point::new(cx + i as f32 * w * 0.16, y1));
                scene.line(a, b, cyan.with_alpha(0.14), 2.2);
                scene.line(a, b, cyan.with_alpha(0.62), 0.7);
            }
            for j in 1..=6 {
                let f = (j as f32) / 6.0;
                let y = hy + f * f * (y1 - hy);
                hline(scene, y, x0, x1, cyan.with_alpha(0.14), 1.0 + 1.6 * f);
                hline(scene, y, x0, x1, cyan.with_alpha(0.62), 0.6 + 0.4 * f);
            }
            hline(scene, hy, x0, x1, cyan.with_alpha(0.25), 2.4);
            hline(scene, hy, x0, x1, Color(0xffd8fbff).with_alpha(0.85), 0.8);
        }
        FINISH_ANODISED => tile_texture(scene, &texture(Tex::Grain), face, if light_base { 0.10 } else { 0.13 }, seed),
        FINISH_POWDER => tile_texture(scene, &texture(Tex::Speckle), face, if light_base { 0.16 } else { 0.2 }, seed),
        FINISH_HAMMERTONE => {
            tile_texture(scene, &texture(Tex::Hammer), face, if light_base { 0.2 } else { 0.26 }, seed);
            tile_texture(scene, &texture(Tex::Grain), face, 0.06, seed ^ 9);
        }
        FINISH_SATIN_BLACK => tile_texture(scene, &texture(Tex::Grain), face, 0.07, seed),
        _ => {
            // Grid (5) and anything unknown.
            let mut x = x0 + 14.0;
            while x < x1 {
                vline(scene, x.trunc() + 0.5, y0, y1, Color(0x12ffffff), 1.0);
                x += 14.0;
            }
            let mut y = y0 + 14.0;
            while y < y1 {
                hline(scene, y.trunc() + 0.5, x0, x1, Color(0x12ffffff), 1.0);
                y += 14.0;
            }
            for c in [Point::new(x0, y0), Point::new(x1, y0), Point::new(x0, y1), Point::new(x1, y1)] {
                let sx = if c.x == x0 { 1.0 } else { -1.0 };
                let sy = if c.y == y0 { 1.0 } else { -1.0 };
                let c = Point::new(c.x + sx * 1.5, c.y + sy * 1.5);
                scene.line(Point::new(c.x, c.y + sy * 10.0), c, Color(0x77ffffff), 1.2);
                scene.line(c, Point::new(c.x + sx * 10.0, c.y), Color(0x77ffffff), 1.2);
            }
        }
    }
}

/// Brushed-metal strip (rack ears, cheeks). `seed` varies the brushing.
pub fn draw_brushed_panel(scene: &mut Scene, r: Rect, top: Color, bottom: Color, seed: u32) {
    scene.fill_rect(r, top, bottom, true);
    scene.clip_round(r, 0.0);
    let light_base = top.mix(bottom, 0.5).perceived_brightness() > 0.55;
    // On a narrow strip the long streaks of the tile are wider than the strip and read as coarse
    // full-width rules: ease them and carry the brushing with fine grain and short streaks.
    let narrow = (r.w.min(r.h) / 120.0).clamp(0.35, 1.0);
    tile_texture(scene, &texture(Tex::Brushed), r, if light_base { 0.2 } else { 0.14 } * narrow, hash_u(seed));
    if narrow < 1.0 { tile_texture(scene, &texture(Tex::Grain), r, 0.09 * (1.0 - narrow) + 0.03, hash_u(seed ^ 0x77)); }
    brushed_streaks(scene, r, hash_u(seed ^ 0xabcd), if light_base { 1.0 } else { 0.9 } * (0.5 + 0.5 * narrow));
    let shape = Shape::Round(r, 0.0);
    scene.fill_shape(shape, Paint::Linear { stops: Stops::new(&[(0.0, WHITE.with_alpha(0.10)), (0.2, WHITE.with_alpha(0.0))]), angle: PI * 0.5 }, 0.0, Blend::Screen);
    scene.fill_shape(shape, Paint::Linear { stops: Stops::new(&[(0.0, WHITE.with_alpha(0.0)), (0.4, WHITE.with_alpha(0.07)), (0.7, WHITE.with_alpha(0.0))]), angle: 0.0 }, 0.0, Blend::Screen);
    scene.fill_rect(Rect::new(r.x, r.y, r.w, 1.2), Color(0x40ffffff), Color(0x00ffffff), true);
    scene.fill_rect(Rect::new(r.x, r.bottom() - 1.4, r.w, 1.4), Color(0x00000000), Color(0x90000000), true);
    scene.pop_clip();
}

// ---------------------------------------------------------------------------------------------
// Screws
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrewHead { Slot, Cross, Hex }

/// A screw head radius that suits a plate: about 3.5% of its shorter side, 2.5..7 units.
pub fn screw_radius_for(plate: Rect) -> f32 { (plate.w.min(plate.h) * 0.035).clamp(2.5, 7.0) }

/// Slotted screw, head radius 4.25 (the original size), studio light.
pub fn draw_screw(scene: &mut Scene, x: f32, y: f32) {
    draw_screw_head(scene, Point::new(x, y), 4.25, ScrewHead::Slot);
}

pub fn draw_screw_head(scene: &mut Scene, c: Point, radius: f32, head: ScrewHead) {
    draw_screw_with(scene, c, radius, head, &LightRig::studio());
}

/// Four screws inset into the corners of `plate`, sized with [`screw_radius_for`].
pub fn draw_corner_screws(scene: &mut Scene, plate: Rect, head: ScrewHead) {
    let rad = screw_radius_for(plate);
    let inset = rad * 2.6;
    for (x, y) in [(plate.x + inset, plate.y + inset), (plate.right() - inset, plate.y + inset), (plate.x + inset, plate.bottom() - inset), (plate.right() - inset, plate.bottom() - inset)] {
        draw_screw_head(scene, Point::new(x, y), rad, head);
    }
}

fn rotated_rect(c: Point, half_len: f32, half_w: f32, ang: f32) -> Vec<Point> {
    let (ca, sa) = (ang.cos(), ang.sin());
    let (ux, uy, vx, vy) = (ca * half_len, sa * half_len, -sa * half_w, ca * half_w);
    vec![
        Point::new(c.x - ux - vx, c.y - uy - vy),
        Point::new(c.x + ux - vx, c.y + uy - vy),
        Point::new(c.x + ux + vx, c.y + uy + vy),
        Point::new(c.x - ux + vx, c.y - uy + vy),
    ]
}

fn shift(pts: &[Point], d: Point) -> Vec<Point> { pts.iter().map(|p| p.add(d)).collect() }

/// Domed screw head in a countersink. Rotation comes from a hash of the position, so a plate's
/// screws are not all aligned but always draw the same.
pub fn draw_screw_with(scene: &mut Scene, c: Point, radius: f32, head: ScrewHead, rig: &LightRig) {
    let r = radius.max(1.0);
    let d = rig.screen_dir();
    let ang = away_angle(rig);
    let rot = unit(pos_seed(c.x, c.y)) * PI;

    // Countersink: the cone's wall toward the light is in shade, the far wall catches light.
    let cs = Scene::ellipse_at(c, r * 1.24);
    scene.fill_ellipse_linear(cs, Stops::new(&[(0.0, BLACK.with_alpha(0.55)), (0.55, BLACK.with_alpha(0.25)), (1.0, BLACK.with_alpha(0.05))]), ang);
    scene.fill_shape(Shape::Ellipse(cs), Paint::Linear { stops: Stops::new(&[(0.0, WHITE.with_alpha(0.0)), (0.6, WHITE.with_alpha(0.0)), (1.0, WHITE.with_alpha(0.35))]), angle: ang }, 0.0, Blend::Screen);
    scene.inner_shadow(Shape::Ellipse(cs), rig.shadow_offset(r * 0.18), r * 0.12 + 0.3, Color(0xb0000000));

    // Head shadow onto the cone, then the dome.
    let hr = Scene::ellipse_at(c, r);
    let so = rig.shadow_offset(r * 0.25);
    scene.shadow_ellipse(hr.translated(so.x, so.y), rig.shadow_sigma(r * 0.25) + 0.25, Color(0xcc000000));
    let focus = Point::new(c.x + d.x * r * 0.42, c.y + d.y * r * 0.42);
    scene.fill_ellipse_radial(hr, Stops::new(&[
        (0.0, Color(0xfff4f5f4)), (0.22, Color(0xffd3d7d8)), (0.55, Color(0xff989fa3)), (0.85, Color(0xff586065)), (1.0, Color(0xff30363a)),
    ]), focus);

    scene.clip_round(hr, r);
    let lit = Point::new(-d.x * r * 0.09, -d.y * r * 0.09);
    let dark = Point::new(d.x * r * 0.03, d.y * r * 0.03);
    let recess = |scene: &mut Scene, pts: Vec<Point>| {
        scene.fill_poly(shift(&pts, lit), Color(0x99e8ecee), Color(0x99e8ecee));
        scene.fill_poly(shift(&pts, dark), Color(0xff15191b), Color(0xff2a3034));
    };
    match head {
        ScrewHead::Slot => recess(scene, rotated_rect(c, r * 1.1, r * 0.15, rot)),
        ScrewHead::Cross => {
            recess(scene, rotated_rect(c, r * 0.62, r * 0.13, rot));
            recess(scene, rotated_rect(c, r * 0.62, r * 0.13, rot + PI * 0.5));
            scene.fill_ellipse_radial(Scene::ellipse_at(c, r * 0.22), Stops::two(Color(0xff0d1012), Color(0xff2a3034)), Point::new(c.x + d.x * r * 0.1, c.y + d.y * r * 0.1));
        }
        ScrewHead::Hex => {
            let hex: Vec<Point> = (0..6).map(|k| {
                let a = rot + k as f32 * PI / 3.0;
                Point::new(c.x + a.cos() * r * 0.46, c.y + a.sin() * r * 0.46)
            }).collect();
            recess(scene, hex);
        }
    }
    scene.pop_clip();
    scene.stroke_ellipse(hr, Color(0x55000000), 0.5);

    // Specular: a soft pool and a small sharp dot toward the light.
    let sp = Point::new(c.x + d.x * r * 0.45, c.y + d.y * r * 0.45);
    scene.glow_ellipse(Rect::new(sp.x - r * 0.42, sp.y - r * 0.32, r * 0.84, r * 0.64), Color(0x88ffffff), r * 0.3, Blend::Screen);
    scene.glow_ellipse(Scene::ellipse_at(sp, (r * 0.13).max(0.5)), Color(0xeeffffff), (r * 0.1).max(0.4), Blend::Screen);
}

// ---------------------------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------------------------

pub fn draw_key(scene: &mut Scene, r: Rect, base: Color, down: bool) {
    let rig = LightRig::studio();
    let well = r.expanded(2.5);
    scene.fill_round_linear(well, 3.5, Stops::two(Color(0xff050607), Color(0xff0d0f11)), PI * 0.5);
    scene.inner_shadow(Shape::Round(well, 3.5), rig.shadow_offset(1.5), 1.0, Color(0xcc000000));
    scene.stroke_round(well.expanded(0.4), 3.9, Color(0x30ffffff), 0.7);
    let face = r.translated(0.0, if down { 1.6 } else { 0.0 });
    let hgt = if down { 0.6 } else { 3.0 };
    let o = rig.shadow_offset(hgt);
    scene.shadow_round(face.translated(o.x, o.y), 2.0, rig.shadow_sigma(hgt) + 0.4, Color(0xcc000000));
    let hi = base.brighter(if down { 0.06 } else { 0.26 });
    let lo = base.darker(if down { 0.45 } else { 0.25 });
    scene.fill_round_linear(face, 2.0, Stops::new(&[(0.0, hi), (0.45, base.mix(hi, 0.3)), (1.0, lo)]), away_angle(&rig));
    let rim = face.reduced(1.0, 1.0);
    scene.fill_shape(Shape::Round(Rect::new(rim.x, rim.y, rim.w, rim.h * 0.42), 1.5), Paint::Linear {
        stops: Stops::two(WHITE.with_alpha(if down { 0.08 } else { 0.24 }), WHITE.with_alpha(0.0)), angle: PI * 0.5,
    }, 0.0, Blend::Screen);
    scene.fill_rect(Rect::new(face.x + 2.0, face.bottom() - 1.4, face.w - 4.0, 1.2), Color(0x00000000), Color(0x60000000), true);
    scene.stroke_round(face, 2.0, Color(0x55000000), 0.8);
}

pub fn use_dark_marks(marks: &str, backdrop: Color) -> bool {
    if marks == "dark" { return true; }
    if marks == "light" { return false; }
    backdrop.a() > 0 && backdrop.perceived_brightness() > 0.55
}

// ---------------------------------------------------------------------------------------------
// Rack ears
// ---------------------------------------------------------------------------------------------

/// Dark brushed rack ear with two oval slots. `left_side` puts the bend on the right edge.
pub fn draw_rack_ear(scene: &mut Scene, r: Rect, left_side: bool) {
    draw_rack_ear_with(scene, r, left_side, Color(0xff2d2e31), Color(0xff141516), &LightRig::studio());
}

/// Rack ear in any metal colour: cast shadow onto what is under it, chamfered cut edges, a
/// brushed face, the bent flange on the inner edge, and two oval slots with a lip and depth.
pub fn draw_rack_ear_with(scene: &mut Scene, r: Rect, left_side: bool, top: Color, bottom: Color, rig: &LightRig) {
    let o = rig.shadow_offset(3.0);
    scene.shadow_round(r.translated(o.x, o.y), 1.5, rig.shadow_sigma(3.0) * 1.8, Color(0xa0000000));
    let o = rig.shadow_offset(0.8);
    scene.shadow_round(r.translated(o.x, o.y), 1.0, 0.6, Color(0x80000000));
    let b = (r.w * 0.05).clamp(0.6, 1.2);
    chamfer(scene, r, 1.0, b, top, bottom, rig);
    let face = r.reduced(b, b);
    draw_brushed_panel(scene, face, top, bottom, if left_side { 11 } else { 12 });

    // Bend: the inner edge curls back toward the chassis.
    let fw = (r.w * 0.16).clamp(2.0, 5.0);
    let lit = facing(rig, if left_side { 1.0 } else { -1.0 }, 0.0) > 0.0;
    let (band, ang) = if left_side { (Rect::new(face.right() - fw, face.y, fw, face.h), 0.0) } else { (Rect::new(face.x, face.y, fw, face.h), PI) };
    let catch = if lit { 0.24 } else { 0.10 };
    scene.fill_shape(Shape::Round(band, 0.0), Paint::Linear { stops: Stops::new(&[(0.0, WHITE.with_alpha(0.0)), (0.45, WHITE.with_alpha(catch)), (0.7, WHITE.with_alpha(catch * 0.4)), (1.0, WHITE.with_alpha(0.0))]), angle: ang }, 0.0, Blend::Screen);
    scene.fill_shape(Shape::Round(band, 0.0), Paint::Linear { stops: Stops::new(&[(0.0, BLACK.with_alpha(0.0)), (0.6, BLACK.with_alpha(0.0)), (1.0, BLACK.with_alpha(if lit { 0.45 } else { 0.7 }))]), angle: ang }, 0.0, Blend::Over);

    // Slots.
    let body = if left_side { Rect::new(face.x, face.y, face.w - fw, face.h) } else { Rect::new(face.x + fw, face.y, face.w - fw, face.h) };
    let sw = (body.w * 0.6).clamp(4.0, 22.0);
    let sh = (sw * 0.55).max(3.0);
    let ys: Vec<f32> = if body.h > sh * 5.0 { vec![body.y + body.h * 0.17, body.bottom() - body.h * 0.17] } else { vec![body.center().y] };
    for cy in ys {
        let slot = Rect::new(body.center().x - sw * 0.5, cy - sh * 0.5, sw, sh);
        rack_slot(scene, slot, rig);
    }
}

fn rack_slot(scene: &mut Scene, slot: Rect, rig: &LightRig) {
    let ang = away_angle(rig);
    let k = slot.h * 0.5;
    let e = (slot.h * 0.14).clamp(0.8, 1.6);
    let lip = slot.expanded(e);
    // Pressed lip: the rim toward the light dips into shade, the far rim catches the key.
    scene.fill_round_linear(lip, k + e, Stops::new(&[(0.0, BLACK.with_alpha(0.55)), (0.45, BLACK.with_alpha(0.2)), (0.7, BLACK.with_alpha(0.0))]), ang);
    scene.fill_shape(Shape::Round(lip, k + e), Paint::Linear { stops: Stops::new(&[(0.0, WHITE.with_alpha(0.0)), (0.5, WHITE.with_alpha(0.0)), (1.0, WHITE.with_alpha(0.55))]), angle: ang }, 0.0, Blend::Screen);
    // Through the hole: the rack rail, further back, lit from above and in the ear's shade.
    scene.fill_round_linear(slot, k, Stops::new(&[(0.0, Color(0xff1a1b1e)), (0.6, Color(0xff3a3d42)), (1.0, Color(0xff2a2c30))]), PI * 0.5);
    scene.inner_shadow(Shape::Round(slot, k), rig.shadow_offset(slot.h * 0.5), slot.h * 0.2 + 0.4, Color(0xff000000));
    scene.stroke_round(slot, k, Color(0x66000000), 0.6);
}

pub fn draw_led_key(scene: &mut Scene, r: Rect, led: Color, lit: bool, down: bool) {
    let rig = LightRig::studio();
    let well = r.expanded(1.6);
    scene.fill_round(well, 2.6, Color(0xff030303), Color(0xff0a0b0c), true);
    scene.inner_shadow(Shape::Round(well, 2.6), rig.shadow_offset(1.0), 0.8, Color(0xcc000000));
    scene.stroke_round(well.expanded(0.4), 3.0, Color(0x30ffffff), 0.7);
    let cap = r.reduced(0.6, 0.6).translated(0.0, if down { 0.8 } else { 0.0 });
    if lit {
        scene.glow_round(cap.expanded(6.0), 8.0, led.with_alpha(0.35), 6.0, Blend::Add);
    }
    let hi = if lit { led.brighter(0.5) } else { led.darker(0.78) };
    let lo = if lit { led.darker(0.1) } else { led.darker(0.95) };
    scene.fill_round(cap, 2.0, hi, lo, true);
    if lit {
        scene.glow_ellipse(cap.reduced(cap.w * 0.08, cap.h * 0.12), led.brighter(0.7).with_alpha(0.55), cap.h * 0.35, Blend::Screen);
    }
    let sheen = cap.reduced(1.5, 1.5);
    scene.fill_shape(Shape::Round(Rect::new(sheen.x, sheen.y, sheen.w, cap.h * 0.4), 1.5), Paint::Linear {
        stops: Stops::two(WHITE.with_alpha(if lit { 0.4 } else { 0.16 }), WHITE.with_alpha(0.0)), angle: PI * 0.5,
    }, 0.0, Blend::Screen);
    scene.stroke_round(cap, 2.0, Color(0x66000000), 0.8);
}

// ---------------------------------------------------------------------------------------------
// Legends
// ---------------------------------------------------------------------------------------------

/// How a legend is put on the plate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Legend {
    /// Silk-screen ink in this colour: near-white ink is warmed slightly off-white, and the
    /// edges bleed a little.
    Silk(Color),
    /// Engraved and filled with this colour: a dark wall toward the light, a lit wall away from
    /// it. Pass the plate colour (a little darker) for an unfilled engraving.
    Engraved(Color),
}

fn silk_ink(c: Color) -> Color {
    let c = if c.perceived_brightness() > 0.85 { c.mix(Color(0xfff1ede2).with_alpha(c.alpha_f()), 0.65) } else { c };
    c.with_alpha(c.alpha_f() * 0.94)
}

/// Offsets of the shaded wall (toward the light) and the lit wall (away from it) of an
/// engraving. Glyphs land on whole device pixels, so both offsets are whole pixels of the scale
/// given to [`set_device_scale`] and never round away to nothing.
fn engrave_offsets(size: f32) -> (Point, Point) {
    let d = LightRig::studio().screen_dir();
    let s = device_scale();
    let px = ((size * 0.045 * s).round()).max(1.0);
    let (mut ox, mut oy) = ((d.x * px).round(), (d.y * px).round());
    if ox == 0.0 && oy == 0.0 { if d.x.abs() > d.y.abs() { ox = d.x.signum(); } else { oy = if d.y == 0.0 { -1.0 } else { d.y.signum() }; } }
    (Point::new(ox / s, oy / s), Point::new(-ox / s, -oy / s))
}

/// (shaded wall alpha, lit wall alpha) for an engraving on a plate of this luminance: the lit
/// wall has to be strong to show on a pale plate, the shaded wall on a dark one.
fn engrave_alphas(plate_lum: f32) -> (f32, f32) {
    let l = plate_lum.clamp(0.0, 1.0);
    (0.92 - 0.30 * l, 0.34 + 0.56 * l)
}

/// Plate luminance guessed from the fill when the caller does not say: a pale fill is cut into
/// something darker, a dark fill into something paler.
fn guess_plate_lum(fill: Color) -> f32 { if fill.perceived_brightness() > 0.6 { 0.38 } else { 0.72 } }

fn engraved_text(scene: &mut Scene, fill: Color, plate_lum: f32, at: Point, text: &str, size: f32, center: bool) {
    let (dark, light) = engrave_offsets(size);
    let (mut ad, mut al) = engrave_alphas(plate_lum);
    let mut fill = fill;
    // An unfilled cut (fill close to the plate) is carried by its walls alone. At low device
    // scales those walls are a single pixel, so give them full contrast and let the floor of
    // the cut sit a little darker than the plate.
    let unfilled = (fill.perceived_brightness() - plate_lum).abs() < 0.28;
    let low = (2.0 - device_scale()).clamp(0.0, 1.0);
    if unfilled {
        ad = (ad + 0.12 + 0.25 * low).min(1.0);
        al = (al + 0.12 + 0.30 * low).min(1.0);
        fill = fill.darker(0.10 + 0.22 * low);
        if low > 0.0 { scene.text(at.add(dark.mul(2.0)), text, BLACK.with_alpha(0.30 * low), size, center); }
    }
    scene.text(at.add(light), text, WHITE.with_alpha(al), size, center);
    scene.text(at.add(dark), text, BLACK.with_alpha(ad), size, center);
    // On a mid-tone plate neither wall has much contrast alone: seat the cut with a soft
    // contact shade a pixel further toward the light.
    if (0.25..0.75).contains(&plate_lum) && size >= 9.0 {
        scene.text(at.add(dark.mul(2.0)), text, BLACK.with_alpha(ad * 0.28), size, center);
    }
    scene.text(at, text, fill, size, center);
}

/// Text in a [`Legend`] style. `at` and `center` as in [`Scene::text`].
pub fn legend_text(scene: &mut Scene, legend: Legend, at: Point, text: &str, size: f32, center: bool) {
    match legend {
        Legend::Silk(ink) => {
            let ink = silk_ink(ink);
            let bleed = ink.with_alpha(ink.alpha_f() * 0.16);
            scene.text(Point::new(at.x + 0.45, at.y + 0.25), text, bleed, size, center);
            scene.text(Point::new(at.x - 0.3, at.y - 0.2), text, bleed, size, center);
            scene.text(at, text, ink, size, center);
        }
        Legend::Engraved(fill) => engraved_text(scene, fill, guess_plate_lum(fill), at, text, size, center),
    }
}

/// [`legend_text`] on a plate of a known colour: the engraving's walls take their contrast from
/// the plate's luminance, so the cut reads on pale, mid-tone and dark plates alike. Silk-screen
/// ink is unchanged.
pub fn legend_text_on(scene: &mut Scene, legend: Legend, plate: Color, at: Point, text: &str, size: f32, center: bool) {
    match legend {
        Legend::Engraved(fill) => engraved_text(scene, fill, plate.perceived_brightness(), at, text, size, center),
        silk => legend_text(scene, silk, at, text, size, center),
    }
}

/// Printed or engraved line (ticks, rules).
pub fn legend_line(scene: &mut Scene, legend: Legend, a: Point, b: Point, width: f32) {
    match legend {
        Legend::Silk(ink) => {
            let ink = silk_ink(ink);
            scene.line(a, b, ink.with_alpha(ink.alpha_f() * 0.18), width + 0.7);
            scene.line(a, b, ink, width);
        }
        Legend::Engraved(fill) => {
            let (dark, light) = engrave_offsets(width * 10.0);
            let (ad, al) = engrave_alphas(guess_plate_lum(fill));
            scene.line(a.add(light), b.add(light), WHITE.with_alpha(al), width);
            scene.line(a.add(dark), b.add(dark), BLACK.with_alpha(ad * 0.85), width);
            scene.line(a, b, fill, width * 0.85);
        }
    }
}

/// Printed or engraved dot.
pub fn legend_dot(scene: &mut Scene, legend: Legend, c: Point, radius: f32) {
    match legend {
        Legend::Silk(ink) => {
            let ink = silk_ink(ink);
            scene.glow_ellipse(Scene::ellipse_at(c, radius + 0.45), ink.with_alpha(ink.alpha_f() * 0.9), 0.7, Blend::Over);
            scene.fill_ellipse_solid(Scene::ellipse_at(c, radius), ink);
        }
        Legend::Engraved(fill) => {
            let (dark, light) = engrave_offsets(radius * 8.0);
            let (ad, al) = engrave_alphas(guess_plate_lum(fill));
            scene.fill_ellipse_solid(Scene::ellipse_at(c.add(light), radius), WHITE.with_alpha(al));
            scene.fill_ellipse_solid(Scene::ellipse_at(c.add(dark), radius), BLACK.with_alpha(ad * 0.85));
            scene.fill_ellipse_solid(Scene::ellipse_at(c, radius * 0.9), fill);
        }
    }
}

/// A title on a rule: two silk-screen lines with a gap where the title sits.
pub fn draw_legend_line(scene: &mut Scene, ink: Color, x0: f32, x1: f32, y: f32, title: &str, font_size: f32) {
    let tw = font_size * 0.55 * title.chars().count() as f32 + 10.0;
    let cx = 0.5 * (x0 + x1);
    let legend = Legend::Silk(ink);
    let w = (font_size * 0.08).clamp(0.7, 1.4);
    if cx - tw * 0.5 > x0 {
        legend_line(scene, legend, Point::new(x0, y), Point::new(cx - tw * 0.5, y), w);
        legend_line(scene, legend, Point::new(cx + tw * 0.5, y), Point::new(x1, y), w);
    }
    legend_text(scene, legend, Point::new(cx, y), title, font_size, true);
}

/// Mark style for [`ScaleRing`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScaleMarks { Ticks, Dots }

/// A printed knob scale. Angles are measured from straight up, clockwise; the scale runs from
/// `-sweep` to `+sweep`.
#[derive(Clone, Copy, Debug)]
pub struct ScaleRing<'a> {
    pub center: Point,
    /// Inner radius of the marks (just outside the knob skirt).
    pub radius: f32,
    pub sweep: f32,
    pub n_major: usize,
    pub minor_per_major: usize,
    /// One per major mark, or fewer spread evenly (two labels go on the ends). Empty for none.
    pub labels: &'a [&'a str],
    pub legend: Legend,
    pub marks: ScaleMarks,
    pub font_size: f32,
    /// Length of a major tick; dots are sized from it.
    pub tick: f32,
}

impl<'a> ScaleRing<'a> {
    pub fn new(center: Point, radius: f32, n_major: usize, minor_per_major: usize, labels: &'a [&'a str]) -> Self {
        Self {
            center, radius, sweep: 0.8 * PI, n_major, minor_per_major, labels,
            legend: Legend::Silk(Color(0xffefebe0)), marks: ScaleMarks::Ticks,
            font_size: (radius * 0.3).clamp(7.0, 13.0), tick: (radius * 0.16).clamp(2.5, 7.0),
        }
    }
    /// Screen position at `angle` (from up, clockwise) and distance `dist` from the centre.
    pub fn at(&self, angle: f32, dist: f32) -> Point {
        Point::new(self.center.x + angle.sin() * dist, self.center.y - angle.cos() * dist)
    }
}

/// Printed scale around a knob: `n_major` majors over a ±0.8π sweep (zero straight up), minors
/// between them, and numerals outside. Off-white silk-screen ink.
pub fn draw_scale_ring(scene: &mut Scene, center: Point, radius: f32, n_major: usize, minor_per_major: usize, labels: &[&str]) {
    draw_scale_ring_with(scene, &ScaleRing::new(center, radius, n_major, minor_per_major, labels));
}

pub fn draw_scale_ring_with(scene: &mut Scene, s: &ScaleRing) {
    let n = s.n_major.max(2);
    let angle = |t: f32| -s.sweep + 2.0 * s.sweep * t;
    let steps = (n - 1) * (s.minor_per_major + 1);
    let w = (s.tick * 0.22).clamp(0.7, 1.5);
    for i in 0..=steps {
        let major = i % (s.minor_per_major + 1) == 0;
        let a = angle(i as f32 / steps as f32);
        match s.marks {
            ScaleMarks::Ticks => {
                let len = if major { s.tick } else { s.tick * 0.55 };
                legend_line(scene, s.legend, s.at(a, s.radius), s.at(a, s.radius + len), if major { w } else { w * 0.75 });
            }
            ScaleMarks::Dots => {
                let r = if major { (s.tick * 0.26).max(0.9) } else { (s.tick * 0.15).max(0.6) };
                legend_dot(scene, s.legend, s.at(a, s.radius + s.tick * 0.5), r);
            }
        }
    }
    let m = s.labels.len();
    if m == 0 { return; }
    let dist = s.radius + s.tick + s.font_size * 0.85;
    for (k, label) in s.labels.iter().enumerate() {
        let t = if m == n { k as f32 / (n - 1) as f32 } else if m == 1 { 0.5 } else { k as f32 / (m - 1) as f32 };
        let a = angle(t);
        // Labels sit a little further out toward the sides, where text is widest.
        let p = s.at(a, dist + a.sin().abs() * s.font_size * 0.28 * label.chars().count().min(4) as f32);
        legend_text(scene, s.legend, p, label, s.font_size, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panels_stay_within_command_budget() {
        for finish in 0..FINISH_COUNT {
            let mut sc = Scene::new();
            draw_panel(&mut sc, Rect::new(0.0, 0.0, 1000.0, 600.0), Color(0xffc0c4c6), Color(0xffa0a4a6), finish);
            assert!(sc.cmds.len() < 3000, "finish {finish}: {} commands", sc.cmds.len());
            let clips = sc.cmds.iter().filter(|c| matches!(c, crate::scene::Cmd::ClipRound { .. })).count();
            let pops = sc.cmds.iter().filter(|c| matches!(c, crate::scene::Cmd::PopClip)).count();
            assert_eq!(clips, pops);
        }
    }

    #[test]
    fn hardware_is_deterministic() {
        let draw = || {
            let mut sc = Scene::new();
            draw_panel(&mut sc, Rect::new(10.0, 10.0, 300.0, 120.0), Color(0xffdfe2e4), Color(0xffc3c8cb), FINISH_BRUSHED);
            draw_screw(&mut sc, 20.0, 20.0);
            draw_rack_ear(&mut sc, Rect::new(0.0, 0.0, 24.0, 120.0), true);
            draw_scale_ring(&mut sc, Point::new(100.0, 60.0), 20.0, 11, 1, &["0", "10"]);
            sc
        };
        let (a, b) = (draw(), draw());
        assert_eq!(a.cmds.len(), b.cmds.len());
        for (x, y) in a.cmds.iter().zip(b.cmds.iter()) {
            match (x, y) {
                (crate::scene::Cmd::Image { img: i, dest: d, .. }, crate::scene::Cmd::Image { img: j, dest: e, .. }) => { assert_eq!(i, j); assert_eq!(d, e); }
                _ => assert_eq!(x, y),
            }
        }
    }

    #[test]
    fn textures_are_seamless_sizes() {
        for t in [Tex::Grain, Tex::Brushed, Tex::Speckle, Tex::Hammer, Tex::Wood] {
            for scale in [1.0, 1.5, 2.0, 3.0] {
                let img = texture_at(t, scale);
                assert_eq!(img.px.len(), (img.w * img.h) as usize);
                assert_eq!(img.h, (128.0 * scale) as u32);
                assert!(img.px.iter().any(|p| *p >> 24 != 0));
            }
        }
    }

    #[test]
    fn scale_ring_is_symmetric() {
        let s = ScaleRing::new(Point::new(0.0, 0.0), 10.0, 11, 0, &[]);
        let l = s.at(-s.sweep, 10.0);
        let r = s.at(s.sweep, 10.0);
        assert!((l.x + r.x).abs() < 1e-4 && (l.y - r.y).abs() < 1e-4);
        let up = s.at(0.0, 10.0);
        assert!(up.x.abs() < 1e-5 && (up.y + 10.0).abs() < 1e-5);
    }
}
