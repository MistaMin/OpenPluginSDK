//! Tooltips: a small override-redirect window with the text of the
//! [`Tooltip`](openplugin_core::editor::Tooltip) rectangle the pointer rests in.

use super::popup::{self, EDGE, FONT, TEXT_BRIGHT};
use super::{EditorView, Window};
use goodlookindri::scene::{Point, Scene};
use goodlookindri_shell::text;
use openplugin_core::editor::DamageRect;
use std::time::{Duration, Instant};
use x11rb::protocol::xproto::{self, EventMask};

/// How long the pointer rests inside a tooltip rectangle before the tip
/// shows, in milliseconds.
pub const TOOLTIP_DELAY_MS: u64 = 600;

// The tip's measures, in points.
const PAD_X: f32 = 8.0;
const PAD_Y: f32 = 5.0;
const MAX_TEXT_WIDTH: f32 = 280.0;
/// From the pointer to the tip, so the pointer is never inside it.
const OFFSET: (f32, f32) = (12.0, 18.0);

/// The tip on screen.
struct Tip {
    window: Window,
    /// Device pixels.
    size: (u32, u32),
    /// The rectangle it belongs to, in editor points: leaving it hides the tip.
    rect: DamageRect,
    lines: Vec<String>,
}

/// Where the pointer is and since when it rests.
#[derive(Default)]
pub(super) struct Tips {
    /// The pointer's place in editor points and in window pixels.
    at: (f32, f32),
    pixel: (i16, i16),
    /// When the pointer last moved; `None` when no tip is due (it left, a
    /// button or a key went down, the tip for this rest was already looked up).
    rest: Option<Instant>,
    shown: Option<Tip>,
}

impl Tips {
    pub(super) fn window(&self) -> Option<Window> {
        self.shown.as_ref().map(|t| t.window)
    }

    /// No tip until the pointer moves again.
    pub(super) fn forget(&mut self) {
        self.rest = None;
    }
}

fn inside(rect: &DamageRect, at: (f32, f32)) -> bool {
    rect.is_finite() && !rect.is_empty() && at.0 >= rect.x && at.0 < rect.x + rect.w && at.1 >= rect.y && at.1 < rect.y + rect.h
}

/// The tip's lines and its size in points.
fn tip_layout(tip: &str) -> (Vec<String>, (f32, f32)) {
    let lines = text::wrap(tip.trim(), FONT, MAX_TEXT_WIDTH);
    let widest = lines.iter().map(|l| text::width(l, FONT)).fold(0.0, f32::max);
    let height = text::line_height(FONT) * lines.len() as f32;
    (lines, ((widest + 2.0 * PAD_X).ceil(), (height + 2.0 * PAD_Y).ceil()))
}

impl Tip {
    fn draw(&self, scene: &mut Scene, w: f32, h: f32) {
        popup::panel(scene, w, h, EDGE);
        let line = text::line_height(FONT);
        for (i, text) in self.lines.iter().enumerate() {
            scene.text(Point::new(PAD_X, PAD_Y + line * (i as f32 + 0.5)), text.as_str(), TEXT_BRIGHT, FONT, false);
        }
    }
}

impl EditorView {
    /// The tooltip rectangles in editor points (x, y, width, height) with the
    /// text the editor answers for each now. Empty and non-finite rectangles
    /// are left out.
    #[doc(hidden)]
    pub fn tooltip_rects(&self) -> Vec<([f64; 4], String)> {
        let tips = self.editor.tooltips();
        tips.into_iter()
            .filter(|t| t.rect.is_finite() && !t.rect.is_empty())
            .map(|t| ([f64::from(t.rect.x), f64::from(t.rect.y), f64::from(t.rect.w), f64::from(t.rect.h)], t.text))
            .collect()
    }

    /// The tooltip window on screen (an override-redirect child of the root), if any.
    pub fn tooltip_window(&self) -> Option<Window> {
        self.tips.window()
    }

    /// The text of the tooltip on screen, if any.
    pub fn tooltip_text(&self) -> Option<String> {
        self.tips.shown.as_ref().map(|t| t.lines.join("\n"))
    }

    /// The pointer moved over the view.
    pub(super) fn tip_motion(&mut self, e: &xproto::MotionNotifyEvent) {
        let at = self.point(e.event_x, e.event_y);
        let moved = (e.event_x, e.event_y) != self.tips.pixel;
        self.tips.at = at;
        self.tips.pixel = (e.event_x, e.event_y);
        if let Some(tip) = self.tips.shown.as_ref() {
            // The tip stays while the pointer is in its rectangle.
            if inside(&tip.rect, at) {
                return;
            }
            self.hide_tip();
        }
        // A drag shows no tip.
        let dragging = u16::from(e.state) & 0x1f00 != 0;
        if dragging {
            self.tips.rest = None;
        } else if moved || self.tips.rest.is_none() {
            self.tips.rest = Some(Instant::now());
        }
    }

    /// Show the tip once the pointer has rested long enough in a rectangle.
    pub(super) fn tip_check(&mut self) {
        let due = self.tips.rest.is_some_and(|since| since.elapsed() >= Duration::from_millis(TOOLTIP_DELAY_MS));
        if !due || self.tips.shown.is_some() {
            return;
        }
        // One look per rest: the editor is not asked again until the pointer moves.
        self.tips.rest = None;
        if self.field.is_some() || self.menu.is_some() || !self.shown() {
            return;
        }
        let at = self.tips.at;
        // The text is asked now, so a value that changed since is current.
        let Some(tip) = self.editor.tooltips().into_iter().find(|t| inside(&t.rect, at)) else { return };
        let (lines, (w, h)) = tip_layout(&tip.text);
        if lines.iter().all(|l| l.is_empty()) {
            return;
        }
        let size = (self.px(w), self.px(h));
        let Some((pointer, screen)) = self.on_root(i32::from(self.tips.pixel.0), i32::from(self.tips.pixel.1)) else { return };
        let (dx, dy) = ((OFFSET.0 * self.scale) as i32, (OFFSET.1 * self.scale) as i32);
        let mut at = (pointer.0 + dx, pointer.1 + dy);
        // No room below the pointer: above it.
        if at.1 + size.1 as i32 > screen.1 {
            at.1 = pointer.1 - dy / 2 - size.1 as i32;
        }
        let at = popup::place(at, size, screen);
        let Some(window) = self.make_window(Some(b"_NET_WM_WINDOW_TYPE_TOOLTIP"), at, size, EventMask::EXPOSURE) else { return };
        self.tips.shown = Some(Tip { window, size, rect: tip.rect, lines });
        self.paint_tip();
    }

    pub(super) fn hide_tip(&mut self) {
        if let Some(tip) = self.tips.shown.take() {
            self.drop_window(tip.window);
        }
    }

    pub(super) fn paint_tip(&mut self) {
        let Some(tip) = self.tips.shown.take() else { return };
        let scale = self.scale;
        self.blit(tip.window, tip.size, |scene| tip.draw(scene, tip.size.0 as f32 / scale, tip.size.1 as f32 / scale));
        self.tips.shown = Some(tip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tips_wrap_and_rectangles_hold_the_pointer() {
        let (lines, (w, h)) = tip_layout("Gain: 3.0 dB");
        assert_eq!(lines, ["Gain: 3.0 dB"]);
        assert!(w > 2.0 * PAD_X && h > 2.0 * PAD_Y);
        let (lines, (wide, tall)) = tip_layout(&"a long explanation of what this control does ".repeat(4));
        assert!(lines.len() > 1 && wide <= MAX_TEXT_WIDTH + 2.0 * PAD_X + 1.0 && tall > h);
        let r = DamageRect::new(10.0, 10.0, 20.0, 5.0);
        assert!(inside(&r, (10.0, 10.0)) && inside(&r, (29.9, 14.9)));
        assert!(!inside(&r, (30.0, 12.0)) && !inside(&r, (9.9, 12.0)));
        assert!(!inside(&DamageRect::new(0.0, 0.0, 0.0, 5.0), (0.0, 1.0)));
        assert!(!inside(&DamageRect::new(f32::NAN, 0.0, 5.0, 5.0), (1.0, 1.0)));
    }
}
