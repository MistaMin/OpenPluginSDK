//! A gain plugin with an editor, exported in every format the SDK has.
//!
//! The engine shares a peak meter with its editor through
//! [`Plugin::editor_state`]. The editor draws its own pixels: a fader for the
//! gain (drag it; double-click resets), a switch for Warm (click it), and the
//! meter. Its changes go through [`EditorHost`] as gestures, and the host's
//! values come back through [`Editor::param_changed`]. The same editor runs in
//! CLAP (`clap.gui`), LV2 (`ui:CocoaUI`), and the VST3, Audio Unit and AAX
//! shells (`op_editor_*`). The factory presets are text files in `presets/`,
//! embedded by `build.rs`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use openplugin::core::{Editor, EditorHost, EditorState, Frame, PresetBank};
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
    version: "0.3.0",
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
const FADER: (f32, f32, f32, f32) = (20.0, 20.0, 40.0, 120.0); // x, y, w, h
const SWITCH: (f32, f32, f32, f32) = (90.0, 20.0, 60.0, 30.0);
const METER: (f32, f32, f32, f32) = (180.0, 20.0, 20.0, 120.0);

fn inside(r: (f32, f32, f32, f32), x: f32, y: f32) -> bool {
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}

/// The editor: three controls drawn straight into a pixel buffer.
pub struct GainEditor {
    host: EditorHost,
    values: [f32; 2],
    shared: Option<Arc<Shared>>,
    level: f32,
    drag: Option<(f32, f32)>,
    pixels: Vec<u32>,
    width: usize,
    height: usize,
}

impl GainEditor {
    pub fn new(host: EditorHost) -> Self {
        Self {
            host,
            values: [PARAMS[0].default, PARAMS[1].default],
            shared: None,
            level: 0.0,
            drag: None,
            pixels: Vec::new(),
            width: 0,
            height: 0,
        }
    }

    pub fn values(&self) -> [f32; 2] {
        self.values
    }

    pub fn is_bound(&self) -> bool {
        self.shared.is_some()
    }

    fn fill(&mut self, r: (f32, f32, f32, f32), scale: f32, color: u32) {
        let x0 = (r.0 * scale) as usize;
        let y0 = (r.1 * scale) as usize;
        let x1 = (((r.0 + r.2) * scale) as usize).min(self.width);
        let y1 = (((r.1 + r.3) * scale) as usize).min(self.height);
        for y in y0..y1 {
            self.pixels[y * self.width + x0.min(x1)..y * self.width + x1].fill(color);
        }
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
        if let Some(slot) = self.values.get_mut(index) {
            *slot = PARAMS[index].clamp(plain);
        }
    }

    fn tick(&mut self, dt: f32) {
        let peak = self.shared.as_ref().map(|s| s.take_peak()).unwrap_or(0.0);
        // A meter that rises at once and falls at 20 dB a second.
        let fall = 10f32.powf(-20.0 * dt / 20.0);
        self.level = peak.max(self.level * fall);
    }

    fn render(&mut self, scale: f32) -> Frame<'_> {
        self.width = (WIDTH as f32 * scale) as usize;
        self.height = (HEIGHT as f32 * scale) as usize;
        self.pixels.resize(self.width * self.height, 0);
        self.pixels.fill(0x0020_2428);
        self.fill(FADER, scale, 0x0038_3c44);
        let t = PARAMS[0].to_normalized(self.values[0]);
        let (x, y, w, h) = FADER;
        self.fill((x, y + h * (1.0 - t), w, h * t), scale, 0x0040_a0e0);
        let warm = if self.values[1] > 0.5 { 0x00e0_8030 } else { 0x0050_5560 };
        self.fill(SWITCH, scale, warm);
        self.fill(METER, scale, 0x0010_1214);
        let db = 20.0 * self.level.max(1e-6).log10();
        let m = ((db + 60.0) / 66.0).clamp(0.0, 1.0);
        let (x, y, w, h) = METER;
        self.fill((x, y + h * (1.0 - m), w, h * m), scale, 0x0060_d070);
        Frame { pixels: &self.pixels, width: self.width, height: self.height }
    }

    fn mouse_down(&mut self, x: f32, y: f32, clicks: u32) {
        if inside(FADER, x, y) {
            if clicks >= 2 {
                self.values[0] = PARAMS[0].default;
                self.host.set(0, self.values[0]);
                return;
            }
            self.host.begin(0);
            self.drag = Some((y, PARAMS[0].to_normalized(self.values[0])));
        } else if inside(SWITCH, x, y) {
            self.values[1] = if self.values[1] > 0.5 { 0.0 } else { 1.0 };
            self.host.set(1, self.values[1]);
        }
    }

    fn mouse_move(&mut self, _x: f32, y: f32) {
        if let Some((start_y, start)) = self.drag {
            let t = (start + (start_y - y) / FADER.3).clamp(0.0, 1.0);
            let v = PARAMS[0].from_normalized(t);
            if v != self.values[0] {
                self.values[0] = v;
                self.host.perform(0, v);
            }
        }
    }

    fn mouse_up(&mut self, _x: f32, _y: f32) {
        if self.drag.take().is_some() {
            self.host.end(0);
        }
    }
}

openplugin::targets::export_clap!(EditorGain, editor = make_editor);
openplugin::targets::export_lv2!(EditorGain, editor = make_editor);
openplugin::targets::export_shell!(EditorGain, editor = make_editor);
