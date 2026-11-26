//! Widgets demo for goodlookindri-shell: buttons, a lamp, a bar, a readout, a scrolling list
//! and a wrapped text block.
//!
//! `gldri-widgets`                          opens the window
//! `gldri-widgets --smoke [ms]`             opens it, draws, exits 0 after `ms` (default 800) if the frame is not blank
//! `gldri-widgets --render out.ppm [scale]` replays a click, a list selection and Tab, then writes one frame

use goodlookindri::color::Color;
use goodlookindri::hardware::draw_panel;
use goodlookindri::scene::{Point, Rect, Scene};
use goodlookindri::switches::PushStyle;
use goodlookindri_shell::ui::{Action, Kind, Ui, Widget};
use goodlookindri_shell::{render_offscreen, run, App, Config, Event, Key};

const W: u32 = 560;
const H: u32 = 340;
const INTRO: &str = "Apply counts presses and toggles the lamp. Reset clears the count. \
Pick a preset in the list, or scroll it with the wheel. Tab moves focus; Enter or Space \
presses the focused button; Up and Down move the list selection.";

struct Demo {
    ui: Ui,
    time: f32,
    presses: u32,
    lit: bool,
}

fn demo_ui() -> Ui {
    let rows = (1..=20).map(|i| format!("Preset {:02}", i)).collect();
    Ui::new(vec![
        Widget::new(1, Rect::new(20.0, 50.0, 120.0, 34.0), Kind::Button { label: "Apply".into(), colour: Color(0xffd98a3d), style: PushStyle::Square }),
        Widget::new(2, Rect::new(150.0, 50.0, 120.0, 34.0), Kind::Button { label: "Reset".into(), colour: Color(0xff8fa3b8), style: PushStyle::Flat }),
        Widget::new(3, Rect::new(300.0, 58.0, 18.0, 18.0), Kind::Lamp { colour: Color(0xff5be85a), lit: false }),
        Widget::new(4, Rect::new(340.0, 46.0, 200.0, 42.0), Kind::Readout { text: "0000".into(), colour: Color(0xffff9a1f) }),
        Widget::new(5, Rect::new(20.0, 108.0, 300.0, 14.0), Kind::Bar { value: 0.5, colour: Color(0xff39ff7a) }),
        Widget::new(6, Rect::new(20.0, 140.0, 240.0, 180.0), Kind::List { rows, selected: 0, top: 0 }),
        Widget::new(7, Rect::new(280.0, 140.0, 260.0, 180.0), Kind::Text { body: INTRO.into(), colour: Color(0xffd8d4c4) }),
    ])
}

impl Demo {
    fn new() -> Self {
        Self { ui: demo_ui(), time: 0.0, presses: 0, lit: false }
    }

    fn say(&mut self, message: String) {
        if let Some(Kind::Text { body, .. }) = self.ui.kind_mut(7) {
            *body = format!("{}\n\n{}", message, INTRO);
        }
    }

    fn apply(&mut self, a: Action) {
        match a {
            Action::Pressed(1) => {
                self.presses += 1;
                self.lit = !self.lit;
                self.sync_status();
                self.say(format!("Applied {} time(s).", self.presses));
            }
            Action::Pressed(2) => {
                self.presses = 0;
                self.lit = false;
                self.sync_status();
                self.say("Reset.".into());
            }
            Action::Selected { index, .. } => self.say(format!("Selected Preset {:02}.", index + 1)),
            _ => {}
        }
    }

    fn sync_status(&mut self) {
        if let Some(Kind::Readout { text, .. }) = self.ui.kind_mut(4) {
            *text = format!("{:04}", self.presses);
        }
        if let Some(Kind::Lamp { lit, .. }) = self.ui.kind_mut(3) {
            *lit = self.lit;
        }
    }
}

impl App for Demo {
    fn event(&mut self, e: Event) -> bool {
        if let Event::Tick { dt } = e {
            self.time += dt;
            let level = 0.5 + 0.5 * (self.time * 1.7).sin();
            if let Some(Kind::Bar { value, .. }) = self.ui.kind_mut(5) {
                *value = level;
            }
            return true;
        }
        let r = self.ui.event(e);
        if let Some(a) = r.action {
            self.apply(a);
        }
        r.redraw
    }

    fn paint(&mut self, scene: &mut Scene, w: f32, h: f32) {
        draw_panel(scene, Rect::new(0.0, 0.0, w, h), Color(0xff4a4f55), Color(0xff2a2d31), 1);
        scene.text(Point::new(w * 0.5, 24.0), "GOODLOOKINDRI WIDGETS", Color(0xffe8e2d0), 16.0, true);
        self.ui.paint(scene);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(|s| s.as_str()) {
        Some("--render") => {
            let path = args.get(1).cloned().unwrap_or_else(|| "widgets.ppm".into());
            let scale = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.0);
            let mut app = Demo::new();
            for _ in 0..30 {
                app.event(Event::Tick { dt: 1.0 / 60.0 });
            }
            app.event(Event::MouseMove { x: 80.0, y: 67.0 });
            app.event(Event::MouseDown { x: 80.0, y: 67.0 });
            app.event(Event::MouseUp { x: 80.0, y: 67.0 });
            app.event(Event::MouseDown { x: 60.0, y: 211.0 });
            app.event(Event::MouseUp { x: 60.0, y: 211.0 });
            app.event(Event::Key { key: Key::Tab, shift: false });
            app.event(Event::Tick { dt: 0.5 });
            let fb = render_offscreen(&mut app, W, H, scale);
            match std::fs::write(&path, fb.to_ppm()) {
                Ok(()) => {
                    println!("wrote {} ({}x{})", path, fb.width, fb.height);
                    0
                }
                Err(e) => {
                    eprintln!("{}: {}", path, e);
                    1
                }
            }
        }
        other => {
            let smoke_ms = if other == Some("--smoke") { Some(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(800)) } else { None };
            run(Config { title: "GoodLookin DRI widgets".into(), width: W, height: H, smoke_ms }, Box::new(Demo::new()))
        }
    };
    std::process::exit(code);
}
