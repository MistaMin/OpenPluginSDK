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

use openplugin_core::{EditorHost, ParamFlags, ParamInfo, Unit};

pub use openplugin_core::editor::Modifiers;

/// Points of vertical drag for the full range of a knob.
pub const DEFAULT_DRAG_RANGE: f32 = 200.0;
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
    drag_range: f32,
}

impl Control {
    fn new(kind: ControlKind, area: Area, param: usize, info: &'static ParamInfo) -> Self {
        Self { area, kind, param, info, value: info.clamp(info.default), hover: false, pressed: false, drag_range: 0.0 }
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
        let held = self.grab.as_ref().map(|g| self.controls[g.id].param);
        if held == Some(param) {
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
        let Some(id) = self.hit(x, y) else { return };
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
}
