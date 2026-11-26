//! "3D 2.0" knobs: each knob is a height field (radial profile plus angular detail) with
//! per-segment materials, shaded per pixel in linear light under a fixed studio rig and
//! seen by a slightly tilted camera, so the side wall and the cast shadow are visible.
//!
//! [`render`] returns a premultiplied [`Sprite`] that is blitted over any panel. The
//! lighting stays fixed in panel space while the knob turns.
//!
//! Angles follow [`crate::knobs`]: zero is straight up, positive is clockwise.

mod bake;
mod cache;
mod field;
mod model;
mod presets;
mod render;
mod shade;
mod styles;
mod vec;

pub use bake::KnobBake;
pub use cache::{KnobCache, ANGLE_STEPS};
pub use model::{Angular, AngularKind, Insert, KnobModel, Material, Part, Plan, Pointer, PointerKind, Rig, Segment, SetScrew, Shape, Sprite};
pub use presets::{preset, PRESETS};
pub use render::render;
pub use styles::{style_model, STYLE_CODES};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;

    fn small(code: &str, angle: f32) -> Sprite {
        render(&preset(code).unwrap(), &Rig::studio(), 14.0, angle, Color(0xffff_9a1f))
    }

    #[test]
    fn presets_exist() {
        for code in PRESETS {
            assert!(preset(code).is_some(), "{code}");
        }
        assert!(preset("nope").is_none());
    }

    #[test]
    fn corners_transparent_and_body_opaque() {
        for code in PRESETS {
            let s = small(code, 0.3);
            assert_eq!(s.px.len(), (s.w * s.h) as usize);
            let (w, h) = (s.w as usize, s.h as usize);
            for i in [0, w - 1, (h - 1) * w, h * w - 1] {
                assert_eq!(s.px[i], 0, "{code} corner {i}");
            }
            let at = |x: usize, y: usize| s.px[y * w + x];
            assert_eq!(at(s.origin.0 as usize, s.origin.1 as usize) >> 24, 0xff, "{code} centre");
            assert!(s.px.iter().any(|p| (1..255).contains(&(p >> 24))), "{code} has soft shadow");
            // premultiplied: no channel above alpha
            for p in &s.px {
                let a = p >> 24;
                assert!((p >> 16 & 0xff) <= a && (p >> 8 & 0xff) <= a && (p & 0xff) <= a);
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(small("spun", 1.0), small("spun", 1.0));
    }

    #[test]
    fn angle_changes_image() {
        for code in PRESETS {
            assert_ne!(small(code, -1.0).px, small(code, 1.0).px, "{code}");
        }
    }

    #[test]
    fn cache_returns_same_arc() {
        let cache = KnobCache::with_capacity(1 << 20);
        let (rig, acc, floor) = (Rig::studio(), Color(0xffff_9a1f), Color(0xff20_3060));
        let a = cache.get_preset("satin", &rig, 14.0, 0.5, acc, floor).unwrap();
        let b = cache.get_preset("satin", &rig, 14.1, 0.5004, acc, floor).unwrap();
        assert!(std::sync::Arc::ptr_eq(&a, &b));
        let c = cache.get_preset("satin", &rig, 14.0, 0.5, acc, Color(0xffc0_c0c0)).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&a, &c));
        let d = cache.get_preset("satin", &rig, 14.0, -0.5, acc, floor).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&a, &d));
        assert_eq!(cache.stats().0, 3);
        assert!(cache.get_preset("nope", &rig, 14.0, 0.0, acc, floor).is_none());
    }

    #[test]
    fn cache_respects_byte_budget() {
        let cache = KnobCache::with_capacity(60_000);
        let (rig, acc) = (Rig::studio(), Color(0xffff_9a1f));
        for i in 0..12 {
            cache.get_preset("ivory", &rig, 12.0, i as f32 * 0.1, acc, rig.floor).unwrap();
        }
        let (n, bytes) = cache.stats();
        assert!(n < 12 && bytes <= 60_000, "{n} sprites, {bytes} bytes");
    }

    /// Mean of the largest channel difference per pixel, and the count of pixels off by
    /// more than 24.
    fn diff(a: &Sprite, b: &Sprite) -> (f64, usize) {
        assert_eq!((a.w, a.h, a.origin), (b.w, b.h, b.origin));
        let (mut sum, mut big) = (0u64, 0usize);
        for (p, q) in a.px.iter().zip(&b.px) {
            let d = (0..4).map(|c| ((p >> (c * 8) & 0xff) as i32 - (q >> (c * 8) & 0xff) as i32).unsigned_abs()).max().unwrap();
            sum += d as u64;
            big += (d > 24) as usize;
        }
        (sum as f64 / a.px.len() as f64, big)
    }

    /// The cap-space buffer against the analytic field it was sampled from, same angle,
    /// same sampling pattern: this isolates what baking costs. Round, scalloped, fluted,
    /// bar-shaped and two-tier bodies all go through the bake.
    #[test]
    fn baked_frame_matches_direct_render() {
        let (rig, acc) = (Rig::studio(), Color(0xffff_9a1f));
        for code in PRESETS {
            let model = preset(code).unwrap();
            let bake = KnobBake::new(&model, &rig, 20.0, acc).expect("every model bakes");
            for angle in [-2.0f32, 0.4] {
                let (mean, big) = diff(&bake.frame(angle), &bake.frame_unbaked(angle));
                println!("{code} {angle}: mean {mean:.3}, {big} outliers");
                assert!(mean < 0.5 && big * 200 < (bake.frame(angle).px.len()), "{code} {angle}: mean {mean}, {big} outliers");
            }
        }
    }

    /// A frame (4 samples per pixel from the bake) against the public reference render (16
    /// jittered samples from the model). The gap is mostly sampling noise on mirror
    /// reflections and knurling, so the bound is looser.
    #[test]
    fn baked_frame_close_to_reference_render() {
        let (rig, acc) = (Rig::studio(), Color(0xffff_9a1f));
        for code in PRESETS {
            let model = preset(code).unwrap();
            let bake = KnobBake::new(&model, &rig, 20.0, acc).unwrap();
            for angle in [-2.0f32, 0.4] {
                let a = bake.frame(angle);
                let (mean, big) = diff(&a, &render(&model, &rig, 20.0, angle, acc));
                println!("{code} {angle}: mean {mean:.3}, {big} outliers");
                assert!(mean < 1.6 && big * 40 < a.px.len(), "{code} {angle}: mean {mean}, {big} outliers");
            }
        }
    }

    #[test]
    fn shadow_falls_below_the_knob() {
        let s = small("satin", 0.0);
        let (w, ox, oy) = (s.w as usize, s.origin.0 as usize, s.origin.1 as usize);
        let alpha = |x: usize, y: usize| s.px[y * w + x] >> 24;
        assert!(alpha(ox, oy + 18) > alpha(ox, oy.saturating_sub(24)));
    }
}
