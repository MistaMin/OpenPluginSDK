//! The type-in field: a child window over a control, drawn by the view, that
//! takes the keyboard while it is open. The rules are `cocoa.rs`'s.

use super::clipboard::PasteTo;
use super::popup::{self, BAD, FOCUS, FONT, PICK_BOTTOM, PICK_TOP, TEXT_BRIGHT};
use super::{EditorView, Window, CONTROL, SHIFT};
use goodlookindri::scene::{Point, Rect, Scene};
use goodlookindri_shell::text;
use openplugin_core::editor::{DamageRect, TextEdit};
use x11rb::protocol::xproto::{self, ConnectionExt as _, EventMask, InputFocus};

/// The field is at least this wide and exactly this high, in points (as on macOS).
const MIN_WIDTH: f32 = 72.0;
const HEIGHT: f32 = 22.0;
/// Between the field's edge and its text, in points.
const PAD: f32 = 5.0;

/// The open field: its window and the text being edited.
pub(super) struct Field {
    pub(super) window: Window,
    /// Device pixels.
    size: (u32, u32),
    chars: Vec<char>,
    /// Where typing goes: an index between characters.
    caret: usize,
    /// The other end of the selection; equal to `caret` when nothing is selected.
    anchor: usize,
    /// The editor refused the text: it shows red until it is edited.
    invalid: bool,
    /// The view took the input focus for the field from this window.
    focus_from: Option<Window>,
}

impl Field {
    fn new(window: Window, size: (u32, u32), initial: &str) -> Field {
        let chars: Vec<char> = initial.chars().filter(|c| !c.is_control()).collect();
        // The text starts selected: typing replaces it.
        Field { window, size, caret: chars.len(), anchor: 0, chars, invalid: false, focus_from: None }
    }

    pub(super) fn text(&self) -> String {
        self.chars.iter().collect()
    }

    fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    fn selected(&self) -> String {
        let (from, to) = self.selection();
        self.chars[from..to].iter().collect()
    }

    fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.chars.len();
    }

    /// Take the selection out. True when there was one.
    fn delete_selection(&mut self) -> bool {
        let (from, to) = self.selection();
        self.chars.drain(from..to);
        self.caret = from;
        self.anchor = from;
        self.invalid &= from == to;
        from != to
    }

    /// Put text where the caret is, in place of the selection. One line: control characters are left out.
    pub(super) fn insert(&mut self, text: &str) {
        self.delete_selection();
        for c in text.chars().filter(|c| !c.is_control()) {
            self.chars.insert(self.caret, c);
            self.caret += 1;
        }
        self.anchor = self.caret;
        self.invalid = false;
    }

    fn backspace(&mut self) {
        if !self.delete_selection() && self.caret > 0 {
            self.caret -= 1;
            self.chars.remove(self.caret);
            self.anchor = self.caret;
            self.invalid = false;
        }
    }

    fn delete(&mut self) {
        if !self.delete_selection() && self.caret < self.chars.len() {
            self.chars.remove(self.caret);
            self.invalid = false;
        }
    }

    /// Move the caret; `extend` keeps the other end of the selection.
    fn move_to(&mut self, to: usize, extend: bool) {
        self.caret = to.min(self.chars.len());
        if !extend {
            self.anchor = self.caret;
        }
    }

    /// Left and Right: a selection collapses to its end on that side.
    fn step(&mut self, right: bool, extend: bool) {
        let (from, to) = self.selection();
        let target = if !extend && from != to {
            if right { to } else { from }
        } else if right {
            self.caret + 1
        } else {
            self.caret.saturating_sub(1)
        };
        self.move_to(target, extend);
    }

    /// Where the text starts and where each caret position is, in points from the field's left edge.
    fn layout(&self, width: f32) -> Vec<f32> {
        let mut prefix = String::new();
        let mut stops = vec![0.0];
        for c in &self.chars {
            prefix.push(*c);
            stops.push(text::width(&prefix, FONT));
        }
        let total = stops.last().copied().unwrap_or(0.0);
        let inner = (width - 2.0 * PAD).max(1.0);
        // Centred while it fits; then the caret stays in sight.
        let start = if total <= inner { (width - total) * 0.5 } else { PAD - (stops[self.caret.min(self.chars.len())] - inner).max(0.0) };
        stops.iter().map(|s| start + s).collect()
    }

    /// The caret position nearest to `x` points.
    fn index_at(&self, x: f32, width: f32) -> usize {
        let stops = self.layout(width);
        let mut best = 0;
        for (i, s) in stops.iter().enumerate() {
            if (s - x).abs() < (stops[best] - x).abs() {
                best = i;
            }
        }
        best
    }

    fn draw(&self, scene: &mut Scene, w: f32, h: f32) {
        popup::panel(scene, w, h, if self.invalid { BAD } else { FOCUS });
        let stops = self.layout(w);
        scene.clip_round(Rect::new(2.0, 2.0, w - 4.0, h - 4.0), 1.0);
        let (from, to) = self.selection();
        if from != to {
            scene.fill_rect(Rect::new(stops[from], 3.0, stops[to] - stops[from], h - 6.0), PICK_TOP, PICK_BOTTOM, true);
        }
        scene.text(Point::new(stops[0], h * 0.5), self.text(), if self.invalid { BAD } else { TEXT_BRIGHT }, FONT, false);
        if from == to {
            let x = stops[self.caret.min(self.chars.len())].round() + 0.5;
            scene.line(Point::new(x, 4.0), Point::new(x, h - 4.0), TEXT_BRIGHT, 1.0);
        }
        scene.pop_clip();
    }
}

/// The field's place in the window, in device pixels (x, y, width, height):
/// centred on the control, at least [`MIN_WIDTH`] wide, inside the window.
/// `None` for a rectangle that is not finite.
fn field_rect(rect: &DamageRect, editor: (u32, u32), win: (u32, u32), scale: f32) -> Option<(i32, i32, u32, u32)> {
    if !rect.is_finite() || editor.0 == 0 || editor.1 == 0 || win.0 == 0 || win.1 == 0 {
        return None;
    }
    let (sx, sy) = (win.0 as f32 / editor.0 as f32, win.1 as f32 / editor.1 as f32);
    let w = (rect.w * sx).max(MIN_WIDTH * scale).min(win.0 as f32).round().max(1.0);
    let h = (HEIGHT * scale).min(win.1 as f32).round().max(1.0);
    let x = (rect.x * sx + (rect.w * sx - w) * 0.5).round().clamp(0.0, win.0 as f32 - w);
    let y = (rect.y * sy + (rect.h * sy - h) * 0.5).round().clamp(0.0, win.1 as f32 - h);
    Some((x as i32, y as i32, w as u32, h as u32))
}

impl EditorView {
    /// The open type-in field's window (a child of [`EditorView::raw`]), if any.
    pub fn text_field(&self) -> Option<Window> {
        self.field.as_ref().map(|f| f.window)
    }

    /// The text in the open type-in field, if any.
    pub fn text_field_text(&self) -> Option<String> {
        self.field.as_ref().map(Field::text)
    }

    /// Open the field over the control, with its text selected.
    pub(super) fn open_field(&mut self, edit: TextEdit) {
        self.close_field(true);
        self.close_menu();
        self.hide_tip();
        let place = if self.shown() { field_rect(&edit.rect, self.size, self.win, self.scale) } else { None };
        let window = place.and_then(|(x, y, w, h)| Some((self.make_window(None, (x, y), (w, h), EventMask::EXPOSURE | EventMask::BUTTON_PRESS)?, (w, h))));
        let Some((window, size)) = window else {
            // No field can show: the edit ends at the editor too.
            self.guarded(|v| v.editor.text_edit_cancel());
            return;
        };
        let mut field = Field::new(window, size, &edit.text);
        // Keys follow the input focus: take it for the field's time.
        if !self.focused {
            field.focus_from = self.conn.get_input_focus().ok().and_then(|cookie| cookie.reply().ok()).map(|reply| reply.focus);
            x!(self, self.conn.set_input_focus(InputFocus::PARENT, self.window, self.last_time));
        }
        self.field = Some(field);
        self.select_keys();
        self.paint_field();
    }

    /// Take the field away. `cancel` tells the editor nothing was committed.
    pub(super) fn close_field(&mut self, cancel: bool) {
        let Some(field) = self.field.take() else { return };
        if cancel {
            self.guarded(|v| v.editor.text_edit_cancel());
        }
        if self.usable() {
            // The focus goes back where the field took it from, unless the editor keeps the keys.
            if let Some(from) = field.focus_from.filter(|_| self.focused && !self.editor.wants_keys()) {
                x!(self, self.conn.set_input_focus(InputFocus::PARENT, from, self.last_time));
                // Unfocused now: the FocusOut this sends must not close a menu the same right click opens.
                self.focused = false;
                self.guarded(|v| v.editor.focus_changed(false));
            }
            self.drop_window(field.window);
        }
        self.select_keys();
        self.follow(true);
        self.touched = true;
    }

    /// Hand the text to the editor. Accepted: the field closes. Refused: it
    /// stays, turns red and the bell rings.
    fn commit_field(&mut self) {
        let Some(text) = self.text_field_text() else { return };
        if self.guarded(|v| v.editor.text_edit_commit(&text)) {
            self.close_field(false);
        } else {
            if let Some(field) = self.field.as_mut() {
                field.invalid = true;
            }
            if self.usable() {
                x!(self, self.conn.bell(0));
            }
            self.paint_field();
        }
    }

    /// Focus left the field: valid text is committed, invalid text is dropped without a sound.
    pub(super) fn field_lost_focus(&mut self) {
        let Some(text) = self.text_field_text() else { return };
        let done = self.guarded(|v| v.editor.text_edit_commit(&text));
        // A refused commit leaves the edit open at the editor: cancel closes it.
        self.close_field(!done);
    }

    pub(super) fn paint_field(&mut self) {
        let Some(field) = self.field.take() else { return };
        let scale = self.scale;
        self.blit(field.window, field.size, |scene| field.draw(scene, field.size.0 as f32 / scale, field.size.1 as f32 / scale));
        self.field = Some(field);
    }

    /// A press inside the field: the caret goes there; twice selects everything.
    pub(super) fn field_click(&mut self, e: &xproto::ButtonPressEvent) {
        let scale = self.scale;
        let Some(field) = self.field.as_mut() else { return };
        if e.detail != 1 {
            return;
        }
        let again = self.click.is_some_and(|c| c.window == field.window && e.time.wrapping_sub(c.time) <= super::DOUBLE_CLICK_MS);
        self.click = Some(super::Click { time: e.time, x: e.event_x, y: e.event_y, count: 1, window: field.window });
        if again {
            field.select_all();
        } else {
            let at = field.index_at(f32::from(e.event_x) / scale, field.size.0 as f32 / scale);
            field.move_to(at, u16::from(e.state) & SHIFT != 0);
        }
        self.paint_field();
    }

    /// A key while the field is open: all of them are the field's.
    pub(super) fn field_key(&mut self, e: &xproto::KeyPressEvent) {
        let state = u16::from(e.state);
        let (sym, typed) = self.keymap.typed(e.detail, state);
        let (shift, ctrl) = (state & SHIFT != 0, state & CONTROL != 0);
        let other = state & (self.keymap.alt | self.keymap.command) != 0;
        match sym {
            0xff0d | 0xff8d => return self.commit_field(),
            0xff1b => return self.close_field(true),
            // Tab leaves the field, as it does on macOS.
            0xff09 | 0xfe20 | 0xff89 => return self.field_lost_focus(),
            _ => {}
        }
        if ctrl && !other {
            match typed.map(|c| c.to_ascii_lowercase()) {
                Some('c') | Some('x') => {
                    let text = self.field.as_ref().map(Field::selected).unwrap_or_default();
                    if !text.is_empty() && self.set_clipboard_text(&text) && typed.map(|c| c.to_ascii_lowercase()) == Some('x') {
                        if let Some(field) = self.field.as_mut() {
                            field.delete_selection();
                        }
                    }
                }
                Some('v') => self.paste(PasteTo::Field),
                Some('a') => {
                    if let Some(field) = self.field.as_mut() {
                        field.select_all();
                    }
                }
                _ => {}
            }
            return self.paint_field();
        }
        let Some(field) = self.field.as_mut() else { return };
        match sym {
            0xff08 => field.backspace(),
            0xffff | 0xff9f => field.delete(),
            0xff51 | 0xff96 => field.step(false, shift),
            0xff53 | 0xff98 => field.step(true, shift),
            0xff50 | 0xff95 | 0xff52 | 0xff97 => field.move_to(0, shift),
            0xff57 | 0xff9c | 0xff54 | 0xff99 => field.move_to(usize::MAX, shift),
            _ => match typed {
                Some(c) if !other => field.insert(c.encode_utf8(&mut [0; 4])),
                _ => {}
            },
        }
        self.paint_field();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_replaces_the_selection_and_keys_edit() {
        let mut f = Field::new(1, (100, 22), "1.5 dB");
        assert_eq!(f.selected(), "1.5 dB");
        f.insert("2");
        assert_eq!(f.text(), "2");
        f.insert(".75\n");
        assert_eq!(f.text(), "2.75");
        f.backspace();
        assert_eq!(f.text(), "2.7");
        f.move_to(0, false);
        f.delete();
        assert_eq!(f.text(), ".7");
        f.backspace();
        assert_eq!(f.text(), ".7");
        f.move_to(usize::MAX, false);
        f.delete();
        f.insert("x");
        assert_eq!((f.text().as_str(), f.caret), (".7x", 3));
        // Shift extends; a plain step collapses to the side it goes to.
        f.step(false, true);
        f.step(false, true);
        assert_eq!(f.selected(), "7x");
        f.step(false, false);
        assert_eq!((f.caret, f.anchor), (1, 1));
        f.select_all();
        f.step(true, false);
        assert_eq!((f.caret, f.anchor), (3, 3));
        f.select_all();
        f.invalid = true;
        f.backspace();
        assert!(f.text().is_empty() && !f.invalid);
    }

    #[test]
    fn the_caret_is_found_under_a_click() {
        let mut f = Field::new(1, (200, 22), "0000");
        f.move_to(0, false);
        let stops = f.layout(200.0);
        assert_eq!(stops.len(), 5);
        // Centred text.
        assert!((stops[0] + stops[4] - 200.0).abs() < 0.01);
        assert_eq!(f.index_at(0.0, 200.0), 0);
        assert_eq!(f.index_at(199.0, 200.0), 4);
        assert_eq!(f.index_at(stops[2] + 0.4, 200.0), 2);
        // Text wider than the field: the caret stays inside.
        let mut long = Field::new(1, (72, 22), &"0".repeat(60));
        long.move_to(usize::MAX, false);
        let stops = long.layout(72.0);
        assert!(stops[60] <= 72.0 - PAD + 0.01 && stops[0] < 0.0);
    }

    #[test]
    fn the_field_sits_on_the_control_inside_the_window() {
        let r = DamageRect::new(100.0, 50.0, 40.0, 40.0);
        // 72 wide, 22 high, centred on the control.
        assert_eq!(field_rect(&r, (300, 200), (300, 200), 1.0), Some((84, 59, 72, 22)));
        assert_eq!(field_rect(&r, (300, 200), (600, 400), 2.0), Some((168, 118, 144, 44)));
        // At the edge it moves in; a wide control makes a wide field.
        assert_eq!(field_rect(&DamageRect::new(280.0, 190.0, 20.0, 10.0), (300, 200), (300, 200), 1.0), Some((228, 178, 72, 22)));
        assert_eq!(field_rect(&DamageRect::new(0.0, 0.0, 200.0, 30.0), (300, 200), (300, 200), 1.0), Some((0, 4, 200, 22)));
        assert_eq!(field_rect(&DamageRect::new(f32::NAN, 0.0, 1.0, 1.0), (300, 200), (300, 200), 1.0), None);
    }
}
