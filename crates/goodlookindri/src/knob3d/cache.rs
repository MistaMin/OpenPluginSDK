//! Process-wide sprite cache with a byte budget.

use std::collections::HashMap;
use std::f32::consts::PI;
use std::sync::{Arc, Mutex, OnceLock};

use super::bake::KnobBake;
use super::model::{KnobModel, Rig, Sprite};
use super::presets::preset;
use super::shade::Lights;
use crate::color::Color;

/// Angle steps across the knob sweep of ±0.8π.
pub const ANGLE_STEPS: u32 = 1024;
const SWEEP: f32 = 1.6 * PI;
const DEFAULT_BYTES: usize = 64 << 20;
const MAX_BAKES: usize = 24;

type SpriteKey = (u64, u64, u32, i32, u32);
type BakeKey = (u64, u32, u32);

#[derive(Default)]
struct Inner {
    sprites: HashMap<SpriteKey, (Arc<Sprite>, u64)>,
    bakes: HashMap<BakeKey, (Arc<KnobBake>, u64)>,
    bytes: usize,
    tick: u64,
}

/// Shared cache of rendered knob sprites, least recently used out first.
pub struct KnobCache {
    inner: Mutex<Inner>,
    cap: usize,
}

impl Default for KnobCache {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_BYTES)
    }
}

fn sprite_bytes(s: &Sprite) -> usize {
    s.px.len() * 4 + 64
}

impl KnobCache {
    pub fn with_capacity(bytes: usize) -> Self {
        Self { inner: Mutex::new(Inner::default()), cap: bytes }
    }

    /// The cache shared by the whole process.
    pub fn global() -> &'static KnobCache {
        static G: OnceLock<KnobCache> = OnceLock::new();
        G.get_or_init(KnobCache::default)
    }

    /// Radius snapped to half pixels, angle snapped to [`ANGLE_STEPS`] over the sweep.
    pub fn quantise(radius_px: f32, angle: f32) -> (f32, f32) {
        let step = SWEEP / ANGLE_STEPS as f32;
        ((radius_px * 2.0).round().max(4.0) / 2.0, (angle / step).round() * step)
    }

    /// Sprite for a model. `floor` is the panel colour under the knob and replaces
    /// `rig.floor`.
    pub fn get(&self, model: &KnobModel, rig: &Rig, radius_px: f32, angle: f32, accent: Color, floor: Color) -> Arc<Sprite> {
        let rig = Rig { floor, ..*rig };
        let (radius, angle) = Self::quantise(radius_px, angle);
        let (mf, rf) = (model.fingerprint(), rig.fingerprint());
        let r2 = (radius * 2.0) as u32;
        let aq = (angle / (SWEEP / ANGLE_STEPS as f32)).round() as i32;
        let key: SpriteKey = (mf, rf, r2, aq, accent.0);
        // the bake holds no lighting, so one serves every panel colour
        let bkey: BakeKey = (mf, r2, accent.0);
        let bake = {
            let mut g = self.inner.lock().unwrap();
            g.tick += 1;
            let tick = g.tick;
            if let Some(e) = g.sprites.get_mut(&key) {
                e.1 = tick;
                return e.0.clone();
            }
            if g.bakes.len() >= MAX_BAKES && !g.bakes.contains_key(&bkey) {
                if let Some(old) = g.bakes.iter().min_by_key(|e| e.1 .1).map(|e| *e.0) {
                    g.bakes.remove(&old);
                }
            }
            let e = g.bakes.entry(bkey).or_insert_with(|| (Arc::new(KnobBake::build(model, &rig, radius, accent)), tick));
            e.1 = tick;
            e.0.clone()
        };
        // render outside the lock
        let sprite = Arc::new(bake.frame_lit(&Lights::new(&rig), angle));
        let mut g = self.inner.lock().unwrap();
        let tick = g.tick;
        if let Some(e) = g.sprites.get(&key) {
            return e.0.clone(); // another thread got there first
        }
        g.bytes += sprite_bytes(&sprite);
        g.sprites.insert(key, (sprite.clone(), tick));
        // bakes are the bulk of the memory: keep them to half the budget, oldest out first
        let mut bake_bytes: usize = g.bakes.values().map(|b| b.0.bytes()).sum();
        while bake_bytes > self.cap / 2 && g.bakes.len() > 1 {
            let Some(old) = g.bakes.iter().filter(|e| *e.0 != bkey).min_by_key(|e| e.1 .1).map(|e| *e.0) else { break };
            if let Some((b, _)) = g.bakes.remove(&old) {
                bake_bytes -= b.bytes();
            }
        }
        while g.bytes + bake_bytes > self.cap && g.sprites.len() > 1 {
            let Some(old) = g.sprites.iter().filter(|e| *e.0 != key).min_by_key(|e| e.1 .1).map(|e| *e.0) else { break };
            if let Some((s, _)) = g.sprites.remove(&old) {
                g.bytes -= sprite_bytes(&s);
            }
        }
        sprite
    }

    /// Same as [`KnobCache::get`] for a built-in preset code.
    pub fn get_preset(&self, code: &str, rig: &Rig, radius_px: f32, angle: f32, accent: Color, floor: Color) -> Option<Arc<Sprite>> {
        Some(self.get(&preset(code)?, rig, radius_px, angle, accent, floor))
    }

    /// Number of cached sprites and their total size in bytes.
    pub fn stats(&self) -> (usize, usize) {
        let g = self.inner.lock().unwrap();
        (g.sprites.len(), g.bytes)
    }

    pub fn clear(&self) {
        *self.inner.lock().unwrap() = Inner::default();
    }
}
