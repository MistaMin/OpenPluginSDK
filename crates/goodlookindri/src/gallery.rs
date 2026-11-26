//! Catalogue of every part, matching the six gallery pages. No window: the host paints the scene.

use crate::color::Color;
use crate::design::{self, KnobMode, FACEPLATES, KNOB_STYLES};
use crate::draw::knob_named_mode;
use crate::hardware::{draw_panel, draw_screw};
use crate::meters::{self, FaderCap, VuFace};
use crate::retro::{self, Icon, SliderStyle};
use crate::scene::{Point, Rect, Scene};
use crate::spectrum_draw::{self, SpectrumSettings, SpectrumStyle};
use crate::switches::{self, PushStyle, RockerStyle, ToggleStyle};

pub const PAGE_NAMES: [&str; 6] = ["Knobs", "Plates", "Meters", "Sliders", "Switches & lamps", "Spectrum"];

pub fn paint_page(scene: &mut Scene, page: usize, flat: bool, glow: bool) {
    paint_page_mode(scene, page, if flat { KnobMode::Flat } else { KnobMode::Classic }, glow);
}

pub fn paint_page_mode(scene: &mut Scene, page: usize, mode: KnobMode, glow: bool) {
    match page {
        0 => {
            for (i, s) in KNOB_STYLES.iter().enumerate() {
                let col = (i % 8) as f32;
                let row = (i / 8) as f32;
                let c = Point::new(70.0 + col * 150.0, 70.0 + row * 120.0);
                knob_named_mode(scene, s.code, c, 36.0, 0.4, Color(0xffc96f4a), false, mode);
                scene.text(Point::new(c.x, c.y + 48.0), s.code, Color(0xffdde1d5), 11.0, true);
            }
            if mode == KnobMode::Lit {
                // One row across the 1280-wide page, however many presets there are.
                let n = crate::knob3d::PRESETS.len().max(1) as f32;
                let step = (1280.0 - 100.0) / n;
                let radius = (step * 0.32).min(32.0);
                for (i, &code) in crate::knob3d::PRESETS.iter().enumerate() {
                    let c = Point::new(50.0 + step * (i as f32 + 0.5), 790.0);
                    knob_named_mode(scene, code, c, radius, 0.4, Color(0xffc96f4a), false, mode);
                    scene.text(Point::new(c.x, c.y + radius + 16.0), code, Color(0xffdde1d5), 11.0, true);
                }
            }
        }
        1 => {
            for (i, p) in FACEPLATES.iter().enumerate() {
                let col = (i % 8) as f32;
                let row = (i / 8) as f32;
                let r = Rect::new(16.0 + col * 156.0, 16.0 + row * 110.0, 148.0, 96.0);
                draw_panel(scene, r, Color(p.panel_top), Color(p.panel_bottom), p.finish);
                // Clear of the hazard stripes, the corner brackets and the neon tube.
                let (dx, dy) = match p.finish {
                    crate::hardware::FINISH_HAZARD => (10.0, 20.0),
                    crate::hardware::FINISH_NEON_EDGE => (14.0, 17.0),
                    crate::hardware::FINISH_GRID => (12.0, 13.0),
                    _ => (9.0, 11.0),
                };
                scene.text(Point::new(r.x + dx, r.y + dy), p.code, Color(p.text), 12.0, false);
            }
        }
        2 => {
            // Row 1: moving-coil meters, readout, lamp, lit key.
            meters::draw_vu_meter(scene, Rect::new(20.0, 20.0, 220.0, 140.0), 0.62, VuFace::Cream);
            meters::draw_vu_meter(scene, Rect::new(260.0, 20.0, 220.0, 140.0), 0.4, VuFace::Black);
            meters::draw_ppm(scene, Rect::new(500.0, 20.0, 280.0, 90.0), 0.7);
            meters::draw_seven_segment(scene, Rect::new(800.0, 20.0, 200.0, 60.0), "-12.4", Color(0xffff9a1f), true);
            meters::draw_lamp(scene, Point::new(1040.0, 50.0), 10.0, Color(0xffff3b4a), true);
            meters::draw_lamp(scene, Point::new(1080.0, 50.0), 10.0, Color(0xffff3b4a), false);
            meters::draw_lit_key(scene, Rect::new(1115.0, 36.0, 64.0, 28.0), "IN", true, Color(0xff5be85a), false);
            meters::draw_lit_key(scene, Rect::new(1195.0, 36.0, 64.0, 28.0), "OUT", false, Color(0xff5be85a), false);
            meters::draw_led_ladder(scene, Rect::new(500.0, 128.0, 280.0, 14.0), true, -9.0, -4.0, 28);
            meters::draw_fader(scene, Rect::new(800.0, 104.0, 240.0, 34.0), 0.45, FaderCap::Console, Color(0xffd0d0d0), false, true);
            // Row 2: ladders, faders, arc, fan, display windows.
            meters::draw_led_ladder(scene, Rect::new(20.0, 180.0, 40.0, 220.0), false, -12.0, -6.0, 16);
            meters::draw_stereo_ladder(scene, Rect::new(80.0, 180.0, 70.0, 220.0), -8.0, -14.0, -3.0);
            meters::draw_slot_fader(scene, Rect::new(172.0, 180.0, 28.0, 220.0), 0.35, FaderCap::Oval, Color(0xfff0f0ee));
            meters::draw_slot_fader(scene, Rect::new(218.0, 180.0, 36.0, 220.0), 0.6, FaderCap::Console, Color(0xffd0d0d0));
            // The same faders on a pale plate, with the scale in dark ink.
            let pale = Rect::new(270.0, 176.0, 104.0, 228.0);
            draw_panel(scene, pale, Color(0xffd9dcde), Color(0xffbfc4c7), crate::hardware::FINISH_ANODISED);
            meters::draw_fader_on(scene, Rect::new(282.0, 184.0, 28.0, 212.0), 0.7, FaderCap::Oval, Color(0xff2b2d30), true, true, Color(0xffd0d3d5));
            meters::draw_fader_on(scene, Rect::new(326.0, 184.0, 36.0, 212.0), 0.25, FaderCap::Console, Color(0xffc23a2e), true, true, Color(0xffd0d3d5));
            meters::draw_segment_arc(scene, Rect::new(392.0, 190.0, 170.0, 110.0), 0.66, Color(0xff39ff7a));
            meters::draw_vector_fan(scene, Rect::new(392.0, 316.0, 170.0, 84.0), 0.5, Color(0xffffc34d));
            retro::draw_bar_wedge(scene, Rect::new(590.0, 180.0, 200.0, 78.0), 0.55, Color(0xff39ff7a), 1.0, glow);
            retro::draw_mountains(scene, Rect::new(810.0, 180.0, 200.0, 78.0), 0.4, 0.7, Color(0xff39ff7a), 1.0, glow);
            retro::draw_lcd_panel(scene, Rect::new(1030.0, 180.0, 220.0, 78.0), "440", "HZ", "-3.2", "DB", Color(0xffffc34d), 1.0, glow);
            retro::draw_colour_columns(scene, Rect::new(590.0, 280.0, 200.0, 60.0), 0.6, 0.4, 0.8, 0.5, 1.0, glow);
            retro::draw_flat_bars(scene, Rect::new(810.0, 280.0, 200.0, 60.0), 0.4, 0.6, 0.7, 0.8, Color(0xff5be85a));
            retro::draw_scope(scene, Rect::new(1030.0, 280.0, 220.0, 100.0), &[0.1, 0.4, -0.2, 0.8, 0.0, -0.5, 0.3], false, Color(0xff39f2ff), 1.0, glow);
            retro::draw_scope(scene, Rect::new(590.0, 356.0, 200.0, 44.0), &[0.0, 0.3, 0.6, 0.2, 0.7, 0.9, 0.4, 0.1], true, Color(0xffffb02e), 0.2, glow);
            retro::draw_flat_bars(scene, Rect::new(810.0, 356.0, 200.0, 44.0), 0.8, 0.9, 0.9, 0.95, Color(0xff35e0d0));
            // Row 3: gauges and framed parts.
            retro::draw_arc_gauge(scene, Rect::new(20.0, 430.0, 180.0, 120.0), 0.5, Color(0xffff9a1f), 1.0, glow);
            retro::draw_hex_ring(scene, Rect::new(220.0, 430.0, 130.0, 130.0), 0.45, Color(0xff39f2ff), Color(0xffff4fd8), 1.0, glow);
            retro::draw_neon_bars(scene, Rect::new(372.0, 430.0, 200.0, 100.0), 0.7, 0.85, Color(0xff39f2ff), Color(0xffff4fd8), 1.0, glow);
            retro::draw_chassis(scene, Rect::new(594.0, 430.0, 200.0, 110.0), 0.5, Color(0xffc7a812), Color(0xff16b4c4), 1.0, glow);
            retro::draw_arc_gauge(scene, Rect::new(816.0, 430.0, 140.0, 100.0), 0.92, Color(0xff39f2ff), 1.0, glow);
            retro::draw_hex_ring(scene, Rect::new(978.0, 430.0, 100.0, 100.0), 0.9, Color(0xff9dff3a), Color(0xffff3b4a), 1.0, glow);
            retro::draw_lcd_panel(scene, Rect::new(1100.0, 430.0, 150.0, 60.0), "96", "KHZ", "24", "BIT", Color(0xff39f2ff), 1.0, glow);
        }
        3 => {
            const STYLES: [SliderStyle; 5] = [SliderStyle::Neon, SliderStyle::Bar, SliderStyle::Chamfer, SliderStyle::Wedge, SliderStyle::Flat];
            // On the dark page: two colours and values of each style, both ways up.
            for (i, st) in STYLES.iter().enumerate() {
                let y = 34.0 + i as f32 * 58.0;
                retro::draw_slider(scene, Rect::new(34.0, y, 290.0, 28.0), 0.35 + i as f32 * 0.1, *st, Color(0xff39f2ff), false, 0.5, glow);
                retro::draw_slider(scene, Rect::new(366.0, y, 290.0, 28.0), 0.9 - i as f32 * 0.15, *st, Color(0xff9dff3a), false, 0.5, glow);
                retro::draw_slider(scene, Rect::new(706.0 + i as f32 * 54.0, 30.0, 28.0, 280.0), 0.6, *st, Color(0xffff4fd8), true, 0.5, glow);
                retro::draw_slider(scene, Rect::new(990.0 + i as f32 * 54.0, 30.0, 28.0, 280.0), 0.2 + i as f32 * 0.18, *st, Color(0xffffb02e), true, 0.5, glow);
            }
            // On plates: a dark blue powder coat and a pale brushed one, with the slot faders
            // whose scale ink follows the plate.
            for (ox, top, bottom, finish, accent) in [
                (16.0f32, 0xff2a418au32, 0xff1d2f66u32, crate::hardware::FINISH_POWDER, 0xff39f2ffu32),
                (648.0, 0xffdfe2e4, 0xffc3c8cb, crate::hardware::FINISH_BRUSHED, 0xffff7a1f),
            ] {
                let plate = Rect::new(ox, 344.0, 616.0, 500.0);
                draw_panel(scene, plate, Color(top), Color(bottom), finish);
                crate::hardware::draw_corner_screws(scene, plate, crate::hardware::ScrewHead::Cross);
                for (i, st) in STYLES.iter().enumerate() {
                    retro::draw_slider(scene, Rect::new(ox + 44.0, 392.0 + i as f32 * 56.0, 270.0, 28.0), 0.3 + i as f32 * 0.13, *st, Color(accent), false, 0.5, glow);
                    retro::draw_slider(scene, Rect::new(ox + 356.0 + i as f32 * 50.0, 388.0, 28.0, 266.0), 0.85 - i as f32 * 0.14, *st, Color(accent), true, 0.5, glow);
                }
                let on = Color(top).mix(Color(bottom), 0.5);
                meters::draw_fader_on(scene, Rect::new(ox + 44.0, 690.0, 528.0, 36.0), 0.62, FaderCap::Console, Color(0xffd0d0d0), false, true, on);
                meters::draw_fader_on(scene, Rect::new(ox + 44.0, 748.0, 528.0, 30.0), 0.3, FaderCap::Oval, Color(0xff2b2d30), false, true, on);
                meters::draw_fader_on(scene, Rect::new(ox + 44.0, 796.0, 250.0, 26.0), 0.8, FaderCap::Console, Color(0xffc23a2e), false, false, on);
                meters::draw_fader_on(scene, Rect::new(ox + 322.0, 796.0, 250.0, 26.0), 0.15, FaderCap::Oval, Color(0xfff0f0ee), false, false, on);
            }
        }
        4 => {
            // The same set twice: on the dark page, and on a pale brushed plate where bloom has
            // to show against a bright surface.
            for (ox, pale) in [(0.0f32, false), (640.0, true)] {
                if pale {
                    draw_panel(scene, Rect::new(ox + 8.0, 12.0, 616.0, 536.0), Color(0xffdfe2e4), Color(0xffc3c8cb), crate::hardware::FINISH_BRUSHED);
                }
                let ink = if pale { Color(0xff24282b) } else { Color(0xff9fb0b8) };
                for (i, st) in [RockerStyle::Lamp, RockerStyle::Classic, RockerStyle::Slim, RockerStyle::Neon].iter().enumerate() {
                    switches::draw_rocker(scene, Rect::new(ox + 34.0 + i as f32 * 70.0, 34.0, 44.0, 70.0), true, Color(0xffff3b4a), *st, false, glow);
                    switches::draw_rocker(scene, Rect::new(ox + 330.0 + i as f32 * 70.0, 34.0, 44.0, 70.0), false, Color(0xffff3b4a), *st, false, glow);
                }
                scene.text(Point::new(ox + 34.0, 120.0), "rockers on", ink, 10.0, false);
                scene.text(Point::new(ox + 330.0, 120.0), "rockers off", ink, 10.0, false);
                for (i, st) in [RockerStyle::Lamp, RockerStyle::Classic, RockerStyle::Slim, RockerStyle::Neon].iter().enumerate() {
                    switches::draw_rocker(scene, Rect::new(ox + 30.0 + i as f32 * 90.0, 140.0, 70.0, 36.0), i % 2 == 0, Color(0xff5be85a), *st, true, glow);
                }
                for (i, st) in [ToggleStyle::Bat, ToggleStyle::Lever, ToggleStyle::Mini].iter().enumerate() {
                    switches::draw_toggle(scene, Rect::new(ox + 400.0 + i as f32 * 70.0, 130.0, 40.0, 70.0), true, *st, Color(0xfff2a31b));
                    switches::draw_toggle_at(scene, Rect::new(ox + 400.0 + i as f32 * 70.0, 205.0, 40.0, 70.0), [-1.0, 0.0, 0.45][i], *st, Color(0xfff2a31b));
                }
                for (i, st) in [PushStyle::Square, PushStyle::Round, PushStyle::Dome, PushStyle::Chamfer, PushStyle::Neon, PushStyle::Flat, PushStyle::Arcade, PushStyle::Pill].iter().enumerate() {
                    let cell = |row: f32| Rect::new(ox + 30.0 + (i % 4) as f32 * 92.0, 200.0 + row * 46.0 + (i / 4) as f32 * 92.0, 80.0, 34.0);
                    switches::draw_push(scene, cell(0.0), "KEY", *st, Color(0xff3f6fd0), true, false, glow);
                    switches::draw_push(scene, cell(1.0), "KEY", *st, Color(0xff3f6fd0), false, false, glow);
                }
                switches::draw_key_bank(scene, Rect::new(ox + 30.0, 400.0, 360.0, 36.0), &["A", "B", "C"], 1, PushStyle::Square, Color(0xffc96f4a), glow);
                switches::draw_key_bank(scene, Rect::new(ox + 410.0, 400.0, 200.0, 32.0), &["I", "II", "III"], 2, PushStyle::Chamfer, Color(0xff35e0d0), glow);
                for (i, icon) in [Icon::Battery, Icon::Warning, Icon::ArrowLeft, Icon::ArrowRight, Icon::Beam, Icon::Temp].iter().enumerate() {
                    retro::draw_icon_lamp(scene, Rect::new(ox + 30.0 + i as f32 * 48.0, 462.0, 36.0, 36.0), *icon, Color(0xffffc34d), i != 3);
                }
                draw_screw(scene, ox + 340.0, 480.0);
                crate::hardware::draw_screw_head(scene, Point::new(ox + 366.0, 480.0), 6.0, crate::hardware::ScrewHead::Cross);
                crate::hardware::draw_screw_head(scene, Point::new(ox + 396.0, 480.0), 7.0, crate::hardware::ScrewHead::Hex);
                meters::draw_lamp(scene, Point::new(ox + 440.0, 480.0), 9.0, Color(0xff5be85a), true);
                meters::draw_lamp(scene, Point::new(ox + 476.0, 480.0), 9.0, Color(0xffffb02e), true);
                meters::draw_lit_key(scene, Rect::new(ox + 506.0, 466.0, 70.0, 28.0), "LINK", true, Color(0xffff9a1f), false);
            }
            // Lower half: pressed states, key banks in the other styles and more colours, on a
            // dark blue anodised plate and a cream one.
            const PUSH: [PushStyle; 8] = [PushStyle::Square, PushStyle::Round, PushStyle::Dome, PushStyle::Chamfer, PushStyle::Neon, PushStyle::Flat, PushStyle::Arcade, PushStyle::Pill];
            for (ox, top, bottom, finish) in [
                (8.0f32, 0xff24397fu32, 0xff182a63u32, crate::hardware::FINISH_ANODISED),
                (648.0, 0xfff6efd9, 0xffe4dbbd, crate::hardware::FINISH_FLAT),
            ] {
                let oy = 560.0;
                draw_panel(scene, Rect::new(ox, oy, 616.0, 288.0), Color(top), Color(bottom), finish);
                for (i, st) in PUSH.iter().enumerate() {
                    let x = ox + 22.0 + i as f32 * 73.0;
                    switches::draw_push(scene, Rect::new(x, oy + 20.0, 62.0, 30.0), "ON", *st, Color(0xffe8452c), true, true, glow);
                    switches::draw_push(scene, Rect::new(x, oy + 62.0, 62.0, 30.0), "OFF", *st, Color(0xffe8452c), false, true, glow);
                }
                switches::draw_key_bank(scene, Rect::new(ox + 24.0, oy + 110.0, 250.0, 30.0), &["LO", "MID", "HI"], 0, PushStyle::Flat, Color(0xff5be85a), glow);
                switches::draw_key_bank(scene, Rect::new(ox + 300.0, oy + 110.0, 292.0, 30.0), &["2:1", "4:1", "8:1", "20:1"], 1, PushStyle::Pill, Color(0xffffb02e), glow);
                switches::draw_key_bank(scene, Rect::new(ox + 22.0, oy + 156.0, 254.0, 32.0), &["A", "B", "C"], 2, PushStyle::Neon, Color(0xffff4fd8), glow);
                switches::draw_key_bank(scene, Rect::new(ox + 300.0, oy + 154.0, 292.0, 36.0), &["1", "2", "3", "4", "5"], 3, PushStyle::Dome, Color(0xff39f2ff), glow);
                for (i, icon) in [Icon::Battery, Icon::Warning, Icon::ArrowLeft, Icon::ArrowRight, Icon::Beam, Icon::Temp].iter().enumerate() {
                    retro::draw_icon_lamp(scene, Rect::new(ox + 24.0 + i as f32 * 46.0, oy + 216.0, 36.0, 36.0), *icon, [Color(0xff39f2ff), Color(0xffff3b4a), Color(0xff5be85a)][i % 3], i % 2 == 0);
                }
                switches::draw_rocker(scene, Rect::new(ox + 312.0, oy + 216.0, 70.0, 36.0), true, Color(0xffffb02e), RockerStyle::Lamp, true, glow);
                switches::draw_rocker(scene, Rect::new(ox + 398.0, oy + 216.0, 70.0, 36.0), true, Color(0xff39f2ff), RockerStyle::Neon, true, glow);
                switches::draw_toggle_at(scene, Rect::new(ox + 490.0, oy + 208.0, 40.0, 70.0), 1.0, ToggleStyle::Lever, Color(0xff5be85a));
                switches::draw_toggle_at(scene, Rect::new(ox + 545.0, oy + 200.0, 40.0, 70.0), -1.0, ToggleStyle::Bat, Color(0xff5be85a));
            }
        }
        _ => paint_spectrum_page(scene),
    }
}

/// Deterministic 0..1 noise for the synthetic spectra.
fn demo_noise(a: u32, b: u32) -> f32 {
    let mut x = a.wrapping_mul(0x9e37_79b1) ^ b.wrapping_mul(0x85eb_ca77).wrapping_add(0x27d4_eb2f);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2c1b_3c6d);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297a_2d39);
    x ^= x >> 15;
    (x >> 8) as f32 / 16_777_216.0
}

/// Level in dB of a made-up programme at column position `f` (0..1, log frequency) and time
/// `age` (rows back from now): a held note with harmonics and vibrato, a gliding tone, a bass
/// pulse, broadband hits every so often, and a noise floor.
fn demo_level(f: f32, age: f32, c: u32, r: u32) -> f32 {
    let peak = |centre: f32, width: f32| (-(f - centre).powi(2) / (width * width)).exp();
    let floor = -86.0 + 7.0 * demo_noise(c, r) + 10.0 * (1.0 - f) * (1.0 - f);
    // Held note: fundamental and partials, swelling slowly, with a little vibrato.
    let swell = 0.62 + 0.38 * (age * 0.055 + 0.6).sin();
    let vib = 0.004 * (age * 0.55).sin();
    let mut note = 0.0f32;
    for (k, (pos, amp)) in [(0.30f32, 56.0f32), (0.40, 46.0), (0.46, 40.0), (0.50, 33.0), (0.535, 28.0), (0.56, 22.0)].iter().enumerate() {
        note = note.max(amp * swell * peak(pos + vib * (k as f32 + 1.0), 0.0075));
    }
    // Gliding tone that rises and falls across the upper half.
    let glide_at = 0.74 + 0.16 * (age * 0.043).sin();
    let glide = 44.0 * peak(glide_at, 0.010) + 18.0 * peak(glide_at + 0.06, 0.010);
    // Bass pulse, and a broadband hit that decays over a few rows.
    let beat = (age / 24.0).fract();
    let pulse = 60.0 * (-beat * 5.0).exp().max(0.25) * peak(0.13, 0.05);
    let hit = 34.0 * (-(age / 48.0 + 0.35).fract() * 9.0).exp() * (0.55 + 0.45 * f);
    (floor + note.max(glide).max(pulse).max(hit)).min(-6.0)
}

fn paint_spectrum_page(scene: &mut Scene) {
    use spectrum_draw::SpectrumPalette as Pal;
    // (name, style, smoothing, spectrogram map, palette)
    let looks: [(&str, SpectrumStyle, i32, i32, Pal); 12] = [
        ("Standard, accent", SpectrumStyle::Standard, 0, 0, Pal::Accent),
        ("Standard, amber", SpectrumStyle::Standard, 0, 0, Pal::Amber),
        ("Standard, cyan", SpectrumStyle::Standard, 0, 0, Pal::Cyan),
        ("Multicolour, accent", SpectrumStyle::Multicolour, 0, 0, Pal::Accent),
        ("Multicolour, amber, 1/3-octave smoothing", SpectrumStyle::Multicolour, 3, 0, Pal::Amber),
        ("Multicolour, range colours", SpectrumStyle::Multicolour, 0, 0, Pal::Range),
        ("Bars, accent", SpectrumStyle::Bars, 0, 0, Pal::Accent),
        ("Bars, amber", SpectrumStyle::Bars, 0, 0, Pal::Amber),
        ("Bars, cyan", SpectrumStyle::Bars, 0, 0, Pal::Cyan),
        ("Spectrogram, heat map", SpectrumStyle::Spectrogram, 0, spectrum_draw::MAP_INFERNO, Pal::Accent),
        ("Spectrogram, ice map", SpectrumStyle::Spectrogram, 0, spectrum_draw::MAP_ICE, Pal::Accent),
        ("Spectrogram, shaded range colours", SpectrumStyle::Spectrogram, 0, spectrum_draw::MAP_SHADE, Pal::Range),
    ];
    let n = 192usize;
    let rows = 128usize;
    let mut gram = vec![0u8; rows * n];
    for r in 0..rows {
        for c in 0..n {
            let db = demo_level((c as f32 + 0.5) / n as f32, r as f32, c as u32, r as u32);
            gram[r * n + c] = (((db + 90.0) / 80.0).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    // The trace is the newest row, with the noise floor averaged over a few rows.
    let cols: Vec<f32> = (0..n).map(|c| {
        let f = (c as f32 + 0.5) / n as f32;
        (0..4).map(|k| demo_level(f, 2.0, c as u32, 900 + k)).sum::<f32>() / 4.0
    }).collect();
    let hold: Vec<f32> = (0..n).map(|c| {
        let f = (c as f32 + 0.5) / n as f32;
        (0..10).map(|a| demo_level(f, a as f32 * 3.0, c as u32, 950)).fold(-120.0f32, f32::max).min(0.0) + 2.0
    }).collect();
    for (i, look) in looks.iter().enumerate() {
        let col = (i % 3) as f32;
        let row = (i / 3) as f32;
        let cell = Rect::new(16.0 + col * 420.0, 14.0 + row * 210.0, 408.0, 198.0);
        scene.fill_round(cell, 6.0, Color(0xff0e161a), Color(0xff0b1114), true);
        scene.stroke_round(cell, 6.0, Color(0x14ffffff), 0.6);
        let mut settings = SpectrumSettings::default();
        settings.style = look.1;
        settings.smoothing = look.2;
        settings.spectrogram_map = look.3;
        settings.palette = look.4;
        settings.size_index = 4;
        let plot = cell.reduced(8.0, 8.0);
        let plot = Rect::new(plot.x, plot.y, plot.w, (plot.h - 16.0).max(0.0));
        let shown: Vec<f32> = if look.2 > 0 {
            // A plain moving average stands in for the analyser's fractional-octave smoothing.
            (0..n).map(|c| { let (a, b) = (c.saturating_sub(4), (c + 4).min(n - 1)); cols[a..=b].iter().sum::<f32>() / (b - a + 1) as f32 }).collect()
        } else { cols.clone() };
        spectrum_draw::SpectrumView::paint_columns(scene, plot, &settings, &shown, Some(&hold), Some(&gram), 20.0, 30000.0, 48000.0);
        scene.text(Point::new(cell.center().x, cell.bottom() - 12.0), look.0, Color(0xff9fb0b8), 10.0, true);
    }
}

pub fn every_style_draws(flat: bool) -> bool {
    every_style_draws_mode(if flat { KnobMode::Flat } else { KnobMode::Classic })
}

pub fn every_style_draws_mode(mode: KnobMode) -> bool {
    for s in KNOB_STYLES {
        let mut scene = Scene::new();
        knob_named_mode(&mut scene, s.code, Point::new(40.0, 40.0), 32.0, 0.2, Color(0xffc96f4a), false, mode);
        if scene.cmds.is_empty() { return false; }
        let _ = design::find_style(s.code);
    }
    true
}
