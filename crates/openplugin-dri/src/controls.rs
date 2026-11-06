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
