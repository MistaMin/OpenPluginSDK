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
}

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
        let _paint = paint_lock();
        let (scale, w, h) = self.begin(scale);
        let wanted = (key, w, h, scale.to_bits());
        if self.layer_for != Some(wanted) {
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
        self.framebuffer.replay(&self.list.scene, scale);
        Frame { pixels: &self.framebuffer.pixels, width: w, height: h }
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
