//! Regression test for the rasteriser and the widgets: one small frame is compared with a stored
//! PPM. A few channel values may differ, because float maths in `sin`, `cos` and font hinting
//! can vary a little between platforms. Anything larger means the drawing changed.
//!
//! To accept a deliberate change: `GLDRI_UPDATE_GOLDEN=1 cargo test -p goodlookindri-shell --test golden`

use goodlookindri::color::Color;
use goodlookindri::hardware::draw_panel;
use goodlookindri::scene::{Rect, Scene};
use goodlookindri::switches::PushStyle;
use goodlookindri_shell::ui::{Kind, Ui, Widget};
use goodlookindri_shell::{render_offscreen, App, Event};

const W: u32 = 240;
const H: u32 = 120;
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/widgets_240x120.ppm");

struct Sample {
    ui: Ui,
}

impl Sample {
    fn new() -> Self {
        let rows = ["alpha", "beta", "gamma"].iter().map(|s| s.to_string()).collect();
        let ui = Ui::new(vec![
            Widget::new(1, Rect::new(8.0, 8.0, 90.0, 28.0), Kind::Button { label: "Apply".into(), colour: Color(0xffd98a3d), style: PushStyle::Square }),
            Widget::new(2, Rect::new(110.0, 12.0, 20.0, 20.0), Kind::Lamp { colour: Color(0xff5be85a), lit: true }),
            Widget::new(3, Rect::new(8.0, 44.0, 224.0, 12.0), Kind::Bar { value: 0.6, colour: Color(0xff39ff7a) }),
            Widget::new(4, Rect::new(8.0, 62.0, 110.0, 52.0), Kind::List { rows, selected: 1, top: 0 }),
            Widget::new(5, Rect::new(128.0, 62.0, 104.0, 52.0), Kind::Text { body: "Wrapped text sample".into(), colour: Color(0xffd8d4c4) }),
            Widget::new(6, Rect::new(140.0, 8.0, 92.0, 28.0), Kind::Readout { text: "0.42".into(), colour: Color(0xffff9a1f) }),
        ]);
        Self { ui }
    }
}

impl App for Sample {
    fn event(&mut self, e: Event) -> bool {
        self.ui.event(e).redraw
    }

    fn paint(&mut self, scene: &mut Scene, w: f32, h: f32) {
        draw_panel(scene, Rect::new(0.0, 0.0, w, h), Color(0xff4a4f55), Color(0xff2a2d31), 1);
        self.ui.paint(scene);
    }
}

/// Reads a binary PPM (P6, 8-bit) into (width, height, RGB bytes).
fn read_ppm(bytes: &[u8]) -> (usize, usize, Vec<u8>) {
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        fields.push(String::from_utf8_lossy(&bytes[start..i]).to_string());
    }
    assert_eq!(fields[0], "P6");
    let (w, h): (usize, usize) = (fields[1].parse().unwrap(), fields[2].parse().unwrap());
    (w, h, bytes[i + 1..].to_vec())
}

#[test]
fn widgets_frame_matches_golden() {
    let mut app = Sample::new();
    let fb = render_offscreen(&mut app, W, H, 1.0);
    let actual = fb.to_ppm();
    if std::env::var_os("GLDRI_UPDATE_GOLDEN").is_some() {
        std::fs::write(GOLDEN, &actual).expect("write golden");
        return;
    }
    let expected_bytes = std::fs::read(GOLDEN).expect("golden file missing; run with GLDRI_UPDATE_GOLDEN=1");
    let (ew, eh, expected) = read_ppm(&expected_bytes);
    let (aw, ah, got) = read_ppm(&actual);
    assert_eq!((ew, eh), (aw, ah), "size changed");

    let mut far = 0usize;
    let mut worst = 0i32;
    for (a, b) in got.iter().zip(expected.iter()) {
        let d = (*a as i32 - *b as i32).abs();
        worst = worst.max(d);
        if d > 3 {
            far += 1;
        }
    }
    let total = got.len();
    assert!(
        (far as f64) / (total as f64) < 0.001,
        "{} of {} channel values differ by more than 3 (worst {}); rerun with GLDRI_UPDATE_GOLDEN=1 if the change is intended",
        far, total, worst
    );
}

#[test]
fn golden_is_not_blank() {
    let mut app = Sample::new();
    let fb = render_offscreen(&mut app, W, H, 1.0);
    let first = fb.pixels[0];
    assert!(fb.pixels.iter().filter(|p| **p != first).count() > fb.pixels.len() / 4);
}
