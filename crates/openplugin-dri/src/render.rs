//! Paint a GoodLookin DRI [`Scene`] into the pixels an [`openplugin_core::Editor`]
//! returns from `render`.
//!
//! [`SceneCanvas::frame`] does the bookkeeping the lit tier needs, then
//! rasterises with `goodlookindri-shell`'s software renderer, which knows every
//! command: the original shapes, the multi-stop and radial fills, soft and inner
//! shadows, and the image blits the lit knobs are made of.
//!
//! - It tells GoodLookin DRI the backing scale before painting, so lit sprites
//!   and panel grain are baked at device resolution and stay sharp.
//! - It tells it the colour the knobs stand on, so their reflections and
//!   bounced light match the plate.
//! - It keeps the scene and the framebuffer between frames and only
//!   reallocates when the size changes.
//! - [`SceneCanvas::frame_layered`] keeps a static layer (the plate, printed
//!   legends, screws) as pixels and repaints only what moves on top of it. A
//!   GoodLookin DRI 0.2 plate is shaded per pixel and is by far the most
//!   expensive part of a frame (about 16 ms for 240 × 160 points at 2×, 300 ms
//!   for 1080 × 700), against well under a millisecond for a cached lit knob.
//! - [`SceneCanvas::frame_damaged`] goes one step further: given the rectangles
//!   that changed, it restores the cached layer under each, replays the moving
//!   parts clipped to it, and leaves every other pixel as it was. The pixels
//!   equal a full [`SceneCanvas::frame_layered`] frame, provided the rectangles
//!   cover everything that differs from the last frame.
//!
//! ```ignore
//! fn render(&mut self, scale: f32) -> Frame<'_> {
//!     let (gain, warm) = (self.gain, self.warm);
//!     self.canvas.frame(scale, |dl| {
//!         draw_faceplate_code(dl, 240.0, 160.0, "Navy");
//!         draw_knob(dl, 60.0, 70.0, 28.0, gain, "GAIN", "0.0 dB", KnobStyle::Brit, Color::BRIT_RED);
//!     })
//! }
//! ```

use std::sync::{Mutex, MutexGuard};

use goodlookindri::color::Color as GlColor;
use goodlookindri::{draw, hardware};
use goodlookindri_shell::Framebuffer;
use goodlookindri::scene::{Cmd, Rect};
use openplugin_core::editor::DamageRect;
use openplugin_core::Frame;

use crate::canvas::DrawList;

/// GoodLookin DRI keeps the knob scale, the knob floor colour and the device scale
/// process-wide. A frame sets them, paints and rasterises under this lock, so two
/// canvases on different threads (a snapshot tool, tests) cannot bake a sprite at the
/// other's scale. Editors paint on the main thread, where it is never contended.
static PAINT: Mutex<()> = Mutex::new(());

pub(crate) fn paint_lock() -> MutexGuard<'static, ()> {
    PAINT.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A reusable drawing surface of a size in points ([`Self::resize`] changes it).
pub struct SceneCanvas {
    width: f32,
    height: f32,
    list: DrawList,
    framebuffer: Framebuffer,
    background: GlColor,
    /// The static layer's pixels, and what they were made for: (key, w, h, scale bits).
    layer: Vec<u32>,
    layer_for: Option<(u64, usize, usize, u32)>,
    /// The framebuffer holds the last complete layered frame.
    layered: bool,
    /// Pixels saved around a damaged rectangle while it is repainted.
    ring: Vec<(usize, u32)>,
}

/// A rectangle in device pixels, `x0..x1` by `y0..y1`.
type PixelRect = (usize, usize, usize, usize);

/// More disjoint rectangles than this are repainted as one frame.
const MAX_PIXEL_RECTS: usize = 6;
/// Past this share of the frame, a full repaint is cheaper than clipped ones.
const FULL_SHARE: f64 = 0.6;
/// How far the clip reaches past the pixel edges, in device pixels. Pixel
/// centres sit half a pixel inside, so this keeps them at full coverage
/// whatever the rounding of `pixels / scale * scale`.
const CLIP_SLACK: f32 = 0.01;

impl SceneCanvas {
    /// `width` and `height` in points: the editor's size.
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width: width.max(1.0),
            height: height.max(1.0),
            list: DrawList::new(),
            framebuffer: Framebuffer::new(1, 1),
            background: GlColor(0xff10_1214),
            layer: Vec::new(),
            layer_for: None,
            layered: false,
            ring: Vec::new(),
        }
    }

    /// Forget the static layer, so the next [`Self::frame_layered`] paints it again.
    pub fn invalidate_layer(&mut self) {
        self.layer_for = None;
    }

    /// What shows where nothing is painted. Opaque; alpha is ignored.
    pub fn set_background(&mut self, rgb: u32) {
        self.background = GlColor(0xff00_0000 | (rgb & 0x00ff_ffff));
    }

    pub fn size(&self) -> (f32, f32) {
        (self.width, self.height)
    }

    /// Draw at a new size in points. The next frame has the new pixel size,
    /// and the static layer is dropped so [`Self::frame_layered`] paints it
    /// again at that size. The same size changes nothing.
    pub fn resize(&mut self, width: f32, height: f32) {
        let (width, height) = (width.max(1.0), height.max(1.0));
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            self.invalidate_layer();
        }
    }

    /// Paint with `paint` at `scale` device pixels per point and return the
    /// pixels (`0x00RRGGBB`, top row first). The [`DrawList`] starts empty
    /// every frame; its `scene` is what is rasterised.
    pub fn frame(&mut self, scale: f32, paint: impl FnOnce(&mut DrawList)) -> Frame<'_> {
        let _paint = paint_lock();
        let (scale, w, h) = self.begin(scale);
        self.layered = false;
        self.framebuffer.clear(self.background);
        self.list.clear();
        paint(&mut self.list);
        self.framebuffer.replay(&self.list.scene, scale);
        Frame { pixels: &self.framebuffer.pixels, width: w, height: h }
    }

    /// [`Self::frame`] in two layers. `layer` paints what does not move (the
    /// plate, legends, screws). It runs, and is rasterised, only when `key`,
    /// the size or the scale changed since the last time, or after
    /// [`Self::invalidate_layer`]; otherwise its pixels are reused. `paint`
    /// paints the moving parts on top, every frame. Put anything that changes
    /// the static look into `key` (a theme, a plate code, a disabled state).
    pub fn frame_layered(
        &mut self,
        scale: f32,
        key: u64,
        layer: impl FnOnce(&mut DrawList),
        paint: impl FnOnce(&mut DrawList),
    ) -> Frame<'_> {
        self.frame_damaged(scale, key, layer, paint, None)
    }

    /// [`Self::frame_layered`] that repaints only `damage`, in points:
    ///
    /// * `None`: everything, as [`Self::frame_layered`].
    /// * `Some(&[])`: nothing changed. The last frame is returned and neither
    ///   closure runs.
    /// * `Some(rects)`: for each rectangle (rounded outwards to whole device
    ///   pixels) the cached layer is restored and `paint`'s scene is replayed
    ///   clipped to it. `paint` runs once; pixels outside the rectangles are
    ///   not touched.
    ///
    /// The result is pixel-identical to a full frame when the rectangles
    /// cover every pixel that differs from the previous frame (anti-aliased
    /// edges, shadows and glows included). Whatever makes this impossible (a
    /// new size, scale, `key`, an invalidated layer, a [`Self::frame`] in
    /// between, many or large rectangles) falls back to a full frame.
    pub fn frame_damaged(
        &mut self,
        scale: f32,
        key: u64,
        layer: impl FnOnce(&mut DrawList),
        paint: impl FnOnce(&mut DrawList),
        damage: Option<&[DamageRect]>,
    ) -> Frame<'_> {
        let _paint = paint_lock();
        let (scale, w, h) = self.begin(scale);
        let wanted = (key, w, h, scale.to_bits());
        let layer_ready = self.layer_for == Some(wanted);
        let partial = match damage {
            Some(rects) if layer_ready && self.layered => pixel_rects(rects, scale, w, h),
            _ => None,
        };
        if let Some(rects) = &partial {
            if rects.is_empty() {
                return Frame { pixels: &self.framebuffer.pixels, width: w, height: h };
            }
        } else if !layer_ready {
            self.layered = false;
            self.framebuffer.clear(self.background);
            self.list.clear();
            layer(&mut self.list);
            self.framebuffer.replay(&self.list.scene, scale);
            self.layer.clear();
            self.layer.extend_from_slice(&self.framebuffer.pixels);
            self.layer_for = Some(wanted);
        } else {
            self.framebuffer.pixels.copy_from_slice(&self.layer);
        }
        // The floor colour a plate recorded stays on the list for the knobs.
        self.list.clear();
        paint(&mut self.list);
        match partial {
            None => self.framebuffer.replay(&self.list.scene, scale),
            Some(rects) => self.replay_in(&rects, scale),
        }
        self.layered = true;
        Frame { pixels: &self.framebuffer.pixels, width: w, height: h }
    }

    /// Restore the layer under each rectangle and replay the list inside it.
    fn replay_in(&mut self, rects: &[PixelRect], scale: f32) {
        let width = self.framebuffer.width;
        let height = self.framebuffer.height;
        let clip = |r: &PixelRect| Cmd::ClipRound {
            rect: Rect::new(
                (r.0 as f32 - CLIP_SLACK) / scale,
                (r.1 as f32 - CLIP_SLACK) / scale,
                ((r.2 - r.0) as f32 + 2.0 * CLIP_SLACK) / scale,
                ((r.3 - r.1) as f32 + 2.0 * CLIP_SLACK) / scale,
            ),
            radius: 0.0,
        };
        self.list.scene.cmds.insert(0, clip(&rects[0]));
        self.list.scene.cmds.push(Cmd::PopClip);
        for r in rects {
            let &(x0, y0, x1, y1) = r;
            for y in y0..y1 {
                let row = y * width;
                self.framebuffer.pixels[row + x0..row + x1].copy_from_slice(&self.layer[row + x0..row + x1]);
            }
            // The clip reaches a hair past the rectangle, so the ring of pixels around it can
            // pick up a trace of paint. Put the ring back exactly as it was.
            self.ring.clear();
            let (rx0, ry0) = (x0.saturating_sub(1), y0.saturating_sub(1));
            let (rx1, ry1) = ((x1 + 1).min(width), (y1 + 1).min(height));
            for y in ry0..ry1 {
                for x in rx0..rx1 {
                    if x < x0 || x >= x1 || y < y0 || y >= y1 {
                        self.ring.push((y * width + x, self.framebuffer.pixels[y * width + x]));
                    }
                }
            }
            self.list.scene.cmds[0] = clip(r);
            self.framebuffer.replay(&self.list.scene, scale);
            for &(i, v) in &self.ring {
                self.framebuffer.pixels[i] = v;
            }
        }
        self.list.scene.cmds.pop();
        self.list.scene.cmds.remove(0);
    }

    /// Clamp the scale, tell GoodLookin DRI about it, and size the framebuffer.
    fn begin(&mut self, scale: f32) -> (f32, usize, usize) {
        let scale = if scale.is_finite() { scale.clamp(1.0, 4.0) } else { 2.0 };
        draw::set_knob_scale(scale);
        hardware::set_device_scale(scale);
        let w = (self.width * scale).round().max(1.0) as usize;
        let h = (self.height * scale).round().max(1.0) as usize;
        if self.framebuffer.width != w || self.framebuffer.height != h {
            self.framebuffer = Framebuffer::new(w, h);
        }
        (scale, w, h)
    }

    /// The scene of the last frame (for tests, or a second backend).
    pub fn last_scene(&self) -> &goodlookindri::scene::Scene {
        &self.list.scene
    }
}

/// `damage` in device pixels: rounded outwards, cut to the frame, merged where
/// they touch. `None` when a full repaint is the better choice (a rectangle is
/// not finite, there are many, or together they cover most of the frame).
fn pixel_rects(damage: &[DamageRect], scale: f32, width: usize, height: usize) -> Option<Vec<PixelRect>> {
    let mut out: Vec<PixelRect> = Vec::new();
    for d in damage {
        if !d.is_finite() {
            return None;
        }
        let x0 = (d.x * scale).floor().clamp(0.0, width as f32) as usize;
        let y0 = (d.y * scale).floor().clamp(0.0, height as f32) as usize;
        let x1 = ((d.x + d.w) * scale).ceil().clamp(0.0, width as f32) as usize;
        let y1 = ((d.y + d.h) * scale).ceil().clamp(0.0, height as f32) as usize;
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let mut r = (x0, y0, x1, y1);
        while let Some(i) = out.iter().position(|o| r.0 <= o.2 && o.0 <= r.2 && r.1 <= o.3 && o.1 <= r.3) {
            let o = out.swap_remove(i);
            r = (r.0.min(o.0), r.1.min(o.1), r.2.max(o.2), r.3.max(o.3));
        }
        out.push(r);
    }
    let area: usize = out.iter().map(|r| (r.2 - r.0) * (r.3 - r.1)).sum();
    if out.len() > MAX_PIXEL_RECTS || area as f64 > FULL_SHARE * (width * height) as f64 {
        return None;
    }
    // A stable order, so the same damage always paints the same way.
    out.sort_unstable();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{draw_faceplate_code, draw_knob_code, Color, KnobTier};

    #[test]
    fn lit_knob_reaches_the_pixels() {
        let mut canvas = SceneCanvas::new(120.0, 120.0);
        let lit = canvas
            .frame(2.0, |dl| {
                draw_faceplate_code(dl, 120.0, 120.0, "Navy");
                draw_knob_code(dl, 60.0, 50.0, 30.0, 0.5, "GAIN", "0 dB", "Brit", Color::BRIT_RED);
            })
            .pixels
            .to_vec();
        let images = |scene: &goodlookindri::scene::Scene| {
            scene.cmds.iter().filter(|c| matches!(c, goodlookindri::scene::Cmd::Image { .. })).count()
        };
        let lit_images = images(canvas.last_scene());
        assert_eq!(lit.len(), 240 * 240);

        let classic = canvas
            .frame(2.0, |dl| {
                dl.knob_tier = KnobTier::Classic;
                draw_faceplate_code(dl, 120.0, 120.0, "Navy");
                draw_knob_code(dl, 60.0, 50.0, 30.0, 0.5, "GAIN", "0 dB", "Brit", Color::BRIT_RED);
            })
            .pixels
            .to_vec();
        // The lit tier is the default: one sprite more than the classic drawing.
        assert_eq!(lit_images, images(canvas.last_scene()) + 1);
        // Same plate, different knob: the knob area differs, a corner does not.
        let at = |px: &[u32], x: usize, y: usize| px[y * 240 + x];
        assert_eq!(at(&lit, 4, 230), at(&classic, 4, 230));
        let differs = (80..140).any(|y| (90..150).any(|x| at(&lit, x, y) != at(&classic, x, y)));
        assert!(differs, "the lit and classic knobs are drawn differently");
    }

    #[test]
    fn layered_frame_matches_a_full_frame_and_reuses_the_layer() {
        let paint_knob = |dl: &mut DrawList| draw_knob_code(dl, 60.0, 50.0, 30.0, 0.7, "GAIN", "3 dB", "Brit", Color::BRIT_RED);
        let mut full = SceneCanvas::new(120.0, 120.0);
        let expected = full
            .frame(2.0, |dl| {
                draw_faceplate_code(dl, 120.0, 120.0, "Navy");
                paint_knob(dl);
            })
            .pixels
            .to_vec();
        let mut layered = SceneCanvas::new(120.0, 120.0);
        let mut layer_runs = 0;
        for _ in 0..3 {
            let got = layered.frame_layered(2.0, 7, |dl| {
                layer_runs += 1;
                draw_faceplate_code(dl, 120.0, 120.0, "Navy");
            }, paint_knob);
            assert!(got.pixels == expected.as_slice(), "same pixels as one full frame");
        }
        assert_eq!(layer_runs, 1, "the layer is painted once");
        layered.frame_layered(1.0, 7, |_| layer_runs += 1, |_| {});
        layered.frame_layered(1.0, 8, |_| layer_runs += 1, |_| {});
        layered.invalidate_layer();
        layered.frame_layered(1.0, 8, |_| layer_runs += 1, |_| {});
        assert_eq!(layer_runs, 4, "a new scale, a new key and an invalidation repaint it");
    }

    /// What the moving parts look like at one step of the damage tests: a lit knob that turns,
    /// and a text label elsewhere that changes with a counter.
    fn moving_parts(knob: f32, label: u32) -> impl Fn(&mut DrawList) {
        move |dl| {
            draw_knob_code(dl, 60.0, 50.0, 30.0, knob, "GAIN", &format!("{knob:.2}"), "Brit", Color::BRIT_RED);
            let mut s = goodlookindri::scene::Scene::new();
            s.text(goodlookindri::scene::Point::new(150.0, 100.0), &format!("N{label}"), GlColor(0xffffffff), 14.0, true);
            dl.append(s);
        }
    }

    const KNOB_BOX: DamageRect = DamageRect::new(10.0, 0.0, 100.0, 120.0);
    const LABEL_BOX: DamageRect = DamageRect::new(120.0, 80.0, 60.0, 30.0);

    #[test]
    fn damaged_frames_equal_full_frames_in_every_step() {
        // Steps: (knob, label, damage). A step that changes only one part damages only that part.
        let steps: [(f32, u32, Option<Vec<DamageRect>>); 9] = [
            (0.2, 0, None),
            (0.2, 0, Some(vec![])),
            (0.7, 0, Some(vec![KNOB_BOX])),
            (0.7, 1, Some(vec![LABEL_BOX])),
            (0.4, 2, Some(vec![KNOB_BOX, LABEL_BOX])),
            (0.4, 2, Some(vec![DamageRect::new(0.0, 0.0, 0.0, 0.0)])),
            // Fractional edges and a rectangle hanging off the frame.
            (0.9, 3, Some(vec![DamageRect::new(10.3, -5.0, 99.4, 121.7), LABEL_BOX.expanded(0.37)])),
            // Not finite or too large: a full repaint, still the same pixels.
            (0.1, 3, Some(vec![DamageRect::new(f32::NAN, 0.0, 1.0, 1.0)])),
            (0.5, 3, Some(vec![DamageRect::new(0.0, 0.0, 120.0, 120.0)])),
        ];
        for scale in [1.0f32, 1.5, 2.0, 2.37] {
            let mut full = SceneCanvas::new(200.0, 120.0);
            let mut part = SceneCanvas::new(200.0, 120.0);
            let plate = |dl: &mut DrawList| draw_faceplate_code(dl, 200.0, 120.0, "Navy");
            let mut painted = 0;
            for (i, (knob, label, damage)) in steps.iter().enumerate() {
                let want = full.frame_layered(scale, 3, plate, moving_parts(*knob, *label)).pixels.to_vec();
                let got = part.frame_damaged(scale, 3, plate, |dl| { painted += 1; moving_parts(*knob, *label)(dl) }, damage.as_deref());
                assert!(got.pixels == want.as_slice(), "scale {scale}, step {i}: damage-only differs from a full repaint");
            }
            assert_eq!(painted, steps.len() - 2, "an empty damage list paints nothing");
        }
    }

    #[test]
    fn damage_that_misses_a_change_shows_in_the_comparison() {
        // The comparison above can fail: a rectangle that leaves out the knob keeps the old knob.
        let mut part = SceneCanvas::new(200.0, 120.0);
        let plate = |dl: &mut DrawList| draw_faceplate_code(dl, 200.0, 120.0, "Navy");
        part.frame_damaged(2.0, 3, plate, moving_parts(0.2, 0), None);
        let got = part.frame_damaged(2.0, 3, plate, moving_parts(0.8, 0), Some(&[LABEL_BOX])).pixels.to_vec();
        let mut full = SceneCanvas::new(200.0, 120.0);
        let want = full.frame_layered(2.0, 3, plate, moving_parts(0.8, 0)).pixels.to_vec();
        assert!(got != want);
    }

    #[test]
    fn damage_falls_back_to_a_full_frame_when_the_cache_is_gone() {
        let mut canvas = SceneCanvas::new(200.0, 120.0);
        let plate = |dl: &mut DrawList| draw_faceplate_code(dl, 200.0, 120.0, "Navy");
        // Nothing painted yet: an empty damage list must not return a blank frame.
        let first = canvas.frame_damaged(2.0, 3, plate, moving_parts(0.5, 0), Some(&[])).pixels.to_vec();
        let mut full = SceneCanvas::new(200.0, 120.0);
        assert!(first == full.frame_layered(2.0, 3, plate, moving_parts(0.5, 0)).pixels);
        // A plain frame in between replaces the layered pixels.
        canvas.frame(2.0, |_| {});
        let again = canvas.frame_damaged(2.0, 3, plate, moving_parts(0.5, 0), Some(&[LABEL_BOX])).pixels.to_vec();
        assert!(again == first);
    }

    #[test]
    fn resize_repaints_the_layer_at_the_new_size() {
        let mut canvas = SceneCanvas::new(60.0, 40.0);
        canvas.set_background(0x000000);
        let mut runs = 0;
        let plate = |dl: &mut DrawList| draw_faceplate_code(dl, 60.0, 40.0, "Navy");
        let small = canvas.frame_layered(1.0, 1, |dl| { runs += 1; plate(dl) }, |_| {}).pixels.to_vec();
        assert_eq!(small.len(), 60 * 40);
        canvas.resize(120.0, 80.0);
        assert_eq!(canvas.size(), (120.0, 80.0));
        let plate_big = |dl: &mut DrawList| draw_faceplate_code(dl, 120.0, 80.0, "Navy");
        let big = canvas.frame_layered(1.0, 1, |dl| { runs += 1; plate_big(dl) }, |_| {});
        assert_eq!((big.width, big.height), (120, 80));
        // The plate covers the larger surface: its far corner is painted, not background.
        assert_ne!(big.pixels[119 + 79 * 120], 0, "painted at the new size");
        assert_eq!(runs, 2, "a new size repaints the layer");
        // The same size again keeps the layer.
        canvas.resize(120.0, 80.0);
        canvas.frame_layered(1.0, 1, |_| runs += 1, |_| {});
        assert_eq!(runs, 2);
        // Back to the first size: repainted, and the pixels equal the first frame.
        canvas.resize(60.0, 40.0);
        let again = canvas.frame_layered(1.0, 1, |dl| { runs += 1; plate(dl) }, |_| {}).pixels.to_vec();
        assert_eq!(runs, 3);
        assert!(again == small);
    }

    #[test]
    fn frame_follows_the_scale() {
        let mut canvas = SceneCanvas::new(50.0, 20.0);
        canvas.set_background(0x123456);
        let f = canvas.frame(1.0, |_| {});
        assert_eq!((f.width, f.height), (50, 20));
        assert!(f.pixels.iter().all(|p| *p == 0x123456));
        let f = canvas.frame(3.0, |_| {});
        assert_eq!((f.width, f.height), (150, 60));
        let f = canvas.frame(f32::NAN, |_| {});
        assert_eq!((f.width, f.height), (100, 40));
    }
}
