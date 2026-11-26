//! Host framework for GoodLookin DRI: opens a window, replays [`goodlookindri::scene::Scene`]
//! with a software rasteriser, and delivers mouse input.
//!
//! No GPU API is used. Windows draws with GDI (Windows 7 to 11), macOS with Cocoa and
//! CoreGraphics (10.6 to current). Everything the app draws is a `Scene`.

mod core;
pub mod text;
pub mod ui;
pub mod raster;

#[cfg(target_os = "macos")]
mod cocoa;
#[cfg(windows)]
mod win32;

pub use crate::core::{App, Config, Event, Key};
pub use crate::raster::Framebuffer;

/// Opens the window and runs until it closes. Returns the process exit code.
pub fn run(cfg: Config, app: Box<dyn App>) -> i32 {
    let core = crate::core::Core::new(cfg, app);
    #[cfg(windows)]
    { return win32::run(core); }
    #[cfg(target_os = "macos")]
    { return cocoa::run(core); }
    #[cfg(not(any(windows, target_os = "macos")))]
    { let _ = core; eprintln!("goodlookindri-shell: no window backend for this system"); 2 }
}

/// Paints one frame without a window. `width` and `height` are logical; the buffer is
/// `scale` times larger. Used for tests, CI and systems with no display.
pub fn render_offscreen(app: &mut dyn App, width: u32, height: u32, scale: f32) -> Framebuffer {
    goodlookindri::hardware::set_device_scale(scale);
    let mut scene = goodlookindri::scene::Scene::new();
    app.paint(&mut scene, width as f32, height as f32);
    let mut fb = Framebuffer::new((width as f32 * scale).round() as usize, (height as f32 * scale).round() as usize);
    fb.clear(goodlookindri::color::Color(0xff000000));
    fb.replay(&scene, scale);
    fb
}
