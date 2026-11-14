//! A gain plugin with an editor, exported in every format the SDK has.
//!
//! The engine shares a peak meter with its editor through
//! [`Plugin::editor_state`]. The editor is drawn with GoodLookin DRI 0.2 and
//! rasterised by [`SceneCanvas`]: a Navy plate, a lit Brit knob for the gain
//! (drag it up or down, Shift for fine; double-click resets), a bat toggle for Warm (click it),
//! and an LED ladder. Hit areas and drags live in a [`ControlSet`]. Its changes go through [`EditorHost`] as gestures, and the host's
//! values come back through [`Editor::param_changed`]. The same editor runs in
//! CLAP (`clap.gui`), LV2 (`ui:CocoaUI`), and the VST3, Audio Unit and AAX
//! shells (`op_editor_*`). The factory presets are text files in `presets/`,
//! embedded by `build.rs`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use openplugin::core::editor::Modifiers;
use openplugin::core::{Editor, EditorHost, EditorState, Frame, PresetBank};
use openplugin::dri::goodlookindri::color::Color as GlColor;
use openplugin::dri::goodlookindri::scene::{Rect, Scene};
use openplugin::dri::goodlookindri::{meters, switches};
use openplugin::dri::{draw_faceplate_code, draw_knob_code, Area, Control, ControlKind, ControlSet, SceneCanvas};
use openplugin::prelude::*;

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
    ParamInfo::new("warm", "Warm", 0.0, 1.0, 0.0, Unit::Toggle),
];

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
/// Hit areas in points (x, y, w, h): the knob, the toggle, the meter.
pub const KNOB: (f32, f32, f32, f32) = (25.0, 38.0, 60.0, 60.0);
pub const SWITCH: (f32, f32, f32, f32) = (118.0, 40.0, 34.0, 56.0);
pub const METER: (f32, f32, f32, f32) = (190.0, 20.0, 18.0, 120.0);
/// How far a drag moves the knob from end to end, in points.
const DRAG_RANGE: f32 = 150.0;

fn rect(r: (f32, f32, f32, f32)) -> Rect {
    Rect::new(r.0, r.1, r.2, r.3)
}

/// The editor: GoodLookin DRI parts on a [`SceneCanvas`], with its hit areas
/// and gestures in a [`ControlSet`].
pub struct GainEditor {
    controls: ControlSet,
    shared: Option<Arc<Shared>>,
    level: f32,
    canvas: SceneCanvas,
}

impl GainEditor {
    pub fn new(host: EditorHost) -> Self {
        let controls = ControlSet::new(host)
            .with(Control::knob(Area::from_tuple(KNOB), 0, &PARAMS[0]).drag_range(DRAG_RANGE))
            .with(Control::toggle(Area::from_tuple(SWITCH), 1, &PARAMS[1]));
        Self { controls, shared: None, level: 0.0, canvas: SceneCanvas::new(WIDTH as f32, HEIGHT as f32) }
    }

    pub fn values(&self) -> [f32; 2] {
        let v = |i: usize| self.controls.for_param(i).map_or(PARAMS[i].default, |c| c.value());
        [v(0), v(1)]
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
        (WIDTH, HEIGHT)
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

    fn render(&mut self, scale: f32) -> Frame<'_> {
        let level_db = 20.0 * self.level.max(1e-6).log10();
        let controls = &self.controls;
        // The plate is static: painted once per scale, then reused under the moving parts.
        let plate = |dl: &mut openplugin::dri::DrawList| draw_faceplate_code(dl, WIDTH as f32, HEIGHT as f32, "Navy");
        self.canvas.frame_layered(scale, 0, plate, |dl| {
            let mut parts = Scene::new();
            for c in controls.iter() {
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
                            openplugin::dri::goodlookindri::scene::Point::new(x + w * 0.5, y + h + 16.0),
                            "WARM",
                            GlColor(0xffffffff),
                            11.0,
                            true,
                        );
                    }
                    _ => {}
                }
            }
            meters::draw_led_ladder(&mut parts, rect(METER), false, level_db, level_db, 16);
            dl.append(parts);
        })
    }

    fn modifiers_changed(&mut self, modifiers: Modifiers) {
        self.controls.set_modifiers(modifiers);
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
}

openplugin::targets::export_clap!(EditorGain, editor = make_editor);
openplugin::targets::export_lv2!(EditorGain, editor = make_editor);
openplugin::targets::export_shell!(EditorGain, editor = make_editor);
