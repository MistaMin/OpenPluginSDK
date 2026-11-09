//! Hardware controls. Geometry is drawn by GoodLookinDri. The names on [`crate::KnobStyle`]
//! and [`crate::FaceplateStyle`] pick a GoodLookin style. Every other style is available
//! through [`draw_knob_code`] and [`draw_faceplate_code`].

use goodlookindri::color::Color as GlColor;
use goodlookindri::scene::{Cmd, Point, Rect, Scene};
use goodlookindri::{design, hardware, knobs, meters};

use crate::canvas::DrawList;
use crate::styles::{Color, FaceplateStyle, KnobStyle, MeterStyle};

fn gl_color(color: Color) -> GlColor {
    GlColor::rgba(
        (color.r.clamp(0.0, 1.0) * 255.0).round() as u32,
        (color.g.clamp(0.0, 1.0) * 255.0).round() as u32,
        (color.b.clamp(0.0, 1.0) * 255.0).round() as u32,
        (color.a.clamp(0.0, 1.0) * 255.0).round() as u32,
    )
}

fn sdk_color(color: GlColor) -> Color {
    Color::rgba(
        color.r() as f32 / 255.0,
        color.g() as f32 / 255.0,
        color.b() as f32 / 255.0,
        color.a() as f32 / 255.0,
    )
}

fn commit(dl: &mut DrawList, scene: Scene) {
    for cmd in &scene.cmds {
        match cmd {
            Cmd::FillEllipse { rect, top, .. } => {
                let center = rect.center();
                dl.fill_circle(center.x, center.y, rect.w.min(rect.h) * 0.5, sdk_color(*top));
            }
            Cmd::StrokeEllipse { rect, color, width } => {
                let center = rect.center();
                dl.stroke_circle(center.x, center.y, rect.w.min(rect.h) * 0.5, sdk_color(*color), *width);
            }
            Cmd::FillRoundRect { rect, radius, top, .. } => {
                dl.fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, *radius, sdk_color(*top));
            }
            Cmd::FillRect { rect, top, .. } => {
                dl.fill_rect(rect.x, rect.y, rect.w, rect.h, sdk_color(*top));
            }
            Cmd::StrokeRoundRect { rect, radius, color, width } => {
                dl.stroke_rounded_rect(rect.x, rect.y, rect.w, rect.h, *radius, sdk_color(*color), *width);
            }
            Cmd::Line { a, b, color, width } => dl.line(a.x, a.y, b.x, b.y, sdk_color(*color), *width),
            Cmd::StrokeArc { center, radius, from, to, color, width } => {
                dl.arc(center.x, center.y, *radius, *from, *to, sdk_color(*color), *width);
            }
            Cmd::Text { at, text, color, size, center } => {
                dl.text(text.clone(), at.x, at.y, *size, sdk_color(*color), *center);
            }
            Cmd::FillPoly { pts, top, .. } if pts.len() >= 2 => {
                dl.line(pts[0].x, pts[0].y, pts[1].x, pts[1].y, sdk_color(*top), 1.0);
            }
            Cmd::StrokePoly { pts, color, width, .. } if pts.len() >= 2 => {
                dl.line(pts[0].x, pts[0].y, pts[1].x, pts[1].y, sdk_color(*color), *width);
            }
            _ => {}
        }
    }
    dl.scene.cmds.extend(scene.cmds);
}

fn knob_code(style: KnobStyle) -> &'static str {
    match style {
        KnobStyle::Brit => "Brit",
        KnobStyle::N => "N",
        KnobStyle::A => "A",
        KnobStyle::Bakelite => "Bk",
        KnobStyle::Metal => "Mtl",
        KnobStyle::Chrome => "Chr",
        KnobStyle::Wood => "Wd",
        KnobStyle::Neon => "Neo",
        KnobStyle::Digital => "Seg",
    }
}

fn plate_code(style: FaceplateStyle) -> &'static str {
    match style {
        FaceplateStyle::BrushedAluminum => "Silver",
        FaceplateStyle::DarkAnodized => "Stealth",
        FaceplateStyle::BritishConsole => "Navy",
        FaceplateStyle::AmericanConsole => "Slate",
        FaceplateStyle::Cyberpunk => "Cyber",
        FaceplateStyle::WoodChassis => "Walnut",
    }
}

/// Travel for a rotary control. 0 is the start of the sweep, 0.5 points up, 1 is the end.
fn travel_angle(normalized: f32) -> f32 {
    (-135.0 + normalized.clamp(0.0, 1.0) * 270.0).to_radians()
}

pub fn draw_faceplate(dl: &mut DrawList, w: f32, h: f32, style: FaceplateStyle) {
    draw_faceplate_code(dl, w, h, plate_code(style));
}

/// Draws one of the 40 GoodLookin faceplates by its code (`"Navy"`, `"Walnut"`, `"Cyber"`, ...).
pub fn draw_faceplate_code(dl: &mut DrawList, w: f32, h: f32, code: &str) {
    let Some(plate) = design::find_faceplate(code) else { return };
    let mut scene = Scene::new();
    hardware::draw_panel(
        &mut scene,
        Rect::new(0.0, 0.0, w, h),
        GlColor(plate.bg_top),
        GlColor(plate.bg_bottom),
        plate.finish,
    );
    commit(dl, scene);
}

pub fn draw_knob(
    dl: &mut DrawList,
    cx: f32,
    cy: f32,
    radius: f32,
    normalized: f32,
    label: &str,
    value_text: &str,
    style: KnobStyle,
    cap_color: Color,
) {
    draw_knob_code(dl, cx, cy, radius, normalized, label, value_text, knob_code(style), cap_color);
}

/// Draws any GoodLookin knob by style code (`"Brit"`, `"Chk"`, `"Rck"`, ...).
pub fn draw_knob_code(
    dl: &mut DrawList,
    cx: f32,
    cy: f32,
    radius: f32,
    normalized: f32,
    label: &str,
    value_text: &str,
    style_code: &str,
    cap_color: Color,
) {
    let mut scene = Scene::new();
    knobs::draw_style(
        &mut scene,
        style_code,
        Point::new(cx, cy),
        radius,
        travel_angle(normalized),
        gl_color(cap_color),
        false,
    );
    scene.text(Point::new(cx, cy + radius + 14.0), label, GlColor(0xffffffff), 11.0, true);
    scene.text(
        Point::new(cx, cy + radius + 26.0),
        value_text,
        GlColor(0xffb3cccc),
        10.0,
        true,
    );
    commit(dl, scene);
}

pub fn draw_vu_meter(dl: &mut DrawList, x: f32, y: f32, w: f32, h: f32, level_db: f32, style: MeterStyle) {
    let mut scene = Scene::new();
    let rect = Rect::new(x, y, w, h);
    match style {
        MeterStyle::AnalogVu => {
            meters::draw_vu_meter(&mut scene, rect, meters::vu_position_for_db(level_db), meters::VuFace::Cream);
        }
        MeterStyle::Ppm => {
            meters::draw_ppm(&mut scene, rect, meters::db_position(level_db, -60.0, 0.0));
        }
        MeterStyle::LedLadder => {
            meters::draw_led_ladder(&mut scene, rect, false, level_db, level_db, 12);
        }
        MeterStyle::RetroBar => {
            meters::draw_segment_arc(&mut scene, rect, meters::db_position(level_db, -48.0, 0.0), GlColor(0xffff9a1f));
        }
    }
    commit(dl, scene);
}

/// Fader taper conversion: 0.0..1.0 position to dB (-144.0 .. +12.0)
pub fn fader_pos_to_db(pos: f32) -> f32 {
    let p = if pos.is_nan() { 0.0 } else { pos.clamp(0.0, 1.0) };
    if p <= 0.0 {
        return -144.0;
    }
    if p >= 0.25 {
        -24.0 + (p - 0.25) / 0.75 * 36.0
    } else {
        -24.0 - (1.0 - p / 0.25).powf(1.5) * 120.0
    }
}

/// Fader taper conversion: dB to 0.0..1.0 position
pub fn fader_db_to_pos(db: f32) -> f32 {
    let db = if db.is_nan() { -144.0 } else { db };
    if db <= -144.0 {
        return 0.0;
    }
    if db >= -24.0 {
        (0.25 + (db.min(12.0) + 24.0) / 36.0 * 0.75).min(1.0)
    } else {
        let x = ((-24.0 - db) / 120.0).clamp(0.0, 1.0).powf(1.0 / 1.5);
        ((1.0 - x) * 0.25).max(0.0)
    }
}

pub fn draw_fader(dl: &mut DrawList, cx: f32, y: f32, height: f32, db: f32, label: &str) {
    let mut scene = Scene::new();
    let travel = height.max(36.0);
    meters::draw_slot_fader(
        &mut scene,
        Rect::new(cx - travel * 0.5, y, travel, 40.0),
        fader_db_to_pos(db),
        meters::FaderCap::Console,
        GlColor(0xffb8bcc0),
    );
    scene.text(Point::new(cx, y + 52.0), label, GlColor(0xffffffff), 10.0, true);
    let readout = if db <= -90.0 { "-inf dB".to_string() } else { format!("{db:.1} dB") };
    scene.text(Point::new(cx, y + 66.0), readout, GlColor(0xffb3cccc), 9.0, true);
    commit(dl, scene);
}

pub fn draw_slider(dl: &mut DrawList, x: f32, y: f32, w: f32, normalized: f32, label: &str, value_text: &str) {
    let mut scene = Scene::new();
    meters::draw_slot_fader(
        &mut scene,
        Rect::new(x, y, w, 28.0),
        normalized,
        meters::FaderCap::Oval,
        GlColor(0xffe6eaee),
    );
    scene.text(Point::new(x, y - 10.0), label, GlColor(0xffffffff), 10.0, false);
    scene.text(Point::new(x + w, y - 10.0), value_text, GlColor(0xffb3cccc), 10.0, false);
    commit(dl, scene);
}

pub fn draw_pan_knob(dl: &mut DrawList, cx: f32, cy: f32, radius: f32, pan: f32, label: &str) {
    let p = pan.clamp(-1.0, 1.0);
    draw_knob_code(
        dl,
        cx,
        cy,
        radius,
        p * 0.5 + 0.5,
        label,
        &format_pan(p),
        "Ptr",
        Color::rgb(0.75, 0.77, 0.80),
    );
}

fn format_pan(p: f32) -> String {
    let percent = (p * 100.0).round() as i32;
    if percent == 0 {
        "< C >".to_string()
    } else if percent < 0 {
        format!("< L{}", -percent)
    } else {
        format!("R{} >", percent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fader_taper_roundtrip() {
        for db in [12.0, 6.0, 0.0, -6.0, -12.0, -24.0, -48.0, -96.0] {
            let pos = fader_db_to_pos(db);
            let recovered = fader_pos_to_db(pos);
            assert!((recovered - db).abs() < 0.1, "Mismatch at {db} dB: recovered {recovered}");
        }
        assert_eq!(fader_pos_to_db(0.0), -144.0);
        assert_eq!(fader_db_to_pos(f32::NAN), 0.0);
    }

    #[test]
    fn brit_knob_uses_goodlookin_geometry() {
        let mut dl = DrawList::new();
        draw_knob(&mut dl, 80.0, 80.0, 32.0, 0.5, "GAIN", "0.0 dB", KnobStyle::Brit, Color::BRIT_RED);
        assert!(dl.scene.cmds.len() > 20, "Brit drawer produced {} commands", dl.scene.cmds.len());
        assert!(dl.commands.len() > 10);
    }

    #[test]
    fn catalog_is_the_goodlookin_set() {
        assert_eq!(design::KNOB_STYLES.len(), 47);
        assert_eq!(design::FACEPLATES.len(), 40);
        let mut dl = DrawList::new();
        draw_faceplate(&mut dl, 200.0, 80.0, FaceplateStyle::BritishConsole);
        draw_knob_code(&mut dl, 40.0, 40.0, 16.0, 0.2, "Q", "1.2", "Chk", Color::WHITE);
        assert!(!dl.scene.cmds.is_empty());
    }

    #[test]
    fn test_draw_fader_and_slider() {
        let mut dl = DrawList::new();
        draw_fader(&mut dl, 50.0, 10.0, 140.0, 0.0, "MASTER");
        draw_slider(&mut dl, 100.0, 50.0, 120.0, 0.75, "DRIVE", "75%");
        draw_pan_knob(&mut dl, 250.0, 70.0, 24.0, -0.5, "PAN");
        assert!(dl.commands.len() > 10);
    }
}
