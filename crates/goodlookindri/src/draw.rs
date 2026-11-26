//! Host entry points for flat, classic vector and per-pixel lit knobs.

use crate::color::Color;
use crate::design::{self, Item};
pub use crate::design::KnobMode;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use crate::knob3d::{KnobCache, Rig, Sprite};
use crate::knobs;
use crate::knobs_flat;
use crate::scene::{Image, Point, Rect, Scene};

pub use crate::hardware::use_dark_marks;

/// When true, [`knob`] draws the flat family. A `flat_` style prefix forces flat for that one knob.
pub static FLAT_KNOBS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn knob(scene: &mut Scene, bounds: Rect, proportion: f64, item: &Item, active: bool, dark_marks: bool) {
    let d = bounds.w.min(bounds.h);
    let c = bounds.center();
    let r = d * 0.5;
    let a = ((proportion.clamp(0.0, 1.0) as f32) * 1.6 - 0.8) * std::f32::consts::PI;
    let accent = Color::from_hex(&item.colour).unwrap_or(Color(0xff697170));
    let accent = if active { accent } else { Color(0xff697170) };
    knob_named_mode(scene, &item.style, c, r, a, accent, dark_marks, knob_mode());
}

/// Compatibility wrapper: false selects classic, true selects flat; prefixes override it.
pub fn knob_named(scene: &mut Scene, style: &str, c: Point, r: f32, angle: f32, accent: Color, dark_marks: bool, flat: bool) {
    knob_named_mode(scene, style, c, r, angle, accent, dark_marks,
        if flat { KnobMode::Flat } else { KnobMode::Classic });
}

static MODE: AtomicU8 = AtomicU8::new(0);
// 0x4000_0000 is 2.0f32.to_bits(); the const call is stable only from Rust 1.83.
static SCALE: AtomicU32 = AtomicU32::new(0x4000_0000);
static FLOOR: AtomicU32 = AtomicU32::new(0xff808080);

/// Select the default for `knob`, updating the legacy flat flag as well.
pub fn set_knob_mode(mode: KnobMode) {
    MODE.store(if mode == KnobMode::Lit { 1 } else { 0 }, Ordering::Relaxed);
    FLAT_KNOBS.store(mode == KnobMode::Flat, Ordering::Relaxed);
}

/// Direct writes to FLAT_KNOBS still override the default tier.
pub fn knob_mode() -> KnobMode {
    if FLAT_KNOBS.load(Ordering::Relaxed) { KnobMode::Flat }
    else if MODE.load(Ordering::Relaxed) == 1 { KnobMode::Lit }
    else { KnobMode::Classic }
}

/// Device pixels per logical unit (default 2). Invalid scales are ignored.
pub fn set_knob_scale(scale: f32) {
    if scale.is_finite() && scale > 0.0 { SCALE.store(scale.to_bits(), Ordering::Relaxed); }
}

/// Panel colour used for reflected and bounced light (default mid grey).
pub fn set_knob_floor(floor: Color) { FLOOR.store(floor.0, Ordering::Relaxed); }

/// Preset behind a code: the 15 presets map to themselves. The style mappings are the
/// older shared look; drawing uses [`crate::knob3d::style_model`] for catalogue styles.
pub fn lit_preset_for(code: &str) -> Option<&'static str> {
    if let Some(code) = crate::knob3d::PRESETS.iter().find(|&&p| p == code) { return Some(*code); }
    Some(match code {
        "Mtl" | "metal" | "Knl" | "Rck" => "spun",
        "Bk" | "Pie" => "bakelite",
        "Chk" | "Ptr" | "Tab" => "chicken",
        "A" => "wing",
        "Brit" | "console" | "N" | "FS" => "strip",
        "Rd" => "gloss",
        "Chr" | "Slv" => "chrome",
        "Ivr" => "ivory",
        "Snk" | "Skt" | "Tl" | "Fat" | "Mini" => "phenolic",
        "Dbl" => "collet",
        _ => return None,
    })
}

/// Sprite of a lit knob: a preset by its own code, else the model of the catalogue style.
fn lit_sprite(code: &str, r: f32, angle: f32, accent: Color) -> Option<Arc<Sprite>> {
    let scale = f32::from_bits(SCALE.load(Ordering::Relaxed));
    let floor = Color(FLOOR.load(Ordering::Relaxed));
    let (cache, rig) = (KnobCache::global(), Rig::studio());
    if crate::knob3d::PRESETS.contains(&code) {
        return cache.get_preset(code, &rig, r * scale, angle, accent, floor);
    }
    if let Some(model) = crate::knob3d::style_model(code) {
        return Some(cache.get(&model, &rig, r * scale, angle, accent, floor));
    }
    cache.get_preset(lit_preset_for(code)?, &rig, r * scale, angle, accent, floor)
}

/// Printed scale of a lit knob, as on the classic drawing of the style.
#[derive(Clone, Copy, PartialEq)]
enum LitScale { None, Dots(usize), Ticks }

fn lit_scale(code: &str) -> LitScale {
    match code {
        "Brit" | "console" | "Snk" => LitScale::Ticks,
        "N" | "A" | "Mic" | "Slv" | "ivory" | "Chr" | "Wnt" => LitScale::Dots(21),
        "FS" | "Mtl" | "metal" | "Rd" | "Ptr" | "Dm" | "Lvr" | "Tab" | "Skt" | "Fat" | "Chk" | "Mini" => LitScale::Dots(11),
        _ if crate::knob3d::PRESETS.contains(&code) => LitScale::Dots(11),
        _ => LitScale::None,
    }
}

/// Where a lit knob appears to sit: lift of its silhouette centre above the base centre
/// and the radius of the silhouette, both in knob radii. Tall knobs lean towards the top
/// of the screen under the tilted camera, so a scale centred on the base looks off.
fn lit_frame(code: &str) -> (f32, f32) {
    static FRAMES: OnceLock<Mutex<HashMap<String, (f32, f32)>>> = OnceLock::new();
    let mut frames = FRAMES.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap();
    if let Some(f) = frames.get(code) { return *f; }
    let model = crate::knob3d::preset(code)
        .or_else(|| crate::knob3d::style_model(code))
        .or_else(|| lit_preset_for(code).and_then(crate::knob3d::preset));
    let f = model.map_or((0.0, 1.0), |m| m.visual_frame(&Rig::studio()));
    frames.insert(code.to_string(), f);
    f
}

/// One weight for every style: fine dots or hairline ticks just outside the skirt,
/// centred on where the knob appears to sit.
fn draw_lit_scale(scene: &mut Scene, code: &str, c: Point, r: f32, dark_marks: bool) {
    let ink = if dark_marks { Color(0xff252a2d) } else { Color(0xffc7ccce) };
    let (lift, reach) = lit_frame(code);
    let c = Point::new(c.x, c.y - lift * r);
    let r = r * reach.max(1.0);
    let at = |a: f32, d: f32| Point::new(c.x + a.sin() * d, c.y - a.cos() * d);
    let sweep = |i: usize, n: usize| (-0.8 + 1.6 * i as f32 / (n - 1) as f32) * std::f32::consts::PI;
    match lit_scale(code) {
        LitScale::None => {}
        LitScale::Dots(n) => {
            let d = (r * 0.045).max(1.0);
            for i in 0..n {
                let p = at(sweep(i, n), r * 1.12);
                scene.fill_ellipse_solid(Rect::new(p.x - d * 0.5, p.y - d * 0.5, d, d), ink);
            }
        }
        LitScale::Ticks => {
            let w = (r * 0.025).clamp(0.6, 1.1);
            for i in 0..13 {
                let major = i % 3 == 0;
                let a = sweep(i, 13);
                scene.line(at(a, r * 1.08), at(a, r * if major { 1.20 } else { 1.14 }), ink, if major { w } else { w * 0.7 });
            }
        }
    }
}

// Retaining the sprite prevents address reuse. Copy pixels only on a cache miss.
// LRU bounds both the entry count and retained pixel memory (sprite + image).
#[derive(Default)]
struct Images {
    entries: HashMap<usize, (Arc<Sprite>, Arc<Image>, u64)>,
    tick: u64,
    bytes: usize,
}
fn sprite_image(sprite: &Arc<Sprite>) -> Arc<Image> {
    static IMAGES: OnceLock<Mutex<Images>> = OnceLock::new();
    let mut cache = IMAGES.get_or_init(|| Mutex::new(Images::default())).lock().unwrap();
    cache.tick += 1;
    let tick = cache.tick;
    let key = Arc::as_ptr(sprite) as usize;
    if let Some(entry) = cache.entries.get_mut(&key) {
        entry.2 = tick;
        return entry.1.clone();
    }
    let img = Arc::new(Image::new(sprite.w, sprite.h, sprite.px.clone()));
    cache.bytes += sprite.px.len() * 8;
    cache.entries.insert(key, (sprite.clone(), img.clone(), tick));
    while cache.entries.len() > 128 || (cache.bytes > 32 << 20 && cache.entries.len() > 1) {
        let old = *cache.entries.iter().min_by_key(|(_, e)| e.2).unwrap().0;
        let entry = cache.entries.remove(&old).unwrap();
        cache.bytes -= entry.0.px.len() * 8;
    }
    img
}

/// Draw in a chosen tier; explicit style prefixes take precedence.
pub fn knob_named_mode(scene: &mut Scene, style: &str, c: Point, r: f32, angle: f32, accent: Color, dark_marks: bool, mode: KnobMode) {
    let (explicit, code) = design::parse_style(style);
    let mode = if explicit == KnobMode::Classic { mode } else { explicit };
    if mode == KnobMode::Flat {
        knobs_flat::draw_style_flat(scene, code, c, r, angle, accent, dark_marks);
        return;
    }
    if mode == KnobMode::Lit {
        if let Some(sprite) = lit_sprite(code, r, angle, accent) {
            let scale = f32::from_bits(SCALE.load(Ordering::Relaxed));
            draw_lit_scale(scene, code, c, r, dark_marks);
            let img = sprite_image(&sprite);
            let dest = Rect::new(c.x - sprite.origin.0 / scale, c.y - sprite.origin.1 / scale,
                sprite.w as f32 / scale, sprite.h as f32 / scale);
            scene.image(&img, dest, 1.0);
            return;
        }
    }
    knobs::draw_style(scene, code, c, r, angle, accent, dark_marks);
}

pub fn style_count() -> usize { design::KNOB_STYLES.len() }
