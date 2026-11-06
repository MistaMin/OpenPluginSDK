//! Procedural Hardware Control Renderers (Knobs, VU Meters, Faceplates).

use std::f32::consts::PI;
use crate::canvas::DrawList;
use crate::styles::{Color, FaceplateStyle, KnobStyle, MeterStyle};

/// Draws a procedural faceplate texture.
pub fn draw_faceplate(dl: &mut DrawList, w: f32, h: f32, style: FaceplateStyle) {
    match style {
        FaceplateStyle::BrushedAluminum => {
            dl.fill_rect(0.0, 0.0, w, h, Color::rgb(0.82, 0.83, 0.85));
            // Subtle brushed grain lines
            let mut y = 2.0;
            while y < h {
                dl.line(0.0, y, w, y, Color::rgba(1.0, 1.0, 1.0, 0.12), 1.0);
                y += 4.0;
            }
        }
        FaceplateStyle::DarkAnodized => {
            dl.fill_rect(0.0, 0.0, w, h, Color::rgb(0.12, 0.13, 0.14));
            // Outer bezel stroke
            dl.stroke_rect(1.0, 1.0, w - 2.0, h - 2.0, Color::rgb(0.25, 0.26, 0.28), 1.5);
        }
        FaceplateStyle::BritishConsole => {
            dl.fill_rect(0.0, 0.0, w, h, Color::rgb(0.18, 0.24, 0.22));
            dl.stroke_rect(2.0, 2.0, w - 4.0, h - 4.0, Color::rgb(0.30, 0.38, 0.34), 2.0);
        }
        FaceplateStyle::AmericanConsole => {
            dl.fill_rect(0.0, 0.0, w, h, Color::rgb(0.20, 0.22, 0.26));
            dl.stroke_rect(1.0, 1.0, w - 2.0, h - 2.0, Color::rgb(0.35, 0.38, 0.44), 1.5);
        }
        FaceplateStyle::Cyberpunk => {
            dl.fill_rect(0.0, 0.0, w, h, Color::rgb(0.05, 0.05, 0.08));
            dl.stroke_rect(1.0, 1.0, w - 2.0, h - 2.0, Color::rgba(0.0, 0.95, 1.0, 0.6), 1.5);
        }
        FaceplateStyle::WoodChassis => {
            dl.fill_rect(0.0, 0.0, w, h, Color::rgb(0.36, 0.22, 0.14));
        }
    }
}

/// Draws an authentic procedural hardware knob.
pub fn draw_knob(
    dl: &mut DrawList,
    cx: f32,
    cy: f32,
    radius: f32,
    normalized: f32, // 0.0 .. 1.0
    label: &str,
    value_text: &str,
    style: KnobStyle,
    cap_color: Color,
) {
    let t = normalized.clamp(0.0, 1.0);
    // 270 degree rotation sweep (from -135 deg to +135 deg)
    let min_angle = -135.0_f32.to_radians();
    let max_angle = 135.0_f32.to_radians();
    let angle = min_angle + t * (max_angle - min_angle);

    // 1. Drop shadow
    dl.fill_circle(cx + 1.5, cy + 2.5, radius, Color::rgba(0.0, 0.0, 0.0, 0.35));

    // 2. Base skirt
    let skirt_color = match style {
        KnobStyle::Brit | KnobStyle::N | KnobStyle::Bakelite => Color::rgb(0.12, 0.12, 0.12),
        KnobStyle::Metal | KnobStyle::Chrome => Color::rgb(0.65, 0.66, 0.68),
        KnobStyle::Wood => Color::rgb(0.25, 0.14, 0.08),
        KnobStyle::Neon | KnobStyle::Digital => Color::rgb(0.08, 0.09, 0.12),
        KnobStyle::A => Color::rgb(0.20, 0.20, 0.22),
    };
    dl.fill_circle(cx, cy, radius, skirt_color);
    dl.stroke_circle(cx, cy, radius, Color::rgba(0.0, 0.0, 0.0, 0.4), 1.0);

    // 3. Fluted grooves or knurling ring
    let inner_radius = radius * 0.82;
    dl.fill_circle(cx, cy, inner_radius, cap_color);

    // 4. Pointer indicator line
    let p_len = inner_radius * 0.85;
    let (sin_a, cos_a) = angle.sin_cos();
    let px = cx + sin_a * p_len;
    let py = cy - cos_a * p_len;
    let indicator_color = if style == KnobStyle::Bakelite || style == KnobStyle::Brit {
        Color::WHITE
    } else {
        Color::BLACK
    };
    dl.line(cx, cy, px, py, indicator_color, 2.5);

    // 5. Specular highlight across upper body
    dl.arc(cx, cy, inner_radius - 1.0, -PI * 0.75, -PI * 0.25, Color::rgba(1.0, 1.0, 1.0, 0.3), 1.5);

    // 6. Labels
    dl.text(label, cx, cy + radius + 14.0, 11.0, Color::WHITE, true);
    dl.text(value_text, cx, cy + radius + 26.0, 10.0, Color::rgb(0.7, 0.8, 0.8), true);
}

/// Draws an authentic ballistic analog VU meter.
pub fn draw_vu_meter(
    dl: &mut DrawList,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    level_db: f32,
    style: MeterStyle,
) {
    // 1. Bezel frame
    dl.fill_rounded_rect(x, y, w, h, 4.0, Color::rgb(0.10, 0.10, 0.12));
    dl.stroke_rounded_rect(x, y, w, h, 4.0, Color::rgb(0.3, 0.3, 0.35), 1.5);

    // 2. Yellow illuminated meter face
    let inset = 4.0;
    let face_w = w - inset * 2.0;
    let face_h = h - inset * 2.0;
    let face_color = match style {
        MeterStyle::AnalogVu => Color::VU_YELLOW,
        MeterStyle::Ppm => Color::BLACK,
        MeterStyle::LedLadder | MeterStyle::RetroBar => Color::rgb(0.04, 0.05, 0.07),
    };
    dl.fill_rounded_rect(x + inset, y + inset, face_w, face_h, 2.0, face_color);

    // 3. Needle
    // Map -20 dB .. +3 dB onto arc angles
    let norm = ((level_db + 20.0) / 23.0).clamp(0.0, 1.0);
    let min_needle = -40.0_f32.to_radians();
    let max_needle = 40.0_f32.to_radians();
    let needle_angle = min_needle + norm * (max_needle - min_needle);

    let pivot_x = x + w * 0.5;
    let pivot_y = y + h * 1.35; // virtual pivot below meter
    let needle_len = h * 1.1;
    let (sin_n, cos_n) = needle_angle.sin_cos();
    let tip_x = pivot_x + sin_n * needle_len;
    let tip_y = pivot_y - cos_n * needle_len;

    dl.line(pivot_x, pivot_y, tip_x, tip_y, Color::rgb(0.85, 0.1, 0.1), 1.5);

    // 4. Dial text
    let text_color = if style == MeterStyle::AnalogVu { Color::BLACK } else { Color::WHITE };
    dl.text("VU", x + w - 24.0, y + 18.0, 10.0, text_color, false);
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

/// Draws an authentic studio console mixer fader with calibrated dB scale.
pub fn draw_fader(
    dl: &mut DrawList,
    cx: f32,
    y: f32,
    height: f32,
    db: f32,
    label: &str,
) {
    let cap_h = 36.0;
    let cap_w = 22.0;
    let slot_h = height - cap_h;
    let slot_w = 4.0;
    let slot_y = y + cap_h * 0.5;

    // 1. Fader slot track with inner shadow
    dl.fill_rounded_rect(cx - slot_w * 0.5, slot_y, slot_w, slot_h, 2.0, Color::rgb(0.08, 0.08, 0.09));
    dl.stroke_rounded_rect(cx - slot_w * 0.5, slot_y, slot_w, slot_h, 2.0, Color::rgb(0.22, 0.22, 0.25), 1.0);

    // 2. Calibrated scale markings
    let marks = [12.0, 6.0, 0.0, -5.0, -10.0, -20.0, -30.0, -40.0, -60.0];
    for &m in &marks {
        let pos = fader_db_to_pos(m);
        let my = slot_y + slot_h * (1.0 - pos);
        // Tick mark
        dl.line(cx - 14.0, my, cx - 6.0, my, Color::rgba(1.0, 1.0, 1.0, 0.4), 1.0);
        // Mark label
        if (m == 0.0) || (m == 12.0) || (m == -10.0) || (m == -30.0) || (m == -60.0) {
            dl.text(format!("{:.0}", m.abs()), cx - 18.0, my - 4.0, 8.0, Color::rgba(1.0, 1.0, 1.0, 0.6), false);
        }
    }

    // 3. Fader Cap
    let pos = fader_db_to_pos(db);
    let cap_y = slot_y + slot_h * (1.0 - pos) - cap_h * 0.5;

    // Drop shadow
    dl.fill_rounded_rect(cx - cap_w * 0.5 + 1.0, cap_y + 2.0, cap_w, cap_h, 3.0, Color::rgba(0.0, 0.0, 0.0, 0.45));
    // Metallic body
    dl.fill_rounded_rect(cx - cap_w * 0.5, cap_y, cap_w, cap_h, 3.0, Color::rgb(0.72, 0.73, 0.76));
    dl.stroke_rounded_rect(cx - cap_w * 0.5, cap_y, cap_w, cap_h, 3.0, Color::rgb(0.35, 0.36, 0.38), 1.0);

    // Center indicator line
    let center_line_y = cap_y + cap_h * 0.5;
    dl.line(cx - cap_w * 0.5 + 2.0, center_line_y, cx + cap_w * 0.5 - 2.0, center_line_y, Color::rgb(0.1, 0.1, 0.1), 2.0);

    // Top highlight line on cap
    dl.line(cx - cap_w * 0.5 + 3.0, cap_y + 2.0, cx + cap_w * 0.5 - 3.0, cap_y + 2.0, Color::rgba(1.0, 1.0, 1.0, 0.6), 1.0);

    // Label and readout
    dl.text(label, cx, y + height + 8.0, 10.0, Color::WHITE, true);
    let db_str = if db <= -90.0 { "-inf dB".to_string() } else { format!("{:.1} dB", db) };
    dl.text(db_str, cx, y + height + 20.0, 9.0, Color::rgb(0.7, 0.8, 0.8), true);
}

/// Draws a horizontal parameter slider.
pub fn draw_slider(
    dl: &mut DrawList,
    x: f32,
    y: f32,
    w: f32,
    normalized: f32,
    label: &str,
    value_text: &str,
) {
    let h = 8.0;
    let norm = normalized.clamp(0.0, 1.0);

    // Background slot
    dl.fill_rounded_rect(x, y, w, h, 4.0, Color::rgb(0.12, 0.13, 0.15));
    dl.stroke_rounded_rect(x, y, w, h, 4.0, Color::rgb(0.25, 0.26, 0.28), 1.0);

    // Active fill track
    let active_w = w * norm;
    if active_w > 4.0 {
        dl.fill_rounded_rect(x, y, active_w, h, 4.0, Color::rgb(0.2, 0.55, 0.85));
    }

    // Thumb knob
    let thumb_x = x + active_w;
    let thumb_radius = 8.0;
    dl.fill_circle(thumb_x, y + h * 0.5 + 1.0, thumb_radius, Color::rgba(0.0, 0.0, 0.0, 0.3));
    dl.fill_circle(thumb_x, y + h * 0.5, thumb_radius, Color::rgb(0.9, 0.92, 0.94));
    dl.stroke_circle(thumb_x, y + h * 0.5, thumb_radius, Color::rgb(0.3, 0.32, 0.35), 1.0);

    // Labels
    dl.text(label, x, y - 10.0, 10.0, Color::WHITE, false);
    dl.text(value_text, x + w, y - 10.0, 10.0, Color::rgb(0.7, 0.8, 0.8), false);
}

/// Draws a studio pan knob with an arc meter track around it (-1.0 .. +1.0).
pub fn draw_pan_knob(
    dl: &mut DrawList,
    cx: f32,
    cy: f32,
    radius: f32,
    pan: f32, // -1.0 .. +1.0
    label: &str,
) {
    let p = pan.clamp(-1.0, 1.0);
    let start_angle = PI * 0.75;
    let sweep = PI * 1.5;

    // Track arc
    dl.arc(cx, cy, radius + 4.0, start_angle, start_angle + sweep, Color::rgb(0.2, 0.22, 0.25), 2.0);

    // Active pan indicator arc from center
    let center_angle = start_angle + sweep * 0.5;
    let at_angle = start_angle + sweep * (p * 0.5 + 0.5);
    let (a0, a1) = if at_angle < center_angle { (at_angle, center_angle) } else { (center_angle, at_angle) };
    dl.arc(cx, cy, radius + 4.0, a0, a1, Color::rgb(0.3, 0.7, 1.0), 2.5);

    // Knob body
    draw_knob(dl, cx, cy, radius, p * 0.5 + 0.5, label, &format_pan(p), KnobStyle::Metal, Color::rgb(0.75, 0.77, 0.80));
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
    fn test_draw_fader_and_slider() {
        let mut dl = DrawList::new();
        draw_fader(&mut dl, 50.0, 10.0, 140.0, 0.0, "MASTER");
        draw_slider(&mut dl, 100.0, 50.0, 120.0, 0.75, "DRIVE", "75%");
        draw_pan_knob(&mut dl, 250.0, 70.0, 24.0, -0.5, "PAN");
        assert!(dl.commands.len() > 10);
    }
}
