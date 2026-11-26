//! Contact sheet for the 3D knobs: every preset at three sizes and several angles over
//! three panels (brushed silver, dark blue, near-black), then a row of material swatches
//! and a row of shape modifiers. Writes a binary PPM and prints timings.
//!
//! `cargo run --release --example knob3d_sheet -- out.ppm`

use std::f32::consts::PI;
use std::time::Instant;

use goodlookindri::color::Color;
use goodlookindri::knob3d::{preset, render, style_model, STYLE_CODES, Angular, KnobBake, KnobCache, KnobModel, Material, Plan, Pointer, Rig, Segment, Shape, Sprite, PRESETS};

const COL: usize = 640;
const ROW: usize = 190;
const ANGLES: [f32; 6] = [-0.2 * PI, -0.8 * PI, 0.55 * PI, 0.0, -0.45 * PI, 0.8 * PI];
/// Panel colours handed to the renderer for the three columns.
const FLOORS: [u32; 3] = [0xffcc_cccc, 0xff17_3366, 0xff14_181b];

fn hash(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9e37_79b1) ^ (y as u32).wrapping_mul(0x85eb_ca6b);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h & 0xffff) as f32 / 65535.0
}

fn panel(x: usize, y: usize, rows: usize) -> [f32; 3] {
    let (xi, yi) = (x as i32, y as i32);
    let v = y as f32 / (ROW * rows) as f32;
    match x / COL {
        0 => {
            // brushed aluminium: long horizontal streaks plus a soft vertical gradient
            let streak = 0.5 * hash(xi / 90, yi) + 0.3 * hash((xi + 40) / 23, yi + 1000) + 0.2 * hash(xi, yi);
            let g = 0.80 + 0.07 * (streak - 0.5) - 0.05 * v;
            [g, g, g * 1.005]
        }
        1 => {
            let n = 0.012 * (hash(xi, yi) - 0.5);
            let k = 1.0 - 0.12 * v;
            [(0.09 + n) * k, (0.20 + n) * k, (0.40 + n) * k]
        }
        _ => {
            let n = 0.008 * (hash(xi, yi) - 0.5);
            [0.078 + n, 0.094 + n, 0.106 + n]
        }
    }
}

fn blit(img: &mut [u8], w: usize, h: usize, s: &Sprite, cx: i32, cy: i32) {
    let (x0, y0) = (cx - s.origin.0.round() as i32, cy - s.origin.1.round() as i32);
    for sy in 0..s.h as i32 {
        for sx in 0..s.w as i32 {
            let (x, y) = (x0 + sx, y0 + sy);
            if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                continue;
            }
            let p = s.px[(sy * s.w as i32 + sx) as usize];
            let ia = 255 - (p >> 24);
            let o = (y as usize * w + x as usize) * 3;
            for (c, shift) in [16u32, 8, 0].iter().enumerate() {
                let d = img[o + c] as u32;
                img[o + c] = ((p >> shift & 0xff) + (d * ia + 127) / 255).min(255) as u8;
            }
        }
    }
}

fn dots(img: &mut [u8], w: usize, h: usize, cx: i32, cy: i32, r: f32, ink: [u8; 3]) {
    for i in 0..11 {
        let a = -0.8 * PI + 1.6 * PI * i as f32 / 10.0;
        let (px, py) = (cx as f32 + a.sin() * r * 1.32, cy as f32 - a.cos() * r * 1.32);
        let dr = (r * 0.045).max(1.2);
        for y in (py - dr - 1.0) as i32..=(py + dr + 1.0) as i32 {
            for x in (px - dr - 1.0) as i32..=(px + dr + 1.0) as i32 {
                if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                    continue;
                }
                let d = ((x as f32 + 0.5 - px).powi(2) + (y as f32 + 0.5 - py).powi(2)).sqrt();
                let k = (dr + 0.5 - d).clamp(0.0, 1.0);
                let o = (y as usize * w + x as usize) * 3;
                for c in 0..3 {
                    img[o + c] = (img[o + c] as f32 * (1.0 - k) + ink[c] as f32 * k) as u8;
                }
            }
        }
    }
}

/// Plain knob that shows off one material.
fn swatch(m: Material, light_pointer: bool) -> KnobModel {
    let p = if light_pointer { Material::white_paint() } else { Material::ink() };
    KnobModel::new(vec![
        Segment::new(Shape::Dome, 0.0, 0.60, 0.80, 0.775, m),
        Segment::new(Shape::Dome, 0.60, 0.86, 0.775, 0.60, m),
        Segment::new(Shape::Line, 0.86, 0.93, 0.60, 0.03, m),
    ])
    .with_edge(0.02)
    .with_pointer(Pointer::line(0.34, 0.82, 0.07, p))
}

/// Flat-topped cylinder used to show the shape modifiers.
fn puck(m: Material) -> KnobModel {
    KnobModel::new(vec![
        Segment::new(Shape::Flat, 0.0, 0.84, 0.72, 0.72, m),
        Segment::new(Shape::Line, 0.84, 0.88, 0.72, 0.68, m),
        Segment::new(Shape::Line, 0.88, 0.93, 0.68, 0.03, m),
    ])
    .with_edge(0.012)
}

fn materials() -> Vec<KnobModel> {
    vec![
        swatch(Material::walnut(), true),
        swatch(Material::maple(), false),
        swatch(Material::brass(), false),
        swatch(Material::bronze(), false),
        swatch(Material::anodised(0.16, 0.36, 0.78), true),
        swatch(Material::rubber(0.12, 0.12, 0.13), true),
        swatch(Material::cream(), false),
        swatch(Material::translucent(0.80, 0.10, 0.08), true),
    ]
}

fn modifiers() -> Vec<KnobModel> {
    let alu = Material::satin_aluminium();
    let black = Material::black_matte();
    vec![
        // concentric lathe rings on the face
        puck(Material::satin_aluminium().with_roughness(0.2).with_rings(1.0)).with_pointer(Pointer::groove(0.3, 0.84, 0.07, 0.015, Material::ink())),
        // dot pointer
        puck(black).with_pointer(Pointer::dot(0.58, 0.2, 0.02, Material::white_paint())),
        // raised ridge pointer
        puck(black).with_pointer(Pointer::ridge(0.0, 0.8, 0.14, 0.08, black)).with_mark(Pointer::line(0.3, 0.76, 0.04, Material::white_paint())),
        // recessed insert disc
        puck(alu).with_insert(0.56, 0.06, Material::gloss(0.75, 0.12, 0.10)).with_pointer(Pointer::line(0.6, 0.9, 0.06, Material::ink())),
        // stepped two-tier body
        KnobModel::two_tier(0.34, 0.55, 0.80, black, alu).with_pointer(Pointer::line(0.1, 0.52, 0.06, Material::ink())),
        // hexagon
        puck(black).with_plan(Plan::Polygon { sides: 6, rounding: 0.15 }).with_pointer(Pointer::line(0.3, 0.76, 0.07, Material::white_paint())),
        // octagon
        puck(alu).with_plan(Plan::Polygon { sides: 8, rounding: 0.1 }).with_pointer(Pointer::line(0.3, 0.8, 0.07, Material::ink())),
        // rounded square
        puck(black).with_plan(Plan::Polygon { sides: 4, rounding: 0.35 }).with_pointer(Pointer::line(0.2, 0.62, 0.07, Material::white_paint())),
        // set screw in the wall, here under a knurled grip
        puck(alu).with_angular(Angular::knurl(30, 0.03, 0.86)).with_set_screw(0.45 * PI, 0.34, 0.13).with_pointer(Pointer::groove(0.3, 0.84, 0.07, 0.015, Material::ink())),
    ]
}

/// Close-up sheet of chosen knobs: `knob3d_sheet out.ppm focus <code>...`. Each knob is
/// shown large on the near-black and on the silver panel, then medium and at device
/// radius 24 at several angles, which is where pointers have to stay readable.
fn focus(out: &str, codes: &[String]) {
    const CW: usize = 1100;
    const CH: usize = 300;
    let (w, h) = (CW, CH * codes.len().max(1));
    let mut img = vec![0u8; w * h * 3];
    let split = [560usize, 820];
    for y in 0..h {
        for x in 0..w {
            let silver = (x >= 280 && x < split[0]) || x >= 960;
            let p = if silver { panel(x % 600, y % CH, 1) } else { panel(2 * COL, y % CH, 1) };
            for c in 0..3 {
                img[(y * w + x) * 3 + c] = (p[c].clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    let (dark, silver) = (Color(FLOORS[2]), Color(FLOORS[0]));
    let rig = Rig::studio();
    let cache = KnobCache::global();
    for (row, code) in codes.iter().enumerate() {
        let Some(model) = preset(code).or_else(|| style_model(code)) else {
            eprintln!("unknown knob {code}");
            continue;
        };
        let accent = Color(0xffe0_7a5a);
        let cy = (row * CH + CH / 2 + 14) as i32;
        let spots: [(f32, i32, f32, Color); 9] = [
            (100.0, 140, -0.2 * PI, dark),
            (100.0, 420, -0.2 * PI, silver),
            (48.0, 640, 0.55 * PI, dark),
            (48.0, 760, -0.8 * PI, dark),
            (24.0, 850, -0.2 * PI, dark),
            (24.0, 910, 0.8 * PI, dark),
            (24.0, 990, -0.2 * PI, silver),
            (24.0, 1050, 0.8 * PI, silver),
            (14.0, 880, 0.3 * PI, dark),
        ];
        for (k, (radius, x, angle, floor)) in spots.iter().enumerate() {
            let s = cache.get(&model, &rig, *radius, *angle, accent, *floor);
            let cy = if k == 8 { cy + 90 } else { cy };
            blit(&mut img, w, h, &s, *x, cy);
        }
    }
    let mut data = format!("P6\n{w} {h}\n255\n").into_bytes();
    data.extend_from_slice(&img);
    std::fs::write(out, data).expect("write ppm");
}

/// Every style and preset at one small device radius on the near-black panel, at two
/// angles: `knob3d_sheet out.ppm small 24`. Shows whether each pointer is still readable.
fn small(out: &str, radius: f32) {
    let codes: Vec<&str> = STYLE_CODES.iter().chain(PRESETS.iter()).copied().collect();
    let cell = (radius * 6.4) as usize;
    let cols = 10;
    let rows = codes.len().div_ceil(cols);
    let (w, h) = (cols * cell, rows * (cell / 2 + 8));
    let mut img = vec![0u8; w * h * 3];
    for (i, px) in img.chunks_mut(3).enumerate() {
        let p = panel(2 * COL, i / w, 1);
        for c in 0..3 {
            px[c] = (p[c].clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    let (rig, cache, dark) = (Rig::studio(), KnobCache::global(), Color(FLOORS[2]));
    for (i, code) in codes.iter().enumerate() {
        let model = if i < STYLE_CODES.len() { style_model(code) } else { preset(code) }.unwrap();
        let (cx, cy) = ((i % cols * cell) as i32, ((i / cols) * (cell / 2 + 8) + cell / 4 + 8) as i32);
        for (k, angle) in [-0.3 * PI, 0.65 * PI].iter().enumerate() {
            let s = cache.get(&model, &rig, radius, *angle, Color(0xffe0_7a5a), dark);
            blit(&mut img, w, h, &s, cx + (cell as i32 * (2 * k as i32 + 1)) / 4, cy);
        }
    }
    let mut data = format!("P6\n{w} {h}\n255\n").into_bytes();
    data.extend_from_slice(&img);
    std::fs::write(out, data).expect("write ppm");
}

/// Knobs at five angles across the sweep on the near-black panel:
/// `knob3d_sheet out.ppm angles 24` for every style and preset, or
/// `knob3d_sheet out.ppm angles 72 A wing` for chosen ones. A pointer has to stay
/// visible and unambiguous at each of them.
fn angles(out: &str, radius: f32, pick: &[String]) {
    let all: Vec<String> = STYLE_CODES.iter().chain(PRESETS.iter()).map(|c| c.to_string()).collect();
    let codes: &[String] = if pick.is_empty() { &all } else { pick };
    let cell = (radius * 2.7) as usize;
    let group = cell * 5 + cell / 3;
    let per_row = (1600 / group).clamp(1, 4).min(codes.len());
    let rows = codes.len().div_ceil(per_row);
    let (w, h) = (per_row * group, rows * cell);
    let mut img = vec![0u8; w * h * 3];
    for (i, px) in img.chunks_mut(3).enumerate() {
        let p = panel(2 * COL, i / w, 1);
        for c in 0..3 {
            px[c] = (p[c].clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    let (rig, cache, dark) = (Rig::studio(), KnobCache::global(), Color(FLOORS[2]));
    for (i, code) in codes.iter().enumerate() {
        // a preset name wins only where it is not also a style alias in the catalogue
        let model = if pick.is_empty() && i < STYLE_CODES.len() { style_model(code) } else { preset(code).or_else(|| style_model(code)) };
        let Some(model) = model else {
            eprintln!("unknown knob {code}");
            continue;
        };
        let (gx, cy) = ((i % per_row * group) as i32, ((i / per_row) * cell + cell / 2 + cell / 12) as i32);
        for (k, a) in [-0.8f32, -0.4, 0.0, 0.4, 0.8].iter().enumerate() {
            let s = cache.get(&model, &rig, radius, a * PI, Color(0xffe0_7a5a), dark);
            blit(&mut img, w, h, &s, gx + (cell * k + cell / 2) as i32, cy);
        }
    }
    let mut data = format!("P6\n{w} {h}\n255\n").into_bytes();
    data.extend_from_slice(&img);
    std::fs::write(out, data).expect("write ppm");
}

fn main() {
    let out = std::env::args().nth(1).expect("usage: knob3d_sheet <out.ppm> [focus <code>... | small <radius> | angles <radius> [code...]]");
    let rest: Vec<String> = std::env::args().skip(2).collect();
    if rest.first().map(String::as_str) == Some("focus") {
        focus(&out, &rest[1..]);
        return;
    }
    if rest.first().map(String::as_str) == Some("angles") {
        angles(&out, rest.get(1).and_then(|r| r.parse().ok()).unwrap_or(24.0), rest.get(2..).unwrap_or(&[]));
        return;
    }
    if rest.first().map(String::as_str) == Some("small") {
        small(&out, rest.get(1).and_then(|r| r.parse().ok()).unwrap_or(24.0));
        return;
    }
    let rows = PRESETS.len() + 2;
    let (w, h) = (COL * 3, ROW * rows);
    let mut img = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let p = panel(x, y, rows);
            for c in 0..3 {
                img[(y * w + x) * 3 + c] = (p[c].clamp(0.0, 1.0) * 255.0 + hash(x as i32 + c as i32 * 7, y as i32 + 31) - 0.5).round() as u8;
            }
        }
    }
    let floors = FLOORS.map(Color);
    let inks = [[40, 40, 42], [235, 235, 230], [200, 200, 196]];
    let cache = KnobCache::global();
    let rig0 = Rig { floor: floors[0], ..Rig::studio() };
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
    println!("{:<9} {:>9} {:>9} {:>9}   (radius 72: direct render, bake, mean frame per new angle)", "preset", "direct ms", "bake ms", "frame ms");
    let mut worst = 0.0f64;
    for (row, code) in PRESETS.iter().enumerate() {
        let model = preset(code).unwrap();
        let accent = Color(match *code {
            "gloss" => 0xff2e_8f6a,
            "strip" => 0xff2f_6fd0,
            "wing" => 0xffc8_3228,
            _ => 0xffe0_a020u32,
        });
        let t = Instant::now();
        let direct = render(&model, &rig0, 72.0, ANGLES[0], accent);
        let direct_ms = ms(t);
        let bake = KnobBake::new(&model, &rig0, 72.0, accent).unwrap();
        let t = Instant::now();
        bake.warm();
        let bake_ms = ms(t);
        let t = Instant::now();
        let n = 24;
        let mut sum = 0u32;
        for i in 0..n {
            sum = sum.wrapping_add(bake.frame(-2.4 + i as f32 * 0.2).px[direct.px.len() / 2]);
        }
        let frame_ms = ms(t) / n as f64;
        assert!(sum != 1);
        worst = worst.max(frame_ms);
        println!("{code:<9} {direct_ms:>9.1} {bake_ms:>9.1} {frame_ms:>9.2}");
        let cy = (row * ROW + ROW / 2 - 6) as i32;
        for (ci, floor) in floors.iter().enumerate() {
            // one large, two medium, three small
            let spots: [(f32, i32); 6] = [(64.0, 92), (40.0, 240), (40.0, 345), (26.0, 440), (26.0, 510), (26.0, 580)];
            for (k, (radius, x)) in spots.iter().enumerate() {
                let s = cache.get(&model, &rig0, *radius, ANGLES[k], accent, *floor);
                let cx = x + (ci * COL) as i32;
                dots(&mut img, w, h, cx, cy, *radius, inks[ci]);
                blit(&mut img, w, h, &s, cx, cy);
            }
        }
    }
    println!("slowest frame: {worst:.2} ms");
    for (extra, models) in [materials(), modifiers()].iter().enumerate() {
        let cy = ((PRESETS.len() + extra) * ROW + ROW / 2 - 4) as i32;
        let n = models.len() as i32;
        let radius = if n > 8 { 30.0 } else { 34.0 };
        for (ci, floor) in floors.iter().enumerate() {
            for (k, model) in models.iter().enumerate() {
                let s = cache.get(model, &rig0, radius, ANGLES[k % 3], Color(0xffe0_a020), *floor);
                let cx = (COL as i32 * (2 * k as i32 + 1)) / (2 * n) + (ci * COL) as i32;
                blit(&mut img, w, h, &s, cx, cy);
            }
        }
    }
    let (n, bytes) = cache.stats();
    println!("cache: {n} sprites, {:.1} MiB", bytes as f64 / (1 << 20) as f64);
    let mut data = format!("P6\n{w} {h}\n255\n").into_bytes();
    data.extend_from_slice(&img);
    std::fs::write(&out, data).expect("write ppm");
    println!("wrote {out}");
}
