//! Controls with hit areas: a registry an [`openplugin_core::Editor`] uses
//! instead of hand-coded hit rectangles and drag code.
//!
//! A [`ControlSet`] holds [`Control`]s: a rotary knob, a vertical or horizontal
//! fader, a toggle, a momentary button and a key bank (a choice parameter laid
//! out as keys). Each has an [`Area`] in points, a parameter index with its
//! [`ParamInfo`], and the current plain value. The editor forwards its mouse
//! and wheel calls to the set; the set turns them into gestures through the
//! [`EditorHost`] and keeps hover and pressed state for drawing.
//!
//! Rules of the set:
//!
//! * A value sent to the host is always `ParamInfo::clamp`ed: in range, and
//!   snapped for stepped, toggle and choice parameters. NaN and infinite
//!   input is dropped.
//! * Drags are relative. The value moves by the pointer travel, so a grab never
//!   jumps. The travel is tracked clamped to the range, so reversing at an
//!   end responds at once.
//! * A knob moves its full range per [`DEFAULT_DRAG_RANGE`] points of vertical
//!   travel. 200 points is about one hand movement on a trackpad and about the
//!   height of a small plugin window, and it is fine enough that one point is
//!   a quarter of a percent of the range. A fader moves 1:1 with its own
//!   length, so the cap follows the pointer. While a fine modifier is held
//!   ([`Modifiers::fine`]) travel counts a tenth as much. Changing the
//!   modifier mid-drag does not jump the value.
//! * A double-click on a knob or fader sets the default in one gesture.
//! * The wheel steps a continuous control by [`WHEEL_STEP`] of its range per
//!   unit (a tenth of that when fine) and a stepped control by one position
//!   per whole unit of accumulated scroll.
//! * [`ControlSet::param_changed`] stores the host's value and never sends a
//!   gesture. It is ignored for a parameter while the user holds a control on
//!   it, so a late echo cannot make the control flicker; the host's value
//!   arrives again after the release.
//! * Read-only parameters never react to input.
//! * Keyboard focus is on one control at most. A click focuses the control it
//!   hits (a click on bare plate clears focus). Tab and Shift-Tab walk the
//!   interactive controls in the order they were added; a Tab past either end
//!   clears focus and is left unhandled so the host can move on. Arrows and
//!   Page Up or Down step the focused control through the same gestures as the
//!   wheel (a whole begin, perform, end per step; Shift or Command makes the
//!   step a tenth). Home and End go to the minimum and maximum, Return or
//!   Space flip a toggle or press a button (held while the key is down), and
//!   Delete or Backspace reset to the default. Escape clears focus.
//!   [`ControlSet::key_down`] returns false for every key it did not use,
//!   which is every key while nothing is focused, so the view can pass those
//!   to the host (transport, shortcuts).
//! * What a control looks like is its value, hover, pressed and focus state.
//!   [`ControlSet::take_changed`] lists the controls whose look changed since
//!   the last call, so an editor repaints only those ([`openplugin_core::Editor::damage`]).

use openplugin_core::{EditorHost, ParamFlags, ParamInfo, Unit};

pub use openplugin_core::editor::{Key, Modifiers};

/// Points of vertical drag for the full range of a knob.
pub const DEFAULT_DRAG_RANGE: f32 = 200.0;
/// Fraction of the range a continuous control moves per arrow key.
pub const KEY_STEP: f32 = 0.01;
/// Fraction of the range a continuous control moves per Page Up or Down.
pub const PAGE_STEP: f32 = 0.1;
/// Fraction of the range a continuous control moves per unit of wheel.
pub const WHEEL_STEP: f32 = 0.02;
/// Drag and wheel factor while the fine modifier is held.
pub const FINE_FACTOR: f32 = 0.1;

/// Where a control answers the mouse, in points, origin top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Area {
    Rect { x: f32, y: f32, w: f32, h: f32 },
    Circle { cx: f32, cy: f32, r: f32 },
}

impl Area {
    pub const fn rect(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self::Rect { x, y, w, h }
    }

    /// From an `(x, y, w, h)` tuple.
    pub const fn from_tuple(r: (f32, f32, f32, f32)) -> Self {
        Self::Rect { x: r.0, y: r.1, w: r.2, h: r.3 }
    }

    pub const fn circle(cx: f32, cy: f32, r: f32) -> Self {
        Self::Circle { cx, cy, r }
    }

    /// Rectangles are half-open (the right and bottom edges are outside);
    /// circles include the rim. NaN is outside.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        match *self {
            Self::Rect { x, y, w, h } => px >= x && px < x + w && py >= y && py < y + h,
            Self::Circle { cx, cy, r } => {
                let (dx, dy) = (px - cx, py - cy);
                dx * dx + dy * dy <= r * r
            }
        }
    }

    /// The bounding box `(x, y, w, h)`.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        match *self {
            Self::Rect { x, y, w, h } => (x, y, w, h),
            Self::Circle { cx, cy, r } => (cx - r, cy - r, 2.0 * r, 2.0 * r),
        }
    }

    pub fn center(&self) -> (f32, f32) {
        let (x, y, w, h) = self.bounds();
        (x + w * 0.5, y + h * 0.5)
    }

    /// Half the shorter side of the bounding box, or the radius.
    pub fn radius(&self) -> f32 {
        let (_, _, w, h) = self.bounds();
        w.min(h) * 0.5
    }
}

/// What a control is and how it answers input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    /// Rotary knob, driven by vertical drag.
    Knob,
    /// Vertical fader: up raises the value.
    FaderV,
    /// Horizontal fader: right raises the value.
    FaderH,
    /// Flips on every click.
    Toggle,
    /// Holds the maximum while pressed, the minimum after.
    Button,
    /// A choice parameter as equal keys along one axis. Click or drag across
    /// keys to select.
    KeyBank { vertical: bool },
}

/// Index of a control in its [`ControlSet`], in the order they were added.
pub type ControlId = usize;

/// One control. Built with the constructors, added with [`ControlSet::add`].
#[derive(Clone, Debug)]
pub struct Control {
    pub area: Area,
    pub kind: ControlKind,
    /// Parameter descriptor index, as sent in gestures.
    pub param: usize,
    pub info: &'static ParamInfo,
    value: f32,
    hover: bool,
    pressed: bool,
    focused: bool,
    drag_range: f32,
    /// The look last reported by [`ControlSet::take_changed`].
    seen: Option<Look>,
}

/// Everything about a control that changes how it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Look {
    value: u32,
    hover: bool,
    pressed: bool,
    focused: bool,
}

impl Control {
    fn new(kind: ControlKind, area: Area, param: usize, info: &'static ParamInfo) -> Self {
        Self { area, kind, param, info, value: info.clamp(info.default), hover: false, pressed: false, focused: false, drag_range: 0.0, seen: None }
    }

    pub fn knob(area: Area, param: usize, info: &'static ParamInfo) -> Self {
        Self::new(ControlKind::Knob, area, param, info)
    }
    pub fn fader_v(area: Area, param: usize, info: &'static ParamInfo) -> Self {
        Self::new(ControlKind::FaderV, area, param, info)
    }
    pub fn fader_h(area: Area, param: usize, info: &'static ParamInfo) -> Self {
        Self::new(ControlKind::FaderH, area, param, info)
    }
    pub fn toggle(area: Area, param: usize, info: &'static ParamInfo) -> Self {
        Self::new(ControlKind::Toggle, area, param, info)
    }
    pub fn button(area: Area, param: usize, info: &'static ParamInfo) -> Self {
        Self::new(ControlKind::Button, area, param, info)
    }
    pub fn key_bank(area: Area, param: usize, info: &'static ParamInfo, vertical: bool) -> Self {
        Self::new(ControlKind::KeyBank { vertical }, area, param, info)
    }

    /// Points of drag for the full range, replacing the default (knobs and
    /// faders). Ignored unless positive and finite.
    pub fn drag_range(mut self, points: f32) -> Self {
        if points.is_finite() && points > 0.0 {
            self.drag_range = points;
        }
        self
    }

    /// Plain value.
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Value in 0..1 along the parameter's taper, for drawing.
    pub fn normalized(&self) -> f32 {
        self.info.to_normalized(self.value)
    }

    /// The pointer is over the control (or holds it).
    pub fn hover(&self) -> bool {
        self.hover
    }

    /// The control is held by the mouse.
    pub fn pressed(&self) -> bool {
        self.pressed
    }

    /// The control has keyboard focus (draw a focus ring).
    pub fn focused(&self) -> bool {
        self.focused
    }

    /// Toggle on, button down, or a value above the middle of the range.
    pub fn is_on(&self) -> bool {
        self.value > 0.5 * (self.info.min + self.info.max)
    }

    /// Number of keys in a key bank (1 for other kinds).
    pub fn key_count(&self) -> usize {
        match self.kind {
            ControlKind::KeyBank { .. } => ((self.info.max - self.info.min).round().max(0.0) as usize) + 1,
            _ => 1,
        }
    }

    /// The selected key in a key bank.
    pub fn selected_key(&self) -> usize {
        ((self.value - self.info.min).round().max(0.0) as usize).min(self.key_count() - 1)
    }

    fn interactive(&self) -> bool {
        !self.info.flags.contains(ParamFlags::READ_ONLY)
    }

    fn is_stepped(&self) -> bool {
        matches!(self.info.unit, Unit::Toggle | Unit::Choice) || self.info.steps > 1
    }

    /// Plain size of one wheel step for a stepped parameter.
    fn plain_step(&self) -> f32 {
        if matches!(self.info.unit, Unit::Toggle | Unit::Choice) || self.info.steps <= 1 {
            1.0
        } else {
            (self.info.max - self.info.min) / (self.info.steps - 1) as f32
        }
    }

    fn key_at(&self, x: f32, y: f32) -> f32 {
        let (bx, by, bw, bh) = self.area.bounds();
        let vertical = matches!(self.kind, ControlKind::KeyBank { vertical: true });
        let (pos, len) = if vertical { (y - by, bh) } else { (x - bx, bw) };
        let n = self.key_count();
        let i = if len > 0.0 { ((pos / len * n as f32).floor().max(0.0) as usize).min(n - 1) } else { 0 };
        self.info.clamp(self.info.min + i as f32)
    }
}

/// State of the one control the mouse holds.
struct Grab {
    id: ControlId,
    x: f32,
    y: f32,
    /// Normalized position, tracked by travel.
    norm: f32,
    /// A `begin` went to the host and an `end` is owed.
    open: bool,
}

/// The registry. Create it with the editor's [`EditorHost`], add the controls,
/// forward input, draw by iterating.
pub struct ControlSet {
    host: EditorHost,
    controls: Vec<Control>,
    grab: Option<Grab>,
    hovered: Option<ControlId>,
    focus: Option<ControlId>,
    /// Return or Space held down on a toggle or button, until released.
    hold: Option<(Key, ControlId)>,
    /// Keys whose down was handled, so their up is too.
    handled_down: Vec<Key>,
    modifiers: Modifiers,
    wheel_acc: f32,
    wheel_id: Option<ControlId>,
}

impl ControlSet {
    pub fn new(host: EditorHost) -> Self {
        Self {
            host,
            controls: Vec::new(),
            grab: None,
            hovered: None,
            focus: None,
            hold: None,
            handled_down: Vec::new(),
            modifiers: Modifiers::NONE,
            wheel_acc: 0.0,
            wheel_id: None,
        }
    }

    /// Add a control. A later one is on top where areas overlap.
    pub fn add(&mut self, control: Control) -> ControlId {
        self.controls.push(control);
        self.controls.len() - 1
    }

    /// Builder form of [`Self::add`].
    pub fn with(mut self, control: Control) -> Self {
        self.add(control);
        self
    }

    pub fn len(&self) -> usize {
        self.controls.len()
    }

    pub fn is_empty(&self) -> bool {
        self.controls.is_empty()
    }

    pub fn get(&self, id: ControlId) -> Option<&Control> {
        self.controls.get(id)
    }

    /// The controls in the order they were added (back to front), to draw.
    pub fn iter(&self) -> std::slice::Iter<'_, Control> {
        self.controls.iter()
    }

    /// The controls whose look (value, hover, pressed, focus) changed since the
    /// last call, in order. The first call reports every control, and so does
    /// the first call after a control is added. An editor turns these into the
    /// rectangles of [`openplugin_core::Editor::damage`].
    pub fn take_changed(&mut self) -> Vec<ControlId> {
        let mut changed = Vec::new();
        for (id, c) in self.controls.iter_mut().enumerate() {
            let now = Look { value: c.value.to_bits(), hover: c.hover, pressed: c.pressed, focused: c.focused };
            if c.seen != Some(now) {
                c.seen = Some(now);
                changed.push(id);
            }
        }
        changed
    }

    /// The first control on a parameter.
    pub fn for_param(&self, param: usize) -> Option<&Control> {
        self.controls.iter().find(|c| c.param == param)
    }

    /// The control under the pointer (or held), for tooltips.
    pub fn hovered(&self) -> Option<ControlId> {
        self.hovered
    }

    /// The held control.
    pub fn pressed(&self) -> Option<ControlId> {
        self.grab.as_ref().map(|g| g.id)
    }

    /// Record the modifier keys. [`openplugin_core::Editor::modifiers_changed`]
    /// forwards here.
    pub fn set_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
    }

    /// A value from the host. Stores it clamped; sends nothing. NaN and
    /// infinite values are ignored, and so is any value for a parameter whose
    /// control the user is holding.
    pub fn param_changed(&mut self, param: usize, plain: f32) {
        if !plain.is_finite() {
            return;
        }
        let held = self.grab.as_ref().map(|g| g.id).or(self.hold.map(|h| h.1));
        if held.map(|id| self.controls[id].param) == Some(param) {
            return;
        }
        for c in self.controls.iter_mut().filter(|c| c.param == param) {
            c.value = c.info.clamp(plain);
        }
    }

    fn hit(&self, x: f32, y: f32) -> Option<ControlId> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        self.controls.iter().rposition(|c| c.interactive() && c.area.contains(x, y))
    }

    pub fn mouse_down(&mut self, x: f32, y: f32, clicks: u32) {
        if self.grab.is_some() {
            // A release was lost; close the gesture it owed.
            self.release();
        }
        self.release_keys();
        let hit = self.hit(x, y);
        self.set_focus(hit);
        let Some(id) = hit else { return };
        let (param, info, kind) = {
            let c = &self.controls[id];
            (c.param, c.info, c.kind)
        };
        self.wheel_acc = 0.0;
        let mut grab = Grab { id, x, y, norm: self.controls[id].normalized(), open: false };
        match kind {
            ControlKind::Knob | ControlKind::FaderV | ControlKind::FaderH => {
                if clicks >= 2 {
                    let d = info.clamp(info.default);
                    self.controls[id].value = d;
                    self.host.set(param, d);
                    self.set_hover(Some(id));
                    return;
                }
                self.host.begin(param);
                grab.open = true;
            }
            ControlKind::Toggle => {
                let v = info.clamp(if self.controls[id].is_on() { info.min } else { info.max });
                self.controls[id].value = v;
                self.host.set(param, v);
            }
            ControlKind::Button => {
                let v = info.clamp(info.max);
                self.controls[id].value = v;
                self.host.begin(param);
                self.host.perform(param, v);
                grab.open = true;
            }
            ControlKind::KeyBank { .. } => {
                let v = self.controls[id].key_at(x, y);
                self.controls[id].value = v;
                self.host.begin(param);
                self.host.perform(param, v);
                grab.open = true;
            }
        }
        self.controls[id].pressed = true;
        self.grab = Some(grab);
        self.set_hover(Some(id));
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        let fine = if self.modifiers.fine() { FINE_FACTOR } else { 1.0 };
        let Some(g) = self.grab.as_mut() else {
            let h = self.hit(x, y);
            self.set_hover(h);
            return;
        };
        let c = &mut self.controls[g.id];
        let new = match c.kind {
            ControlKind::Knob | ControlKind::FaderV | ControlKind::FaderH => {
                let (_, _, w, h) = c.area.bounds();
                let (travel, own) = match c.kind {
                    ControlKind::FaderH => (x - g.x, w),
                    ControlKind::FaderV => (g.y - y, h),
                    _ => (g.y - y, DEFAULT_DRAG_RANGE),
                };
                let range = if c.drag_range > 0.0 { c.drag_range } else { own };
                g.x = x;
                g.y = y;
                if range <= 0.0 {
                    return;
                }
                g.norm = (g.norm + travel / range * fine).clamp(0.0, 1.0);
                c.info.from_normalized(g.norm)
            }
            ControlKind::KeyBank { .. } => c.key_at(x, y),
            ControlKind::Toggle | ControlKind::Button => return,
        };
        if new != c.value {
            c.value = new;
            self.host.perform(c.param, new);
        }
    }

    pub fn mouse_up(&mut self, x: f32, y: f32) {
        if self.grab.is_some() {
            self.mouse_move(x, y);
        }
        self.release();
        let h = self.hit(x, y);
        self.set_hover(h);
    }

    /// The pointer left the view: clear hover.
    pub fn mouse_exit(&mut self) {
        if self.grab.is_none() {
            self.set_hover(None);
        }
    }

    /// Wheel at `(x, y)`, positive `dy` raising the value. Ignored while a
    /// control is held, over read-only controls, and for toggles and buttons.
    pub fn wheel(&mut self, x: f32, y: f32, dy: f32) {
        if self.grab.is_some() || !dy.is_finite() || dy == 0.0 {
            return;
        }
        let Some(id) = self.hit(x, y) else { return };
        let c = &mut self.controls[id];
        if matches!(c.kind, ControlKind::Toggle | ControlKind::Button) {
            return;
        }
        if self.wheel_id != Some(id) || self.wheel_acc * dy < 0.0 {
            self.wheel_acc = 0.0;
        }
        self.wheel_id = Some(id);
        let fine = if self.modifiers.fine() { FINE_FACTOR } else { 1.0 };
        let new = if c.is_stepped() {
            self.wheel_acc += dy;
            let whole = self.wheel_acc.trunc();
            if whole == 0.0 {
                return;
            }
            self.wheel_acc -= whole;
            c.info.clamp(c.value + whole * c.plain_step())
        } else {
            let norm = c.info.to_normalized(c.value) + dy * WHEEL_STEP * fine;
            c.info.from_normalized(norm)
        };
        if new != c.value {
            c.value = new;
            self.host.set(c.param, new);
        }
    }

    /// The control with keyboard focus.
    pub fn focused(&self) -> Option<ControlId> {
        self.focus
    }

    /// Focus a control, or clear focus with `None`. A control that does not
    /// exist or is read-only cannot take focus (focus is cleared). Keys held
    /// on the control that loses focus are let go.
    pub fn set_focus(&mut self, id: Option<ControlId>) {
        let id = id.filter(|&i| self.controls.get(i).is_some_and(Control::interactive));
        if self.focus != id {
            self.release_keys();
            if let Some(old) = self.focus {
                self.controls[old].focused = false;
            }
            if let Some(new) = id {
                self.controls[new].focused = true;
            }
            self.focus = id;
        }
    }

    /// The set has a control that takes keys: the view should take keyboard
    /// focus. An editor forwards [`openplugin_core::Editor::wants_keys`] here.
    pub fn wants_keys(&self) -> bool {
        self.controls.iter().any(Control::interactive)
    }

    /// Let go of keys held: a pressed button is released and the keys whose
    /// down was handled are forgotten. Call when the view loses keyboard
    /// focus ([`openplugin_core::Editor::focus_changed`]); focus stays.
    pub fn release_keys(&mut self) {
        self.handled_down.clear();
        self.release_hold();
    }

    fn release_hold(&mut self) {
        if let Some((_, id)) = self.hold.take() {
            let c = &mut self.controls[id];
            c.pressed = false;
            if c.kind == ControlKind::Button {
                let off = c.info.clamp(c.info.min);
                c.value = off;
                self.host.perform(c.param, off);
                self.host.end(c.param);
            }
        }
    }

    /// Move focus to the next (or, `back`, the previous) interactive control.
    /// False, with focus cleared, when there is none past the end.
    fn move_focus(&mut self, back: bool) -> bool {
        let order: Vec<ControlId> = (0..self.controls.len()).filter(|&i| self.controls[i].interactive()).collect();
        let at = self.focus.and_then(|f| order.iter().position(|&i| i == f));
        let next = match (at, back) {
            (None, false) => order.first().copied(),
            (None, true) => order.last().copied(),
            (Some(p), false) => order.get(p + 1).copied(),
            (Some(p), true) => p.checked_sub(1).map(|q| order[q]),
        };
        self.set_focus(next);
        next.is_some()
    }

    /// A key went down. True when the set used it; false for every other key,
    /// so the view hands it to the host. Nothing is handled while the mouse
    /// holds a control. See the module notes for the keys.
    pub fn key_down(&mut self, key: Key, modifiers: Modifiers) -> bool {
        if self.grab.is_some() {
            return false;
        }
        // Command, Control and Alt keep the host's shortcuts, except on the
        // arrows and Page keys, where Command asks for a fine step.
        let plain = !(modifiers.command || modifiers.ctrl || modifiers.alt);
        let handled = match key {
            Key::Tab => plain && self.move_focus(modifiers.shift),
            Key::Escape => {
                let had = plain && self.focus.is_some();
                if had {
                    self.set_focus(None);
                }
                had
            }
            Key::Left | Key::Right | Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                self.focus.is_some_and(|id| self.step_key(id, key, modifiers.fine()))
            }
            Key::Home | Key::End => plain && self.focus.is_some_and(|id| self.jump_key(id, key == Key::End)),
            Key::Return | Key::Space => plain && self.focus.is_some_and(|id| self.press_key(id, key)),
            Key::Delete | Key::Backspace => plain && self.focus.is_some_and(|id| self.reset_key(id)),
            Key::Char(_) => false,
        };
        if handled && !self.handled_down.contains(&key) {
            self.handled_down.push(key);
        }
        handled
    }

    /// A key went up. True when its down was handled here.
    pub fn key_up(&mut self, key: Key, _modifiers: Modifiers) -> bool {
        if self.hold.is_some_and(|(k, _)| k == key) {
            self.release_hold();
        }
        match self.handled_down.iter().position(|&k| k == key) {
            Some(i) => {
                self.handled_down.swap_remove(i);
                true
            }
            None => false,
        }
    }

    /// Arrow or Page key on a control. False when the key means nothing to it.
    fn step_key(&mut self, id: ControlId, key: Key, fine: bool) -> bool {
        let c = &self.controls[id];
        let page = matches!(key, Key::PageUp | Key::PageDown);
        // Up and Right raise a value. A vertical key bank counts its keys from
        // the top, so there Up and Left go to the previous key.
        let up = matches!(key, Key::Up | Key::Right | Key::PageUp);
        let dir = match c.kind {
            ControlKind::Button => return false,
            ControlKind::KeyBank { vertical: true } => {
                if matches!(key, Key::Up | Key::Left | Key::PageUp) { -1.0 } else { 1.0 }
            }
            _ if up => 1.0,
            _ => -1.0,
        };
        let new = if c.is_stepped() {
            let n = match c.info.unit {
                Unit::Toggle | Unit::Choice => ((c.info.max - c.info.min).round().max(0.0) as u32) + 1,
                _ => c.info.steps,
            };
            let positions = if page { (n as f32 * PAGE_STEP).ceil().max(1.0) } else { 1.0 };
            c.info.clamp(c.value + dir * positions * c.plain_step())
        } else {
            let amount = if page { PAGE_STEP } else { KEY_STEP } * if fine { FINE_FACTOR } else { 1.0 };
            c.info.from_normalized(c.info.to_normalized(c.value) + dir * amount)
        };
        self.send_value(id, new);
        true
    }

    /// Home or End: the minimum or the maximum.
    fn jump_key(&mut self, id: ControlId, end: bool) -> bool {
        let c = &self.controls[id];
        if c.kind == ControlKind::Button {
            return false;
        }
        let new = c.info.clamp(if end { c.info.max } else { c.info.min });
        self.send_value(id, new);
        true
    }

    /// Delete or Backspace: the default.
    fn reset_key(&mut self, id: ControlId) -> bool {
        let c = &self.controls[id];
        if c.kind == ControlKind::Button {
            return false;
        }
        let new = c.info.clamp(c.info.default);
        self.send_value(id, new);
        true
    }

    /// Return or Space: flip a toggle, press a button until the key is up.
    /// While one of the two is held, the repeat of the key is swallowed.
    fn press_key(&mut self, id: ControlId, key: Key) -> bool {
        let c = &self.controls[id];
        match c.kind {
            ControlKind::Toggle | ControlKind::Button => {}
            _ => return false,
        }
        if self.hold.is_some() {
            return true;
        }
        let (param, info, kind) = (c.param, c.info, c.kind);
        if kind == ControlKind::Toggle {
            let v = info.clamp(if c.is_on() { info.min } else { info.max });
            self.send_value(id, v);
        } else {
            let v = info.clamp(info.max);
            self.controls[id].value = v;
            self.host.begin(param);
            self.host.perform(param, v);
        }
        self.controls[id].pressed = true;
        self.hold = Some((key, id));
        true
    }

    /// A whole gesture for one new value, when it differs from the current one.
    fn send_value(&mut self, id: ControlId, new: f32) {
        let c = &mut self.controls[id];
        if new != c.value {
            c.value = new;
            self.host.set(c.param, new);
        }
    }

    fn release(&mut self) {
        if let Some(g) = self.grab.take() {
            let c = &mut self.controls[g.id];
            c.pressed = false;
            if c.kind == ControlKind::Button {
                let off = c.info.clamp(c.info.min);
                c.value = off;
                self.host.perform(c.param, off);
            }
            if g.open {
                self.host.end(c.param);
            }
        }
    }

    fn set_hover(&mut self, id: Option<ControlId>) {
        if self.hovered != id {
            if let Some(old) = self.hovered {
                self.controls[old].hover = false;
            }
            if let Some(new) = id {
                self.controls[new].hover = true;
            }
            self.hovered = id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openplugin_core::Gesture::{self, Begin, End, Perform};
    use std::cell::RefCell;
    use std::rc::Rc;

    static GAIN: ParamInfo = ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db);
    static WARM: ParamInfo = ParamInfo::new("warm", "Warm", 0.0, 1.0, 0.0, Unit::Toggle);
    static MODE: ParamInfo =
        ParamInfo::new("mode", "Mode", 0.0, 3.0, 1.0, Unit::Choice).with_choices(&["A", "B", "C", "D"]);
    static STEPS: ParamInfo = ParamInfo::new("steps", "Steps", 0.0, 10.0, 0.0, Unit::None).with_steps(6);
    static LOCKED: ParamInfo =
        ParamInfo::new("lock", "Lock", 0.0, 1.0, 0.0, Unit::None).flag(ParamFlags::READ_ONLY);

    type Log = Rc<RefCell<Vec<Gesture>>>;

    fn rig() -> (ControlSet, Log) {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let sink = log.clone();
        let set = ControlSet::new(EditorHost::new(move |g| sink.borrow_mut().push(g)));
        (set, log)
    }

    fn take(log: &Log) -> Vec<Gesture> {
        std::mem::take(&mut *log.borrow_mut())
    }

    fn knob_rig() -> (ControlSet, Log) {
        let (mut set, log) = rig();
        set.add(Control::knob(Area::rect(0.0, 0.0, 60.0, 60.0), 0, &GAIN));
        (set, log)
    }

    #[test]
    fn knob_drag_is_begin_performs_end() {
        let (mut set, log) = knob_rig();
        set.mouse_down(30.0, 30.0, 1);
        assert_eq!(take(&log), vec![Begin(0)]);
        // 50 points up of 200 is a quarter of the 48 dB range: +12 dB.
        set.mouse_move(30.0, -20.0);
        set.mouse_up(30.0, -20.0);
        assert_eq!(take(&log), vec![Perform(0, 12.0), End(0)]);
        assert_eq!(set.get(0).unwrap().value(), 12.0);
    }

    #[test]
    fn drag_far_outside_clamps_and_reversal_responds_at_once() {
        let (mut set, log) = knob_rig();
        set.mouse_down(30.0, 30.0, 1);
        set.mouse_move(30.0, -5000.0);
        set.mouse_move(30.0, -9000.0);
        set.mouse_up(30.0, -9000.0);
        assert_eq!(take(&log), vec![Begin(0), Perform(0, 24.0), End(0)]);
        set.mouse_down(30.0, 30.0, 1);
        set.mouse_move(30.0, -9000.0);
        set.mouse_move(30.0, -9000.0 + 50.0);
        assert_eq!(take(&log).last(), Some(&Perform(0, 12.0)));
    }

    #[test]
    fn fine_drag_is_a_tenth() {
        let (mut set, log) = knob_rig();
        set.set_modifiers(Modifiers { shift: true, ..Modifiers::NONE });
        set.mouse_down(30.0, 30.0, 1);
        set.mouse_move(30.0, -20.0);
        // 50 points * 0.1 / 200 * 48 dB = 1.2 dB
        let g = take(&log);
        let Perform(0, v) = g[1] else { panic!("{g:?}") };
        assert!((v - 1.2).abs() < 1e-4, "{v}");
        // Releasing the modifier mid-drag continues from here without a jump.
        set.set_modifiers(Modifiers::NONE);
        set.mouse_move(30.0, -20.0);
        assert!(take(&log).is_empty());
        set.mouse_move(30.0, -70.0);
        let g = take(&log);
        let Perform(0, v2) = g[0] else { panic!("{g:?}") };
        assert!((v2 - 13.2).abs() < 1e-3, "{v2}");
    }

    #[test]
    fn double_click_resets_to_default() {
        let (mut set, log) = knob_rig();
        set.param_changed(0, 10.0);
        set.mouse_down(30.0, 30.0, 2);
        set.mouse_up(30.0, 30.0);
        assert_eq!(take(&log), vec![Begin(0), Perform(0, 0.0), End(0)]);
        assert_eq!(set.get(0).unwrap().value(), 0.0);
    }

    #[test]
    fn click_without_move_sends_no_value() {
        let (mut set, log) = knob_rig();
        set.mouse_down(30.0, 30.0, 1);
        set.mouse_up(30.0, 30.0);
        assert_eq!(take(&log), vec![Begin(0), End(0)]);
    }

    #[test]
    fn wheel_steps_and_fine_wheel_steps_less() {
        let (mut set, log) = knob_rig();
        set.wheel(30.0, 30.0, 1.0);
        // 0.02 of 48 dB
        let g = take(&log);
        let Perform(0, v) = g[1] else { panic!("{g:?}") };
        assert!((v - 0.96).abs() < 1e-4);
        assert_eq!((g[0], g[2]), (Begin(0), End(0)));
        set.param_changed(0, 0.0);
        set.set_modifiers(Modifiers { command: true, ..Modifiers::NONE });
        set.wheel(30.0, 30.0, -1.0);
        let g = take(&log);
        let Perform(0, v) = g[1] else { panic!("{g:?}") };
        assert!((v + 0.096).abs() < 1e-4);
        set.wheel(300.0, 30.0, 1.0);
        assert!(take(&log).is_empty(), "outside any control");
    }

    #[test]
    fn wheel_at_the_end_sends_nothing() {
        let (mut set, log) = knob_rig();
        set.param_changed(0, 24.0);
        set.wheel(30.0, 30.0, 5.0);
        assert!(take(&log).is_empty());
    }

    #[test]
    fn toggle_flips_on_click() {
        let (mut set, log) = rig();
        set.add(Control::toggle(Area::rect(10.0, 10.0, 30.0, 50.0), 1, &WARM));
        set.mouse_down(20.0, 20.0, 1);
        assert!(set.get(0).unwrap().pressed());
        set.mouse_up(20.0, 20.0);
        assert!(!set.get(0).unwrap().pressed());
        set.mouse_down(20.0, 20.0, 1);
        set.mouse_up(20.0, 20.0);
        assert_eq!(
            take(&log),
            vec![Begin(1), Perform(1, 1.0), End(1), Begin(1), Perform(1, 0.0), End(1)]
        );
        set.wheel(20.0, 20.0, 3.0);
        assert!(take(&log).is_empty(), "the wheel does not touch a toggle");
    }

    #[test]
    fn button_sends_one_while_held_and_zero_after() {
        let (mut set, log) = rig();
        set.add(Control::button(Area::circle(50.0, 50.0, 10.0), 1, &WARM));
        set.mouse_down(50.0, 50.0, 1);
        assert_eq!(take(&log), vec![Begin(1), Perform(1, 1.0)]);
        assert!(set.get(0).unwrap().is_on());
        // Released far away: the button still lets go.
        set.mouse_up(400.0, 400.0);
        assert_eq!(take(&log), vec![Perform(1, 0.0), End(1)]);
        assert!(!set.get(0).unwrap().is_on());
        // The corner of a circle's bounding box misses.
        set.mouse_down(59.0, 59.0, 1);
        assert!(take(&log).is_empty());
    }

    #[test]
    fn stepped_parameter_snaps_on_drag_and_wheel() {
        let (mut set, log) = rig();
        set.add(Control::knob(Area::rect(0.0, 0.0, 60.0, 60.0), 2, &STEPS).drag_range(100.0));
        set.mouse_down(30.0, 30.0, 1);
        // 30 points of 100 is 3.0 of 10; the positions are 0,2,4,6,8,10, so it snaps to 4.
        set.mouse_move(30.0, 0.0);
        set.mouse_move(30.0, -1.0);
        set.mouse_up(30.0, -1.0);
        assert_eq!(take(&log), vec![Begin(2), Perform(2, 4.0), End(2)]);
        set.wheel(30.0, 30.0, 1.0);
        assert_eq!(take(&log), vec![Begin(2), Perform(2, 6.0), End(2)]);
        // Half a unit of scroll is not a step; two halves are.
        set.wheel(30.0, 30.0, 0.5);
        assert!(take(&log).is_empty());
        set.wheel(30.0, 30.0, 0.5);
        assert_eq!(take(&log), vec![Begin(2), Perform(2, 8.0), End(2)]);
    }

    #[test]
    fn key_bank_selects_and_glides() {
        let (mut set, log) = rig();
        set.add(Control::key_bank(Area::rect(0.0, 0.0, 100.0, 20.0), 3, &MODE, false));
        assert_eq!(set.get(0).unwrap().key_count(), 4);
        assert_eq!(set.get(0).unwrap().selected_key(), 1);
        set.mouse_down(80.0, 10.0, 1);
        set.mouse_move(60.0, 10.0);
        set.mouse_move(-50.0, 10.0);
        set.mouse_up(-50.0, 10.0);
        assert_eq!(
            take(&log),
            vec![Begin(3), Perform(3, 3.0), Perform(3, 2.0), Perform(3, 0.0), End(3)]
        );
        set.wheel(10.0, 10.0, -1.0);
        assert!(take(&log).is_empty(), "already at the first key");
        set.wheel(10.0, 10.0, 1.0);
        assert_eq!(take(&log), vec![Begin(3), Perform(3, 1.0), End(3)]);
    }

    #[test]
    fn faders_follow_the_pointer_one_to_one() {
        let (mut set, log) = rig();
        set.add(Control::fader_v(Area::rect(0.0, 0.0, 20.0, 100.0), 0, &GAIN));
        set.add(Control::fader_h(Area::rect(50.0, 0.0, 100.0, 20.0), 1, &WARM));
        set.mouse_down(10.0, 50.0, 1);
        set.mouse_move(10.0, 25.0);
        set.mouse_up(10.0, 25.0);
        // 25 of 100 points up: a quarter of 48 dB.
        assert_eq!(take(&log), vec![Begin(0), Perform(0, 12.0), End(0)]);
        set.mouse_down(60.0, 10.0, 1);
        set.mouse_move(110.0, 10.0);
        set.mouse_up(110.0, 10.0);
        assert_eq!(take(&log), vec![Begin(1), Perform(1, 1.0), End(1)]);
    }

    #[test]
    fn host_value_updates_without_a_gesture() {
        let (mut set, log) = knob_rig();
        set.param_changed(0, 6.0);
        assert_eq!(set.get(0).unwrap().value(), 6.0);
        set.param_changed(0, 1000.0);
        assert_eq!(set.get(0).unwrap().value(), 24.0);
        set.param_changed(0, f32::NAN);
        set.param_changed(0, f32::INFINITY);
        assert_eq!(set.get(0).unwrap().value(), 24.0);
        set.param_changed(9, 1.0);
        assert!(take(&log).is_empty());
        // While held, an echo does not move the control; after the release it does.
        set.mouse_down(30.0, 30.0, 1);
        set.param_changed(0, -3.0);
        assert_eq!(set.get(0).unwrap().value(), 24.0);
        set.mouse_up(30.0, 30.0);
        set.param_changed(0, -3.0);
        assert_eq!(set.get(0).unwrap().value(), -3.0);
        assert_eq!(take(&log), vec![Begin(0), End(0)]);
    }

    #[test]
    fn nothing_out_of_range_or_non_finite_reaches_the_host() {
        let (mut set, log) = knob_rig();
        set.mouse_down(30.0, 30.0, 1);
        for y in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1e30, 1e30] {
            set.mouse_move(30.0, y);
        }
        set.mouse_move(f32::NAN, 30.0);
        set.mouse_up(30.0, 30.0);
        set.wheel(30.0, 30.0, f32::NAN);
        set.wheel(30.0, 30.0, f32::INFINITY);
        let mut performs = 0;
        for g in take(&log) {
            if let Perform(_, v) = g {
                performs += 1;
                assert!((-24.0..=24.0).contains(&v), "{v}");
            }
        }
        assert!(performs > 0);
    }

    #[test]
    fn hover_and_pressed_state() {
        let (mut set, _log) = rig();
        set.add(Control::knob(Area::rect(0.0, 0.0, 60.0, 60.0), 0, &GAIN));
        set.add(Control::toggle(Area::rect(100.0, 0.0, 30.0, 30.0), 1, &WARM));
        set.mouse_move(10.0, 10.0);
        assert_eq!(set.hovered(), Some(0));
        assert!(set.get(0).unwrap().hover() && !set.get(1).unwrap().hover());
        set.mouse_move(110.0, 10.0);
        assert!(!set.get(0).unwrap().hover() && set.get(1).unwrap().hover());
        set.mouse_down(10.0, 10.0, 1);
        assert_eq!(set.pressed(), Some(0));
        assert!(set.get(0).unwrap().pressed());
        set.mouse_up(500.0, 500.0);
        assert_eq!(set.pressed(), None);
        assert_eq!(set.hovered(), None);
        set.mouse_move(10.0, 10.0);
        set.mouse_exit();
        assert_eq!(set.hovered(), None);
        assert_eq!(set.iter().count(), 2);
    }

    #[test]
    fn read_only_ignores_input_and_a_lost_release_closes_the_gesture() {
        let (mut set, log) = rig();
        set.add(Control::knob(Area::rect(0.0, 0.0, 60.0, 60.0), 0, &GAIN));
        set.add(Control::knob(Area::rect(100.0, 0.0, 60.0, 60.0), 4, &LOCKED));
        set.mouse_down(110.0, 10.0, 1);
        set.wheel(110.0, 10.0, 1.0);
        assert!(take(&log).is_empty());
        set.mouse_down(10.0, 10.0, 1);
        set.mouse_down(10.0, 10.0, 1);
        assert_eq!(take(&log), vec![Begin(0), End(0), Begin(0)]);
    }

    // ---- keyboard focus and keys ----

    const NO: Modifiers = Modifiers::NONE;
    const SHIFT: Modifiers = Modifiers { shift: true, ..Modifiers::NONE };
    const CMD: Modifiers = Modifiers { command: true, ..Modifiers::NONE };
    const CTRL: Modifiers = Modifiers { ctrl: true, ..Modifiers::NONE };
    const ALT: Modifiers = Modifiers { alt: true, ..Modifiers::NONE };

    /// Knob 0 (gain), toggle 1, read-only knob 2, key bank 3 (horizontal),
    /// stepped knob 4, button 5, vertical key bank 6; the control ids are the
    /// positions in that order.
    fn key_rig() -> (ControlSet, Log) {
        let (mut set, log) = rig();
        set.add(Control::knob(Area::rect(0.0, 0.0, 60.0, 60.0), 0, &GAIN));
        set.add(Control::toggle(Area::rect(100.0, 0.0, 30.0, 30.0), 1, &WARM));
        set.add(Control::knob(Area::rect(200.0, 0.0, 60.0, 60.0), 9, &LOCKED));
        set.add(Control::key_bank(Area::rect(0.0, 100.0, 100.0, 20.0), 3, &MODE, false));
        set.add(Control::knob(Area::rect(300.0, 0.0, 60.0, 60.0), 2, &STEPS));
        set.add(Control::button(Area::circle(450.0, 50.0, 10.0), 7, &WARM));
        set.add(Control::key_bank(Area::rect(500.0, 0.0, 20.0, 100.0), 8, &MODE, true));
        (set, log)
    }

    fn focus(set: &mut ControlSet, id: ControlId) {
        set.set_focus(Some(id));
        assert_eq!(set.focused(), Some(id));
    }

    fn near(g: Gesture, param: u32, want: f32) {
        let Perform(p, v) = g else { panic!("{g:?}") };
        assert_eq!(p, param);
        assert!((v - want).abs() < 1e-3, "{v} != {want}");
    }

    #[test]
    fn tab_walks_the_interactive_controls_and_leaves_at_the_ends() {
        let (mut set, log) = key_rig();
        assert!(set.wants_keys());
        assert_eq!(set.focused(), None);
        let mut order = Vec::new();
        for _ in 0..6 {
            assert!(set.key_down(Key::Tab, NO));
            order.push(set.focused().unwrap());
            assert!(set.get(*order.last().unwrap()).unwrap().focused());
        }
        // The read-only knob (2) is skipped.
        assert_eq!(order, vec![0, 1, 3, 4, 5, 6]);
        // Past the last control: not handled, focus cleared.
        assert!(!set.key_down(Key::Tab, NO));
        assert_eq!(set.focused(), None);
        assert!(set.iter().all(|c| !c.focused()));
        // Shift-Tab from nothing goes to the last control, then back.
        assert!(set.key_down(Key::Tab, SHIFT));
        assert_eq!(set.focused(), Some(6));
        assert!(set.key_down(Key::Tab, SHIFT));
        assert_eq!(set.focused(), Some(5));
        focus(&mut set, 0);
        assert!(!set.key_down(Key::Tab, SHIFT), "before the first control");
        assert_eq!(set.focused(), None);
        // Control-Tab belongs to the host.
        focus(&mut set, 1);
        assert!(!set.key_down(Key::Tab, CTRL));
        assert_eq!(set.focused(), Some(1));
        assert!(take(&log).is_empty(), "moving focus sends no gesture");
    }

    #[test]
    fn a_set_with_nothing_to_key_does_not_want_keys() {
        let (mut set, _log) = rig();
        assert!(!set.wants_keys());
        assert!(!set.key_down(Key::Tab, NO));
        set.add(Control::knob(Area::rect(0.0, 0.0, 10.0, 10.0), 0, &LOCKED));
        assert!(!set.wants_keys());
        assert!(!set.key_down(Key::Tab, NO));
        set.set_focus(Some(0));
        assert_eq!(set.focused(), None, "a read-only control takes no focus");
        set.set_focus(Some(99));
        assert_eq!(set.focused(), None);
    }

    #[test]
    fn a_click_focuses_the_control_and_a_click_on_the_plate_clears_it() {
        let (mut set, _log) = key_rig();
        set.mouse_down(310.0, 10.0, 1);
        set.mouse_up(310.0, 10.0);
        assert_eq!(set.focused(), Some(4));
        assert!(set.get(4).unwrap().focused());
        set.mouse_down(110.0, 10.0, 1);
        set.mouse_up(110.0, 10.0);
        assert_eq!(set.focused(), Some(1));
        assert!(!set.get(4).unwrap().focused());
        // A double-click reset focuses too.
        set.mouse_down(30.0, 30.0, 2);
        set.mouse_up(30.0, 30.0);
        assert_eq!(set.focused(), Some(0));
        // Read-only: no focus, and the old focus goes.
        set.mouse_down(210.0, 10.0, 1);
        assert_eq!(set.focused(), None);
        set.mouse_up(210.0, 10.0);
        set.mouse_down(30.0, 30.0, 1);
        set.mouse_up(30.0, 30.0);
        set.mouse_down(700.0, 700.0, 1);
        assert_eq!(set.focused(), None);
    }

    #[test]
    fn arrows_step_a_continuous_control_by_gestures() {
        let (mut set, log) = key_rig();
        focus(&mut set, 0);
        assert!(set.key_down(Key::Up, NO));
        let g = take(&log);
        assert_eq!((g[0], g[2]), (Begin(0), End(0)));
        near(g[1], 0, 0.48);
        assert!(set.key_down(Key::Right, NO));
        near(take(&log)[1], 0, 0.96);
        assert!(set.key_down(Key::Down, NO));
        assert!(set.key_down(Key::Left, NO));
        let g = take(&log);
        near(g[1], 0, 0.48);
        near(g[4], 0, 0.0);
        // Page keys move a tenth of the range.
        assert!(set.key_down(Key::PageUp, NO));
        near(take(&log)[1], 0, 4.8);
        assert!(set.key_down(Key::PageDown, NO));
        near(take(&log)[1], 0, 0.0);
        // Shift and Command make it a tenth of that.
        assert!(set.key_down(Key::Up, SHIFT));
        near(take(&log)[1], 0, 0.048);
        assert!(set.key_down(Key::PageUp, CMD));
        near(take(&log)[1], 0, 0.048 + 0.48);
    }

    #[test]
    fn keys_clamp_at_the_ends_and_send_nothing_there() {
        let (mut set, log) = key_rig();
        focus(&mut set, 0);
        set.param_changed(0, 23.9);
        assert!(set.key_down(Key::PageUp, NO));
        assert_eq!(take(&log), vec![Begin(0), Perform(0, 24.0), End(0)]);
        // Handled, but already at the maximum: no gesture.
        assert!(set.key_down(Key::Up, NO));
        assert!(set.key_down(Key::PageUp, NO));
        assert!(take(&log).is_empty());
        set.param_changed(0, -24.0);
        assert!(set.key_down(Key::PageDown, NO));
        assert!(take(&log).is_empty());
        assert_eq!(set.get(0).unwrap().value(), -24.0);
    }

    #[test]
    fn home_and_end_go_to_the_ends() {
        let (mut set, log) = key_rig();
        focus(&mut set, 0);
        assert!(set.key_down(Key::End, NO));
        assert_eq!(take(&log), vec![Begin(0), Perform(0, 24.0), End(0)]);
        assert!(set.key_down(Key::Home, SHIFT));
        assert_eq!(take(&log), vec![Begin(0), Perform(0, -24.0), End(0)]);
        // Stepped and choice parameters land on a position.
        focus(&mut set, 4);
        assert!(set.key_down(Key::End, NO));
        assert_eq!(take(&log), vec![Begin(2), Perform(2, 10.0), End(2)]);
        focus(&mut set, 3);
        assert!(set.key_down(Key::End, NO));
        assert_eq!(take(&log), vec![Begin(3), Perform(3, 3.0), End(3)]);
        assert!(set.key_down(Key::Home, NO));
        assert_eq!(take(&log), vec![Begin(3), Perform(3, 0.0), End(3)]);
        // Toggle: Home is off, End is on. A button has no ends.
        focus(&mut set, 1);
        assert!(set.key_down(Key::End, NO));
        assert!(set.key_down(Key::Home, NO));
        assert_eq!(take(&log), vec![Begin(1), Perform(1, 1.0), End(1), Begin(1), Perform(1, 0.0), End(1)]);
        focus(&mut set, 5);
        assert!(!set.key_down(Key::End, NO));
        assert!(!set.key_down(Key::Up, NO));
        assert!(take(&log).is_empty());
    }

    #[test]
    fn stepped_controls_move_by_positions() {
        let (mut set, log) = key_rig();
        // Positions 0,2,4,6,8,10: an arrow is one position, a page is a tenth of six, rounded up, so one.
        focus(&mut set, 4);
        assert!(set.key_down(Key::Up, NO));
        assert!(set.key_down(Key::PageUp, SHIFT));
        assert!(set.key_down(Key::Right, CMD));
        assert_eq!(
            take(&log),
            vec![Begin(2), Perform(2, 2.0), End(2), Begin(2), Perform(2, 4.0), End(2), Begin(2), Perform(2, 6.0), End(2)]
        );
        assert!(set.key_down(Key::Down, NO));
        assert_eq!(take(&log), vec![Begin(2), Perform(2, 4.0), End(2)]);
        // Choice: a horizontal bank raises with Right.
        focus(&mut set, 3);
        assert_eq!(set.get(3).unwrap().selected_key(), 1);
        assert!(set.key_down(Key::Right, NO));
        assert!(set.key_down(Key::PageUp, NO));
        assert!(set.key_down(Key::Right, NO), "already on the last key");
        assert!(set.key_down(Key::Left, NO));
        assert_eq!(
            take(&log),
            vec![Begin(3), Perform(3, 2.0), End(3), Begin(3), Perform(3, 3.0), End(3), Begin(3), Perform(3, 2.0), End(3)]
        );
        // A vertical bank counts from the top: Up goes to the previous key.
        focus(&mut set, 6);
        assert!(set.key_down(Key::Up, NO));
        assert!(set.key_down(Key::Up, NO));
        assert!(set.key_down(Key::Down, NO));
        assert_eq!(take(&log), vec![Begin(8), Perform(8, 0.0), End(8), Begin(8), Perform(8, 1.0), End(8)]);
        // A toggle: up is on, down is off, a repeat sends nothing.
        focus(&mut set, 1);
        assert!(set.key_down(Key::Up, NO));
        assert!(set.key_down(Key::Up, NO));
        assert!(set.key_down(Key::Left, NO));
        assert_eq!(take(&log), vec![Begin(1), Perform(1, 1.0), End(1), Begin(1), Perform(1, 0.0), End(1)]);
    }

    #[test]
    fn return_and_space_flip_a_toggle_once_per_press() {
        let (mut set, log) = key_rig();
        focus(&mut set, 1);
        assert!(set.key_down(Key::Space, NO));
        assert_eq!(take(&log), vec![Begin(1), Perform(1, 1.0), End(1)]);
        assert!(set.get(1).unwrap().pressed());
        // Auto-repeat of the held key, and another key meanwhile, change nothing.
        assert!(set.key_down(Key::Space, NO));
        assert!(set.key_down(Key::Space, NO));
        assert!(set.key_down(Key::Return, NO));
        assert!(take(&log).is_empty());
        assert!(set.key_up(Key::Space, NO));
        assert!(!set.get(1).unwrap().pressed());
        assert!(take(&log).is_empty());
        assert!(set.key_down(Key::Return, NO));
        assert!(set.key_up(Key::Return, NO));
        assert_eq!(take(&log), vec![Begin(1), Perform(1, 0.0), End(1)]);
        assert!(!set.get(1).unwrap().is_on());
    }

    #[test]
    fn return_and_space_hold_a_button_until_the_key_is_up() {
        let (mut set, log) = key_rig();
        focus(&mut set, 5);
        assert!(set.key_down(Key::Return, NO));
        assert_eq!(take(&log), vec![Begin(7), Perform(7, 1.0)]);
        assert!(set.get(5).unwrap().is_on() && set.get(5).unwrap().pressed());
        assert!(set.key_down(Key::Return, NO), "repeat");
        assert!(take(&log).is_empty());
        // The host's echo of the held parameter is ignored.
        set.param_changed(7, 0.0);
        assert!(set.get(5).unwrap().is_on());
        assert!(set.key_up(Key::Return, NO));
        assert_eq!(take(&log), vec![Perform(7, 0.0), End(7)]);
        assert!(!set.get(5).unwrap().is_on() && !set.get(5).unwrap().pressed());
        // Losing keyboard focus while held lets the button go.
        assert!(set.key_down(Key::Space, NO));
        take(&log);
        set.release_keys();
        assert_eq!(take(&log), vec![Perform(7, 0.0), End(7)]);
        assert!(!set.key_up(Key::Space, NO));
        // Moving focus away does the same.
        assert!(set.key_down(Key::Space, NO));
        take(&log);
        assert!(set.key_down(Key::Tab, NO));
        assert_eq!(take(&log), vec![Perform(7, 0.0), End(7)]);
        // Return and Space mean nothing to a knob or a key bank.
        focus(&mut set, 0);
        assert!(!set.key_down(Key::Return, NO));
        focus(&mut set, 3);
        assert!(!set.key_down(Key::Space, NO));
        assert!(take(&log).is_empty());
    }

    #[test]
    fn delete_and_backspace_reset_to_the_default() {
        let (mut set, log) = key_rig();
        focus(&mut set, 0);
        set.param_changed(0, 10.0);
        assert!(set.key_down(Key::Delete, NO));
        assert_eq!(take(&log), vec![Begin(0), Perform(0, 0.0), End(0)]);
        assert!(set.key_down(Key::Backspace, NO), "handled even at the default");
        assert!(take(&log).is_empty());
        focus(&mut set, 3);
        set.param_changed(3, 3.0);
        assert!(set.key_down(Key::Backspace, NO));
        assert_eq!(take(&log), vec![Begin(3), Perform(3, 1.0), End(3)]);
        focus(&mut set, 4);
        set.param_changed(2, 8.0);
        assert!(set.key_down(Key::Delete, NO));
        assert_eq!(take(&log), vec![Begin(2), Perform(2, 0.0), End(2)]);
        focus(&mut set, 5);
        assert!(!set.key_down(Key::Delete, NO), "a button has no value to reset");
        // Command-Delete is the host's.
        focus(&mut set, 0);
        set.param_changed(0, 5.0);
        assert!(!set.key_down(Key::Delete, CMD));
        assert!(take(&log).is_empty());
    }

    #[test]
    fn escape_clears_focus_and_otherwise_goes_to_the_host() {
        let (mut set, _log) = key_rig();
        assert!(!set.key_down(Key::Escape, NO));
        focus(&mut set, 0);
        assert!(!set.key_down(Key::Escape, CMD));
        assert_eq!(set.focused(), Some(0));
        assert!(set.key_down(Key::Escape, NO));
        assert_eq!(set.focused(), None);
        assert!(!set.get(0).unwrap().focused());
        assert!(!set.key_down(Key::Escape, NO));
    }

    #[test]
    fn keys_the_set_does_not_use_reach_the_host() {
        let (mut set, log) = key_rig();
        // Nothing focused: no key is handled, and none sends a gesture.
        let all = [
            Key::Left,
            Key::Up,
            Key::Down,
            Key::PageUp,
            Key::PageDown,
            Key::Home,
            Key::End,
            Key::Return,
            Key::Space,
            Key::Delete,
            Key::Backspace,
            Key::Escape,
            Key::Char('a'),
        ];
        for key in all {
            assert!(!set.key_down(key, NO), "{key:?}");
            assert!(!set.key_up(key, NO), "{key:?}");
        }
        focus(&mut set, 0);
        assert!(!set.key_down(Key::Char('a'), NO));
        assert!(!set.key_down(Key::Char(' '), NO));
        // Command, Control or Alt on a key that is not an arrow or Page key keeps it for the host.
        for m in [CMD, CTRL, ALT] {
            for key in [Key::Home, Key::End, Key::Return, Key::Space, Key::Delete, Key::Backspace, Key::Tab, Key::Escape] {
                assert!(!set.key_down(key, m), "{key:?} {m:?}");
            }
        }
        assert_eq!(set.focused(), Some(0));
        assert!(take(&log).is_empty());
    }

    #[test]
    fn key_up_follows_key_down() {
        let (mut set, _log) = key_rig();
        focus(&mut set, 0);
        assert!(set.key_down(Key::Up, NO));
        assert!(set.key_down(Key::Up, NO), "auto-repeat");
        assert!(set.key_up(Key::Up, NO));
        assert!(!set.key_up(Key::Up, NO), "one up per run of downs");
        assert!(!set.key_down(Key::Char('x'), NO));
        assert!(!set.key_up(Key::Char('x'), NO));
        assert!(set.key_down(Key::Tab, NO));
        assert!(set.key_up(Key::Tab, NO));
        // A Tab that left the set is the host's, down and up.
        focus(&mut set, 6);
        assert!(!set.key_down(Key::Tab, NO));
        assert!(!set.key_up(Key::Tab, NO));
    }

    #[test]
    fn keys_wait_while_the_mouse_holds_a_control() {
        let (mut set, log) = key_rig();
        set.mouse_down(30.0, 30.0, 1);
        take(&log);
        assert!(!set.key_down(Key::Up, NO));
        assert!(!set.key_down(Key::Tab, NO));
        set.mouse_up(30.0, 30.0);
        assert_eq!(take(&log), vec![End(0)]);
        assert!(set.key_down(Key::Up, NO), "the clicked control is focused");
        assert_eq!(take(&log).len(), 3);
    }

    #[test]
    fn keys_send_only_finite_values_in_range() {
        let (mut set, log) = key_rig();
        focus(&mut set, 0);
        set.param_changed(0, 1e30);
        for key in [Key::Up, Key::PageUp, Key::Home, Key::End, Key::Down, Key::PageDown, Key::Delete] {
            for m in [NO, SHIFT, CMD] {
                set.key_down(key, m);
            }
        }
        let g = take(&log);
        assert!(!g.is_empty());
        for g in g {
            if let Perform(_, v) = g {
                assert!((-24.0..=24.0).contains(&v), "{v}");
            }
        }
    }

    #[test]
    fn take_changed_reports_value_hover_pressed_and_focus_once() {
        let (mut set, _log) = key_rig();
        let all: Vec<ControlId> = (0..set.len()).collect();
        assert_eq!(set.take_changed(), all, "the first call reports everything");
        assert!(set.take_changed().is_empty(), "and then only changes");
        // Hover.
        set.mouse_move(30.0, 30.0);
        assert_eq!(set.take_changed(), vec![0]);
        set.mouse_move(31.0, 31.0);
        assert!(set.take_changed().is_empty(), "moving inside a control changes nothing drawn");
        // Press and drag: pressed, focus and value.
        set.mouse_down(30.0, 30.0, 1);
        assert_eq!(set.take_changed(), vec![0]);
        set.mouse_move(30.0, 10.0);
        assert_eq!(set.take_changed(), vec![0]);
        set.mouse_up(30.0, 10.0);
        assert_eq!(set.take_changed(), vec![0], "released");
        // Moving to another control changes the old and the new one.
        set.mouse_move(110.0, 10.0);
        assert_eq!(set.take_changed(), vec![0, 1]);
        // A value from the host, and a value that is already there.
        set.param_changed(1, 1.0);
        assert_eq!(set.take_changed(), vec![1]);
        set.param_changed(1, 1.0);
        assert!(set.take_changed().is_empty());
        // Keyboard focus and a key step.
        set.key_down(Key::Tab, NO);
        assert_eq!(set.take_changed(), vec![0, 1], "focus left the knob and arrived on the toggle");
        set.key_down(Key::Down, NO);
        assert_eq!(set.take_changed().len(), 1);
        // A control added later is reported on its own.
        let id = set.add(Control::knob(Area::rect(600.0, 0.0, 10.0, 10.0), 0, &GAIN));
        assert_eq!(set.take_changed(), vec![id]);
    }
}
