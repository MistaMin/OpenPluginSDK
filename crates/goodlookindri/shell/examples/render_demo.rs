//! One complete plugin faceplate built only from the toolkit's parts: a rack panel with ears
//! and screws, lit knobs with printed scales, a backlit VU meter, a jewel lamp, toggles, lit
//! keys, an LED ladder and a small spectrum. 1100 x 420 logical units.
//! Usage: cargo run --release -p goodlookindri-shell --example render_demo -- <out.ppm> [silver|blue] [scale]

use goodlookindri::color::Color;
use goodlookindri::draw::{self, KnobMode};
use goodlookindri::hardware::*;
use goodlookindri::light::LightRig;
use goodlookindri::meters::{self, LampBezel, VuFace};
use goodlookindri::scene::{Point, Rect, Scene};
use goodlookindri::spectrum_draw::{SpectrumPalette, SpectrumSettings, SpectrumStyle, SpectrumView};
use goodlookindri::switches::{self, PushStyle, ToggleStyle};
use goodlookindri_shell::raster::Framebuffer;

const W: f32 = 1100.0;
const H: f32 = 420.0;

/// Everything that changes between colourways.
struct Look {
    panel: (u32, u32),
    finish: i32,
    ear: (u32, u32),
    /// Legend ink, and whether the knobs' own marks are dark.
    ink: u32,
    dark_marks: bool,
    title_fill: u32,
    accent: u32,
    big: &'static str,
    medium: &'static str,
    blend: &'static str,
    face: VuFace,
    lamp: u32,
    key: u32,
    palette: SpectrumPalette,
    backdrop: u32,
}

const SILVER: Look = Look {
    panel: (0xffdfe2e4, 0xffc3c8cb), finish: FINISH_BRUSHED, ear: (0xffd8dbdd, 0xffb9bec1),
    ink: 0xff1c2022, dark_marks: true, title_fill: 0xff23272a, accent: 0xffc0392b,
    big: "Knl", medium: "Rck", blend: "Dbl", face: VuFace::Cream, lamp: 0xffff3b30, key: 0xffffa52e,
    palette: SpectrumPalette::Amber, backdrop: 0xff14181b,
};
const BLUE: Look = Look {
    panel: (0xff263c86, 0xff182a66), finish: FINISH_ANODISED, ear: (0xff2d2e31, 0xff141516),
    ink: 0xfff0e7cd, dark_marks: false, title_fill: 0xfff0e7cd, accent: 0xfff0e7cd,
    big: "Ivr", medium: "Wh", blend: "Brz", face: VuFace::Black, lamp: 0xffffb02e, key: 0xff39d8ff,
    palette: SpectrumPalette::Cyan, backdrop: 0xff101317,
};

/// A lit knob with a printed scale round it and its name underneath. The scale is centred on
/// the knob's silhouette (tall knobs lean up the screen) and sits just clear of it; the styles
/// used here print no scale of their own.
fn knob(sc: &mut Scene, look: &Look, code: &str, c: Point, r: f32, value: f32, majors: usize, minors: usize, labels: &[&str], dots: bool, name: &str, name_y: f32) {
    let (lift, reach) = goodlookindri::knob3d::style_model(code).map_or((0.0, 1.0), |m| m.visual_frame(&goodlookindri::knob3d::Rig::studio()));
    let legend = Legend::Silk(Color(look.ink));
    let mut ring = ScaleRing::new(Point::new(c.x, c.y - lift * r), r * reach.max(1.0) * 1.10 + 4.0, majors, minors, labels);
    ring.legend = legend;
    ring.font_size = (r * 0.27).clamp(8.5, 10.0);
    ring.tick = (r * 0.15).clamp(3.0, 5.5);
    if dots { ring.marks = ScaleMarks::Dots; }
    draw_scale_ring_with(sc, &ring);
    let angle = (value.clamp(0.0, 1.0) * 1.6 - 0.8) * std::f32::consts::PI;
    draw::knob_named_mode(sc, code, c, r, angle, Color(look.accent), look.dark_marks, KnobMode::Lit);
    legend_text(sc, legend, Point::new(c.x, name_y), name, 10.5, true);
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let out = a.get(1).cloned().unwrap_or_else(|| "demo.ppm".into());
    let look = if a.get(2).map(String::as_str) == Some("blue") { &BLUE } else { &SILVER };
    let scale: f32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    set_device_scale(scale);
    draw::set_knob_scale(scale);
    let plate = Color(look.panel.0).mix(Color(look.panel.1), 0.5);
    draw::set_knob_floor(plate);

    let mut sc = Scene::new();
    let rig = LightRig::studio();
    let ink = Color(look.ink);
    let silk = Legend::Silk(ink);
    let label = |sc: &mut Scene, x: f32, y: f32, text: &str, size: f32| legend_text(sc, silk, Point::new(x, y), text, size, true);

    // Chassis: ears, plate, screws.
    let panel = Rect::new(44.0, 14.0, 1012.0, 392.0);
    draw_rack_ear_with(&mut sc, Rect::new(10.0, panel.y, 36.0, panel.h), true, Color(look.ear.0), Color(look.ear.1), &rig);
    draw_panel(&mut sc, panel, Color(look.panel.0), Color(look.panel.1), look.finish);
    draw_rack_ear_with(&mut sc, Rect::new(panel.right() - 2.0, panel.y, 36.0, panel.h), false, Color(look.ear.0), Color(look.ear.1), &rig);
    draw_corner_screws(&mut sc, panel, ScrewHead::Cross);

    // Five columns carry everything in the middle of the plate.
    let col = |i: usize| 300.0 + 125.0 * i as f32;
    let (top_y, low_y) = (112.0, 292.0);

    // Left block: title, power and bypass, pilot lamp.
    legend_text_on(&mut sc, Legend::Engraved(Color(look.title_fill)), plate, Point::new(78.0, 52.0), "MODEL ONE", 19.0, false);
    legend_text(&mut sc, silk, Point::new(79.0, 72.0), "STEREO  COMPRESSOR", 8.5, false);
    switches::draw_toggle(&mut sc, Rect::new(78.0, 100.0, 40.0, 70.0), true, ToggleStyle::Bat, Color(look.lamp));
    switches::draw_toggle(&mut sc, Rect::new(132.0, 100.0, 40.0, 70.0), false, ToggleStyle::Bat, Color(look.lamp));
    label(&mut sc, 98.0, 98.0, "ON", 7.5);
    label(&mut sc, 152.0, 98.0, "IN", 7.5);
    label(&mut sc, 98.0, 186.0, "POWER", 9.0);
    label(&mut sc, 152.0, 186.0, "BYPASS", 9.0);
    meters::draw_jewel(&mut sc, Point::new(203.0, 139.0), 8.0, Color(look.lamp), 1.0, LampBezel::Chrome);
    label(&mut sc, 203.0, 186.0, "PILOT", 9.0);

    // Top row: input and output either side of the meter.
    knob(&mut sc, look, look.big, Point::new(col(0), top_y), 36.0, 0.42, 5, 4, &["-20", "-10", "0", "+10", "+20"], false, "INPUT", 186.0);
    meters::draw_vu_meter_lit(&mut sc, Rect::new(col(2) - 108.0, 34.0, 216.0, 138.0), 0.56, look.face, 1.0);
    label(&mut sc, col(2), 186.0, "GAIN  REDUCTION", 9.0);
    knob(&mut sc, look, look.big, Point::new(col(4), top_y), 36.0, 0.63, 5, 4, &["-20", "-10", "0", "+10", "+20"], false, "OUTPUT", 186.0);

    // Rule between the rows.
    draw_legend_line(&mut sc, ink, col(0) - 62.0, col(4) + 62.0, 214.0, "DYNAMICS", 8.5);

    // Lower row: five controls on the same columns.
    knob(&mut sc, look, look.medium, Point::new(col(0), low_y), 27.0, 0.35, 5, 1, &["2", "4", "8", "12", "20"], false, "RATIO", 364.0);
    knob(&mut sc, look, look.medium, Point::new(col(1), low_y), 27.0, 0.25, 7, 0, &["FAST", "SLOW"], true, "ATTACK", 364.0);
    knob(&mut sc, look, look.medium, Point::new(col(2), low_y), 27.0, 0.7, 7, 0, &["FAST", "SLOW"], true, "RELEASE", 364.0);
    knob(&mut sc, look, look.medium, Point::new(col(3), low_y), 27.0, 0.5, 7, 0, &["HARD", "SOFT"], true, "KNEE", 364.0);
    knob(&mut sc, look, look.blend, Point::new(col(4), low_y), 27.0, 0.8, 11, 0, &["DRY", "WET"], false, "MIX", 364.0);

    // Lower left: meter select and sidechain keys.
    label(&mut sc, 151.0, 248.0, "METER", 9.0);
    switches::draw_key_bank(&mut sc, Rect::new(78.0, 262.0, 146.0, 28.0), &["GR", "IN", "OUT"], 0, PushStyle::Square, Color(look.key), true);
    meters::draw_lit_key(&mut sc, Rect::new(78.0, 322.0, 68.0, 26.0), "LINK", true, Color(0xff5be85a), false);
    meters::draw_lit_key(&mut sc, Rect::new(156.0, 322.0, 68.0, 26.0), "EXT", false, Color(0xff5be85a), false);
    label(&mut sc, 151.0, 364.0, "SIDECHAIN", 9.0);

    // Right block: output level ladder above, sidechain spectrum below.
    meters::draw_stereo_ladder(&mut sc, Rect::new(921.0, 36.0, 70.0, 134.0), -7.0, -10.0, -3.0);
    label(&mut sc, 956.0, 186.0, "LEVEL", 9.0);
    let n = 44usize;
    let cols: Vec<f32> = (0..n).map(|i| {
        let f = i as f32 / n as f32;
        -74.0 + 44.0 * (-(f - 0.2).powi(2) / 0.012).exp() + 32.0 * (-(f - 0.52).powi(2) / 0.02).exp() + 16.0 * (-(f - 0.8).powi(2) / 0.006).exp() + 3.0 * (i as f32 * 2.3).sin()
    }).collect();
    let hold: Vec<f32> = cols.iter().map(|v| (v + 7.0).min(0.0)).collect();
    let mut st = SpectrumSettings::default();
    st.style = SpectrumStyle::Bars;
    st.palette = look.palette;
    st.labels = false;
    SpectrumView::paint_columns(&mut sc, Rect::new(884.0, 246.0, 144.0, 96.0), &st, &cols, Some(&hold), None, 20.0, 20000.0, 0.0);
    label(&mut sc, 956.0, 364.0, "DETECTOR", 9.0);

    let mut fb = Framebuffer::new((W * scale) as usize, (H * scale) as usize);
    fb.clear(Color(look.backdrop));
    fb.replay(&sc, scale);
    std::fs::write(&out, fb.to_ppm()).expect("write ppm");
}
