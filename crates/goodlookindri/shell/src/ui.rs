//! Controls for tools built on the shell: push buttons, lamps, a level bar, a scrolling list, a
//! wrapped text block and a seven-segment readout.
//!
//! A [`Ui`] owns the widgets, routes [`Event`]s to them, keeps focus, hover and press state, and
//! reports what the user did as an [`Action`]. Each widget paints with GoodLookin DRI parts, so it
//! matches the knobs and plates. Coordinates are logical points, the same as the `Scene`.

use crate::core::{Event, Key};
use crate::text;
use goodlookindri::color::Color;
use goodlookindri::meters::{draw_lamp, draw_seven_segment};
use goodlookindri::scene::{Point, Rect, Scene};
use goodlookindri::switches::{draw_push, PushStyle};

/// Type size for every label in the shell's widgets.
pub const FONT: f32 = 12.0;
const PAD: f32 = 8.0;
const ROW: f32 = 18.0;

#[derive(Clone, Debug)]
pub enum Kind {
    /// Focusable. Pressed on release inside, or activated with Enter or Space.
    Button { label: String, colour: Color, style: PushStyle },
    Lamp { colour: Color, lit: bool },
    /// Horizontal level, 0 to 1.
    Bar { value: f32, colour: Color },
    /// Focusable. The wheel scrolls it; Up and Down move the selection.
    List { rows: Vec<String>, selected: usize, top: usize },
    /// Word-wrapped, `\n` starts a new line. Clipped at the bottom edge.
    Text { body: String, colour: Color },
    Readout { text: String, colour: Color },
}

#[derive(Clone, Debug)]
pub struct Widget {
    pub id: u32,
    pub bounds: Rect,
    pub kind: Kind,
}

impl Widget {
    pub fn new(id: u32, bounds: Rect, kind: Kind) -> Self {
        Self { id, bounds, kind }
    }

    /// Buttons and lists take the mouse and the keyboard. The others only paint.
    fn interactive(&self) -> bool {
        matches!(self.kind, Kind::Button { .. } | Kind::List { .. })
    }
}

/// What the user did, identified by the widget `id` the app gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// A button was pressed and released on it, or activated from the keyboard.
    Pressed(u32),
    /// A list's selection moved to `index`.
    Selected { id: u32, index: usize },
}

/// The result of one event: whether to repaint, and any action for the app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Response {
    pub redraw: bool,
    pub action: Option<Action>,
}

pub struct Ui {
    widgets: Vec<Widget>,
    focus: Option<usize>,
    hover: Option<usize>,
    pressed: Option<usize>,
}

fn visible(r: Rect) -> usize {
    ((r.h - PAD * 2.0) / ROW).floor().max(0.0) as usize
}

impl Ui {
    pub fn new(widgets: Vec<Widget>) -> Self {
        Self { widgets, focus: None, hover: None, pressed: None }
    }

    pub fn widgets(&self) -> &[Widget] {
        &self.widgets
    }

    /// The content of a widget, for the app to change (a readout, a list's rows, a lamp).
    pub fn kind_mut(&mut self, id: u32) -> Option<&mut Kind> {
        self.widgets.iter_mut().find(|w| w.id == id).map(|w| &mut w.kind)
    }

    /// The id of the focused widget.
    pub fn focused(&self) -> Option<u32> {
        self.focus.map(|i| self.widgets[i].id)
    }

    /// Moves focus to `id`. Returns false when no focusable widget has that id.
    pub fn set_focus(&mut self, id: u32) -> bool {
        match self.widgets.iter().position(|w| w.id == id && w.interactive()) {
            Some(i) => {
                self.focus = Some(i);
                true
            }
            None => false,
        }
    }

    /// The topmost interactive widget under the point.
    fn hit(&self, x: f32, y: f32) -> Option<usize> {
        let p = Point::new(x, y);
        (0..self.widgets.len()).rev().find(|&i| self.widgets[i].interactive() && self.widgets[i].bounds.contains(p))
    }

    /// The list row under `y` inside widget `i`, counting the rows the list has scrolled past.
    fn row_at(&self, i: usize, y: f32) -> Option<usize> {
        let w = &self.widgets[i];
        let Kind::List { rows, top, .. } = &w.kind else { return None };
        let k = ((y - w.bounds.y - PAD) / ROW).floor();
        if k < 0.0 || k as usize >= visible(w.bounds) {
            return None;
        }
        let idx = top + k as usize;
        (idx < rows.len()).then_some(idx)
    }

    /// Selects row `index` of list `i`, scrolling it into view. None if nothing changed.
    fn select(&mut self, i: usize, index: usize) -> Option<Action> {
        let id = self.widgets[i].id;
        let n = visible(self.widgets[i].bounds);
        let Kind::List { rows, selected, top } = &mut self.widgets[i].kind else { return None };
        if index >= rows.len() || index == *selected {
            return None;
        }
        *selected = index;
        if index < *top {
            *top = index;
        } else if n > 0 && index >= *top + n {
            *top = index + 1 - n;
        }
        Some(Action::Selected { id, index })
    }

    fn selected_index(&self, i: usize) -> usize {
        match &self.widgets[i].kind {
            Kind::List { selected, .. } => *selected,
            _ => 0,
        }
    }

    fn list_len(&self, i: usize) -> usize {
        match &self.widgets[i].kind {
            Kind::List { rows, .. } => rows.len(),
            _ => 0,
        }
    }

    fn cycle_focus(&mut self, forward: bool) {
        let order: Vec<usize> = (0..self.widgets.len()).filter(|&i| self.widgets[i].interactive()).collect();
        if order.is_empty() {
            self.focus = None;
            return;
        }
        let pos = self.focus.and_then(|f| order.iter().position(|&i| i == f));
        let n = order.len();
        let next = match (pos, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(p), true) => (p + 1) % n,
            (Some(p), false) => (p + n - 1) % n,
        };
        self.focus = Some(order[next]);
    }

    pub fn event(&mut self, e: Event) -> Response {
        match e {
            Event::MouseMove { x, y } => {
                let hover = self.hit(x, y);
                let redraw = hover != self.hover;
                self.hover = hover;
                Response { redraw, action: None }
            }
            Event::MouseDown { x, y } => {
                let Some(i) = self.hit(x, y) else { return Response::default() };
                self.pressed = Some(i);
                self.focus = Some(i);
                let action = self.row_at(i, y).and_then(|row| self.select(i, row));
                Response { redraw: true, action }
            }
            Event::MouseUp { x, y } => {
                let released_on = self.hit(x, y);
                let pressed = self.pressed.take();
                let action = match (pressed, released_on) {
                    (Some(p), Some(r)) if p == r && matches!(self.widgets[p].kind, Kind::Button { .. }) => {
                        Some(Action::Pressed(self.widgets[p].id))
                    }
                    _ => None,
                };
                Response { redraw: pressed.is_some(), action }
            }
            Event::MouseWheel { x, y, dy } => {
                let Some(i) = self.hit(x, y) else { return Response::default() };
                if dy == 0.0 {
                    return Response::default();
                }
                let n = visible(self.widgets[i].bounds);
                let Kind::List { rows, top, .. } = &mut self.widgets[i].kind else { return Response::default() };
                // Small trackpad deltas still move by one row.
                let step = if dy.abs() < 0.5 { dy.signum() as i64 } else { dy.round() as i64 };
                let max = rows.len().saturating_sub(n) as i64;
                let next = (*top as i64 + step).clamp(0, max) as usize;
                let redraw = next != *top;
                *top = next;
                Response { redraw, action: None }
            }
            Event::Key { key, shift } => self.key(key, shift),
            Event::Tick { .. } => Response::default(),
        }
    }

    fn key(&mut self, key: Key, shift: bool) -> Response {
        match key {
            Key::Tab => {
                self.cycle_focus(!shift);
                Response { redraw: true, action: None }
            }
            Key::Escape => {
                let redraw = self.focus.take().is_some();
                Response { redraw, action: None }
            }
            Key::Enter | Key::Space => {
                let action = match self.focus {
                    Some(i) if matches!(self.widgets[i].kind, Kind::Button { .. }) => Some(Action::Pressed(self.widgets[i].id)),
                    _ => None,
                };
                Response { redraw: action.is_some(), action }
            }
            Key::Up | Key::Down => {
                let Some(i) = self.focus else { return Response::default() };
                if !matches!(self.widgets[i].kind, Kind::List { .. }) {
                    return Response::default();
                }
                let len = self.list_len(i);
                if len == 0 {
                    return Response::default();
                }
                let cur = self.selected_index(i);
                let next = if key == Key::Up { cur.saturating_sub(1) } else { (cur + 1).min(len - 1) };
                let action = self.select(i, next);
                Response { redraw: action.is_some(), action }
            }
        }
    }

    /// Paints every widget. The focused one gets a ring.
    pub fn paint(&self, scene: &mut Scene) {
        for (i, w) in self.widgets.iter().enumerate() {
            paint_widget(scene, w, self.focus == Some(i), self.hover == Some(i), self.pressed == Some(i));
        }
    }
}

fn paint_widget(scene: &mut Scene, w: &Widget, focused: bool, hovered: bool, pressed: bool) {
    let r = w.bounds;
    match &w.kind {
        Kind::Button { label, colour, style } => {
            draw_push(scene, r, label, *style, *colour, hovered || focused, pressed, false);
        }
        Kind::Lamp { colour, lit } => {
            draw_lamp(scene, r.center(), r.w.min(r.h) * 0.5, *colour, *lit);
        }
        Kind::Bar { value, colour } => {
            scene.fill_round(r, 2.0, Color(0xff15171a), Color(0xff1e2226), true);
            let v = value.clamp(0.0, 1.0);
            if v > 0.0 {
                let fill = Rect::new(r.x + 1.0, r.y + 1.0, (r.w - 2.0) * v, r.h - 2.0);
                scene.fill_round(fill, 1.0, colour.brighter(0.25), colour.darker(0.25), true);
            }
            scene.stroke_round(r, 2.0, Color(0xff3a3f45), 1.0);
        }
        Kind::List { rows, selected, top } => {
            scene.fill_round(r, 3.0, Color(0xff15171a), Color(0xff1e2226), true);
            scene.clip_round(r, 3.0);
            let n = visible(r);
            let inner_w = r.w - PAD * 2.0;
            for (k, row) in rows.iter().skip(*top).take(n).enumerate() {
                let idx = top + k;
                let y = r.y + PAD + k as f32 * ROW;
                let is_selected = idx == *selected;
                if is_selected {
                    scene.fill_round(Rect::new(r.x + 3.0, y, r.w - 6.0, ROW), 2.0, Color(0xff3d5a74), Color(0xff2f4a60), true);
                }
                let colour = if is_selected { Color(0xfff4f1e8) } else { Color(0xffb9bfc6) };
                scene.text(Point::new(r.x + PAD, y + ROW * 0.5), text::ellipsize(row, FONT, inner_w), colour, FONT, false);
            }
            if rows.len() > n && n > 0 {
                let track = Rect::new(r.right() - 5.0, r.y + 4.0, 2.0, r.h - 8.0);
                let thumb_h = (track.h * n as f32 / rows.len() as f32).max(8.0);
                let travel = (rows.len() - n) as f32;
                let y = track.y + (track.h - thumb_h) * (*top as f32 / travel);
                scene.fill_round(Rect::new(track.x, y, 2.0, thumb_h), 1.0, Color(0xff8a929c), Color(0xff8a929c), true);
            }
            scene.pop_clip();
            scene.stroke_round(r, 3.0, Color(0xff3a3f45), 1.0);
        }
        Kind::Text { body, colour } => {
            let lh = text::line_height(FONT);
            for (k, line) in text::wrap(body, FONT, r.w - PAD * 2.0).iter().enumerate() {
                let mid = r.y + PAD + lh * 0.5 + k as f32 * lh;
                if mid + lh * 0.5 > r.bottom() {
                    break;
                }
                scene.text(Point::new(r.x + PAD, mid), line.as_str(), *colour, FONT, false);
            }
        }
        Kind::Readout { text, colour } => {
            draw_seven_segment(scene, r, text, *colour, true);
        }
    }
    if focused && w.interactive() {
        scene.stroke_round(r.expanded(3.0), 5.0, Color(0xffffd36a), 1.5);
    }
}
