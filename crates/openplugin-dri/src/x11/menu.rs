//! The SDK's own parameter menu: an override-redirect window at the click,
//! drawn by the view, with the pointer and the keyboard grabbed while it is
//! open. What the items do is `cocoa.rs`'s `run_menu_item`.

use super::clipboard::PasteTo;
use super::popup::{self, EDGE, FONT, PICK_BOTTOM, PICK_TOP, TEXT, TEXT_BRIGHT, TEXT_DIM};
use super::{EditorView, Window};
use goodlookindri::scene::{Point, Rect, Scene};
use goodlookindri_shell::text;
use openplugin_core::editor::{ParamMenu, ParamMenuAction};
use x11rb::protocol::xproto::{self, ConnectionExt as _, EventMask, GrabMode, GrabStatus};

/// The items of the SDK's own parameter menu, in order.
#[doc(hidden)]
pub const PARAM_MENU_ITEMS: [&str; 4] = ["Reset to default", "Copy value", "Paste value", "Type value"];

/// Answers a parameter menu request in place of the pop-up window, for
/// tests: return true when the request was taken (no menu is shown; choose
/// with [`EditorView::choose_menu_item`]).
#[doc(hidden)]
pub type MenuHook = Box<dyn FnMut(&ParamMenu) -> bool>;

// The menu's measures, in points.
const ROW: f32 = 22.0;
const TITLE: f32 = 22.0;
const PAD: f32 = 4.0;
const INSET: f32 = 14.0;
const MIN_WIDTH: f32 = 120.0;

/// The open menu.
pub(super) struct Menu {
    pub(super) window: Window,
    /// Device pixels.
    size: (u32, u32),
    /// The parameter's name, shown above the items. Empty for none.
    label: String,
    /// The item under the pointer or chosen with the arrow keys.
    hover: Option<usize>,
}

/// The menu's size in points for a title.
fn menu_size(label: &str) -> (f32, f32) {
    let widest = PARAM_MENU_ITEMS.iter().copied().chain([label]).map(|t| text::width(t, FONT)).fold(0.0, f32::max);
    ((widest + 2.0 * INSET).max(MIN_WIDTH).ceil(), 2.0 * PAD + title_height(label) + ROW * PARAM_MENU_ITEMS.len() as f32)
}

fn title_height(label: &str) -> f32 {
    if label.is_empty() {
        0.0
    } else {
        TITLE
    }
}

/// The item at a place in the menu, in points.
fn item_at(label: &str, x: f32, y: f32) -> Option<usize> {
    let (w, _) = menu_size(label);
    let row = ((y - PAD - title_height(label)) / ROW).floor();
    (x >= 0.0 && x < w && row >= 0.0 && (row as usize) < PARAM_MENU_ITEMS.len()).then_some(row as usize)
}

impl Menu {
    fn draw(&self, scene: &mut Scene) {
        let (w, h) = menu_size(&self.label);
        popup::panel(scene, w, h, EDGE);
        let top = PAD + title_height(&self.label);
        if !self.label.is_empty() {
            scene.text(Point::new(INSET, PAD + TITLE * 0.5), self.label.as_str(), TEXT_DIM, FONT, false);
            scene.line(Point::new(PAD, top - 1.5), Point::new(w - PAD, top - 1.5), EDGE, 1.0);
        }
        for (i, item) in PARAM_MENU_ITEMS.iter().enumerate() {
            let y = top + i as f32 * ROW;
            let picked = self.hover == Some(i);
            if picked {
                scene.fill_round(Rect::new(3.0, y, w - 6.0, ROW), 2.0, PICK_TOP, PICK_BOTTOM, true);
            }
            scene.text(Point::new(INSET, y + ROW * 0.5), *item, if picked { TEXT_BRIGHT } else { TEXT }, FONT, false);
        }
    }
}

impl EditorView {
    /// Answer the SDK's parameter menu with `hook` instead of popping up the
    /// menu window, for tests. See [`MenuHook`].
    #[doc(hidden)]
    pub fn set_menu_hook(&mut self, hook: Option<MenuHook>) {
        self.menu_hook = hook;
    }

    /// Choose item `index` of [`PARAM_MENU_ITEMS`] as if from the menu last
    /// shown, with `text` standing in for the clipboard (what paste reads;
    /// copy returns what it would put there). For tests; the menu's own
    /// items do the same with the real clipboard.
    #[doc(hidden)]
    pub fn choose_menu_item(&mut self, index: usize, text: Option<&str>) -> Option<String> {
        self.run_menu_item(index, text)
    }

    /// The open parameter menu's window (an override-redirect child of the root), if any.
    #[doc(hidden)]
    pub fn menu_window(&self) -> Option<Window> {
        self.menu.as_ref().map(|m| m.window)
    }

    /// Show the SDK's own parameter menu at the click, or hand it to the test hook.
    pub(super) fn show_param_menu(&mut self, menu: ParamMenu) {
        self.menu_param = menu.param;
        if let Some(hook) = self.menu_hook.as_mut() {
            if hook(&menu) {
                return;
            }
        }
        self.close_menu();
        self.hide_tip();
        if !self.shown() || !menu.x.is_finite() || !menu.y.is_finite() {
            return;
        }
        let label: String = menu.label.chars().filter(|c| !c.is_control()).collect();
        let (w, h) = menu_size(&label);
        let size = (self.px(w), self.px(h));
        // The click in window pixels (the window may be shown at another size).
        let ratio = |pixels: u32, points: u32| if points > 0 { pixels as f32 / points as f32 } else { self.scale };
        let click = ((menu.x * ratio(self.win.0, self.size.0)) as i32, (menu.y * ratio(self.win.1, self.size.1)) as i32);
        let Some((at, screen)) = self.on_root(click.0, click.1) else { return };
        let at = popup::place(at, size, screen);
        let mask = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION | EventMask::KEY_PRESS | EventMask::KEY_RELEASE;
        let Some(window) = self.make_window(Some(b"_NET_WM_WINDOW_TYPE_POPUP_MENU"), at, size, mask) else { return };
        self.menu = Some(Menu { window, size, label, hover: None });
        // Every press, wherever it lands, comes to the menu while it is open,
        // and so does every key. A grab the server refuses (another client
        // holds one) leaves a menu that goes with a press on the view.
        let events = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
        let pointer = self.conn.grab_pointer(false, window, events, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, x11rb::CURRENT_TIME);
        let keyboard = self.conn.grab_keyboard(false, window, x11rb::CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC);
        match (pointer, keyboard) {
            (Ok(pointer), Ok(keyboard)) => {
                let held = pointer.reply().is_ok_and(|r| r.status == GrabStatus::SUCCESS);
                let _ = keyboard.reply();
                if !held {
                    log::debug!("openplugin: the parameter menu got no pointer grab");
                }
            }
            _ => self.dead = true,
        }
        self.select_keys();
        self.paint_menu();
    }

    /// Take the menu away and let go of the pointer and the keyboard.
    pub(super) fn close_menu(&mut self) {
        let Some(menu) = self.menu.take() else { return };
        if !self.dead {
            x!(self, self.conn.ungrab_pointer(x11rb::CURRENT_TIME));
            x!(self, self.conn.ungrab_keyboard(x11rb::CURRENT_TIME));
        }
        self.drop_window(menu.window);
        if !self.dead {
            self.select_keys();
        }
    }

    pub(super) fn paint_menu(&mut self) {
        let Some(menu) = self.menu.take() else { return };
        self.blit(menu.window, menu.size, |scene| menu.draw(scene));
        self.menu = Some(menu);
    }

    fn menu_item_at(&self, x: i16, y: i16) -> Option<usize> {
        let menu = self.menu.as_ref()?;
        let inside = x >= 0 && y >= 0 && (x as u32) < menu.size.0 && (y as u32) < menu.size.1;
        if inside {
            item_at(&menu.label, f32::from(x) / self.scale, f32::from(y) / self.scale)
        } else {
            None
        }
    }

    fn menu_hover(&mut self, hover: Option<usize>) {
        if let Some(menu) = self.menu.as_mut() {
            if menu.hover != hover {
                menu.hover = hover;
                self.paint_menu();
            }
        }
    }

    pub(super) fn menu_motion(&mut self, e: &xproto::MotionNotifyEvent) {
        let hover = self.menu_item_at(e.event_x, e.event_y);
        self.menu_hover(hover);
    }

    /// A button in the menu's grab: a press outside dismisses, the left
    /// button let go on an item chooses it.
    pub(super) fn menu_button(&mut self, e: &xproto::ButtonPressEvent, press: bool) {
        let Some(size) = self.menu.as_ref().map(|m| m.size) else { return };
        self.last_time = e.time;
        let inside = e.event_x >= 0 && e.event_y >= 0 && (e.event_x as u32) < size.0 && (e.event_y as u32) < size.1;
        let item = self.menu_item_at(e.event_x, e.event_y);
        if press {
            if inside {
                self.menu_hover(item);
            } else {
                self.close_menu();
            }
        } else if e.detail == 1 {
            if let Some(index) = item {
                self.close_menu();
                self.run_menu_item(index, None);
            }
        }
    }

    /// A key while the menu is open: Escape dismisses, the arrows move, Return chooses.
    pub(super) fn menu_key(&mut self, e: &xproto::KeyPressEvent) {
        self.last_time = e.time;
        let hover = self.menu.as_ref().and_then(|m| m.hover);
        let last = PARAM_MENU_ITEMS.len() - 1;
        match self.keymap.lookup(e.detail, u16::from(e.state)) {
            0xff1b => self.close_menu(),
            0xff52 | 0xff97 => self.menu_hover(Some(hover.map_or(last, |i| i.saturating_sub(1)))),
            0xff54 | 0xff99 => self.menu_hover(Some(hover.map_or(0, |i| (i + 1).min(last)))),
            0xff0d | 0xff8d | 0x20 => {
                if let Some(index) = hover {
                    self.close_menu();
                    self.run_menu_item(index, None);
                }
            }
            _ => {}
        }
    }

    /// Run item `index` of [`PARAM_MENU_ITEMS`] for the parameter the menu
    /// was shown for. Paste reads the clipboard and copy writes it, unless
    /// `text` is given (tests), which stands in for the clipboard both ways.
    fn run_menu_item(&mut self, index: usize, text: Option<&str>) -> Option<String> {
        let param = self.menu_param;
        let action = match (index, text) {
            (0, _) => ParamMenuAction::Reset,
            (1, _) => ParamMenuAction::Copy,
            (2, Some(text)) => ParamMenuAction::Paste(text.to_string()),
            (2, None) => {
                // The clipboard's text may arrive in a later pump; the action follows it.
                self.paste(PasteTo::Menu(param));
                return None;
            }
            (3, _) => ParamMenuAction::Type,
            _ => return None,
        };
        let copied = self.guarded(|v| v.editor.param_menu_action(param, action));
        if let (Some(copy), None) = (copied.as_deref(), text) {
            self.set_clipboard_text(copy);
        }
        self.follow(true);
        self.touched = true;
        copied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_are_found_under_the_pointer() {
        let (w, h) = menu_size("Gain");
        assert!(w >= MIN_WIDTH && h == 2.0 * PAD + TITLE + 4.0 * ROW);
        assert_eq!(item_at("Gain", 10.0, PAD + TITLE * 0.5), None);
        assert_eq!(item_at("Gain", 10.0, PAD + TITLE + 1.0), Some(0));
        assert_eq!(item_at("Gain", 10.0, PAD + TITLE + ROW * 3.5), Some(3));
        assert_eq!(item_at("Gain", 10.0, PAD + TITLE + ROW * 4.0 + 1.0), None);
        assert_eq!(item_at("Gain", w + 1.0, PAD + TITLE + 1.0), None);
        // No title: the items start at the top.
        assert_eq!(item_at("", 10.0, PAD + 1.0), Some(0));
        assert_eq!(menu_size("").1, 2.0 * PAD + 4.0 * ROW);
        // A long name widens the menu.
        assert!(menu_size("A parameter with a very long name indeed").0 > w);
    }
}
