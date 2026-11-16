//! The format-neutral editor contract.
//!
//! An editor is the plugin's own window content. The SDK hosts it in every
//! format: CLAP (`clap.gui`), LV2 (`ui:CocoaUI`), and the VST3, Audio Unit and
//! AAX shells (`op_editor_*`). The plugin writes one [`Editor`] and one
//! [`EditorFactory`]; the format bridges do the rest.
//!
//! The rules are the same in every format:
//!
//! * Every [`Editor`] call is on the main thread.
//! * The editor never writes to the engine. It reports a change through
//!   [`EditorHost`] as a gesture ([`Gesture`]) in plain units, and the host
//!   brings the value back to the engine as an ordinary parameter change.
//! * Values that change elsewhere (automation, a preset the host loads) reach
//!   the editor through [`Editor::param_changed`].
//! * An editor exists before it has an engine, and the engine it shows can be
//!   replaced while it is open (a shell recreates its engine when the host
//!   reconfigures it). [`Editor::bind`] hands it the [`EditorState`] that the
//!   engine published through [`crate::Plugin::editor_state`], or `None`.
//!
//! Size: an editor is fixed-size unless it says otherwise. [`Editor::size_range`]
//! gives the sizes it can take ([`SizeRange`]) and [`Editor::resize`] is how a
//! host moves it to one of them; [`Editor::size`] is always the current size.
//! An editor asks its host for a size with [`EditorHost::request_resize`].
//!
//! Pixels are 32-bit words `0x00RRGGBB`, rows top to bottom, which is what
//! the macOS view in `openplugin-dri` blits.

use std::any::Any;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

/// What an engine shares with its editor: meters, an analyser ring, display
/// choices that survive a reopen. The plugin defines the type and the editor
/// downcasts it (`state.downcast::<MyShared>()`).
pub type EditorState = Arc<dyn Any + Send + Sync>;

/// One editor gesture on one parameter, by descriptor index, in plain units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    Begin(u32),
    Perform(u32, f32),
    End(u32),
}

type ContextMenuFn = Rc<dyn Fn(u32, i32, i32) -> bool>;

/// Where an editor sends its gestures. Created by the format bridge; cheap to
/// clone, main thread only.
#[derive(Clone)]
pub struct EditorHost {
    sink: Rc<dyn Fn(Gesture)>,
    resize: Option<Rc<dyn Fn(u32, u32) -> bool>>,
    context_menu: Option<ContextMenuFn>,
}

impl EditorHost {
    pub fn new(sink: impl Fn(Gesture) + 'static) -> Self {
        Self { sink: Rc::new(sink), resize: None, context_menu: None }
    }

    /// The same host, with a way to ask the format's host for a new editor
    /// size. The handler returns whether the host agreed; the host then
    /// resizes the editor through its own calls (`set_size`, `ui:resize`).
    pub fn with_resize(mut self, request: impl Fn(u32, u32) -> bool + 'static) -> Self {
        self.resize = Some(Rc::new(request));
        self
    }

    /// The same host, with a way to ask the format's host for its context
    /// menu on a parameter (automation, MIDI learn). The handler gets the
    /// descriptor index and the position in editor points, origin top left,
    /// and returns whether the host showed a menu.
    pub fn with_context_menu(mut self, show: impl Fn(u32, i32, i32) -> bool + 'static) -> Self {
        self.context_menu = Some(Rc::new(show));
        self
    }

    /// Ask the host to show its context menu for parameter `param_index` at
    /// (`x`, `y`) points in the editor. True when the host showed one; false
    /// when the format or host has none (the editor then shows the SDK's own
    /// small menu, see [`Editor::take_param_menu`]).
    pub fn context_menu(&self, param_index: usize, x: f32, y: f32) -> bool {
        if !x.is_finite() || !y.is_finite() || param_index > u32::MAX as usize {
            return false;
        }
        match &self.context_menu {
            Some(show) => show(param_index as u32, x.round() as i32, y.round() as i32),
            None => false,
        }
    }

    /// Ask the host to give the editor `width` x `height` points. The size is
    /// brought into the editor's [`SizeRange`] first. Returns whether the host
    /// agreed; a `false` leaves the editor as it is. The editor is resized
    /// later, through [`Editor::resize`], and not inside this call.
    pub fn request_resize(&self, range: &SizeRange, width: u32, height: u32) -> bool {
        let (w, h) = range.clamp(width, height);
        match &self.resize {
            Some(request) => request(w, h),
            None => false,
        }
    }

    /// A host that drops every gesture (previews, tests, a standalone window
    /// that applies values itself).
    pub fn detached() -> Self {
        Self::new(|_| {})
    }

    pub fn send(&self, gesture: Gesture) {
        (self.sink)(gesture);
    }

    /// The user grabbed a control.
    pub fn begin(&self, index: usize) {
        self.send(Gesture::Begin(index as u32));
    }

    /// A new plain value while the control is held.
    pub fn perform(&self, index: usize, plain: f32) {
        if plain.is_finite() {
            self.send(Gesture::Perform(index as u32, plain));
        }
    }

    /// The user let go.
    pub fn end(&self, index: usize) {
        self.send(Gesture::End(index as u32));
    }

    /// A complete change in one call: begin, perform, end. For clicks, steps
    /// and preset loads, where there is no drag to hold the gesture open.
    pub fn set(&self, index: usize, plain: f32) {
        self.begin(index);
        self.perform(index, plain);
        self.end(index);
    }
}

/// Which modifier keys are held. A view that knows them reports a change
/// through [`Editor::modifiers_changed`]; a view that does not never calls it
/// and the editor sees none held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub command: bool,
}

impl Modifiers {
    pub const NONE: Self = Self { shift: false, ctrl: false, alt: false, command: false };

    /// The modifiers that ask for fine adjustment: Shift or Command. Control
    /// is left out because a Control-click is a right click on macOS.
    pub fn fine(&self) -> bool {
        self.shift || self.command
    }
}

/// A key an editor can ask for. The view maps the platform's key codes to
/// these; everything else arrives as [`Key::Char`] (the character the key
/// types without modifiers, in lower case) or is not delivered at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Tab,
    /// Return and the keypad Enter.
    Return,
    Escape,
    /// The key that deletes forward (Fn-Delete on a Mac keyboard).
    Delete,
    /// The key above Return, which deletes backward.
    Backspace,
    Space,
    Char(char),
}

/// A rendered editor frame. `pixels.len() == width * height`.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    pub pixels: &'a [u32],
    pub width: usize,
    pub height: usize,
}

/// A rectangle of the editor in points, origin top left. What [`Editor::damage`]
/// reports.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DamageRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl DamageRect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// From an `(x, y, w, h)` tuple.
    pub const fn from_tuple(r: (f32, f32, f32, f32)) -> Self {
        Self { x: r.0, y: r.1, w: r.2, h: r.3 }
    }

    /// The same rectangle grown by `margin` points on every side.
    pub fn expanded(&self, margin: f32) -> Self {
        Self { x: self.x - margin, y: self.y - margin, w: self.w + 2.0 * margin, h: self.h + 2.0 * margin }
    }

    /// The smallest rectangle holding both.
    pub fn union(&self, other: &Self) -> Self {
        let (x0, y0) = (self.x.min(other.x), self.y.min(other.y));
        let (x1, y1) = ((self.x + self.w).max(other.x + other.w), (self.y + self.h).max(other.y + other.h));
        Self { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }

    /// Whether the two share area or touch.
    pub fn touches(&self, other: &Self) -> bool {
        self.x <= other.x + other.w && other.x <= self.x + self.w && self.y <= other.y + other.h && other.y <= self.y + self.h
    }

    /// All four numbers are finite.
    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.w.is_finite() && self.h.is_finite()
    }

    /// No area.
    pub fn is_empty(&self) -> bool {
        !(self.w > 0.0 && self.h > 0.0)
    }
}

/// Collects what an editor has to repaint, for [`Editor::damage`] and
/// [`Editor::render`]. It starts as "everything" (nothing was drawn yet).
/// [`Self::add`] marks a rectangle, [`Self::add_all`] the whole editor, and
/// the editor calls [`Self::clear`] once a frame has drawn what it held.
#[derive(Clone, Debug)]
pub struct DamageTracker {
    everything: bool,
    rects: Vec<DamageRect>,
}

impl Default for DamageTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl DamageTracker {
    /// More rectangles than this become "everything": a long list costs more than it saves.
    const MAX_RECTS: usize = 16;

    pub fn new() -> Self {
        Self { everything: true, rects: Vec::new() }
    }

    /// The whole editor needs repainting.
    pub fn add_all(&mut self) {
        self.everything = true;
        self.rects.clear();
    }

    /// Mark `rect` (points). Empty rectangles are ignored; a non-finite one
    /// marks everything.
    pub fn add(&mut self, rect: DamageRect) {
        if self.everything {
            return;
        }
        if !rect.is_finite() {
            self.add_all();
            return;
        }
        if rect.is_empty() {
            return;
        }
        let mut rect = rect;
        // Fold in every rectangle it touches, so the list stays disjoint.
        while let Some(i) = self.rects.iter().position(|r| r.touches(&rect)) {
            rect = rect.union(&self.rects.swap_remove(i));
        }
        self.rects.push(rect);
        if self.rects.len() > Self::MAX_RECTS {
            self.add_all();
        }
    }

    /// What is marked: `None` for everything, otherwise the rectangles (none
    /// at all when nothing changed).
    pub fn pending(&self) -> Option<&[DamageRect]> {
        if self.everything { None } else { Some(&self.rects) }
    }

    /// Nothing is marked.
    pub fn is_clean(&self) -> bool {
        !self.everything && self.rects.is_empty()
    }

    /// A frame drew everything marked.
    pub fn clear(&mut self) {
        self.everything = false;
        self.rects.clear();
    }
}

/// The sizes an editor can take, in points. [`SizeRange::fixed`] is the
/// default for every editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SizeRange {
    pub min: (u32, u32),
    pub max: (u32, u32),
    /// Width and height of the shape to keep (3, 2 for 3:2), or `None` for any shape.
    pub aspect: Option<(u32, u32)>,
}

impl SizeRange {
    /// One size only.
    pub const fn fixed(width: u32, height: u32) -> Self {
        Self { min: (width, height), max: (width, height), aspect: None }
    }

    /// Any size from `min` to `max`, in any shape.
    pub const fn between(min: (u32, u32), max: (u32, u32)) -> Self {
        Self { min, max, aspect: None }
    }

    /// The same range, keeping the shape `width`:`height`.
    pub const fn with_aspect(mut self, width: u32, height: u32) -> Self {
        self.aspect = Some((width, height));
        self
    }

    /// Bounds in order and at least 1 x 1, whatever was written.
    fn bounds(&self) -> ((u32, u32), (u32, u32)) {
        let min = (self.min.0.max(1), self.min.1.max(1));
        (min, (self.max.0.max(min.0), self.max.1.max(min.1)))
    }

    /// The editor takes one size only.
    pub fn is_fixed(&self) -> bool {
        let (min, max) = self.bounds();
        min == max
    }

    pub fn can_resize_horizontally(&self) -> bool {
        let (min, max) = self.bounds();
        min.0 < max.0
    }

    pub fn can_resize_vertically(&self) -> bool {
        let (min, max) = self.bounds();
        min.1 < max.1
    }

    /// The shape to keep, if both numbers are set.
    pub fn aspect_ratio(&self) -> Option<(u32, u32)> {
        self.aspect.filter(|a| a.0 > 0 && a.1 > 0)
    }

    /// Whether `width` x `height` is inside the bounds (the shape is a hint).
    pub fn contains(&self, width: u32, height: u32) -> bool {
        let (min, max) = self.bounds();
        (min.0..=max.0).contains(&width) && (min.1..=max.1).contains(&height)
    }

    /// The size nearest to a request that the editor can take. Without an
    /// aspect each side is held to its bounds. With one, the result has that
    /// shape (to a point) and the area nearest to the request, inside both
    /// bounds. Clamping a result again changes nothing.
    pub fn clamp(&self, width: u32, height: u32) -> (u32, u32) {
        let (min, max) = self.bounds();
        let Some((aw, ah)) = self.aspect_ratio() else {
            return (width.clamp(min.0, max.0), height.clamp(min.1, max.1));
        };
        let (aw, ah) = (f64::from(aw), f64::from(ah));
        // The size is `s * (aw, ah)`; find `s` for the requested area, inside both bounds.
        let wanted = (f64::from(width.max(1)) * f64::from(height.max(1)) / (aw * ah)).sqrt();
        let low = (f64::from(min.0) / aw).max(f64::from(min.1) / ah);
        let high = (f64::from(max.0) / aw).min(f64::from(max.1) / ah).max(low);
        let s = wanted.clamp(low, high);
        // Width decides; height follows from it, so a result is its own fixed point.
        let w = ((s * aw).round() as u32).clamp(min.0, max.0);
        let h = ((f64::from(w) * ah / aw).round() as u32).clamp(min.1, max.1);
        (w, h)
    }
}

/// What a screen reader is told an accessibility node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessRole {
    /// A value along a range: knob, fader, choice.
    Slider,
    /// Two states: on or off.
    Switch,
    /// A momentary push.
    Button,
}

/// What a screen reader asks a node to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessAction {
    /// One step up (VoiceOver: Control-Option-Up).
    Increment,
    /// One step down.
    Decrement,
    /// Press (VoiceOver: Control-Option-Space): flip a switch, push a button.
    Press,
}

/// One thing a screen reader can find in an editor: a control with a name, a
/// value as text, a role and a place. What [`Editor::accessibility_nodes`]
/// returns.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessNode {
    /// The name read first (the parameter's name).
    pub label: String,
    /// The value as text (`ParamInfo::format`).
    pub value: String,
    pub role: AccessRole,
    /// A [`AccessRole::Switch`] is on.
    pub on: bool,
    /// Where it is, in editor points, origin top left.
    pub rect: DamageRect,
    /// The node has the editor's keyboard focus.
    pub focused: bool,
    /// False for a read-only control: it is listed but takes no actions.
    pub enabled: bool,
}

/// The numbers behind a [`AccessRole::Slider`] node, for a screen reader that
/// reads and sets a value along a range (UI Automation's range value). What
/// [`Editor::accessibility_range`] returns. Plain values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccessRange {
    pub min: f64,
    pub max: f64,
    pub value: f64,
    /// One step: what a screen reader's "increase" adds.
    pub step: f64,
}

/// A small native text field the editor asks the view to open over a control
/// (see [`Editor::take_text_edit`]).
#[derive(Clone, Debug, PartialEq)]
pub struct TextEdit {
    /// The control the field covers, in editor points, origin top left.
    pub rect: DamageRect,
    /// What the field starts with (`ParamInfo::format` of the value).
    pub text: String,
    /// The name a screen reader gives the field (the parameter's name).
    pub label: String,
}

/// A tooltip for a place in the editor (see [`Editor::tooltips`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Tooltip {
    /// Where the pointer must rest, in editor points, origin top left.
    pub rect: DamageRect,
    /// The text shown. The view asks for it again whenever it shows the tip.
    pub text: String,
}

/// A request for an open-file dialog (see [`Editor::take_file_dialog`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDialog {
    /// The editor's own number for this request, handed back with the answer.
    pub tag: u32,
    /// The message shown in the dialog. Empty for none.
    pub title: String,
    /// File name extensions that can be chosen, without the dot, any case.
    /// Empty allows every file.
    pub extensions: Vec<String>,
    /// More than one file may be chosen.
    pub multiple: bool,
}

/// What the user chose in the SDK's own parameter menu (see
/// [`Editor::take_param_menu`]).
#[derive(Clone, Debug, PartialEq)]
pub enum ParamMenuAction {
    /// Set the parameter to its default.
    Reset,
    /// Put the formatted value on the clipboard (the editor returns the text).
    Copy,
    /// Set the parameter from clipboard text, read with `ParamInfo::parse`.
    Paste(String),
    /// Open the text field over the control.
    Type,
}

/// A request for the SDK's own parameter menu, made when the host has none
/// (see [`Editor::take_param_menu`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ParamMenu {
    /// The parameter's descriptor index.
    pub param: u32,
    /// Where the click was, in editor points, origin top left.
    pub x: f32,
    pub y: f32,
    /// The parameter's name, for the menu's title.
    pub label: String,
}

/// The plugin's editor. Main thread only; it is never sent to another thread.
pub trait Editor {
    /// The current size in points (logical pixels). A fixed editor never
    /// changes it; a resizable one changes it in [`Self::resize`].
    fn size(&self) -> (u32, u32);

    /// The sizes this editor can take. The default is the current size alone,
    /// so an editor that does not override this and [`Self::resize`] stays
    /// fixed in every format.
    fn size_range(&self) -> SizeRange {
        let (w, h) = self.size();
        SizeRange::fixed(w, h)
    }

    /// The host gives the editor a new size in points. Called only with a size
    /// inside [`Self::size_range`]. Afterwards [`Self::size`] is that size and
    /// [`Self::render`] draws at it. The default ignores it.
    fn resize(&mut self, _width: u32, _height: u32) {}

    /// Show the engine whose state this is, or no engine. Called after
    /// creation when the engine is known, and again whenever it changes.
    fn bind(&mut self, _state: Option<EditorState>) {}

    /// A parameter value from the host, plain. Only called when it changed.
    fn param_changed(&mut self, index: usize, plain: f32);

    /// Animation step, about 30 times a second. `dt` is in seconds.
    fn tick(&mut self, _dt: f32) {}

    /// Draw the editor at `scale` device pixels per point.
    fn render(&mut self, scale: f32) -> Frame<'_>;

    /// What changed on screen since the last call: `None` for everything (the
    /// default, so an editor that does not override this is redrawn in full),
    /// or the rectangles in points, none at all when nothing changed. The view
    /// calls it after each [`Self::tick`] and input call, and invalidates only
    /// those rectangles. The rectangles must cover every pixel that differs
    /// from the frame last returned by [`Self::render`], anti-aliasing and
    /// shadows included. A view that does not ask still gets correct frames:
    /// [`Self::render`] repaints whatever is pending itself.
    fn damage(&mut self) -> Option<Vec<DamageRect>> {
        None
    }

    /// The held modifier keys changed. Called before the mouse call that
    /// they belong to, never when nothing changed. Optional: a view that does
    /// not report modifiers leaves this uncalled.
    fn modifiers_changed(&mut self, _modifiers: Modifiers) {}

    /// Whether the editor wants key events. A view takes keyboard focus only
    /// while this is true, so an editor that says no (the default) leaves every
    /// key with the host, as before.
    fn wants_keys(&self) -> bool {
        false
    }

    /// A key went down (auto-repeat sends it again). Return true when the
    /// editor used it. A key the editor does not handle must return false: the
    /// view then passes it to the host, which owns transport and shortcuts.
    fn key_down(&mut self, _key: Key, _modifiers: Modifiers) -> bool {
        false
    }

    /// A key went up. Return true when the editor used it, false to pass it
    /// on to the host.
    fn key_up(&mut self, _key: Key, _modifiers: Modifiers) -> bool {
        false
    }

    /// The view gained or lost keyboard focus. On a loss, forget keys held.
    fn focus_changed(&mut self, _focused: bool) {}

    /// Mouse input in points, origin at the top left.
    fn mouse_down(&mut self, _x: f32, _y: f32, _clicks: u32) {}
    fn mouse_move(&mut self, _x: f32, _y: f32) {}
    fn mouse_up(&mut self, _x: f32, _y: f32) {}
    /// Wheel or trackpad scroll; positive `dy` is away from the user.
    fn wheel(&mut self, _x: f32, _y: f32, _dy: f32) {}

    /// What a screen reader sees: one node per control, in reading order, with
    /// the control's place in editor points. The view turns them into
    /// accessibility elements and asks again whenever it needs them, so the
    /// answer is always current. The default is none, which leaves the editor
    /// as one opaque image to a screen reader. Editors built on the control
    /// registry of `openplugin-dri` forward this to it.
    fn accessibility_nodes(&self) -> Vec<AccessNode> {
        Vec::new()
    }

    /// A screen reader acts on node `node` (an index into
    /// [`Self::accessibility_nodes`]). The editor does what the same input
    /// would do with the mouse, as gestures through its [`EditorHost`], and
    /// returns whether it did anything. The default does nothing.
    fn accessibility_action(&mut self, _node: usize, _action: AccessAction) -> bool {
        false
    }

    /// The range and the plain value of node `node`, when it is a value along
    /// a range. A view whose platform reads numbers (Windows) offers them to
    /// the screen reader; one whose platform reads the value as text (macOS)
    /// never asks. The default is none: the node has its text value only.
    fn accessibility_range(&self, _node: usize) -> Option<AccessRange> {
        None
    }

    /// A screen reader sets node `node` to a plain value. The editor brings it
    /// into the range and sends it as one whole gesture, as
    /// [`Self::accessibility_action`] does for a step, and returns whether the
    /// value changed. The default does nothing.
    fn accessibility_set_value(&mut self, _node: usize, _value: f64) -> bool {
        false
    }

    /// A screen reader moves its cursor to node `node`: take keyboard focus
    /// there. True when it did. The default does nothing.
    fn accessibility_focus(&mut self, _node: usize) -> bool {
        false
    }

    /// The right button (or Control-click) went down at (`x`, `y`) points. An
    /// editor with parameter controls asks the host for the parameter's
    /// context menu through [`EditorHost::context_menu`], and when the host
    /// shows none, queues its own for [`Self::take_param_menu`]. Return true
    /// when the click was on a parameter control (the view then does not pass
    /// it on to the host). The default does nothing and returns false.
    fn right_mouse_down(&mut self, _x: f32, _y: f32) -> bool {
        false
    }

    /// The editor wants the view to show the SDK's own parameter menu (items
    /// "Reset to default", "Copy value", "Paste value", "Type value"). Called
    /// after every input call and tick; return the request once. The choice
    /// comes back through [`Self::param_menu_action`]. The default never asks.
    fn take_param_menu(&mut self) -> Option<ParamMenu> {
        None
    }

    /// The user chose an item of the menu for parameter `param`. The editor
    /// does what the matching gesture would do and, for
    /// [`ParamMenuAction::Copy`], returns the text for the clipboard. For
    /// every other action the result is `None`. The default does nothing.
    fn param_menu_action(&mut self, _param: u32, _action: ParamMenuAction) -> Option<String> {
        None
    }

    /// The host told the plugin about its track (name, colour, kind). Called
    /// when the editor is made, if the host has told by then, and whenever the
    /// host changes it. Main thread. The default ignores it.
    fn track_info(&mut self, _info: &crate::TrackInfo) {}

    /// The editor wants the view to open a native text field. Called after
    /// every input call and tick; return the request once and `None` after.
    /// The view shows a field over [`TextEdit::rect`] with the text selected.
    /// Return (and keypad Enter) commits it through [`Self::text_edit_commit`];
    /// Escape calls [`Self::text_edit_cancel`]. The field losing focus commits
    /// the text if `text_edit_commit` accepts it, and otherwise cancels. While the field
    /// is open every key goes to it and none reaches the editor or the host.
    /// The default never asks.
    fn take_text_edit(&mut self) -> Option<TextEdit> {
        None
    }

    /// The user pressed Return in the field. Parse `text`, send the gestures
    /// through the [`EditorHost`] and return true. Return false for text that
    /// is not a value: the field stays open, turns red and the system beeps.
    fn text_edit_commit(&mut self, _text: &str) -> bool {
        false
    }

    /// The field closed without a commit (Escape, invalid text when focus was
    /// lost, a click on the view, a resize). No gesture is sent.
    fn text_edit_cancel(&mut self) {}

    /// The places that show a tooltip. The view keeps one tooltip rectangle
    /// per entry (empty and non-finite rectangles are skipped) and moves them
    /// with the layout: they are compared after every tick and input call.
    /// The text is read again each time a tip is about to show, so a value
    /// that changed since is current. The default is none.
    fn tooltips(&self) -> Vec<Tooltip> {
        Vec::new()
    }

    /// Whether the editor ever accepts dropped files. Asked when the view is
    /// made and on every tick; only a view whose editor says yes registers as
    /// a drag destination, so an editor that says no (the default) leaves
    /// drops to the host.
    fn wants_drops(&self) -> bool {
        false
    }

    /// A drag of files is over the editor at (`x`, `y`) points. True shows
    /// the copy cursor and lets the drop happen. Called again as it moves.
    fn accepts_drop(&mut self, _x: f32, _y: f32) -> bool {
        false
    }

    /// Files were dropped at (`x`, `y`). True when the editor took them.
    fn files_dropped(&mut self, _paths: &[PathBuf], _x: f32, _y: f32) -> bool {
        false
    }

    /// The editor wants an open-file dialog. Called after every input call
    /// and tick; return the request once. The view shows the system panel
    /// (a sheet on the view's window, or a window of its own when it has none)
    /// and answers later, on the main thread, with [`Self::file_dialog_done`].
    fn take_file_dialog(&mut self) -> Option<FileDialog> {
        None
    }

    /// The answer to a [`FileDialog`]: the chosen files, or `None` when the
    /// user cancelled. Arrives on the main thread, never inside the call that
    /// asked.
    fn file_dialog_done(&mut self, _tag: u32, _paths: Option<Vec<PathBuf>>) {}
}

/// Makes the plugin's editor. Passed to `export_clap!`, `export_lv2!` and
/// `export_shell!` as `editor = path::to::fn`. `None` means no editor could
/// be made; the host then shows its generic controls.
pub type EditorFactory = fn(EditorHost) -> Option<Box<dyn Editor>>;

/// Remembers the last value given to an editor per parameter, so a bridge
/// that polls values calls [`Editor::param_changed`] only on a change.
pub struct ValueMirror {
    seen: Vec<u32>,
    primed: bool,
}

impl ValueMirror {
    pub fn new(count: usize) -> Self {
        Self { seen: vec![0; count], primed: false }
    }

    /// Forget every value, so the next [`Self::update`] reports all of them.
    pub fn invalidate(&mut self) {
        self.primed = false;
    }

    /// Report `value(index)` for every index whose bits changed.
    pub fn update(&mut self, editor: &mut dyn Editor, value: impl Fn(usize) -> Option<f32>) {
        let all = !self.primed;
        for (index, seen) in self.seen.iter_mut().enumerate() {
            let Some(v) = value(index) else { continue };
            if !v.is_finite() {
                continue;
            }
            if all || v.to_bits() != *seen {
                *seen = v.to_bits();
                editor.param_changed(index, v);
            }
        }
        self.primed = true;
    }
}

/// Fixed-capacity gesture queue with one producer and one consumer that may
/// each be on any thread. Both ends take `&self`, so the queue can live in an
/// `Arc` shared by the editor (producer, main thread) and the audio thread or
/// a parameter flush (consumer). Push and pop are wait-free and do not
/// allocate. A full queue rejects the gesture.
pub struct GestureQueue {
    words: Box<[AtomicU32]>,
    values: Box<[AtomicU32]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

const KIND_BEGIN: u32 = 1;
const KIND_PERFORM: u32 = 2;
const KIND_END: u32 = 3;
const INDEX_MASK: u32 = 0x00ff_ffff;

impl GestureQueue {
    /// Room for `capacity` gestures (at least one).
    pub fn new(capacity: usize) -> Self {
        let slots = capacity.max(1) + 1;
        Self {
            words: (0..slots).map(|_| AtomicU32::new(0)).collect(),
            values: (0..slots).map(|_| AtomicU32::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Producer side. False when full or when the index does not fit 24 bits.
    pub fn push(&self, gesture: Gesture) -> bool {
        let (kind, index, value) = match gesture {
            Gesture::Begin(i) => (KIND_BEGIN, i, 0.0),
            Gesture::Perform(i, v) => (KIND_PERFORM, i, v),
            Gesture::End(i) => (KIND_END, i, 0.0),
        };
        if index > INDEX_MASK {
            return false;
        }
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % self.words.len();
        if next == self.tail.load(Ordering::Acquire) {
            return false;
        }
        self.words[head].store((kind << 24) | index, Ordering::Relaxed);
        self.values[head].store(value.to_bits(), Ordering::Relaxed);
        self.head.store(next, Ordering::Release);
        true
    }

    /// Consumer side.
    pub fn pop(&self) -> Option<Gesture> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let word = self.words[tail].load(Ordering::Relaxed);
        let value = f32::from_bits(self.values[tail].load(Ordering::Relaxed));
        self.tail.store((tail + 1) % self.words.len(), Ordering::Release);
        let index = word & INDEX_MASK;
        Some(match word >> 24 {
            KIND_BEGIN => Gesture::Begin(index),
            KIND_PERFORM => Gesture::Perform(index, value),
            _ => Gesture::End(index),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.tail.load(Ordering::Acquire) == self.head.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_tracker_merges_and_clears() {
        let mut d = DamageTracker::new();
        assert!(d.pending().is_none(), "nothing drawn yet: everything");
        d.clear();
        assert!(d.is_clean() && d.pending() == Some(&[][..]));
        d.add(DamageRect::new(0.0, 0.0, 10.0, 10.0));
        d.add(DamageRect::new(50.0, 50.0, 5.0, 5.0));
        d.add(DamageRect::new(0.0, 0.0, 0.0, 9.0));
        assert_eq!(d.pending().map(<[_]>::len), Some(2));
        d.add(DamageRect::new(8.0, 8.0, 50.0, 50.0));
        assert_eq!(d.pending(), Some(&[DamageRect::new(0.0, 0.0, 58.0, 58.0)][..]));
        d.add(DamageRect::new(f32::NAN, 0.0, 1.0, 1.0));
        assert!(d.pending().is_none());
        d.clear();
        for i in 0..40 {
            d.add(DamageRect::new(i as f32 * 20.0, 0.0, 5.0, 5.0));
        }
        assert!(d.pending().is_none(), "too many rectangles become everything");
        assert!(Probe(Vec::new()).damage().is_none(), "the default is a full redraw");
    }

    #[test]
    fn queue_keeps_order_and_rejects_when_full() {
        let q = GestureQueue::new(3);
        assert!(q.push(Gesture::Begin(7)));
        assert!(q.push(Gesture::Perform(7, -2.5)));
        assert!(q.push(Gesture::End(7)));
        assert!(!q.push(Gesture::Begin(1)), "full");
        assert_eq!(q.pop(), Some(Gesture::Begin(7)));
        assert_eq!(q.pop(), Some(Gesture::Perform(7, -2.5)));
        assert_eq!(q.pop(), Some(Gesture::End(7)));
        assert_eq!(q.pop(), None);
        assert!(q.is_empty());
    }

    #[test]
    fn queue_works_across_threads() {
        let q = Arc::new(GestureQueue::new(64));
        let producer = {
            let q = q.clone();
            std::thread::spawn(move || {
                for i in 0..10_000u32 {
                    while !q.push(Gesture::Perform(i % 100, i as f32)) {
                        std::thread::yield_now();
                    }
                }
            })
        };
        let mut next = 0u32;
        while next < 10_000 {
            if let Some(Gesture::Perform(i, v)) = q.pop() {
                assert_eq!((i, v), (next % 100, next as f32));
                next += 1;
            }
        }
        producer.join().unwrap();
    }

    #[test]
    fn size_range_clamps_each_side_without_an_aspect() {
        let r = SizeRange::between((100, 50), (400, 200));
        assert_eq!(r.clamp(10, 10), (100, 50));
        assert_eq!(r.clamp(1000, 1000), (400, 200));
        assert_eq!(r.clamp(250, 60), (250, 60));
        assert_eq!(r.clamp(0, 0), (100, 50));
        assert!(r.contains(100, 200) && !r.contains(99, 100) && !r.contains(100, 201));
        assert!(r.can_resize_horizontally() && r.can_resize_vertically() && !r.is_fixed());
    }

    #[test]
    fn fixed_range_always_gives_its_size() {
        let r = SizeRange::fixed(240, 160);
        assert!(r.is_fixed() && !r.can_resize_horizontally() && !r.can_resize_vertically());
        for (w, h) in [(0, 0), (240, 160), (1, 9999), (u32::MAX, u32::MAX)] {
            assert_eq!(r.clamp(w, h), (240, 160));
        }
        // Written upside down, the bounds are put in order.
        let odd = SizeRange { min: (300, 300), max: (10, 10), aspect: Some((0, 5)) };
        assert_eq!(odd.clamp(5, 5), (300, 300));
        assert!(odd.is_fixed());
    }

    #[test]
    fn aspect_keeps_the_shape_inside_the_bounds() {
        let r = SizeRange::between((180, 120), (480, 320)).with_aspect(3, 2);
        assert_eq!(r.clamp(240, 160), (240, 160));
        for (w, h) in [(300, 160), (100, 100), (900, 100), (1, 1), (481, 321), (333, 777), (u32::MAX, 1)] {
            let (cw, ch) = r.clamp(w, h);
            assert!(r.contains(cw, ch), "{w}x{h} gave {cw}x{ch}");
            assert!((i64::from(cw) * 2 - i64::from(ch) * 3).abs() <= 2, "{w}x{h} gave {cw}x{ch}");
            assert_eq!(r.clamp(cw, ch), (cw, ch), "a result is a fixed point");
        }
        assert_eq!(r.clamp(2000, 2000), (480, 320));
        assert_eq!(r.clamp(1, 1), (180, 120));
        for (aw, ah) in [(3, 2), (16, 9), (1, 1), (7, 5)] {
            let r = SizeRange::between((151, 97), (611, 433)).with_aspect(aw, ah);
            for w in (1..800).step_by(13) {
                for h in (1..600).step_by(11) {
                    let c = r.clamp(w, h);
                    assert!(r.contains(c.0, c.1), "{aw}:{ah} {w}x{h} gave {c:?}");
                    assert_eq!(r.clamp(c.0, c.1), c, "{aw}:{ah} {w}x{h} gave {c:?}");
                }
            }
        }
    }

    #[test]
    fn aspect_with_bounds_of_another_shape_stays_inside_them() {
        // 1:1 inside a wide box: the height limit decides.
        let r = SizeRange::between((100, 100), (400, 200)).with_aspect(1, 1);
        assert_eq!(r.clamp(900, 900), (200, 200));
        assert_eq!(r.clamp(5, 5), (100, 100));
    }

    #[test]
    fn default_editor_is_fixed_and_ignores_resize() {
        let mut probe = Probe(Vec::new());
        assert_eq!(probe.size_range(), SizeRange::fixed(1, 1));
        probe.resize(50, 50);
        assert_eq!(probe.size(), (1, 1));
    }

    #[test]
    fn default_editor_has_no_accessibility_nodes_and_ignores_actions() {
        let mut probe = Probe(Vec::new());
        assert!(probe.accessibility_nodes().is_empty());
        assert!(!probe.accessibility_action(0, AccessAction::Increment));
        assert!(!probe.accessibility_focus(0));
        assert!(probe.accessibility_range(0).is_none());
        assert!(!probe.accessibility_set_value(0, 1.0));
    }

    #[test]
    fn default_editor_ignores_text_tooltips_and_files() {
        let mut probe = Probe(Vec::new());
        assert!(probe.take_text_edit().is_none() && !probe.text_edit_commit("1"));
        assert!(probe.tooltips().is_empty() && !probe.wants_drops());
        assert!(!probe.accepts_drop(1.0, 1.0) && !probe.files_dropped(&[], 1.0, 1.0));
        assert!(probe.take_file_dialog().is_none());
    }

    #[test]
    fn default_editor_has_no_menu_and_ignores_track_info() {
        let mut probe = Probe(Vec::new());
        assert!(!probe.right_mouse_down(1.0, 1.0) && probe.take_param_menu().is_none());
        assert!(probe.param_menu_action(0, ParamMenuAction::Copy).is_none());
        probe.track_info(&crate::TrackInfo::default());
    }

    #[test]
    fn context_menu_needs_a_handler_and_finite_points() {
        assert!(!EditorHost::detached().context_menu(0, 1.0, 1.0));
        let seen = Rc::new(std::cell::Cell::new((0, 0, 0)));
        let sink = seen.clone();
        let host = EditorHost::detached().with_context_menu(move |p, x, y| {
            sink.set((p, x, y));
            true
        });
        assert!(host.context_menu(3, 10.4, 20.6));
        assert_eq!(seen.get(), (3, 10, 21));
        seen.set((0, 0, 0));
        assert!(!host.context_menu(3, f32::NAN, 1.0));
        assert_eq!(seen.get(), (0, 0, 0));
    }

    #[test]
    fn request_resize_needs_a_handler_and_clamps() {
        let range = SizeRange::between((10, 10), (100, 100));
        assert!(!EditorHost::detached().request_resize(&range, 50, 50));
        let seen = Rc::new(std::cell::Cell::new((0, 0)));
        let sink = seen.clone();
        let host = EditorHost::detached().with_resize(move |w, h| {
            sink.set((w, h));
            true
        });
        assert!(host.request_resize(&range, 500, 5));
        assert_eq!(seen.get(), (100, 10));
    }

    struct Probe(Vec<(usize, f32)>);
    impl Editor for Probe {
        fn size(&self) -> (u32, u32) {
            (1, 1)
        }
        fn param_changed(&mut self, index: usize, plain: f32) {
            self.0.push((index, plain));
        }
        fn render(&mut self, _: f32) -> Frame<'_> {
            Frame { pixels: &[0], width: 1, height: 1 }
        }
    }

    #[test]
    fn mirror_reports_everything_once_then_only_changes() {
        let mut mirror = ValueMirror::new(3);
        let mut probe = Probe(Vec::new());
        let values = std::cell::RefCell::new([1.0f32, 2.0, 3.0]);
        mirror.update(&mut probe, |i| Some(values.borrow()[i]));
        assert_eq!(probe.0.len(), 3);
        probe.0.clear();
        mirror.update(&mut probe, |i| Some(values.borrow()[i]));
        assert!(probe.0.is_empty());
        values.borrow_mut()[1] = 5.0;
        mirror.update(&mut probe, |i| Some(values.borrow()[i]));
        assert_eq!(probe.0, vec![(1, 5.0)]);
        mirror.invalidate();
        probe.0.clear();
        mirror.update(&mut probe, |i| Some(values.borrow()[i]));
        assert_eq!(probe.0.len(), 3);
    }

    #[test]
    fn host_set_is_a_full_gesture() {
        let log = Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = log.clone();
        let host = EditorHost::new(move |g| sink.borrow_mut().push(g));
        host.set(4, 0.5);
        host.perform(4, f32::NAN);
        assert_eq!(*log.borrow(), vec![Gesture::Begin(4), Gesture::Perform(4, 0.5), Gesture::End(4)]);
    }
}
