//! A gain plugin with an editor, exported in every format the SDK has.
//!
//! The engine shares a peak meter with its editor through
//! [`Plugin::editor_state`]. The editor is drawn with GoodLookin DRI 0.2 and
//! rasterised by [`SceneCanvas`]: a Navy plate, a lit Brit knob for the gain
//! (drag it up or down, Shift for fine; double-click types a value, Option-double-click resets), a bat toggle for Warm (click it),
//! and an LED ladder. It resizes from 180x120 to 480x320, keeping 3:2, and a
//! double-click on the bare plate asks the host for a larger or the opening size.
//! Hit areas, drags and keys live in a [`ControlSet`]: click a control, or press Tab, to focus it
//! (a ring is drawn), then arrows, Page keys, Home, End, Space and Delete act on it; keys it does
//! not use go back to the host.
//! Hit areas and drags live in a [`ControlSet`]. Its changes go through [`EditorHost`] as gestures, and the host's
//! values come back through [`Editor::param_changed`]. The same editor runs in
//! CLAP (`clap.gui`), LV2 (`ui:CocoaUI`), and the VST3, Audio Unit and AAX
//! shells (`op_editor_*`). The factory presets are text files in `presets/`,
//! embedded by `build.rs`.
//!
//! Typing, tooltips and files: double-click the knob (or focus it and press Return) to type a
//! value such as `-6 dB`; every control shows a tooltip with its name and value, and the meter's
//! tip names the last file the editor was given. Drop a file on the editor, or press Command-O
//! for the open-file dialog (`.wav` and `.aif` files), and the name is shown in the meter's tooltip.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use openplugin::core::editor::{DamageRect, DamageTracker, FileDialog, Key, Modifiers, SizeRange, TextEdit, Tooltip};
use openplugin::core::{Editor, EditorHost, EditorState, Frame, PresetBank};
use openplugin::dri::goodlookindri::color::Color as GlColor;
use openplugin::dri::goodlookindri::scene::{Rect, Scene};
use openplugin::dri::goodlookindri::{meters, switches};
use openplugin::dri::{draw_faceplate_code, draw_knob_code, Area, Control, ControlKind, ControlSet, SceneCanvas};
use openplugin::prelude::*;

// Two groups, one nested in the other, so every format shows them: CLAP module
// paths, LV2 port groups, VST3 units, Audio Unit clumps and AAX pages.
static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db).with_group("Output"),
    ParamInfo::new("warm", "Warm", 0.0, 1.0, 0.0, Unit::Toggle).with_group("Output/Colour"),
];
const _: () = assert!(openplugin::core::groups_valid(&PARAMS));

pub static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.example.editor-gain",
    name: "Editor Gain",
    vendor: "OpenGrid",
    url: "https://github.com/MistaMin/OpenPluginSDK",
    email: "",
    version: "0.3.1",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: true,
};

mod files {
    include!(concat!(env!("OUT_DIR"), "/presets.rs"));
}

/// The factory presets.
pub fn presets() -> &'static PresetBank {
    static BANK: OnceLock<PresetBank> = OnceLock::new();
    BANK.get_or_init(|| PresetBank::parse_all(&INFO, files::FILES).0)
}

/// What the engine shows its editor.
pub struct Shared {
    /// Output peak since the editor last looked, linear, as `f32` bits.
    peak: AtomicU32,
}

impl Shared {
    fn raise(&self, v: f32) {
        if v > f32::from_bits(self.peak.load(Ordering::Relaxed)) {
            self.peak.store(v.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak.swap(0, Ordering::Relaxed))
    }
}

pub struct EditorGain {
    db: f32,
    warm: bool,
    linear: f32,
    shared: Arc<Shared>,
}

impl Default for EditorGain {
    fn default() -> Self {
        Self { db: 0.0, warm: false, linear: 1.0, shared: Arc::new(Shared { peak: AtomicU32::new(0) }) }
    }
}

impl Plugin for EditorGain {
    // All of this plugin's state is its parameters: saves and loads never
    // touch the engine, so a host saving during playback skips no block.
    fn state_is_parameters_only(&self) -> bool {
        true
    }

    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "gain" => {
                self.db = PARAMS[0].clamp(value);
                self.linear = 10f32.powf(self.db / 20.0);
            }
            "warm" => self.warm = PARAMS[1].clamp(value) > 0.5,
            _ => return false,
        }
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "gain" => Some(self.db),
            "warm" => Some(f32::from(u8::from(self.warm))),
            _ => None,
        }
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let mut peak = 0.0f32;
        for channel in io.iter_mut() {
            for s in &mut channel[..frames] {
                let x = *s * self.linear;
                *s = if self.warm { x.tanh() } else { x };
                peak = peak.max(s.abs());
            }
        }
        self.shared.raise(peak);
    }
    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }
    fn editor_state(&self) -> Option<EditorState> {
        Some(self.shared.clone())
    }
    fn aax_ids(&self) -> Option<openplugin::core::AaxIds> {
        Some(openplugin::core::AaxIds { manufacturer: *b"OpGd", product: *b"EdGn", mono: *b"EdGM", stereo: *b"EdGS" })
    }
    fn lv2_uri(&self) -> Option<&'static str> {
        Some("https://github.com/MistaMin/OpenPluginSDK/examples/editor-gain")
    }
    fn preset_count(&self) -> usize {
        presets().len()
    }
    fn preset_name(&self, index: usize) -> &'static str {
        presets().name(index)
    }
    fn preset_values(&self, index: usize) -> Option<&'static [f32]> {
        presets().values(index)
    }
}

pub const WIDTH: u32 = 240;
pub const HEIGHT: u32 = 160;
/// The smallest and largest size, in points. The shape stays 3:2.
pub const MIN_SIZE: (u32, u32) = (180, 120);
pub const MAX_SIZE: (u32, u32) = (480, 320);
/// The larger size a double-click on the plate asks for.
pub const LARGE_SIZE: (u32, u32) = (360, 240);
/// Hit areas at the opening size, in points (x, y, w, h): the knob, the toggle, the meter.
pub const KNOB: (f32, f32, f32, f32) = (25.0, 38.0, 60.0, 60.0);
pub const SWITCH: (f32, f32, f32, f32) = (118.0, 40.0, 34.0, 56.0);
pub const METER: (f32, f32, f32, f32) = (190.0, 20.0, 18.0, 120.0);
/// How far a drag moves the knob from end to end, in points.
const DRAG_RANGE: f32 = 150.0;

/// The hit areas at one size, in points. The layout constants are scaled by
/// the largest factor that fits and centred, so any size has a valid layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// Points per point of the opening layout.
    pub scale: f32,
    pub knob: (f32, f32, f32, f32),
    pub switch: (f32, f32, f32, f32),
    pub meter: (f32, f32, f32, f32),
    /// Where the "WARM" legend sits (centre x, baseline y).
    pub legend: (f32, f32),
}

/// The layout for an editor of `width` x `height` points.
pub fn layout(width: u32, height: u32) -> Layout {
    let scale = (width as f32 / WIDTH as f32).min(height as f32 / HEIGHT as f32).max(0.01);
    let ox = (width as f32 - WIDTH as f32 * scale) * 0.5;
    let oy = (height as f32 - HEIGHT as f32 * scale) * 0.5;
    let place = |r: (f32, f32, f32, f32)| (ox + r.0 * scale, oy + r.1 * scale, r.2 * scale, r.3 * scale);
    Layout {
        scale,
        knob: place(KNOB),
        switch: place(SWITCH),
        meter: place(METER),
        legend: (ox + (SWITCH.0 + SWITCH.2 * 0.5) * scale, oy + (SWITCH.1 + SWITCH.3 + 16.0) * scale),
    }
}

/// The two controls at a layout, with the knob's drag range scaled with it.
fn controls_for(host: EditorHost, lay: &Layout) -> ControlSet {
    ControlSet::new(host)
        .with(Control::knob(Area::from_tuple(lay.knob), 0, &PARAMS[0]).drag_range(DRAG_RANGE * lay.scale))
        .with(Control::toggle(Area::from_tuple(lay.switch), 1, &PARAMS[1]))
}

fn rect(r: (f32, f32, f32, f32)) -> Rect {
    Rect::new(r.0, r.1, r.2, r.3)
}

/// The editor: GoodLookin DRI parts on a [`SceneCanvas`], with its hit areas
/// and gestures in a [`ControlSet`].
pub struct GainEditor {
    host: EditorHost,
    controls: ControlSet,
    modifiers: Modifiers,
    /// The view has keyboard focus (the focus ring is drawn only then).
    keys_active: bool,
    shared: Option<Arc<Shared>>,
    level: f32,
    size: (u32, u32),
    canvas: SceneCanvas,
    /// What the next frame has to repaint.
    damage: DamageTracker,
    /// The picture the meter shows (see [`ladder_picture`]) in the last frame accounted for.
    meter_picture: Option<u32>,
    /// The name of the last file dropped or chosen, shown in the meter's tooltip.
    last_file: Option<String>,
    /// An open-file dialog the view has not taken yet.
    dialog: Option<FileDialog>,
}

/// How far a knob paints past its hit area, in points at the opening layout: its legend and
/// value text sit below it, and its reflections and shadow reach out.
const KNOB_REACH: (f32, f32, f32, f32) = (24.0, 18.0, 24.0, 40.0);
/// The focus ring: 4 points out, 2 wide, plus the anti-aliased edge.
const RING_REACH: f32 = 6.0;
/// The "WARM" legend under the toggle reaches this far below it, and as far to each side.
const LEGEND_REACH: (f32, f32) = (14.0, 24.0);
/// The ladder's recessed channel, bloom and shadow reach out of the meter's rectangle.
const METER_REACH: f32 = 10.0;

/// What `draw_led_ladder` shows for a level: the set of lit segments, as bits. There are 16, each
/// lit from 1.5 dB below its own 3 dB step, and the peak segment is lit whatever the level. Two
/// levels with the same set draw the same pixels, so the meter is repainted only when it changes.
fn ladder_picture(level_db: f32) -> u32 {
    let peak = (((level_db + 48.0) / 48.0).clamp(0.0, 1.0) * 15.0 + 0.5).floor() as i32;
    let mut lit = 0u32;
    for i in 0..16 {
        if level_db >= -48.0 + 48.0 * (i + 1) as f32 / 16.0 - 1.5 || i == peak {
            lit |= 1 << i;
        }
    }
    lit
}

impl GainEditor {
    pub fn new(host: EditorHost) -> Self {
        let controls = controls_for(host.clone(), &layout(WIDTH, HEIGHT));
        Self {
            host,
            controls,
            modifiers: Modifiers::NONE,
            keys_active: true,
            shared: None,
            level: 0.0,
            size: (WIDTH, HEIGHT),
            canvas: SceneCanvas::new(WIDTH as f32, HEIGHT as f32),
            damage: DamageTracker::new(),
            meter_picture: None,
            last_file: None,
            dialog: None,
        }
    }

    fn level_db(&self) -> f32 {
        20.0 * self.level.max(1e-6).log10()
    }

    /// The rectangle a control's drawing can change, in points.
    fn reach(&self, c: &Control) -> DamageRect {
        let k = layout(self.size.0, self.size.1).scale;
        let (x, y, w, h) = c.area.bounds();
        let r = match c.kind {
            ControlKind::Knob => {
                let (l, t, r, b) = KNOB_REACH;
                DamageRect::new(x - l * k, y - t * k, w + (l + r) * k, h + (t + b) * k)
            }
            _ => DamageRect::new(x - LEGEND_REACH.0 * k, y - 4.0 * k, w + 2.0 * LEGEND_REACH.0 * k, h + LEGEND_REACH.1 * k + 4.0 * k),
        };
        // A ring around a focused control reaches in every direction.
        r.union(&DamageRect::new(x, y, w, h).expanded(RING_REACH * k))
    }

    /// Mark what changed since the last time: controls whose look changed, and the meter when its picture did.
    fn collect_damage(&mut self) {
        for id in self.controls.take_changed() {
            if let Some(rect) = self.controls.get(id).map(|c| self.reach(c)) {
                self.damage.add(rect);
            }
        }
        let picture = ladder_picture(self.level_db());
        if self.meter_picture != Some(picture) {
            self.meter_picture = Some(picture);
            let lay = layout(self.size.0, self.size.1);
            self.damage.add(DamageRect::from_tuple(lay.meter).expanded(METER_REACH * lay.scale));
        }
    }

    pub fn values(&self) -> [f32; 2] {
        let v = |i: usize| self.controls.for_param(i).map_or(PARAMS[i].default, |c| c.value());
        [v(0), v(1)]
    }

    /// The name of the last file dropped on the editor or chosen in its dialog.
    pub fn last_file(&self) -> Option<&str> {
        self.last_file.as_deref()
    }

    fn remember(&mut self, paths: &[PathBuf]) -> bool {
        let Some(name) = paths.first().and_then(|p| p.file_name()) else { return false };
        self.last_file = Some(name.to_string_lossy().into_owned());
        true
    }

    pub fn is_bound(&self) -> bool {
        self.shared.is_some()
    }
}

/// [`openplugin::core::EditorFactory`] for every format.
pub fn make_editor(host: EditorHost) -> Option<Box<dyn Editor>> {
    Some(Box::new(GainEditor::new(host)))
}

impl Editor for GainEditor {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn size_range(&self) -> SizeRange {
        SizeRange::between(MIN_SIZE, MAX_SIZE).with_aspect(WIDTH, HEIGHT)
    }

    fn resize(&mut self, width: u32, height: u32) {
        let size = self.size_range().clamp(width, height);
        if size == self.size {
            return;
        }
        self.size = size;
        self.damage.add_all();
        self.canvas.resize(size.0 as f32, size.1 as f32);
        // New hit areas at the new size; the values carry over without a gesture.
        let values = self.values();
        let focus = self.controls.focused();
        self.controls = controls_for(self.host.clone(), &layout(size.0, size.1));
        self.controls.set_modifiers(self.modifiers);
        self.controls.set_focus(focus);
        for (index, value) in values.iter().enumerate() {
            self.controls.param_changed(index, *value);
        }
    }

    fn bind(&mut self, state: Option<EditorState>) {
        self.shared = state.and_then(|s| s.downcast::<Shared>().ok());
    }

    fn param_changed(&mut self, index: usize, plain: f32) {
        self.controls.param_changed(index, plain);
    }

    fn tick(&mut self, dt: f32) {
        let peak = self.shared.as_ref().map(|s| s.take_peak()).unwrap_or(0.0);
        // A meter that rises at once and falls at 20 dB a second.
        let fall = 10f32.powf(-20.0 * dt / 20.0);
        self.level = peak.max(self.level * fall);
    }

    fn damage(&mut self) -> Option<Vec<DamageRect>> {
        self.collect_damage();
        self.damage.pending().map(<[DamageRect]>::to_vec)
    }

    fn render(&mut self, scale: f32) -> Frame<'_> {
        self.collect_damage();
        let level_db = self.level_db();
        let controls = &self.controls;
        let keys_active = self.keys_active;
        let lay = layout(self.size.0, self.size.1);
        let (width, height) = (self.size.0 as f32, self.size.1 as f32);
        // The plate is static: painted once per size and scale, then reused under the moving parts.
        let plate = |dl: &mut openplugin::dri::DrawList| draw_faceplate_code(dl, width, height, "Navy");
        let damage = self.damage.pending();
        // A part that cannot reach into a damaged rectangle is not even built.
        let touched = |r: &DamageRect| damage.is_none_or(|d| d.iter().any(|x| x.touches(r)));
        let reach: Vec<DamageRect> = controls.iter().map(|c| self.reach(c)).collect();
        let meter_reach = DamageRect::from_tuple(lay.meter).expanded(METER_REACH * lay.scale);
        let frame = self.canvas.frame_damaged(scale, 0, plate, |dl| {
            let mut parts = Scene::new();
            for (c, _) in controls.iter().zip(&reach).filter(|(_, r)| touched(r)) {
                let (x, y, w, h) = c.area.bounds();
                match c.kind {
                    ControlKind::Knob => draw_knob_code(
                        dl,
                        x + w * 0.5,
                        y + h * 0.5,
                        w * 0.5,
                        c.normalized(),
                        "GAIN",
                        &c.info.format(c.value()),
                        "Brit",
                        Color::BRIT_RED,
                    ),
                    ControlKind::Toggle => {
                        switches::draw_toggle(&mut parts, rect((x, y, w, h)), c.is_on(), switches::ToggleStyle::Bat, GlColor(0xffe08030));
                        parts.text(
                            openplugin::dri::goodlookindri::scene::Point::new(lay.legend.0, lay.legend.1),
                            "WARM",
                            GlColor(0xffffffff),
                            11.0 * lay.scale,
                            true,
                        );
                    }
                    _ => {}
                }
            }
            if touched(&meter_reach) {
                meters::draw_led_ladder(&mut parts, rect(lay.meter), false, level_db, level_db, 16);
            }
            let ringed = controls.focused().filter(|&id| touched(&reach[id])).and_then(|id| controls.get(id));
            if let Some(c) = ringed.filter(|_| keys_active) {
                let (x, y, w, h) = c.area.bounds();
                let ring = rect((x, y, w, h)).expanded(4.0 * lay.scale);
                let color = GlColor(0xff5cc8ff);
                match c.kind {
                    ControlKind::Knob => parts.stroke_ellipse(ring, color, 2.0 * lay.scale),
                    _ => parts.stroke_round(ring, 6.0 * lay.scale, color, 2.0 * lay.scale),
                }
            }
            dl.append(parts);
        }, damage);
        self.damage.clear();
        frame
    }

    fn modifiers_changed(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
        self.controls.set_modifiers(modifiers);
    }

    fn accessibility_nodes(&self) -> Vec<openplugin::core::editor::AccessNode> {
        self.controls.accessibility_nodes()
    }

    fn accessibility_action(&mut self, node: usize, action: openplugin::core::editor::AccessAction) -> bool {
        self.controls.accessibility_action(node, action)
    }

    fn accessibility_focus(&mut self, node: usize) -> bool {
        self.controls.accessibility_focus(node)
    }

    fn wants_keys(&self) -> bool {
        self.controls.wants_keys()
    }

    fn key_down(&mut self, key: Key, modifiers: Modifiers) -> bool {
        if key == Key::Char('o') && modifiers.command {
            self.dialog = Some(FileDialog { tag: 1, title: "Choose an audio file".into(), extensions: vec!["wav".into(), "aif".into(), "aiff".into()], multiple: false });
            return true;
        }
        self.controls.key_down(key, modifiers)
    }

    fn take_text_edit(&mut self) -> Option<TextEdit> {
        self.controls.take_text_edit()
    }

    fn right_mouse_down(&mut self, x: f32, y: f32) -> bool {
        self.controls.context_click(x, y)
    }

    fn take_param_menu(&mut self) -> Option<openplugin::core::editor::ParamMenu> {
        self.controls.take_param_menu()
    }

    fn param_menu_action(&mut self, param: u32, action: openplugin::core::editor::ParamMenuAction) -> Option<String> {
        self.controls.param_menu_action(param, action)
    }

    fn text_edit_commit(&mut self, text: &str) -> bool {
        self.controls.text_edit_commit(text)
    }

    fn text_edit_cancel(&mut self) {
        self.controls.text_edit_cancel();
    }

    fn tooltips(&self) -> Vec<Tooltip> {
        let mut tips = self.controls.tooltips();
        let meter = DamageRect::from_tuple(layout(self.size.0, self.size.1).meter);
        let text = match &self.last_file {
            Some(name) => format!("Output level. Last file: {name}"),
            None => "Output level. Drop an audio file here, or press Command-O".to_string(),
        };
        tips.push(Tooltip { rect: meter, text });
        tips
    }

    fn wants_drops(&self) -> bool {
        true
    }

    fn accepts_drop(&mut self, _x: f32, _y: f32) -> bool {
        true
    }

    fn files_dropped(&mut self, paths: &[PathBuf], _x: f32, _y: f32) -> bool {
        self.remember(paths)
    }

    fn take_file_dialog(&mut self) -> Option<FileDialog> {
        self.dialog.take()
    }

    fn file_dialog_done(&mut self, _tag: u32, paths: Option<Vec<PathBuf>>) {
        if let Some(paths) = paths {
            self.remember(&paths);
        }
    }

    fn key_up(&mut self, key: Key, modifiers: Modifiers) -> bool {
        self.controls.key_up(key, modifiers)
    }

    fn focus_changed(&mut self, focused: bool) {
        self.keys_active = focused;
        // The ring shows only while the view has the keyboard.
        if let Some(rect) = self.controls.focused().and_then(|id| self.controls.get(id)).map(|c| self.reach(c)) {
            self.damage.add(rect);
        }
        if !focused {
            self.controls.release_keys();
        }
    }

    fn mouse_down(&mut self, x: f32, y: f32, clicks: u32) {
        let on_control = self.controls.iter().any(|c| c.area.contains(x, y));
        let lay = layout(self.size.0, self.size.1);
        let (mx, my, mw, mh) = lay.meter;
        let on_meter = x >= mx && x < mx + mw && y >= my && y < my + mh;
        if !on_control && !on_meter && clicks >= 2 {
            // The bare plate: ask the host to switch between the two sizes.
            let (w, h) = if self.size.0 >= LARGE_SIZE.0 { (WIDTH, HEIGHT) } else { LARGE_SIZE };
            self.host.request_resize(&self.size_range(), w, h);
            return;
        }
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
}

openplugin::targets::export_clap!(EditorGain, editor = make_editor);
openplugin::targets::export_lv2!(EditorGain, editor = make_editor);
openplugin::targets::export_shell!(EditorGain, editor = make_editor);

#[cfg(test)]
mod tests {
    use super::*;

    /// Two editors fed the same events. `part` repaints only what it reports; `whole` is told to
    /// repaint everything before every frame, which is what the view did before damage existed.
    struct Pair {
        part: GainEditor,
        whole: GainEditor,
        peaks: [Arc<Shared>; 2],
        frames: usize,
        skipped: usize,
        partial: usize,
    }

    impl Pair {
        fn new() -> Self {
            let peaks = [0, 1].map(|_| Arc::new(Shared { peak: AtomicU32::new(0) }));
            let mut part = GainEditor::new(EditorHost::detached());
            let mut whole = GainEditor::new(EditorHost::detached());
            part.bind(Some(peaks[0].clone()));
            whole.bind(Some(peaks[1].clone()));
            Self { part, whole, peaks, frames: 0, skipped: 0, partial: 0 }
        }

        fn both(&mut self, f: impl Fn(&mut GainEditor)) {
            f(&mut self.part);
            f(&mut self.whole);
        }

        /// One view tick followed by the frame the view would draw, and the comparison.
        fn frame(&mut self, scale: f32, what: &str) {
            self.both(|e| e.tick(1.0 / 30.0));
            let damage = self.part.damage();
            let expected = {
                self.whole.damage.add_all();
                self.whole.render(scale).pixels.to_vec()
            };
            self.frames += 1;
            match &damage {
                // The view draws nothing. Rendering anyway must give the same frame.
                Some(rects) if rects.is_empty() => self.skipped += 1,
                Some(_) => self.partial += 1,
                None => {}
            }
            let got = self.part.render(scale);
            let w = got.width;
            let bad: Vec<usize> = (0..got.pixels.len()).filter(|&i| got.pixels[i] != expected[i]).collect();
            let bbox = bad.iter().fold((usize::MAX, usize::MAX, 0, 0), |b, &i| (b.0.min(i % w), b.1.min(i / w), b.2.max(i % w), b.3.max(i / w)));
            assert!(bad.is_empty(), "{what}: the damaged frame differs from a full repaint in {} pixels, device bbox {bbox:?} at scale {scale} (damage {damage:?})", bad.len());
        }
    }

    fn run(scale: f32) -> Pair {
        let mut p = Pair::new();
        p.frame(scale, "first frame");
        for i in 0..3 {
            p.frame(scale, &format!("idle {i}"));
        }
        let lay = layout(WIDTH, HEIGHT);
        let centre = |r: (f32, f32, f32, f32)| (r.0 + r.2 * 0.5, r.1 + r.3 * 0.5);
        let (kx, ky) = centre(lay.knob);
        let (sx, sy) = centre(lay.switch);
        // Hover, then a drag through the whole range and back, one frame per move.
        p.both(|e| e.mouse_move(kx, ky));
        p.frame(scale, "hover the knob");
        p.both(|e| e.mouse_down(kx, ky, 1));
        p.frame(scale, "press the knob");
        for i in 1..=14 {
            let y = ky - i as f32 * 8.0;
            p.both(|e| e.mouse_move(kx, y));
            p.frame(scale, &format!("drag up {i}"));
        }
        for i in 1..=14 {
            let y = ky - 112.0 + i as f32 * 14.0;
            p.both(|e| e.mouse_move(kx, y));
            p.frame(scale, &format!("drag down {i}"));
        }
        p.both(|e| e.mouse_up(kx, ky));
        p.frame(scale, "release the knob");
        // Toggle: hover, click, host echo.
        p.both(|e| e.mouse_move(sx, sy));
        p.frame(scale, "hover the toggle");
        p.both(|e| e.mouse_down(sx, sy, 1));
        p.both(|e| e.mouse_up(sx, sy));
        p.frame(scale, "click the toggle");
        p.both(|e| e.param_changed(1, 0.0));
        p.frame(scale, "host turns it off");
        p.both(|e| e.mouse_move(5.0, 5.0));
        p.frame(scale, "leave");
        // Keyboard: focus ring, steps, a lost and regained view focus.
        for key in [Key::Tab, Key::Up, Key::PageDown, Key::Home, Key::End, Key::Tab, Key::Space, Key::Tab, Key::Escape] {
            p.both(|e| {
                e.key_down(key, Modifiers::NONE);
                e.key_up(key, Modifiers::NONE);
            });
            p.frame(scale, &format!("key {key:?}"));
        }
        p.both(|e| {
            e.key_down(Key::Tab, Modifiers::NONE);
        });
        p.frame(scale, "focus the knob");
        p.both(|e| e.focus_changed(false));
        p.frame(scale, "view loses the keyboard");
        p.both(|e| e.focus_changed(true));
        p.frame(scale, "view gets the keyboard");
        // Host automation and the wheel.
        for v in [-24.0, -3.3, 7.1, 24.0, 0.0] {
            p.both(|e| e.param_changed(0, v));
            p.frame(scale, &format!("automation {v}"));
        }
        p.both(|e| e.wheel(kx, ky, 3.0));
        p.frame(scale, "wheel");
        // The meter: peaks, then the fall over a few seconds, a frame at a time.
        for peak in [0.001f32, 0.2, 1.0, 0.05] {
            for peaks in &p.peaks {
                peaks.raise(peak);
            }
            p.frame(scale, &format!("peak {peak}"));
            for i in 0..22 {
                p.frame(scale, &format!("fall after {peak}, tick {i}"));
            }
        }
        // A new size, and back.
        p.both(|e| e.resize(360, 240));
        p.frame(scale, "resized large");
        p.both(|e| e.mouse_move(180.0, 120.0));
        p.frame(scale, "hover at the large size");
        p.both(|e| e.resize(WIDTH, HEIGHT));
        p.frame(scale, "resized back");
        p
    }

    #[test]
    fn damage_only_frames_equal_full_repaints_through_a_long_session() {
        for scale in [2.0, 1.0, 1.5] {
            let p = run(scale);
            // The comparison would be empty if everything were repainted every time.
            assert!(p.partial > 40, "scale {scale}: only {} partial frames of {}", p.partial, p.frames);
            assert!(p.skipped > 10, "scale {scale}: only {} idle frames of {}", p.skipped, p.frames);
        }
    }

    #[test]
    fn an_idle_editor_reports_nothing_and_a_meter_change_reports_only_the_meter() {
        let mut p = Pair::new();
        p.frame(2.0, "first");
        assert_eq!(p.part.damage(), Some(vec![]), "nothing changed");
        p.peaks[0].raise(1.0);
        p.part.tick(1.0 / 30.0);
        let d = p.part.damage().unwrap();
        assert_eq!(d.len(), 1);
        let meter = DamageRect::from_tuple(layout(WIDTH, HEIGHT).meter);
        let r = d[0];
        assert!(r.x <= meter.x && r.y <= meter.y && r.x + r.w >= meter.x + meter.w && r.y + r.h >= meter.y + meter.h);
        assert!(r.w < 60.0 && r.h < 160.0, "the meter's area only: {r:?}");
        // Nothing in the ladder's picture changed: still nothing to draw.
        p.part.render(2.0);
        p.part.level *= 0.999;
        assert_eq!(p.part.damage(), Some(vec![]));
        // A knob move reports the knob and not the meter.
        p.part.param_changed(0, 6.0);
        let d = p.part.damage().unwrap();
        let k = DamageRect::from_tuple(layout(WIDTH, HEIGHT).knob);
        assert!(d.len() == 1 && d[0].x <= k.x && d[0].x + d[0].w >= k.x + k.w && d[0].x + d[0].w < meter.x, "{d:?}");
    }

    #[test]
    fn meter_picture_changes_exactly_when_the_pixels_do() {
        let mut canvas = SceneCanvas::new(WIDTH as f32, HEIGHT as f32);
        let lay = layout(WIDTH, HEIGHT);
        let mut draw = |db: f32| {
            canvas
                .frame(1.0, |dl| {
                    let mut s = Scene::new();
                    meters::draw_led_ladder(&mut s, rect(lay.meter), false, db, db, 16);
                    dl.append(s);
                })
                .pixels
                .to_vec()
        };
        let mut last = (ladder_picture(-130.0), draw(-130.0));
        let mut changes = 0;
        let mut db = -130.0f32;
        while db < 12.0 {
            db += 0.013;
            let now = (ladder_picture(db), draw(db));
            assert!(now.0 != last.0 || now.1 == last.1, "{db} dB: the same picture key, different pixels");
            changes += usize::from(now.0 != last.0);
            last = now;
        }
        assert!(changes >= 15, "{changes}");
    }
}
