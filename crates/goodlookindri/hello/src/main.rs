//! Proof that a GoodLookin DRI scene can be drawn and driven in a window.
//!
//! `gldri-hello`                  opens the window
//! `gldri-hello --smoke [ms]`     opens it, draws, and exits 0 after `ms` (default 800) if the frame is not blank
//! `gldri-hello --render out.ppm [scale]`  draws one frame with no window

use goodlookindri::color::Color;
use goodlookindri::design::{find_style, Motion};
use goodlookindri::draw;
use goodlookindri::hardware::draw_panel;
use goodlookindri::scene::{Point, Rect, Scene};
use goodlookindri_shell::{render_offscreen, run, App, Config, Event};

const W: u32 = 560;
const H: u32 = 300;
const STYLES: [&str; 4] = ["console", "metal", "bakelite", "ivory"];
const ACCENTS: [u32; 4] = [0xffb69b73, 0xff8fa3b8, 0xffc9663d, 0xffe6dcc2];

struct Hello {
    motion: [Motion; 4],
    target: [f64; 4],
    hover: Option<usize>,
    drag: Option<(usize, f32, f64)>,
}

impl Hello {
    fn new() -> Self {
        Self { motion: [Motion::default(); 4], target: [0.25, 0.5, 0.75, 0.4], hover: None, drag: None }
    }
    fn knob_rect(i: usize) -> Rect {
        Rect::new(40.0 + i as f32 * 130.0, 110.0, 100.0, 100.0)
    }
    fn hit(x: f32, y: f32) -> Option<usize> {
        (0..4).find(|i| Self::knob_rect(*i).contains(Point::new(x, y)))
    }
}

impl App for Hello {
    fn event(&mut self, e: Event) -> bool {
        match e {
            Event::MouseMove { x, y } => {
                if let Some((i, y0, v0)) = self.drag {
                    self.target[i] = (v0 + ((y0 - y) / 150.0) as f64).clamp(0.0, 1.0);
                    return true;
                }
                let h = Self::hit(x, y);
                let changed = h != self.hover;
                self.hover = h;
                changed
            }
            Event::MouseDown { x, y } => {
                if let Some(i) = Self::hit(x, y) { self.drag = Some((i, y, self.target[i])); }
                true
            }
            Event::MouseUp { .. } => { self.drag = None; true }
            Event::MouseWheel { .. } | Event::Key { .. } => false,
            Event::Tick { dt } => {
                let mut moving = false;
                for i in 0..4 {
                    self.motion[i].step(self.target[i], dt as f64, 18.0);
                    moving |= (self.motion[i].position - self.target[i]).abs() > 1e-3 || self.motion[i].velocity.abs() > 1e-3;
                }
                moving
            }
        }
    }

    fn paint(&mut self, scene: &mut Scene, width: f32, height: f32) {
        draw_panel(scene, Rect::new(0.0, 0.0, width, height), Color(0xff4a4f55), Color(0xff2a2d31), 1);
        scene.text(Point::new(width * 0.5, 40.0), "GOODLOOKINDRI SHELL", Color(0xffe8e2d0), 18.0, true);
        scene.text(Point::new(width * 0.5, 66.0), "drag a knob", Color(0xffa8a99f), 11.0, true);
        for i in 0..4 {
            let r = Self::knob_rect(i);
            let code = find_style(STYLES[i]).map(|s| s.code).unwrap_or("Brit");
            let angle = (self.motion[i].position.clamp(0.0, 1.0) as f32 * 1.6 - 0.8) * std::f32::consts::PI;
            let accent = Color(ACCENTS[i]);
            let c = r.center();
            draw::knob_named(scene, code, c, r.w * 0.5, angle, accent, false, false);
            if self.hover == Some(i) {
                scene.stroke_ellipse(r.expanded(4.0), Color(0xffffd36a), 1.5);
            }
            scene.text(Point::new(c.x, r.bottom() + 22.0), &format!("{} {:.0}%", STYLES[i], self.motion[i].position * 100.0), Color(0xffd8d4c4), 10.0, true);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(|s| s.as_str()) {
        Some("--render") => {
            let path = args.get(1).cloned().unwrap_or_else(|| "hello.ppm".into());
            let scale = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.0);
            let mut app = Hello::new();
            for _ in 0..120 { app.event(Event::Tick { dt: 1.0 / 60.0 }); }
            let fb = render_offscreen(&mut app, W, H, scale);
            match std::fs::write(&path, fb.to_ppm()) {
                Ok(()) => { println!("wrote {} ({}x{})", path, fb.width, fb.height); 0 }
                Err(e) => { eprintln!("{}: {}", path, e); 1 }
            }
        }
        other => {
            let smoke_ms = if other == Some("--smoke") { Some(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(800)) } else { None };
            run(Config { title: "GoodLookin DRI shell".into(), width: W, height: H, smoke_ms }, Box::new(Hello::new()))
        }
    };
    std::process::exit(code);
}
