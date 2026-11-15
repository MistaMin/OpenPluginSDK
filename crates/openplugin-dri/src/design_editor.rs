//! An [`openplugin_core::Editor`] built from a GoodLookin design file.
//!
//! A design is a CSV (`id,parameter,label,style,colour,x,y,width,height,font size[,marks]`)
//! that GoodLookin DRI reads with [`goodlookindri::design::read_design`]. [`DesignEditor`]
//! turns each row into a control of the [`crate::hit`] registry, bound to the plugin
//! parameter whose id is in the `parameter` column, placed with
//! [`goodlookindri::host::map_rect`], and drawn with the row's style through GoodLookin DRI.
//!
//! # What a row becomes
//!
//! The kind of control follows the parameter, not the row:
//!
//! | Parameter | Control | Drawn as |
//! |---|---|---|
//! | `Unit::Toggle` | switch (`ControlKind::Toggle`) | a toggle: `Lvr` is a lever, `Mini` and `Tab` are mini, anything else a bat |
//! | `Unit::Choice` | key bank (`ControlKind::KeyBank`) | latching keys named by the choices, in the push style of the row's knob style; vertical when the row is taller than wide |
//! | anything else | knob or fader | the row's knob style; a fader when the row is at least twice as tall (vertical) or as wide (horizontal) as it is the other way |
//!
//! GoodLookin DRI's loader refuses a `style` that is not a knob style, so a fader or a
//! switch cannot be named in the file. The shape of the row picks the fader, and the knob
//! style picks the look of the switch, the keys and the fader cap (console cap for `Brit`,
//! `N`, `A`, `FS` and `Mtl`, oval for the rest). The row's colour is the cap colour of
//! a knob, the lamp of a switch, the colour of the keys and the cap of a fader.
//!
//! The top of a row is the control. Under it sits the text strip: the label, and for
//! knobs and faders the value text. The strip is as high as the row's font size asks.
//!
//! # Keys
//!
//! The editor forwards keyboard input to its [`ControlSet`]: Tab and Shift-Tab walk the
//! controls in file order, arrows and Page keys step the focused one, Home and End go to the
//! ends, Space and Return flip a switch, Delete resets, Escape clears focus. A focus ring is
//! drawn around the focused control while the view holds the keyboard. Keys the set does not
//! use go back to the host.
//!
//! # Repaint
//!
//! [`Editor::damage`] reports the rows whose control changed (value, hover, pressed, focus),
//! so a view repaints only those; see [`crate::render::SceneCanvas::frame_damaged`].
//!
//! # Rows that bind to nothing
//!
//! A row with an empty `parameter` or an id the plugin does not have is not a control. It
//! is not drawn, it is not dropped silently either: [`DesignEditor::issues`] lists it and
//! a warning goes to the log. [`DesignOptions::strict`] turns the same rows into an error.
//!
//! ```ignore
//! static DESIGN: &str = include_str!("../design.csv");
//!
//! fn make_editor(host: EditorHost) -> Option<Box<dyn Editor>> {
//!     DesignEditor::from_csv(DESIGN, &INFO, host).map(|e| Box::new(e) as Box<dyn Editor>)
//! }
//! ```

use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};

use goodlookindri::color::Color as GlColor;
use goodlookindri::design::{self, Item};
use goodlookindri::host::map_rect;
use goodlookindri::scene::{Point, Rect, Scene};
use goodlookindri::{meters, switches};
use openplugin_core::editor::{DamageRect, DamageTracker, Key, Modifiers, TextEdit, Tooltip};
use openplugin_core::{Editor, EditorHost, Frame, PluginInfo, Unit};

use crate::canvas::DrawList;
use crate::controls::{draw_faceplate_code, draw_knob_code};
use crate::hit::{Area, Control, ControlId, ControlKind, ControlSet};
use crate::render::SceneCanvas;
use crate::styles::Color;

/// Space kept right of and below the furthest row when the size is derived.
const MARGIN: f32 = 12.0;
/// Smallest editor the derived size can be, in points.
const MIN_SIDE: u32 = 64;
/// Line height of the text strip, in font sizes.
const LINE: f32 = 1.2;

/// Why a row has no control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueKind {
    /// The `parameter` column is empty.
    NoParameter,
    /// The `parameter` column names an id the plugin does not have.
    UnknownParameter,
}

/// A row that is bound to no parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowIssue {
    /// The row's `id`.
    pub id: String,
    /// What the row's `parameter` column said (possibly empty).
    pub parameter: String,
    pub kind: IssueKind,
}

impl fmt::Display for RowIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            IssueKind::NoParameter => write!(f, "row `{}` names no parameter", self.id),
            IssueKind::UnknownParameter => {
                write!(f, "row `{}` names unknown parameter `{}`", self.id, self.parameter)
            }
        }
    }
}

/// Why a design editor could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DesignEditorError {
    /// The CSV is malformed or has an invalid row (GoodLookin DRI's message).
    Design(String),
    /// The faceplate code is not one of GoodLookin DRI's.
    UnknownPlate(String),
    /// An option is out of range (scale, size).
    Options(&'static str),
    /// [`DesignOptions::strict`] was set and these rows are bound to nothing.
    Unbound(Vec<RowIssue>),
}

impl fmt::Display for DesignEditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Design(m) => write!(f, "bad design: {m}"),
            Self::UnknownPlate(p) => write!(f, "unknown faceplate `{p}`"),
            Self::Options(m) => write!(f, "bad option: {m}"),
            Self::Unbound(rows) => {
                write!(f, "{} design row(s) bound to no parameter:", rows.len())?;
                for r in rows {
                    write!(f, " [{r}]")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for DesignEditorError {}

/// Layout and look choices a design file does not carry.
#[derive(Clone, Debug)]
pub struct DesignOptions {
    plate: String,
    scale: f32,
    origin: (f32, f32),
    size: Option<(u32, u32)>,
    strict: bool,
}

impl Default for DesignOptions {
    fn default() -> Self {
        Self { plate: "Navy".into(), scale: 1.0, origin: (0.0, 0.0), size: None, strict: false }
    }
}

impl DesignOptions {
    /// The faceplate under the controls, by GoodLookin code (`"Navy"` by default).
    pub fn plate(mut self, code: &str) -> Self {
        self.plate = code.to_string();
        self
    }

    /// The `scale` of [`goodlookindri::host::map_rect`]: design units to points (1 by default).
    pub fn scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    /// The `origin_x`, `origin_y` of [`goodlookindri::host::map_rect`], in points.
    pub fn origin(mut self, x: f32, y: f32) -> Self {
        self.origin = (x, y);
        self
    }

    /// The editor's size in points. By default it is the furthest edge of the rows plus a margin.
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.size = Some((width, height));
        self
    }

    /// Fail on rows bound to no parameter instead of reporting them.
    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }
}

/// What a bound row is, for tests and tooling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Knob,
    FaderV,
    FaderH,
    Switch,
    Keys { vertical: bool },
}

/// A design row bound to a parameter.
#[derive(Clone, Debug)]
pub struct Binding {
    /// The row's `id`.
    pub row: String,
    /// Descriptor index of the parameter, as sent in gestures.
    pub param: usize,
    /// The control in [`DesignEditor::controls`].
    pub control: ControlId,
    pub look: Look,
}

struct Part {
    item: Item,
    control: ControlId,
    look: Look,
    /// The whole row in points.
    bounds: Rect,
    /// The control's own rectangle (the square of a knob).
    body: Rect,
    colour: GlColor,
    label: String,
    font: f32,
}

/// An editor made from a GoodLookin design. See the [module documentation](self).
pub struct DesignEditor {
    info: &'static PluginInfo,
    controls: ControlSet,
    parts: Vec<Part>,
    bindings: Vec<Binding>,
    issues: Vec<RowIssue>,
    plate: &'static design::Faceplate,
    size: (u32, u32),
    canvas: SceneCanvas,
    layer_key: u64,
    /// The view has keyboard focus (the focus ring is drawn only then).
    keys_active: bool,
    /// What the next frame has to repaint.
    damage: DamageTracker,
}

fn rect_area(r: Rect) -> Area {
    Area::rect(r.x, r.y, r.w, r.h)
}

fn sdk_colour(c: GlColor) -> Color {
    Color::rgb(c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0)
}

fn style_code(item: &Item) -> &str {
    design::parse_style(&item.style).1
}

fn push_style(code: &str) -> switches::PushStyle {
    use switches::PushStyle::*;
    match code {
        "Rd" | "Mini" => Round,
        "Dm" => Dome,
        "Cyb" | "Hex" | "Yel" => Chamfer,
        "Neo" | "Glw" | "Grd" | "Seg" | "Vfd" | "Led" => Neon,
        "Key" | "Tl" | "Tab" => Pill,
        "Wh" | "Pie" => Flat,
        _ => Square,
    }
}

fn toggle_style(code: &str) -> switches::ToggleStyle {
    match code {
        "Lvr" => switches::ToggleStyle::Lever,
        "Mini" | "Tab" => switches::ToggleStyle::Mini,
        _ => switches::ToggleStyle::Bat,
    }
}

fn fader_cap(code: &str) -> meters::FaderCap {
    match code {
        "Brit" | "N" | "A" | "FS" | "Mtl" => meters::FaderCap::Console,
        _ => meters::FaderCap::Oval,
    }
}

impl DesignEditor {
    /// Build an editor from design text, for a plugin's editor factory. A bad design is
    /// logged and gives `None`, so the host shows its generic controls. Default options.
    pub fn from_csv(csv: &str, info: &'static PluginInfo, host: EditorHost) -> Option<Self> {
        match Self::try_new(csv, info, host, &DesignOptions::default()) {
            Ok(editor) => Some(editor),
            Err(e) => {
                log::error!("{}: design editor not built: {e}", info.name);
                None
            }
        }
    }

    /// Build an editor from design text, or say why not. A malformed CSV, an invalid row,
    /// a duplicate id, an unknown faceplate or a bad option is an error. A row bound to no
    /// parameter is an error only under [`DesignOptions::strict`]; otherwise it is listed
    /// by [`Self::issues`] and logged.
    pub fn try_new(
        csv: &str,
        info: &'static PluginInfo,
        host: EditorHost,
        options: &DesignOptions,
    ) -> Result<Self, DesignEditorError> {
        let items = design::read_design(csv).map_err(|e| DesignEditorError::Design(e.0))?;
        Self::from_items(items, info, host, options)
    }

    /// [`Self::try_new`] for rows already read (and validated) by GoodLookin DRI.
    pub fn from_items(
        items: Vec<Item>,
        info: &'static PluginInfo,
        host: EditorHost,
        options: &DesignOptions,
    ) -> Result<Self, DesignEditorError> {
        let plate = design::find_faceplate(&options.plate)
            .ok_or_else(|| DesignEditorError::UnknownPlate(options.plate.clone()))?;
        if !(options.scale.is_finite() && options.scale > 0.0) {
            return Err(DesignEditorError::Options("scale must be positive and finite"));
        }
        if !(options.origin.0.is_finite() && options.origin.1.is_finite()) {
            return Err(DesignEditorError::Options("origin must be finite"));
        }
        if matches!(options.size, Some((w, h)) if w == 0 || h == 0 || w > 16384 || h > 16384) {
            return Err(DesignEditorError::Options("size must be 1 to 16384 points a side"));
        }

        let mut controls = ControlSet::new(host);
        let mut parts = Vec::new();
        let mut bindings = Vec::new();
        let mut issues = Vec::new();
        let (mut right, mut bottom) = (0.0f32, 0.0f32);
        for item in items {
            let bounds = map_rect(&item, options.origin.0, options.origin.1, options.scale);
            right = right.max(bounds.x + bounds.w);
            bottom = bottom.max(bounds.y + bounds.h);
            let index = if item.parameter.is_empty() {
                issues.push(RowIssue { id: item.id.clone(), parameter: String::new(), kind: IssueKind::NoParameter });
                continue;
            } else if let Some(i) = info.params.iter().position(|p| p.id == item.parameter) {
                i
            } else {
                issues.push(RowIssue {
                    id: item.id.clone(),
                    parameter: item.parameter.clone(),
                    kind: IssueKind::UnknownParameter,
                });
                continue;
            };
            let param: &'static openplugin_core::ParamInfo = &info.params[index];

            let font = item.font_size * options.scale;
            let lines = if matches!(param.unit, Unit::Toggle | Unit::Choice) { 1.0 } else { 2.0 };
            let strip = font * LINE * lines;
            let body_h = (bounds.h - strip).max(bounds.h * 0.5);
            let mut body = Rect::new(bounds.x, bounds.y, bounds.w, body_h);

            let (look, control) = match param.unit {
                Unit::Toggle => (Look::Switch, Control::toggle(rect_area(body), index, param)),
                Unit::Choice => {
                    let vertical = body.h > body.w;
                    (Look::Keys { vertical }, Control::key_bank(rect_area(body), index, param, vertical))
                }
                _ if item.height >= 2.0 * item.width => (Look::FaderV, Control::fader_v(rect_area(body), index, param)),
                _ if item.width >= 2.0 * item.height => (Look::FaderH, Control::fader_h(rect_area(body), index, param)),
                _ => {
                    let d = body.w.min(body.h);
                    body = Rect::new(body.x + (body.w - d) * 0.5, body.y, d, d);
                    let c = body.center();
                    (Look::Knob, Control::knob(Area::circle(c.x, c.y, d * 0.5), index, param))
                }
            };
            let control_id = controls.add(control);
            bindings.push(Binding { row: item.id.clone(), param: index, control: control_id, look });
            let label = if item.label.is_empty() { param.name.to_string() } else { item.label.clone() };
            let colour = GlColor::from_hex(&item.colour).unwrap_or(GlColor(0xff697170));
            parts.push(Part { item, control: control_id, look, bounds, body, colour, label, font });
        }

        for issue in &issues {
            log::warn!("{}: {issue}", info.name);
        }
        if options.strict && !issues.is_empty() {
            return Err(DesignEditorError::Unbound(issues));
        }

        let size = options.size.unwrap_or_else(|| {
            let side = |v: f32| (v + MARGIN).ceil().clamp(MIN_SIDE as f32, 16384.0) as u32;
            (side(right), side(bottom))
        });
        let mut hasher = DefaultHasher::new();
        (options.plate.as_str(), size).hash(&mut hasher);
        for p in &parts {
            (p.label.as_str(), p.bounds.x.to_bits(), p.bounds.y.to_bits(), p.font.to_bits()).hash(&mut hasher);
            (p.body.w.to_bits(), p.body.h.to_bits()).hash(&mut hasher);
        }
        Ok(Self {
            info,
            controls,
            parts,
            bindings,
            issues,
            plate,
            size,
            canvas: SceneCanvas::new(size.0 as f32, size.1 as f32),
            layer_key: hasher.finish(),
            keys_active: true,
            damage: DamageTracker::new(),
        })
    }

    /// The controls, with their hit areas and current values.
    /// What a double-click on a knob or fader does (see [`crate::hit::DoubleClick`]).
    pub fn set_double_click(&mut self, action: crate::hit::DoubleClick) {
        self.controls.set_double_click(action);
    }

    pub fn controls(&self) -> &ControlSet {
        &self.controls
    }

    /// The bound rows, in file order.
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// The rows bound to no parameter. They have no control and are not drawn.
    pub fn issues(&self) -> &[RowIssue] {
        &self.issues
    }

    /// The plugin this editor shows.
    pub fn info(&self) -> &'static PluginInfo {
        self.info
    }

    /// Mark the rows whose control changed since the last time.
    fn collect_damage(&mut self) {
        for id in self.controls.take_changed() {
            if let Some(part) = self.parts.iter().find(|p| p.control == id) {
                self.damage.add(part.reach());
            }
        }
    }

    /// The control bound to the first row on parameter `id`.
    pub fn control_for(&self, id: &str) -> Option<&Control> {
        let index = self.info.params.iter().position(|p| p.id == id)?;
        self.controls.for_param(index)
    }
}

/// The focus ring: 4 points out of the control, 2 wide.
const RING_OUT: f32 = 4.0;
const RING_WIDTH: f32 = 2.0;
const RING_COLOUR: GlColor = GlColor(0xff5cc8ff);

impl Part {
    /// Everything this row can change on screen: the row, and the ring around the control
    /// and the light and shadow a lit knob or a fader cap puts past it.
    fn reach(&self) -> DamageRect {
        let side = self.body.w.min(self.body.h);
        let m = (RING_OUT + RING_WIDTH + 2.0).max(side * 0.3);
        let r = self.bounds;
        DamageRect::new(r.x, r.y, r.w, r.h).union(&DamageRect::new(self.body.x, self.body.y, self.body.w, self.body.h)).expanded(m)
    }

    fn text_y(&self, line: f32) -> f32 {
        self.bounds.y + self.body.h + self.font * LINE * line
    }

    fn paint_static(&self, scene: &mut Scene, plate: &design::Faceplate) {
        let x = self.bounds.x + self.bounds.w * 0.5;
        scene.text(Point::new(x, self.text_y(0.85)), self.label.clone(), GlColor(plate.text), self.font, true);
    }

    fn paint(&self, dl: &mut DrawList, controls: &ControlSet, plate: &design::Faceplate) {
        let Some(c) = controls.get(self.control) else { return };
        let code = style_code(&self.item);
        let mut scene = Scene::new();
        match self.look {
            Look::Knob => {
                let m = self.body.center();
                draw_knob_code(dl, m.x, m.y, self.body.w * 0.5, c.normalized(), "", "", &self.item.style, sdk_colour(self.colour));
            }
            Look::FaderV | Look::FaderH => {
                let panel = GlColor(dl.floor.unwrap_or(plate.bg_top));
                let vertical = self.look == Look::FaderV;
                meters::draw_fader_on(&mut scene, self.body, c.normalized(), fader_cap(code), self.colour, vertical, true, panel);
            }
            Look::Switch => switches::draw_toggle(&mut scene, self.body, c.is_on(), toggle_style(code), self.colour),
            Look::Keys { vertical } => {
                let names: Vec<String> = (0..c.key_count())
                    .map(|i| match c.info.choices.get(i) {
                        Some(name) => (*name).to_string(),
                        None => format!("{}", i + 1),
                    })
                    .collect();
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                let style = push_style(code);
                if vertical {
                    let n = names.len() as f32;
                    let gap = 4.0;
                    let h = (self.body.h - gap * (n - 1.0)) / n;
                    let cell = |i: usize| Rect::new(self.body.x, self.body.y + i as f32 * (h + gap), self.body.w, h);
                    for lit in [false, true] {
                        for (i, name) in names.iter().enumerate() {
                            if (i == c.selected_key()) == lit {
                                switches::draw_push(&mut scene, cell(i), name, style, self.colour, lit, lit, true);
                            }
                        }
                    }
                } else {
                    switches::draw_key_bank(&mut scene, self.body, &names, c.selected_key() as i32, style, self.colour, true);
                }
            }
        }
        if matches!(self.look, Look::Knob | Look::FaderV | Look::FaderH) {
            let x = self.bounds.x + self.bounds.w * 0.5;
            scene.text(
                Point::new(x, self.text_y(1.85)),
                c.info.format(c.value()),
                GlColor(plate.text_mid),
                self.font * 0.9,
                true,
            );
        }
        dl.append(scene);
    }
}

impl Editor for DesignEditor {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn param_changed(&mut self, index: usize, plain: f32) {
        self.controls.param_changed(index, plain);
    }

    fn damage(&mut self) -> Option<Vec<DamageRect>> {
        self.collect_damage();
        self.damage.pending().map(<[DamageRect]>::to_vec)
    }

    fn render(&mut self, scale: f32) -> Frame<'_> {
        self.collect_damage();
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let (parts, controls, plate) = (&self.parts, &self.controls, self.plate);
        let ring = self.keys_active;
        // The plate and the printed labels never move: one layer, rasterised once per scale.
        let layer = |dl: &mut DrawList| {
            draw_faceplate_code(dl, w, h, plate.code);
            let mut legend = Scene::new();
            for p in parts {
                p.paint_static(&mut legend, plate);
            }
            dl.append(legend);
        };
        let damage = self.damage.pending();
        let frame = self.canvas.frame_damaged(scale, self.layer_key, layer, |dl| {
            for p in parts {
                p.paint(dl, controls, plate);
            }
            if let Some(c) = controls.focused().and_then(|id| controls.get(id)).filter(|_| ring) {
                let (x, y, w, h) = c.area.bounds();
                let outline = Rect::new(x, y, w, h).expanded(RING_OUT);
                let mut scene = Scene::new();
                match c.kind {
                    ControlKind::Knob => scene.stroke_ellipse(outline, RING_COLOUR, RING_WIDTH),
                    _ => scene.stroke_round(outline, 6.0, RING_COLOUR, RING_WIDTH),
                }
                dl.append(scene);
            }
        }, damage);
        self.damage.clear();
        frame
    }

    fn modifiers_changed(&mut self, modifiers: Modifiers) {
        self.controls.set_modifiers(modifiers);
    }

    fn accessibility_nodes(&self) -> Vec<openplugin_core::editor::AccessNode> {
        self.controls.accessibility_nodes()
    }

    fn accessibility_action(&mut self, node: usize, action: openplugin_core::editor::AccessAction) -> bool {
        self.controls.accessibility_action(node, action)
    }

    fn accessibility_focus(&mut self, node: usize) -> bool {
        self.controls.accessibility_focus(node)
    }

    fn wants_keys(&self) -> bool {
        self.controls.wants_keys()
    }

    fn key_down(&mut self, key: Key, modifiers: Modifiers) -> bool {
        self.controls.key_down(key, modifiers)
    }

    fn key_up(&mut self, key: Key, modifiers: Modifiers) -> bool {
        self.controls.key_up(key, modifiers)
    }

    fn focus_changed(&mut self, focused: bool) {
        self.keys_active = focused;
        // The ring shows only while the view has the keyboard.
        if let Some(rect) = self.controls.focused().and_then(|id| self.parts.iter().find(|p| p.control == id)).map(Part::reach) {
            self.damage.add(rect);
        }
        if !focused {
            self.controls.release_keys();
        }
    }

    fn mouse_down(&mut self, x: f32, y: f32, clicks: u32) {
        self.controls.mouse_down(x, y, clicks);
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        self.controls.mouse_move(x, y);
    }

    fn mouse_up(&mut self, x: f32, y: f32) {
        self.controls.mouse_up(x, y);
    }

    fn wheel(&mut self, x: f32, y: f32, dy: f32) {
        self.controls.wheel(x, y, dy);
    }

    fn take_text_edit(&mut self) -> Option<TextEdit> {
        self.controls.take_text_edit()
    }

    fn text_edit_commit(&mut self, text: &str) -> bool {
        self.controls.text_edit_commit(text)
    }

    fn text_edit_cancel(&mut self) {
        self.controls.text_edit_cancel();
    }

    fn tooltips(&self) -> Vec<Tooltip> {
        self.controls.tooltips()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hit::ControlKind;
    use openplugin_core::{Category, Gesture, ParamInfo};
    use std::cell::RefCell;
    use std::rc::Rc;

    static PARAMS: [ParamInfo; 4] = [
        ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
        ParamInfo::new("warm", "Warm", 0.0, 1.0, 0.0, Unit::Toggle),
        ParamInfo::new("mode", "Mode", 0.0, 2.0, 0.0, Unit::Choice).with_choices(&["Clean", "Warm", "Hot"]),
        ParamInfo::new("drive", "Drive", 0.0, 100.0, 0.0, Unit::Percent),
    ];

    static INFO: PluginInfo = PluginInfo {
        id: "test.design",
        name: "Design Test",
        vendor: "test",
        url: "",
        email: "",
        version: "0",
        category: Category::Utility,
        params: &PARAMS,
        is_instrument: false,
        has_editor: true,
    };

    const HEADER: &str = "GoodLookinUI design version 1\nid,parameter,label,style,colour,x,y,width,height,font size\n";

    fn csv(rows: &[&str]) -> String {
        let mut s = HEADER.to_string();
        for r in rows {
            s.push_str(r);
            s.push('\n');
        }
        s
    }

    fn full() -> String {
        csv(&[
            "gain,gain,Gain,Brit,#CE9435,10,10,70,76,10",
            "warm,warm,Warm,Lvr,#E08030,100,10,40,76,10",
            "mode,mode,Mode,Sq,#4090D0,10,100,120,40,10",
            "drive,drive,,Rd,#80C0A0,150,10,40,150,10",
        ])
    }

    type Log = Rc<RefCell<Vec<Gesture>>>;

    fn rig(text: &str) -> (DesignEditor, Log) {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let sink = log.clone();
        let host = EditorHost::new(move |g| sink.borrow_mut().push(g));
        let editor = DesignEditor::try_new(text, &INFO, host, &DesignOptions::default()).unwrap();
        (editor, log)
    }

    fn take(log: &Log) -> Vec<Gesture> {
        std::mem::take(&mut *log.borrow_mut())
    }

    fn centre(e: &DesignEditor, id: &str) -> (f32, f32) {
        let b = e.bindings().iter().find(|b| b.row == id).unwrap();
        e.controls().get(b.control).unwrap().area.center()
    }

    #[test]
    fn rows_bind_to_parameters_and_the_kind_follows_the_parameter() {
        let (e, _) = rig(&full());
        assert!(e.issues().is_empty());
        let got: Vec<(&str, usize, Look)> = e.bindings().iter().map(|b| (b.row.as_str(), b.param, b.look)).collect();
        assert_eq!(
            got,
            vec![
                ("gain", 0, Look::Knob),
                ("warm", 1, Look::Switch),
                ("mode", 2, Look::Keys { vertical: false }),
                ("drive", 3, Look::FaderV),
            ]
        );
        let kinds: Vec<ControlKind> = e.controls().iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            vec![ControlKind::Knob, ControlKind::Toggle, ControlKind::KeyBank { vertical: false }, ControlKind::FaderV]
        );
        // A continuous parameter on a wide row is a horizontal fader.
        let (e, _) = rig(&csv(&["d,drive,Drive,Brit,#CE9435,0,0,160,40,10"]));
        assert_eq!(e.bindings()[0].look, Look::FaderH);
        // A choice on a tall row is a vertical key bank.
        let (e, _) = rig(&csv(&["m,mode,Mode,Sq,#CE9435,0,0,40,120,10"]));
        assert_eq!(e.bindings()[0].look, Look::Keys { vertical: true });
    }

    #[test]
    fn hit_areas_follow_map_rect_and_the_text_strip_is_left_out() {
        let (e, _) = rig(&full());
        // Knob row 70 x 76 at (10, 10), font 10: strip 24, so a 52-point body, square, centred.
        let knob = e.controls().get(0).unwrap().area;
        assert_eq!(knob, Area::circle(45.0, 36.0, 26.0));
        assert!(knob.contains(45.0, 36.0) && !knob.contains(45.0, 70.0), "the label strip is not the knob");
        let keys = e.controls().get(2).unwrap().area;
        assert_eq!(keys, Area::rect(10.0, 100.0, 120.0, 28.0));
        // The size is the furthest edge plus the margin.
        assert_eq!(e.size(), (202, 172));
        // Scale and origin go through map_rect.
        let log = EditorHost::detached();
        let items = goodlookindri::design::read_design(&full()).unwrap();
        let scaled = DesignEditor::from_items(items, &INFO, log, &DesignOptions::default().scale(2.0).origin(5.0, 0.0)).unwrap();
        let Area::Circle { cx, .. } = scaled.controls().get(0).unwrap().area else { panic!() };
        assert_eq!(cx, 5.0 + 2.0 * 45.0);
    }

    #[test]
    fn a_click_and_a_drag_become_the_right_gestures() {
        let (mut e, log) = rig(&full());
        // Knob: a quarter of 200 points up is a quarter of 48 dB.
        let (x, y) = centre(&e, "gain");
        e.mouse_down(x, y, 1);
        e.mouse_move(x, y - 50.0);
        e.mouse_up(x, y - 50.0);
        assert_eq!(take(&log), vec![Gesture::Begin(0), Gesture::Perform(0, 12.0), Gesture::End(0)]);
        assert_eq!(e.control_for("gain").unwrap().value(), 12.0);
        // Switch: one click is a full gesture.
        let (x, y) = centre(&e, "warm");
        e.mouse_down(x, y, 1);
        e.mouse_up(x, y);
        assert_eq!(take(&log), vec![Gesture::Begin(1), Gesture::Perform(1, 1.0), Gesture::End(1)]);
        // Key bank: the last key, then glide to the first.
        e.mouse_down(125.0, 110.0, 1);
        e.mouse_move(11.0, 110.0);
        e.mouse_up(11.0, 110.0);
        assert_eq!(
            take(&log),
            vec![Gesture::Begin(2), Gesture::Perform(2, 2.0), Gesture::Perform(2, 0.0), Gesture::End(2)]
        );
        // Fader: 1:1 with its own height, so a quarter of it up is a quarter of the range.
        let h = e.controls().get(3).unwrap().area.bounds().3;
        let (x, y) = centre(&e, "drive");
        e.mouse_down(x, y, 1);
        e.mouse_move(x, y - h * 0.25);
        e.mouse_up(x, y - h * 0.25);
        let g = take(&log);
        assert_eq!((g[0], g[2]), (Gesture::Begin(3), Gesture::End(3)));
        let Gesture::Perform(3, v) = g[1] else { panic!("{g:?}") };
        assert!((v - 25.0).abs() < 1e-3, "{v}");
        // Outside every control: nothing. On the text strip: nothing.
        e.mouse_down(190.0, 165.0, 1);
        e.mouse_up(190.0, 165.0);
        e.mouse_down(45.0, 80.0, 1);
        e.mouse_up(45.0, 80.0);
        assert!(take(&log).is_empty());
        // The host's value reaches the control without a gesture.
        e.param_changed(0, -6.0);
        assert_eq!(e.control_for("gain").unwrap().value(), -6.0);
        assert!(take(&log).is_empty());
        // Double-click opens the text field; typing commits through parse.
        let (x, y) = centre(&e, "gain");
        e.mouse_down(x, y, 2);
        e.mouse_up(x, y);
        assert!(take(&log).is_empty());
        let edit = e.take_text_edit().expect("text field request");
        assert_eq!((edit.text.as_str(), edit.label.as_str()), ("-6.0 dB", "Gain"));
        assert!(!e.text_edit_commit("nonsense"));
        assert!(e.text_edit_commit("3 dB"));
        assert_eq!(take(&log), vec![Gesture::Begin(0), Gesture::Perform(0, 3.0), Gesture::End(0)]);
        assert_eq!(e.tooltips().len(), e.controls().len());
        assert_eq!(e.tooltips()[0].text, "Gain: 3.0 dB");
        // Option-double-click resets.
        e.modifiers_changed(Modifiers { alt: true, ..Modifiers::NONE });
        e.mouse_down(x, y, 2);
        e.mouse_up(x, y);
        assert_eq!(take(&log), vec![Gesture::Begin(0), Gesture::Perform(0, 0.0), Gesture::End(0)]);
    }

    #[test]
    fn rows_bound_to_nothing_are_reported_and_not_controls() {
        let text = csv(&[
            "gain,gain,Gain,Brit,#CE9435,10,10,70,76,10",
            "ghost,nope,Ghost,Brit,#CE9435,100,10,70,76,10",
            "label,,Title,Brit,#CE9435,10,100,70,76,10",
        ]);
        let (e, _) = rig(&text);
        assert_eq!(e.controls().len(), 1);
        assert_eq!(e.bindings().len(), 1);
        assert_eq!(
            e.issues(),
            &[
                RowIssue { id: "ghost".into(), parameter: "nope".into(), kind: IssueKind::UnknownParameter },
                RowIssue { id: "label".into(), parameter: String::new(), kind: IssueKind::NoParameter },
            ]
        );
        assert!(e.issues()[0].to_string().contains("nope"));
        // The reported rows still count for the size, so a design lays out as drawn.
        assert_eq!(e.size().0, 182);
        // Strict mode turns them into an error.
        let err = DesignEditor::try_new(&text, &INFO, EditorHost::detached(), &DesignOptions::default().strict(true))
            .err()
            .unwrap();
        let DesignEditorError::Unbound(rows) = &err else { panic!("{err}") };
        assert_eq!(rows.len(), 2);
        assert!(err.to_string().contains("ghost"));
    }

    #[test]
    fn malformed_designs_and_options_are_refused() {
        let build = |text: &str| DesignEditor::try_new(text, &INFO, EditorHost::detached(), &DesignOptions::default()).err();
        let bad = [
            String::new(),
            "not a design\n".to_string(),
            "GoodLookinUI design version 1\nid,wrong\n".to_string(),
            csv(&["gain,gain,Gain,Brit,#CE9435,10,10,70"]),
            csv(&["gain,gain,Gain,Brit,#CE9435,10,10,70,76,x"]),
            csv(&["gain,gain,Gain,NoSuchStyle,#CE9435,10,10,70,76,10"]),
            csv(&["gain,gain,Gain,Brit,red,10,10,70,76,10"]),
            csv(&["gain,gain,Gain,Brit,#CE9435,10,10,5,5,10"]),
            csv(&["gain,gain,\"Gain,Brit,#CE9435,10,10,70,76,10"]),
            csv(&["a,gain,Gain,Brit,#CE9435,10,10,70,76,10", "a,warm,Warm,Brit,#CE9435,10,10,70,76,10"]),
        ];
        for text in &bad {
            assert!(matches!(build(text), Some(DesignEditorError::Design(_))), "{text:?}");
            assert!(DesignEditor::from_csv(text, &INFO, EditorHost::detached()).is_none());
        }
        let err = build(&bad[1]).unwrap();
        assert!(err.to_string().starts_with("bad design"));
        let ok = full();
        let with = |o: DesignOptions| DesignEditor::try_new(&ok, &INFO, EditorHost::detached(), &o).err();
        assert!(matches!(with(DesignOptions::default().plate("Nope")), Some(DesignEditorError::UnknownPlate(_))));
        assert!(matches!(with(DesignOptions::default().scale(0.0)), Some(DesignEditorError::Options(_))));
        assert!(matches!(with(DesignOptions::default().scale(f32::NAN)), Some(DesignEditorError::Options(_))));
        assert!(matches!(with(DesignOptions::default().size(0, 10)), Some(DesignEditorError::Options(_))));
        assert!(DesignEditor::from_csv(&ok, &INFO, EditorHost::detached()).is_some());
    }

    #[test]
    fn crlf_and_the_marks_column_load() {
        let text = "GoodLookinUI design version 1\r\nid,parameter,label,style,colour,x,y,width,height,font size,marks\r\n\
                    gain,gain,Gain,Brit,#CE9435,10,10,70,76,10,dark\r\n";
        let (e, _) = rig(text);
        assert_eq!(e.bindings().len(), 1);
    }

    #[test]
    fn the_frame_has_the_size_and_is_not_blank() {
        let (mut e, _) = rig(&full());
        let (w, h) = e.size();
        let first = {
            let f = e.render(2.0);
            assert_eq!((f.width, f.height), (w as usize * 2, h as usize * 2));
            assert_eq!(f.pixels.len(), f.width * f.height);
            f.pixels.to_vec()
        };
        let mut distinct: Vec<u32> = first.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert!(distinct.len() > 200, "only {} colours", distinct.len());
        // The plate is cached: a second frame is the same pixels.
        assert!(e.render(2.0).pixels == first.as_slice());
        // A control paints over it where it sits, and only there.
        e.param_changed(0, 20.0);
        let moved = e.render(2.0).pixels.to_vec();
        assert_ne!(moved, first);
        let at = |px: &[u32], x: usize, y: usize| px[y * w as usize * 2 + x];
        assert_eq!(at(&moved, 2, h as usize * 2 - 2), at(&first, 2, h as usize * 2 - 2));
        // Another scale is another frame size.
        let f = e.render(1.0);
        assert_eq!((f.width, f.height), (w as usize, h as usize));
    }

    #[test]
    fn every_look_and_every_plate_renders() {
        let rows = csv(&[
            "a,gain,Gain,Chk,#CE9435,0,0,70,76,10",
            "b,warm,Warm,Mini,#E08030,80,0,40,76,10",
            "c,mode,Mode,Cyb,#4090D0,130,0,40,120,10",
            "d,drive,Drive,Brit,#80C0A0,180,0,160,40,10",
            "e,drive,Drive,lit_Brit,#80C0A0,0,100,40,120,10",
        ]);
        for plate in ["Navy", "Walnut", "Cream"] {
            let items = goodlookindri::design::read_design(&rows).unwrap();
            let mut e = DesignEditor::from_items(items, &INFO, EditorHost::detached(), &DesignOptions::default().plate(plate)).unwrap();
            let f = e.render(1.0);
            assert!(f.pixels.iter().any(|p| *p != f.pixels[0]));
        }
    }

    fn key(e: &mut DesignEditor, k: Key) -> bool {
        let down = e.key_down(k, Modifiers::NONE);
        e.key_up(k, Modifiers::NONE);
        down
    }

    #[test]
    fn keys_reach_the_controls_and_a_focus_ring_is_drawn() {
        let (mut e, log) = rig(&full());
        assert!(e.wants_keys());
        let plain = e.render(2.0).pixels.to_vec();
        // Nothing is focused: arrows and Escape belong to the host.
        assert!(!key(&mut e, Key::Up) && !key(&mut e, Key::Escape) && !key(&mut e, Key::Char('a')));
        assert!(take(&log).is_empty());
        // Tab walks the rows in file order: gain, warm, mode, drive; then leaves.
        let mut order = Vec::new();
        for _ in 0..4 {
            assert!(key(&mut e, Key::Tab));
            order.push(e.controls().focused());
        }
        assert_eq!(order, vec![Some(0), Some(1), Some(2), Some(3)]);
        assert!(!key(&mut e, Key::Tab) && e.controls().focused().is_none(), "past the last row Tab goes to the host");
        // The ring appears on the focused control, and goes with focus.
        assert!(key(&mut e, Key::Tab));
        let ringed = e.render(2.0).pixels.to_vec();
        assert_ne!(ringed, plain, "a ring is drawn");
        e.focus_changed(false);
        assert!(e.render(2.0).pixels == plain.as_slice(), "no ring while the view lacks the keyboard");
        e.focus_changed(true);
        assert!(e.render(2.0).pixels == ringed.as_slice());
        // Up steps the focused knob by a hundredth of its range, as a whole gesture.
        assert!(key(&mut e, Key::Up));
        let g = take(&log);
        assert_eq!(g.len(), 3, "{g:?}");
        let Gesture::Perform(0, v) = g[1] else { panic!("{g:?}") };
        assert!((v - 0.48).abs() < 1e-3, "{v}");
        // Space flips the switch, Delete resets the knob, Escape clears focus.
        assert!(key(&mut e, Key::Tab));
        assert!(key(&mut e, Key::Space));
        assert_eq!(take(&log), vec![Gesture::Begin(1), Gesture::Perform(1, 1.0), Gesture::End(1)]);
        e.mouse_down(45.0, 36.0, 1);
        e.mouse_up(45.0, 36.0);
        take(&log);
        assert!(key(&mut e, Key::Delete));
        assert_eq!(take(&log), vec![Gesture::Begin(0), Gesture::Perform(0, 0.0), Gesture::End(0)]);
        assert!(key(&mut e, Key::Escape) && e.controls().focused().is_none());
        // A key up of a key the editor did not take is the host's.
        assert!(!e.key_up(Key::Char('x'), Modifiers::NONE));
        // A design with nothing to control wants no keys.
        let (empty, _) = rig(&csv(&["ghost,nope,Ghost,Brit,#CE9435,10,10,70,76,10"]));
        assert!(!empty.wants_keys());
    }

    type Step = Box<dyn Fn(&mut DesignEditor)>;

    /// Two editors fed the same events: `part` repaints what it reports, `whole` everything.
    fn same_frames(scale: f32, steps: &[(&str, Step)]) -> (usize, usize) {
        let (mut part, _) = rig(&full());
        let (mut whole, _) = rig(&full());
        let (mut partial, mut idle) = (0, 0);
        part.render(scale);
        whole.render(scale);
        for (what, step) in steps {
            step(&mut part);
            step(&mut whole);
            let damage = part.damage();
            match &damage {
                Some(r) if r.is_empty() => idle += 1,
                Some(_) => partial += 1,
                None => {}
            }
            whole.damage.add_all();
            let expected = whole.render(scale).pixels.to_vec();
            let got = part.render(scale);
            let bad = got.pixels.iter().zip(&expected).filter(|(a, b)| a != b).count();
            assert_eq!(bad, 0, "{what}: {bad} pixels differ from a full repaint at scale {scale} (damage {damage:?})");
        }
        (partial, idle)
    }

    #[test]
    fn damage_only_frames_equal_full_repaints() {
        let drag = |x: f32, _from: f32, to: f32| move |e: &mut DesignEditor| e.mouse_move(x, to);
        let knob = centre(&rig(&full()).0, "gain");
        let fader = centre(&rig(&full()).0, "drive");
        let lever = centre(&rig(&full()).0, "warm");
        let keys = (30.0f32, 110.0f32);
        let steps: Vec<(&str, Step)> = vec![
            ("idle", Box::new(|_| {})),
            ("hover the knob", Box::new(move |e| e.mouse_move(knob.0, knob.1))),
            ("press the knob", Box::new(move |e| e.mouse_down(knob.0, knob.1, 1))),
            ("drag the knob 1", Box::new(drag(knob.0, knob.1, knob.1 - 7.0))),
            ("drag the knob 2", Box::new(drag(knob.0, knob.1, knob.1 - 30.0))),
            ("drag the knob 3", Box::new(drag(knob.0, knob.1, knob.1 + 60.0))),
            ("release the knob", Box::new(move |e| e.mouse_up(knob.0, knob.1 + 60.0))),
            ("click the lever", Box::new(move |e| {
                e.mouse_down(lever.0, lever.1, 1);
                e.mouse_up(lever.0, lever.1);
            })),
            ("a key", Box::new(move |e| {
                e.mouse_down(keys.0 + 60.0, keys.1, 1);
                e.mouse_up(keys.0 + 60.0, keys.1);
            })),
            ("fader", Box::new(move |e| {
                e.mouse_down(fader.0, fader.1, 1);
                e.mouse_move(fader.0, fader.1 - 40.0);
                e.mouse_up(fader.0, fader.1 - 40.0);
            })),
            ("host value", Box::new(|e| e.param_changed(0, 17.5))),
            ("host value again", Box::new(|e| e.param_changed(0, 17.5))),
            ("host value, tone", Box::new(|e| e.param_changed(3, 1.0))),
            ("focus a row", Box::new(|e| {
                e.key_down(Key::Tab, Modifiers::NONE);
            })),
            ("step it", Box::new(|e| {
                e.key_down(Key::PageUp, Modifiers::NONE);
            })),
            ("tab on", Box::new(|e| {
                e.key_down(Key::Tab, Modifiers::NONE);
                e.key_down(Key::Tab, Modifiers::NONE);
            })),
            ("lose the keyboard", Box::new(|e| e.focus_changed(false))),
            ("regain the keyboard", Box::new(|e| e.focus_changed(true))),
            ("clear focus", Box::new(|e| {
                e.key_down(Key::Escape, Modifiers::NONE);
            })),
            ("idle again", Box::new(|_| {})),
        ];
        for scale in [2.0, 1.0, 1.5] {
            let (partial, idle) = same_frames(scale, &steps);
            assert!(partial >= 12 && idle >= 2, "scale {scale}: {partial} partial, {idle} idle frames");
        }
    }
}
