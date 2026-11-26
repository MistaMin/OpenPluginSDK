//! Event routing for [`goodlookindri_shell::ui::Ui`]: hit testing, clicks, focus, wheel and keys.

use goodlookindri::color::Color;
use goodlookindri::scene::{Rect, Scene};
use goodlookindri::switches::PushStyle;
use goodlookindri_shell::ui::{Action, Kind, Ui, Widget};
use goodlookindri_shell::{Event, Key};

fn button(id: u32, y: f32) -> Widget {
    Widget::new(id, Rect::new(10.0, y, 100.0, 30.0), Kind::Button { label: format!("B{id}"), colour: Color(0xff999999), style: PushStyle::Square })
}

fn list(id: u32, y: f32, rows: usize) -> Widget {
    let rows = (0..rows).map(|i| format!("row {i}")).collect();
    Widget::new(id, Rect::new(10.0, y, 120.0, 80.0), Kind::List { rows, selected: 0, top: 0 })
}

fn list_state(ui: &Ui, id: u32) -> (usize, usize) {
    match ui.widgets().iter().find(|w| w.id == id).map(|w| &w.kind) {
        Some(Kind::List { selected, top, .. }) => (*selected, *top),
        _ => panic!("no list {id}"),
    }
}

/// A full click. Lists select on press and buttons fire on release, so either may carry the action.
fn click(ui: &mut Ui, x: f32, y: f32) -> Option<Action> {
    let down = ui.event(Event::MouseDown { x, y }).action;
    let up = ui.event(Event::MouseUp { x, y }).action;
    down.or(up)
}

#[test]
fn button_fires_only_when_released_on_it() {
    let mut ui = Ui::new(vec![button(1, 0.0), button(2, 40.0)]);
    assert_eq!(click(&mut ui, 20.0, 10.0), Some(Action::Pressed(1)));
    ui.event(Event::MouseDown { x: 20.0, y: 10.0 });
    assert_eq!(ui.event(Event::MouseUp { x: 20.0, y: 50.0 }).action, None, "dragged off the button");
}

#[test]
fn clicks_outside_widgets_do_nothing() {
    let mut ui = Ui::new(vec![button(1, 0.0)]);
    let r = ui.event(Event::MouseDown { x: 400.0, y: 400.0 });
    assert!(!r.redraw && r.action.is_none());
    assert_eq!(ui.focused(), None);
}

#[test]
fn hover_reports_redraw_only_on_change() {
    let mut ui = Ui::new(vec![button(1, 0.0)]);
    assert!(ui.event(Event::MouseMove { x: 20.0, y: 10.0 }).redraw);
    assert!(!ui.event(Event::MouseMove { x: 21.0, y: 11.0 }).redraw);
    assert!(ui.event(Event::MouseMove { x: 400.0, y: 400.0 }).redraw);
}

#[test]
fn tab_cycles_interactive_widgets_in_order() {
    let mut ui = Ui::new(vec![button(1, 0.0), Widget::new(9, Rect::new(0.0, 60.0, 10.0, 10.0), Kind::Lamp { colour: Color(0), lit: false }), button(2, 40.0)]);
    let tab = |shift| Event::Key { key: Key::Tab, shift };
    ui.event(tab(false));
    assert_eq!(ui.focused(), Some(1));
    ui.event(tab(false));
    assert_eq!(ui.focused(), Some(2));
    ui.event(tab(false));
    assert_eq!(ui.focused(), Some(1), "wraps");
    ui.event(tab(true));
    assert_eq!(ui.focused(), Some(2), "shift-tab goes back");
    ui.event(Event::Key { key: Key::Escape, shift: false });
    assert_eq!(ui.focused(), None);
}

#[test]
fn enter_and_space_press_the_focused_button() {
    let mut ui = Ui::new(vec![button(1, 0.0)]);
    assert_eq!(ui.event(Event::Key { key: Key::Enter, shift: false }).action, None, "nothing focused");
    assert!(ui.set_focus(1));
    assert_eq!(ui.event(Event::Key { key: Key::Enter, shift: false }).action, Some(Action::Pressed(1)));
    assert_eq!(ui.event(Event::Key { key: Key::Space, shift: false }).action, Some(Action::Pressed(1)));
}

#[test]
fn set_focus_refuses_non_interactive_widgets() {
    let mut ui = Ui::new(vec![Widget::new(5, Rect::new(0.0, 0.0, 10.0, 10.0), Kind::Lamp { colour: Color(0), lit: true })]);
    assert!(!ui.set_focus(5));
    assert!(!ui.set_focus(99));
}

#[test]
fn clicking_a_list_row_selects_it() {
    let mut ui = Ui::new(vec![list(3, 0.0, 10)]);
    // Row 2 starts at y = PAD + 2 * ROW = 8 + 36 = 44, so y = 53 is inside it.
    let action = click(&mut ui, 30.0, 53.0);
    assert_eq!(action, Some(Action::Selected { id: 3, index: 2 }));
    assert_eq!(list_state(&ui, 3), (2, 0));
    assert_eq!(click(&mut ui, 30.0, 53.0), None, "already selected");
}

#[test]
fn up_and_down_move_selection_and_scroll_it_into_view() {
    let mut ui = Ui::new(vec![list(3, 0.0, 20)]);
    assert!(ui.set_focus(3));
    let down = |ui: &mut Ui| ui.event(Event::Key { key: Key::Down, shift: false }).action;
    for _ in 0..6 {
        down(&mut ui);
    }
    assert_eq!(list_state(&ui, 3).0, 6);
    let (_, top) = list_state(&ui, 3);
    assert!(top > 0 && top <= 6, "scrolled so row 6 is visible, top = {top}");
    ui.event(Event::Key { key: Key::Up, shift: false });
    assert_eq!(list_state(&ui, 3).0, 5);
    for _ in 0..100 {
        down(&mut ui);
    }
    assert_eq!(list_state(&ui, 3).0, 19, "clamped at the last row");
}

#[test]
fn wheel_scrolls_the_list_and_clamps() {
    let mut ui = Ui::new(vec![list(3, 0.0, 20)]);
    // Positive dy scrolls content up, so the first visible row moves down.
    assert!(ui.event(Event::MouseWheel { x: 20.0, y: 20.0, dy: 3.0 }).redraw);
    assert_eq!(list_state(&ui, 3), (0, 3));
    ui.event(Event::MouseWheel { x: 20.0, y: 20.0, dy: -1.0 });
    assert_eq!(list_state(&ui, 3).1, 2);
    ui.event(Event::MouseWheel { x: 20.0, y: 20.0, dy: 1000.0 });
    let visible = 3; // (80 - 2 * 8) / 18 = 3.5, rounded down
    assert_eq!(list_state(&ui, 3).1, 20 - visible, "clamped to the last page");
    assert!(!ui.event(Event::MouseWheel { x: 20.0, y: 20.0, dy: 1.0 }).redraw, "already at the end");
}

#[test]
fn small_wheel_deltas_still_move_one_row() {
    let mut ui = Ui::new(vec![list(3, 0.0, 20)]);
    ui.event(Event::MouseWheel { x: 20.0, y: 20.0, dy: 0.2 });
    assert_eq!(list_state(&ui, 3).1, 1);
}

#[test]
fn wheel_outside_a_list_is_ignored() {
    let mut ui = Ui::new(vec![list(3, 0.0, 20)]);
    assert!(!ui.event(Event::MouseWheel { x: 300.0, y: 300.0, dy: 2.0 }).redraw);
}

#[test]
fn kind_mut_changes_what_paints() {
    let mut ui = Ui::new(vec![Widget::new(4, Rect::new(0.0, 0.0, 100.0, 30.0), Kind::Readout { text: "0000".into(), colour: Color(0xffff0000) })]);
    if let Some(Kind::Readout { text, .. }) = ui.kind_mut(4) {
        *text = "1234".into();
    }
    let mut scene = Scene::new();
    ui.paint(&mut scene);
    assert!(!scene.cmds.is_empty());
}
