//! What the field, the menu and the tooltip share: the painter (a GoodLookin
//! scene replayed by the shell's software rasteriser, as the editors are
//! drawn), the colours, and the windows they are shown in.

use super::{EditorView, PixelFormat, Window};
use goodlookindri::color::Color;
use goodlookindri::scene::{Rect, Scene};
use goodlookindri_shell::Framebuffer;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, ImageFormat, PropMode, WindowClass};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

// The colours of the shell's own widgets (lists, bars), so a pop-up reads as part of the editor.
pub(super) const PANEL_TOP: Color = Color(0xff15_171a);
pub(super) const PANEL_BOTTOM: Color = Color(0xff1e_2226);
pub(super) const EDGE: Color = Color(0xff3a_3f45);
pub(super) const PICK_TOP: Color = Color(0xff3d_5a74);
pub(super) const PICK_BOTTOM: Color = Color(0xff2f_4a60);
pub(super) const TEXT: Color = Color(0xffb9_bfc6);
pub(super) const TEXT_DIM: Color = Color(0xff8a_929c);
pub(super) const TEXT_BRIGHT: Color = Color(0xfff4_f1e8);
pub(super) const FOCUS: Color = Color(0xffff_d36a);
pub(super) const BAD: Color = Color(0xffff_6a5a);
/// The text size of every pop-up, in points.
pub(super) const FONT: f32 = 12.0;

/// The largest side of a pop-up, in device pixels.
const MAX_SIDE: u32 = 4096;

/// A scene and the pixels it is replayed into.
pub(super) struct Painter {
    framebuffer: Framebuffer,
    scene: Scene,
}

impl Painter {
    pub(super) fn new() -> Painter {
        Painter { framebuffer: Framebuffer::new(1, 1), scene: Scene::new() }
    }

    /// Paint `draw`'s scene (in points) at `scale` into `width` x `height`
    /// device pixels: `0x00RRGGBB`, top row first.
    fn paint(&mut self, width: usize, height: usize, scale: f32, draw: impl FnOnce(&mut Scene)) -> &[u32] {
        let _paint = crate::render::paint_lock();
        self.framebuffer.width = width;
        self.framebuffer.height = height;
        self.framebuffer.pixels.clear();
        self.framebuffer.pixels.resize(width * height, EDGE.0 & 0x00ff_ffff);
        self.scene.clear();
        draw(&mut self.scene);
        self.framebuffer.replay(&self.scene, scale);
        &self.framebuffer.pixels
    }
}

/// The panel every pop-up stands on: an edge and a shaded body.
pub(super) fn panel(scene: &mut Scene, w: f32, h: f32, edge: Color) {
    scene.fill_rect(Rect::new(0.0, 0.0, w, h), edge, edge, true);
    scene.fill_round(Rect::new(1.0, 1.0, w - 2.0, h - 2.0), 2.0, PANEL_TOP, PANEL_BOTTOM, true);
}

/// Where a pop-up of `size` goes so that it starts at `at` and stays on a screen of `screen`.
pub(super) fn place(at: (i32, i32), size: (u32, u32), screen: (i32, i32)) -> (i32, i32) {
    let fit = |at: i32, size: u32, screen: i32| at.min(screen - size as i32).max(0);
    (fit(at.0, size.0, screen.0), fit(at.1, size.1, screen.1))
}

/// Send `width` x `height` pixels to a window. False when the connection is lost.
#[allow(clippy::too_many_arguments)]
fn put_image(conn: &RustConnection, format: &PixelFormat, gc: u32, max_request: usize, buf: &mut Vec<u8>, window: Window, pixels: &[u32], width: usize, height: usize) -> bool {
    if width == 0 || height == 0 || pixels.len() < width * height {
        return true;
    }
    let stride = format.stride(width);
    let rows_per = (max_request.saturating_sub(64) / stride.max(1)).max(1);
    let mut row = 0;
    while row < height {
        let rows = rows_per.min(height - row);
        buf.clear();
        buf.resize(stride * rows, 0);
        for (n, line) in buf.chunks_exact_mut(stride).enumerate() {
            let y = row + n;
            format.write_row(pixels[y * width..(y + 1) * width].iter().copied(), line);
        }
        if conn.put_image(ImageFormat::Z_PIXMAP, window, gc, width as u16, rows as u16, 0, row as i16, 0, format.depth, buf).is_err() {
            return false;
        }
        row += rows;
    }
    true
}

impl EditorView {
    /// Points in device pixels, rounded up, at least one.
    pub(super) fn px(&self, points: f32) -> u32 {
        if points.is_finite() {
            ((points * self.scale).ceil() as u32).clamp(1, MAX_SIDE)
        } else {
            1
        }
    }

    /// An atom by name; zero when the server gives none.
    pub(super) fn atom(&self, name: &[u8]) -> u32 {
        self.conn.intern_atom(false, name).ok().and_then(|cookie| cookie.reply().ok()).map_or(0, |reply| reply.atom)
    }

    /// Where a pixel of the view's window is on the root, and the root's size.
    pub(super) fn on_root(&self, x: i32, y: i32) -> Option<((i32, i32), (i32, i32))> {
        if !self.usable() {
            return None;
        }
        let clamp = |v: i32| v.clamp(-32768, 32767) as i16;
        let at = self.conn.translate_coordinates(self.window, self.root, clamp(x), clamp(y)).ok()?;
        let screen = self.conn.get_geometry(self.root).ok()?;
        let (at, screen) = (at.reply().ok()?, screen.reply().ok()?);
        Some(((i32::from(at.dst_x), i32::from(at.dst_y)), (i32::from(screen.width), i32::from(screen.height))))
    }

    /// Make and map a window with the view's visual: a child of the view's
    /// window, or, with `kind` (a `_NET_WM_WINDOW_TYPE` name), an
    /// override-redirect window on the root. `None` when the server refuses.
    pub(super) fn make_window(&mut self, kind: Option<&[u8]>, at: (i32, i32), size: (u32, u32), mask: EventMask) -> Option<Window> {
        if !self.usable() {
            return None;
        }
        let id = self.conn.generate_id().ok()?;
        let mut aux = CreateWindowAux::new().border_pixel(0).event_mask(mask);
        if kind.is_some() {
            aux = aux.override_redirect(1).save_under(1);
        }
        if let Some(colormap) = self.colormap {
            aux = aux.colormap(colormap);
        }
        let parent = if kind.is_some() { self.root } else { self.window };
        let clamp = |v: i32| v.clamp(-32768, 32767) as i16;
        let side = |v: u32| v.clamp(1, MAX_SIDE) as u16;
        let made = self.conn.create_window(self.format.depth, id, parent, clamp(at.0), clamp(at.1), side(size.0), side(size.1), 0, WindowClass::INPUT_OUTPUT, self.visual, &aux);
        match made.map(|cookie| cookie.check()) {
            Ok(Ok(())) => {}
            Ok(Err(x11rb::errors::ReplyError::X11Error(_))) => return None,
            _ => {
                self.dead = true;
                return None;
            }
        }
        if let Some(kind) = kind {
            let (property, value) = (self.atom(b"_NET_WM_WINDOW_TYPE"), self.atom(kind));
            if property != 0 && value != 0 {
                x!(self, self.conn.change_property32(PropMode::REPLACE, id, property, AtomEnum::ATOM, &[value]));
            }
        }
        x!(self, self.conn.map_window(id));
        // On screen for every client by the time the view says it is open.
        self.settle();
        (!self.dead).then_some(id)
    }

    /// Destroy a window the view made.
    pub(super) fn drop_window(&mut self, window: Window) {
        if !self.dead {
            x!(self, self.conn.destroy_window(window));
            self.settle();
        }
    }

    /// Paint `draw`'s scene (in points) into a window of `size` device pixels.
    pub(super) fn blit(&mut self, window: Window, size: (u32, u32), draw: impl FnOnce(&mut Scene)) {
        if self.dead {
            return;
        }
        let (w, h) = (size.0 as usize, size.1 as usize);
        let pixels = self.painter.paint(w, h, self.scale, draw);
        if !put_image(&self.conn, &self.format, self.gc, self.max_request, &mut self.buf, window, pixels, w, h) {
            self.dead = true;
        }
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popups_stay_on_the_screen() {
        assert_eq!(place((10, 20), (100, 50), (800, 600)), (10, 20));
        assert_eq!(place((750, 580), (100, 50), (800, 600)), (700, 550));
        assert_eq!(place((-5, -5), (100, 50), (800, 600)), (0, 0));
        // Larger than the screen: the top left corner shows.
        assert_eq!(place((10, 10), (900, 700), (800, 600)), (0, 0));
    }
}
