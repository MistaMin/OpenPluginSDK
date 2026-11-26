//! Renders the hardware parts (plates in every finish, screws, rack ears, legends, scale rings)
//! to a PPM. Usage: cargo run -p goodlookindri-shell --example render_hardware -- <out.ppm> [scale] [section]

use goodlookindri::color::Color;
use goodlookindri::draw::{self, KnobMode};
use goodlookindri::hardware::*;
use goodlookindri::light::LightRig;
use goodlookindri::scene::{Point, Rect, Scene};
use goodlookindri_shell::raster::Framebuffer;

/// Where a printed scale goes round a lit knob of this style: the centre of its silhouette
/// (tall knobs lean up the screen under the tilted camera) and a radius just clear of it.
/// These styles print no scale of their own, so the ring is not doubled.
fn knob_frame(code: &str, c: Point, r: f32) -> (Point, f32) {
    let (lift, reach) = goodlookindri::knob3d::style_model(code).map_or((0.0, 1.0), |m| m.visual_frame(&goodlookindri::knob3d::Rig::studio()));
    (Point::new(c.x, c.y - lift * r), r * reach.max(1.0) * 1.12 + 2.0)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let out = a.get(1).cloned().unwrap_or_else(|| "hardware.ppm".into());
    let scale: f32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    set_device_scale(scale);
    draw::set_knob_scale(scale);
    let mut sc = Scene::new();
    let rig = LightRig::studio();

    // Row 1: every finish on a mid plate.
    let looks: [(u32, u32, i32, &str); 12] = [
        (0xff394247, 0xff2a3034, 0, "flat"),
        (0xffdfe2e4, 0xffc3c8cb, 1, "brushed"),
        (0xff75503a, 0xff573a28, 2, "wood"),
        (0xff0b1d12, 0xff06120a, 3, "scan"),
        (0xffe8c820, 0xffcaa913, 4, "hazard"),
        (0xff0a3a40, 0xff062a2f, 5, "grid"),
        (0xff0b1220, 0xff060b16, 6, "neon"),
        (0xff2e0f55, 0xff1e0a3a, 7, "horizon"),
        (0xff2b43b0, 0xff1d3190, 8, "anodised"),
        (0xff2a418a, 0xff1d2f66, 9, "powder"),
        (0xff4f6a62, 0xff3c514b, 10, "hammer"),
        (0xff2a2a2d, 0xff18181a, 11, "satin"),
    ];
    for (i, (t, b, f, name)) in looks.iter().enumerate() {
        let r = Rect::new(16.0 + (i % 6) as f32 * 208.0, 16.0 + (i / 6) as f32 * 136.0, 196.0, 124.0);
        draw_panel(&mut sc, r, Color(*t), Color(*b), *f);
        let ink = if Color(*t).perceived_brightness() > 0.55 { Color(0xff1b1f21) } else { Color(0xffffffff) };
        legend_text(&mut sc, Legend::Silk(ink), Point::new(r.x + if *f == FINISH_NEON_EDGE { 18.0 } else { 12.0 }, r.y + match *f { FINISH_HAZARD => 22.0, FINISH_NEON_EDGE => 20.0, _ => 14.0 }), name, 12.0, false);
    }

    // Row 2: a rack unit, silver brushed with ears, screws, scale rings and legends.
    let unit = Rect::new(16.0 + 40.0, 300.0, 700.0, 250.0);
    draw_rack_ear_with(&mut sc, Rect::new(unit.x - 40.0, unit.y, 42.0, unit.h), true, Color(0xffd8dbdd), Color(0xffb9bec1), &rig);
    draw_panel(&mut sc, unit, Color(0xffdfe2e4), Color(0xffc3c8cb), FINISH_BRUSHED);
    draw_rack_ear_with(&mut sc, Rect::new(unit.right() - 2.0, unit.y, 42.0, unit.h), false, Color(0xffd8dbdd), Color(0xffb9bec1), &rig);
    draw_corner_screws(&mut sc, unit, ScrewHead::Cross);
    let ink = Color(0xff202426);
    for (k, (labels, minors)) in [(&["-20", "-10", "0", "+10", "+20"][..], 4usize), (&["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"][..], 1), (&["FAST", "SLOW"][..], 0)].iter().enumerate() {
        let c = Point::new(unit.x + 130.0 + k as f32 * 200.0, unit.y + 110.0);
        let code = ["Knl", "Rck", "Tl"][k];
        let (rc, rr) = knob_frame(code, c, 30.0);
        draw::set_knob_floor(Color(0xffd1d5d8));
        draw::knob_named_mode(&mut sc, code, c, 30.0, ([0.35f32, 0.7, 0.2][k] * 1.6 - 0.8) * std::f32::consts::PI, Color(0xffc0392b), true, KnobMode::Lit);
        let n = if labels.len() == 2 { 7 } else { labels.len() };
        let mut ring = ScaleRing::new(rc, rr, n, *minors, labels);
        ring.legend = Legend::Silk(ink);
        if k == 2 { ring.marks = ScaleMarks::Dots; }
        draw_scale_ring_with(&mut sc, &ring);
        legend_text(&mut sc, Legend::Silk(ink), Point::new(c.x, c.y + 72.0), ["INPUT", "RATIO", "ATTACK"][k], 11.0, true);
    }
    draw_legend_line(&mut sc, ink, unit.x + 40.0, unit.right() - 40.0, unit.bottom() - 24.0, "LIMITING AMPLIFIER", 11.0);

    // Dark plate: engraved and silk legends, screw variants and sizes.
    let dark = Rect::new(800.0, 300.0, 460.0, 250.0);
    draw_panel(&mut sc, dark, Color(0xff2a418a), Color(0xff1d2f66), FINISH_POWDER);
    draw_corner_screws(&mut sc, dark, ScrewHead::Slot);
    let eng = Legend::Engraved(Color(0xffe8e4d8));
    legend_text_on(&mut sc, eng, Color(0xff2a418a), Point::new(dark.x + 30.0, dark.y + 40.0), "MODEL  ONE", 20.0, false);
    legend_text(&mut sc, Legend::Silk(Color(0xffffffff)), Point::new(dark.x + 30.0, dark.y + 70.0), "OPTICAL   COMPRESSOR", 11.0, false);
    let c = Point::new(dark.x + 330.0, dark.y + 130.0);
    draw::set_knob_floor(Color(0xff24387a));
    let (rc, rr) = knob_frame("Ivr", c, 28.0);
    draw::knob_named_mode(&mut sc, "Ivr", c, 28.0, 0.25 * std::f32::consts::PI, Color(0xffe8e4d8), false, KnobMode::Lit);
    let mut ring = ScaleRing::new(rc, rr, 5, 0, &["-10", "0", "+10", "+20", "+30"]);
    ring.marks = ScaleMarks::Dots;
    ring.legend = Legend::Silk(Color(0xffffffff));
    draw_scale_ring_with(&mut sc, &ring);
    let c = Point::new(dark.x + 190.0, dark.y + 160.0);
    let (rc, rr) = knob_frame("Dbl", c, 22.0);
    draw::knob_named_mode(&mut sc, "Dbl", c, 22.0, -0.3 * std::f32::consts::PI, Color(0xffe8e4d8), false, KnobMode::Lit);
    let mut ring = ScaleRing::new(rc, rr.max(30.0), 11, 1, &["0", "5", "10"]);
    ring.legend = eng;
    draw_scale_ring_with(&mut sc, &ring);
    for (i, head) in [ScrewHead::Slot, ScrewHead::Cross, ScrewHead::Hex].iter().enumerate() {
        for (j, rad) in [3.0f32, 5.0, 8.0].iter().enumerate() {
            draw_screw_head(&mut sc, Point::new(dark.x + 60.0 + j as f32 * 26.0, dark.y + 110.0 + i as f32 * 30.0 + j as f32 * 2.0), *rad, *head);
        }
    }

    // Row 3: keys, plain ears, legacy screw.
    let strip = Rect::new(16.0, 580.0, 1240.0, 120.0);
    draw_panel(&mut sc, strip, Color(0xff242527), Color(0xff161718), FINISH_SATIN_BLACK);
    draw_rack_ear(&mut sc, Rect::new(strip.x + 10.0, strip.y + 10.0, 30.0, 100.0), true);
    draw_rack_ear(&mut sc, Rect::new(strip.right() - 40.0, strip.y + 10.0, 30.0, 100.0), false);
    draw_key(&mut sc, Rect::new(strip.x + 80.0, strip.y + 30.0, 60.0, 26.0), Color(0xffc96f4a), false);
    draw_key(&mut sc, Rect::new(strip.x + 160.0, strip.y + 30.0, 60.0, 26.0), Color(0xffc96f4a), true);
    draw_led_key(&mut sc, Rect::new(strip.x + 240.0, strip.y + 30.0, 40.0, 22.0), Color(0xffff3b4a), true, false);
    draw_led_key(&mut sc, Rect::new(strip.x + 300.0, strip.y + 30.0, 40.0, 22.0), Color(0xffff3b4a), false, false);
    draw_screw(&mut sc, strip.x + 380.0, strip.y + 40.0);
    draw_legend_line(&mut sc, Color(0xffe8e4d8), strip.x + 420.0, strip.x + 800.0, strip.y + 40.0, "OUTPUT", 11.0);
    draw_panel(&mut sc, Rect::new(strip.x + 840.0, strip.y + 14.0, 180.0, 92.0), Color(0xffe8c820), Color(0xffcaa913), FINISH_HAMMERTONE);
    draw_panel(&mut sc, Rect::new(strip.x + 1030.0, strip.y + 14.0, 150.0, 92.0), Color(0xffb4b8bb), Color(0xff9a9ea2), FINISH_ANODISED);

    // Engraved legends on a pale and a mid-tone plate.
    let (yel, grey) = (Rect::new(strip.x + 840.0, strip.y + 14.0, 180.0, 92.0), Rect::new(strip.x + 1030.0, strip.y + 14.0, 150.0, 92.0));
    legend_text_on(&mut sc, Legend::Engraved(Color(0xff2a2410)), Color(0xffd9b91a), yel.center(), "CAUTION", 16.0, true);
    legend_text_on(&mut sc, Legend::Engraved(Color(0xff2b2f33)), Color(0xffa7abae), Point::new(grey.center().x, grey.center().y - 12.0), "THRESHOLD", 12.0, true);
    legend_text_on(&mut sc, Legend::Engraved(Color(0xffa7abae).darker(0.25)), Color(0xffa7abae), Point::new(grey.center().x, grey.center().y + 14.0), "UNFILLED", 12.0, true);

    eprintln!("{} commands", sc.cmds.len());
    let mut fb = Framebuffer::new((1280.0 * scale) as usize, (720.0 * scale) as usize);
    fb.clear(Color(0xff14181b));
    fb.replay(&sc, scale);
    std::fs::write(&out, fb.to_ppm()).expect("write ppm");
}
