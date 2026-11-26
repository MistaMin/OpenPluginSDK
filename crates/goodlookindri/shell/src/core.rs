//! State shared by the platform windows: the app, the scene, the framebuffer and the smoke check.

use crate::raster::Framebuffer;
use goodlookindri::color::Color;
use goodlookindri::scene::Scene;
use std::collections::HashSet;
use std::time::Instant;

/// Keys the shell reports. Letters and digits are not sent yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key { Tab, Enter, Space, Up, Down, Escape }

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    MouseMove { x: f32, y: f32 },
    MouseDown { x: f32, y: f32 },
    MouseUp { x: f32, y: f32 },
    /// Wheel or trackpad scroll at a point. `dy` is in lines: positive scrolls content up.
    MouseWheel { x: f32, y: f32, dy: f32 },
    Key { key: Key, shift: bool },
    /// Sent about every 16 ms. `dt` is seconds since the last tick.
    Tick { dt: f32 },
}

pub trait App {
    /// Returns true when the picture needs repainting.
    fn event(&mut self, e: Event) -> bool;
    /// Records the picture in logical units (points).
    fn paint(&mut self, scene: &mut Scene, width: f32, height: f32);
}

pub struct Config {
    pub title: String,
    pub width: u32,
    pub height: u32,
    /// When set, the window closes by itself after this many milliseconds and the exit code
    /// says whether the frame was drawn.
    pub smoke_ms: Option<u64>,
}

pub(crate) struct Core {
    pub cfg: Config,
    app: Box<dyn App>,
    fb: Framebuffer,
    scene: Scene,
    w: f32,
    h: f32,
    scale: f32,
    started: Instant,
    last_tick: Instant,
    frames: u64,
}

impl Core {
    pub fn new(cfg: Config, app: Box<dyn App>) -> Self {
        let now = Instant::now();
        let (w, h) = (cfg.width as f32, cfg.height as f32);
        Self { cfg, app, fb: Framebuffer::new(1, 1), scene: Scene::new(), w, h, scale: 1.0, started: now, last_tick: now, frames: 0 }
    }

    /// Logical size and backing scale. The buffer is reallocated only when the pixel size changes.
    pub fn resize(&mut self, logical_w: f32, logical_h: f32, scale: f32) {
        self.w = logical_w;
        self.h = logical_h;
        self.scale = scale;
        let (pw, ph) = (((logical_w * scale).round() as usize).max(1), ((logical_h * scale).round() as usize).max(1));
        if pw != self.fb.width || ph != self.fb.height {
            self.fb = Framebuffer::new(pw, ph);
        }
    }

    pub fn event(&mut self, e: Event) -> bool { self.app.event(e) }

    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;
        self.app.event(Event::Tick { dt })
    }

    pub fn render(&mut self) -> &Framebuffer {
        self.scene.clear();
        goodlookindri::hardware::set_device_scale(self.scale);
        self.app.paint(&mut self.scene, self.w, self.h);
        self.fb.clear(Color(0xff000000));
        self.fb.replay(&self.scene, self.scale);
        self.frames += 1;
        &self.fb
    }

    /// `Some(exit code)` once the smoke time has passed and a frame was drawn.
    pub fn smoke_result(&self) -> Option<i32> {
        let ms = self.cfg.smoke_ms?;
        if self.frames == 0 || self.started.elapsed().as_millis() < ms as u128 { return None; }
        let mut seen = HashSet::new();
        for p in &self.fb.pixels {
            seen.insert(*p);
            if seen.len() >= 64 { break; }
        }
        let ok = seen.len() >= 32;
        println!("smoke {} frames={} size={}x{} colours>={}", if ok { "ok" } else { "FAILED: blank frame" }, self.frames, self.fb.width, self.fb.height, seen.len());
        Some(if ok { 0 } else { 1 })
    }
}
