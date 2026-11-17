//! The clipboard: the view owns and serves a selection (`CLIPBOARD`) from its
//! window, and reads it by asking the owner and taking the answer in a later
//! pump. Written from the ICCCM's section on selections.

use super::EditorView;
use openplugin_core::editor::ParamMenuAction;
use std::time::{Duration, Instant};
use x11rb::protocol::xproto::{self, AtomEnum, ConnectionExt as _, EventMask, PropMode, SelectionNotifyEvent};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// How long a paste waits for the clipboard's owner to answer, in
/// milliseconds. The wait is spread over [`EditorView::pump`] calls and never
/// holds the host's thread; an owner that has not answered by then counts as
/// an empty clipboard.
pub const CLIPBOARD_WAIT_MS: u64 = 1000;

/// The most text that is read: larger answers come in pieces (`INCR`), which are not read.
const MAX_TEXT_LONGS: u32 = 1 << 18;

/// Who asked for the clipboard's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PasteTo {
    /// Ctrl+V in the type-in field.
    Field,
    /// "Paste value" of the menu for this parameter.
    Menu(u32),
}

struct Pending {
    to: PasteTo,
    /// The target asked for: `UTF8_STRING` first, then `STRING`.
    target: u32,
    deadline: Instant,
}

pub(super) struct Clipboard {
    selection: u32,
    utf8: u32,
    targets: u32,
    text: u32,
    plain_utf8: u32,
    plain: u32,
    incr: u32,
    /// The property answers arrive in, on the view's window.
    property: u32,
    /// The text the view serves while it owns the selection.
    owned: Option<String>,
    pending: Option<Pending>,
}

impl Clipboard {
    pub(super) fn new(conn: &RustConnection) -> Option<Clipboard> {
        let names: [&[u8]; 8] = [b"CLIPBOARD", b"UTF8_STRING", b"TARGETS", b"TEXT", b"text/plain;charset=utf-8", b"text/plain", b"INCR", b"OPENPLUGIN_CLIPBOARD"];
        let mut cookies = Vec::with_capacity(names.len());
        for name in names {
            cookies.push(conn.intern_atom(false, name).ok()?);
        }
        let mut atoms = [0u32; 8];
        for (atom, cookie) in atoms.iter_mut().zip(cookies) {
            *atom = cookie.reply().ok()?.atom;
        }
        let [selection, utf8, targets, text, plain_utf8, plain, incr, property] = atoms;
        Some(Clipboard { selection, utf8, targets, text, plain_utf8, plain, incr, property, owned: None, pending: None })
    }
}

/// Latin-1 bytes of a text, as the `STRING` target wants them.
fn latin1(text: &str) -> Vec<u8> {
    text.chars().map(|c| if (c as u32) < 256 { c as u32 as u8 } else { b'?' }).collect()
}

impl EditorView {
    /// The text the view has on the clipboard: what it last put there, until
    /// another client takes the clipboard over.
    #[doc(hidden)]
    pub fn clipboard_text(&self) -> Option<String> {
        self.clip.owned.clone()
    }

    /// Put `text` on the clipboard: the view owns the selection and serves
    /// the text from then on. False when the view has no window yet or the
    /// server did not make it the owner.
    #[doc(hidden)]
    pub fn set_clipboard_text(&mut self, text: &str) -> bool {
        if !self.usable() {
            return false;
        }
        let selection = self.clip.selection;
        // CurrentTime: a stale time of the view's own would lose to the present owner.
        x!(self, self.conn.set_selection_owner(self.window, selection, x11rb::CURRENT_TIME));
        let owner = self.conn.get_selection_owner(selection).ok().and_then(|cookie| cookie.reply().ok()).map(|reply| reply.owner);
        if owner != Some(self.window) {
            return false;
        }
        self.clip.owned = Some(text.to_string());
        true
    }

    /// Use another selection than `CLIPBOARD` (by atom name) from now on.
    /// For tests, which then leave the user's clipboard alone. False when the
    /// server gives no atom.
    #[doc(hidden)]
    pub fn set_clipboard_selection(&mut self, name: &str) -> bool {
        let atom = self.atom(name.as_bytes());
        if atom == 0 {
            return false;
        }
        self.clip.selection = atom;
        self.clip.owned = None;
        self.clip.pending = None;
        true
    }

    /// Ask for the clipboard's text for `to`. The view's own text is given at
    /// once; another owner's arrives in a later pump, or never, and then the
    /// paste ends as an empty one after [`CLIPBOARD_WAIT_MS`](super::CLIPBOARD_WAIT_MS).
    pub(super) fn paste(&mut self, to: PasteTo) {
        if let Some(old) = self.clip.pending.take() {
            self.pasted(old.to, None);
        }
        if let Some(text) = self.clip.owned.clone() {
            return self.pasted(to, Some(text));
        }
        if !self.usable() {
            return self.pasted(to, None);
        }
        let target = self.clip.utf8;
        self.convert(target);
        self.clip.pending = Some(Pending { to, target, deadline: Instant::now() + Duration::from_millis(CLIPBOARD_WAIT_MS) });
    }

    fn convert(&mut self, target: u32) {
        let (selection, property) = (self.clip.selection, self.clip.property);
        x!(self, self.conn.convert_selection(self.window, selection, target, property, x11rb::CURRENT_TIME));
        self.flush();
    }

    /// The clipboard's text arrived, or there is none.
    fn pasted(&mut self, to: PasteTo, text: Option<String>) {
        match to {
            PasteTo::Field => {
                if let (Some(field), Some(text)) = (self.field.as_mut(), text) {
                    // One line: the first of them.
                    field.insert(text.trim_matches(['\r', '\n']).lines().next().unwrap_or(""));
                    self.paint_field();
                }
            }
            PasteTo::Menu(param) => {
                self.guarded(|v| v.editor.param_menu_action(param, ParamMenuAction::Paste(text.unwrap_or_default())));
                self.follow(true);
                self.touched = true;
            }
        }
    }

    /// An owner that never answered: the paste ends empty.
    pub(super) fn clipboard_timeout(&mut self) {
        if self.clip.pending.as_ref().is_some_and(|p| Instant::now() >= p.deadline) {
            if let Some(pending) = self.clip.pending.take() {
                self.pasted(pending.to, None);
            }
        }
    }

    /// The answer to the view's `ConvertSelection`.
    pub(super) fn selection_notify(&mut self, e: &SelectionNotifyEvent) {
        if e.requestor != self.window || e.selection != self.clip.selection {
            return;
        }
        let Some(pending) = self.clip.pending.take() else {
            // An answer nobody waits for any more: its property goes.
            if e.property != x11rb::NONE && self.usable() {
                x!(self, self.conn.delete_property(self.window, e.property));
            }
            return;
        };
        if e.target != pending.target {
            // An answer to an earlier request: keep waiting for this one.
            self.clip.pending = Some(pending);
            return;
        }
        if e.property == x11rb::NONE {
            // No UTF-8 from this owner: ask once more for Latin-1.
            if pending.target == self.clip.utf8 {
                let target = u32::from(AtomEnum::STRING);
                self.convert(target);
                self.clip.pending = Some(Pending { target, ..pending });
            } else {
                self.pasted(pending.to, None);
            }
            return;
        }
        let reply = self.conn.get_property(true, self.window, e.property, AtomEnum::ANY, 0, MAX_TEXT_LONGS).ok().and_then(|cookie| cookie.reply().ok());
        let text = reply.filter(|r| r.format == 8 && r.type_ != self.clip.incr && r.bytes_after == 0).map(|r| {
            if pending.target == self.clip.utf8 {
                String::from_utf8_lossy(&r.value).into_owned()
            } else {
                r.value.iter().map(|&b| char::from(b)).collect()
            }
        });
        self.pasted(pending.to, text);
    }

    /// Another client took the selection: the view's text is off the clipboard.
    pub(super) fn selection_clear(&mut self, e: &xproto::SelectionClearEvent) {
        if e.selection == self.clip.selection {
            self.clip.owned = None;
        }
    }

    /// Another client asks for the text the view owns.
    pub(super) fn selection_request(&mut self, e: &xproto::SelectionRequestEvent) {
        if self.dead {
            return;
        }
        // An old client names no property: the target's name is used.
        let property = if e.property == x11rb::NONE { e.target } else { e.property };
        let clip = &self.clip;
        let string = u32::from(AtomEnum::STRING);
        let room = self.max_request.saturating_sub(64);
        let served = match clip.owned.as_deref().filter(|_| e.owner == self.window && e.selection == clip.selection) {
            None => false,
            Some(_) if e.target == clip.targets => {
                let list = [clip.targets, clip.utf8, string, clip.text, clip.plain_utf8, clip.plain];
                self.conn.change_property32(PropMode::REPLACE, e.requestor, property, AtomEnum::ATOM, &list).is_ok()
            }
            Some(text) if text.len() > room => false,
            Some(text) if e.target == clip.utf8 || e.target == clip.plain_utf8 => {
                self.conn.change_property8(PropMode::REPLACE, e.requestor, property, e.target, text.as_bytes()).is_ok()
            }
            // TEXT: the owner picks the encoding.
            Some(text) if e.target == clip.text => self.conn.change_property8(PropMode::REPLACE, e.requestor, property, clip.utf8, text.as_bytes()).is_ok(),
            Some(text) if e.target == string || e.target == clip.plain => {
                self.conn.change_property8(PropMode::REPLACE, e.requestor, property, e.target, &latin1(text)).is_ok()
            }
            Some(_) => false,
        };
        let answer = SelectionNotifyEvent {
            response_type: xproto::SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: e.time,
            requestor: e.requestor,
            selection: e.selection,
            target: e.target,
            property: if served { property } else { x11rb::NONE },
        };
        x!(self, self.conn.send_event(false, e.requestor, EventMask::NO_EVENT, answer));
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_is_latin1() {
        assert_eq!(latin1("3 dB \u{e9}\u{416}"), [b'3', b' ', b'd', b'B', b' ', 0xe9, b'?']);
    }
}
