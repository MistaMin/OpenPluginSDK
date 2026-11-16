//! The Linux view that hosts an [`Editor`]: an X11 child window, a 30 Hz
//! tick, and a blit of the frame the editor renders. Pure Rust: it speaks the
//! X11 protocol through `x11rb` and links no C library.
//!
//! [`EditorView`] goes inside a host's parent window (CLAP `clap.gui` with
//! the `x11` API, LV2 `ui:X11UI`, the VST3 shell's `X11EmbedWindowID`). Linux
//! hosts embed plugin editors as X11 windows, in a Wayland session too
//! (through Xwayland). [`run_window`] puts the same view in a window of its
//! own and runs until it closes (standalone).
//!
//! Event loop: a Linux host runs no event loop for a plugin's own X
//! connection. The view therefore owns its connection and is driven from
//! outside: the format bridge calls [`EditorView::pump`] about every
//! [`TICK_INTERVAL_MS`] milliseconds (CLAP `timer-support`, LV2
//! `ui:idleInterface`, VST3 `IRunLoop` timers) and, where the host can watch
//! file descriptors, whenever [`EditorView::fd`] is readable (CLAP
//! `posix-fd-support`, VST3 `IRunLoop` event handlers). `pump` never blocks.
//!
//! Size, repaint and input follow `cocoa.rs`: the window follows the editor's
//! size inside its [`SizeRange`]; mouse positions are in editor points; after
//! each tick and input call only the editor's damage is presented.
//!
//! Scale: the view reads `Xft.dpi` from the root window's `RESOURCE_MANAGER`
//! property once, when it is made (96 dpi is 1.0, clamped to 1.0..=3.0 as on
//! macOS; 1.0 when the property or the entry is missing). The window is the
//! editor's size times that scale in device pixels, [`Editor::render`] is
//! asked for that scale, and mouse positions are divided back into points.
//! `Xft.dpi` is what desktop settings daemons and toolkits (GTK, Qt) agree
//! on under X11 and Xwayland; a per-monitor scale does not exist in core X11.
//!
//! Pixels: a [`Frame`](openplugin_core::Frame) pixel is a `0x00RRGGBB` word.
//! The view draws on a TrueColor visual (the root's when it is one) and packs
//! each pixel through the visual's colour masks in the server's byte order,
//! at 16, 24 or 32 bits per pixel, then sends it with `PutImage` (ZPixmap) in
//! slices that fit the server's maximum request length (BIG-REQUESTS when the
//! server has it). When the window and the frame differ in size (a host that
//! set the window itself), the frame is stretched, nearest pixel.
//!
//! Size: [`EditorView::resize`], a `ConfigureNotify` for a size the host gave
//! the window, and an editor that changed its own size all end in the window
//! and the editor agreeing. A resize that arrives while the editor is inside
//! one of its own calls waits until that call ends.
//!
//! Input: the left button is `mouse_down` (with a click count: presses within
//! [`DOUBLE_CLICK_MS`] and a few pixels of each other) and `mouse_up`, the
//! right button `right_mouse_down`, buttons 4 and 5 a wheel step of +1 and -1
//! (positive is away from the user). Motion keeps arriving during a drag
//! outside the window (the server's implicit grab). Keys go through the
//! server's core keyboard mapping (`GetKeyboardMapping`; no XKB): the key's
//! unshifted symbol, lower case, as on macOS. Key events are selected only
//! while [`Editor::wants_keys`] is true, so an editor that wants none leaves
//! every key with the host; a click then takes the input focus
//! (`SetInputFocus`), and a key the editor did not use is sent on to the
//! parent window (`SendEvent`).
//!
//! Before `attach`: [`EditorView::new`] connects and reserves the window's id
//! ([`EditorView::raw`]), and `attach` creates the window straight inside the
//! host's parent. The window is never a child of the root, so no window
//! manager ever hears of it. That matters: a window made on the root and
//! reparented away is destroyed out of the window manager's sight, and
//! Hyprland (through Xwayland) keeps such a window's id and geometry and
//! later moves and sizes whichever new window is given the same id.
//!
//! Errors: nothing the server or the host does panics or exits. X errors are
//! ignored; when the window is destroyed with its parent, or the connection
//! is lost, the view stops talking to the server and keeps ticking the editor.
//!
//! Editor services, which the macOS view gets from AppKit widgets, are the
//! view's own here, drawn with the SDK's drawing stack (the GoodLookin scene
//! and its software rasteriser, the same face the editors use), so they look
//! like the editor and not like a foreign toolkit. They follow `cocoa.rs`:
//!
//! - The type-in field ([`Editor::take_text_edit`], `text_field.rs`): a child
//!   window over the control with the text selected. While it is open every
//!   key goes to it and none reaches the editor or the host (the view selects
//!   keys and takes the input focus for that time). Printable characters
//!   (through the core keyboard mapping with Shift, Caps Lock, Num Lock, the
//!   second group and Mod5 as level three), Backspace, Delete, Left, Right,
//!   Home, End (Shift extends the selection), Ctrl+A, Ctrl+C, Ctrl+X, Ctrl+V.
//!   Return and keypad Enter commit; refused text keeps the field open, red,
//!   with a bell. Escape cancels. Focus leaving, Tab, the window being
//!   unmapped or hidden commit valid text and drop invalid text. A click on
//!   the view outside the field, the right button and a size change cancel.
//!   Not there: compose and dead keys, input methods (no XIM/XKB), the legacy
//!   national keysym sets, dragging a selection with the mouse.
//! - The parameter menu ([`Editor::take_param_menu`], `menu.rs`): the items of
//!   [`PARAM_MENU_ITEMS`] in an override-redirect window at the click, kept
//!   on the screen. The pointer and the keyboard are grabbed while it is open
//!   and released on every way out (an item chosen, a press outside, Escape,
//!   the view hidden, unmapped, unfocused or dropped). Up, Down and Return
//!   work too. The menu does not block: the editor keeps ticking under it.
//!   Under Xwayland a press on a Wayland-native window is not seen by X; the
//!   menu then goes when the view loses the focus.
//! - The clipboard (`clipboard.rs`): the view owns and serves the `CLIPBOARD`
//!   selection (`UTF8_STRING`, `STRING`, `TEXT`, `text/plain`, `TARGETS`)
//!   from its window for as long as it lives; the text is gone with the view
//!   (no clipboard manager hand-over, `SAVE_TARGETS`). Reading asks the owner
//!   (`ConvertSelection`) and takes the answer in a later [`EditorView::pump`]:
//!   the host's thread never waits. An owner that has not answered within
//!   [`CLIPBOARD_WAIT_MS`] counts as an empty clipboard. Texts sent in pieces
//!   (`INCR`, texts larger than a request) are neither read nor served.
//! - Tooltips ([`Editor::tooltips`], `tooltip.rs`): once the pointer has
//!   rested [`TOOLTIP_DELAY_MS`] inside a tooltip rectangle, a small
//!   override-redirect window shows the text, asked of the editor at that
//!   moment. It goes when the pointer leaves the rectangle or the window, on
//!   a click, a key the view receives, and when the view is hidden.
//!
//! Hyprland keeps the id and geometry of root windows it never saw destroyed
//! and applies them to a later window with the same id; the first ids of a
//! connection are the likely ones, so the view leaves them unused for its
//! pop-up windows.
//!
//! Changes a host may look for at once (the view hidden or shown, a pop-up
//! mapped or destroyed, the focus taken on a click, a key sent on) are
//! followed by one round trip to the server, so they are done when the call
//! returns; nothing else waits for the server.
//!
//! Not served here (the view never calls these [`Editor`] hooks):
//! `take_file_dialog`, `wants_drops`/`files_dropped` (no XDND), and the
//! accessibility nodes (no AT-SPI).
//!
//! Main thread only: the thread that made the view makes every call.

use openplugin_core::editor::{DamageRect, Key, Modifiers, SizeRange};
use openplugin_core::Editor;
use std::os::fd::{AsRawFd, RawFd};
use std::time::{Duration, Instant};

pub use clipboard::CLIPBOARD_WAIT_MS;
pub use menu::{MenuHook, PARAM_MENU_ITEMS};
pub use tooltip::TOOLTIP_DELAY_MS;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{
    self, AtomEnum, ChangeWindowAttributesAux, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask,
    ImageFormat, ImageOrder, InputFocus, NotifyDetail, NotifyMode, PropMode, VisualClass, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// An X11 window id (an XID), as hosts hand it over.
pub type Window = u32;

/// Called before every tick with the editor, so a format bridge can pass it
/// the values that changed in the host since the last tick.
pub type Poll = Box<dyn FnMut(&mut dyn Editor)>;

/// How often a bridge calls [`EditorView::pump`] from the host's timer.
pub const TICK_INTERVAL_MS: u32 = 33;

/// Two presses of the left button this close in time (server milliseconds)
/// and within a few pixels count as a double click. X11 has no setting for it.
pub const DOUBLE_CLICK_MS: u32 = 400;

/// A timer that fires a little early still ticks: a pump this many
/// milliseconds short of the interval counts as due.
const TICK_SLACK_MS: f32 = 5.0;

/// The largest `PutImage` the view sends, whatever the server allows.
const MAX_IMAGE_BYTES: usize = 1 << 22;

const SHIFT: u16 = 1;
const LOCK: u16 = 2;
/// Mod5: level three (AltGr) in the core mapping XKB servers keep.
const LEVEL3: u16 = 0x80;
const CONTROL: u16 = 4;

/// One colour channel of the visual: where it sits and how wide it is.
#[derive(Clone, Copy)]
struct Channel {
    shift: u32,
    bits: u32,
}

impl Channel {
    fn of(mask: u32) -> Channel {
        if mask == 0 {
            return Channel { shift: 0, bits: 0 };
        }
        Channel { shift: mask.trailing_zeros(), bits: (mask >> mask.trailing_zeros()).trailing_ones() }
    }

    fn pack(self, v: u32) -> u32 {
        let v = if self.bits >= 8 { v << (self.bits - 8).min(24) } else { v >> (8 - self.bits) };
        v << self.shift
    }
}

/// How the window's visual stores a pixel.
#[derive(Clone, Copy)]
struct PixelFormat {
    depth: u8,
    /// 16, 24 or 32.
    bpp: usize,
    /// Rows are padded to this many bits.
    pad: usize,
    msb: bool,
    red: Channel,
    green: Channel,
    blue: Channel,
    /// Bits set in every pixel: the alpha of a 32-bit visual.
    fill: u32,
    /// The visual stores `0x00RRGGBB` as the frame does.
    direct: bool,
}

impl PixelFormat {
    fn stride(&self, width: usize) -> usize {
        (width * self.bpp).div_ceil(self.pad) * self.pad / 8
    }

    fn pack(&self, c: u32) -> u32 {
        if self.direct {
            (c & 0x00ff_ffff) | self.fill
        } else {
            self.red.pack((c >> 16) & 0xff) | self.green.pack((c >> 8) & 0xff) | self.blue.pack(c & 0xff) | self.fill
        }
    }

    fn write_row(&self, src: impl Iterator<Item = u32>, line: &mut [u8]) {
        let bytes = self.bpp / 8;
        for (px, c) in line.chunks_exact_mut(bytes).zip(src) {
            let v = self.pack(c);
            if self.msb {
                px.copy_from_slice(&v.to_be_bytes()[4 - bytes..]);
            } else {
                px.copy_from_slice(&v.to_le_bytes()[..bytes]);
            }
        }
    }
}

/// The last press of the left button, for the click count.
#[derive(Clone, Copy)]
struct Click {
    time: u32,
    x: i16,
    y: i16,
    count: u32,
    /// The window the press was in: the view's, or the field's.
    window: Window,
}

/// The server's keyboard mapping and which modifier bits mean what.
struct Keymap {
    min: u8,
    per: usize,
    syms: Vec<u32>,
    alt: u16,
    command: u16,
    num_lock: u16,
}

impl Keymap {
    fn sym(&self, code: u8, column: usize) -> u32 {
        if code < self.min || column >= self.per {
            return 0;
        }
        self.syms.get(usize::from(code - self.min) * self.per + column).copied().unwrap_or(0)
    }

    /// The symbol a key stands for: the unshifted one of the active group, or
    /// the keypad's second one while Num Lock is on.
    fn lookup(&self, code: u8, state: u16) -> u32 {
        let group = usize::from((state >> 13) & 3);
        let base = if group > 0 && self.sym(code, 2) != 0 { 2 } else { 0 };
        let (first, second) = (self.sym(code, base), self.sym(code, base + 1));
        if state & self.num_lock != 0 && (0xff80..=0xffbd).contains(&second) {
            second
        } else {
            first
        }
    }

    /// The symbol a key types with the modifiers in `state`, and the
    /// character of it (Shift, Caps Lock, Num Lock, group two, level three).
    fn typed(&self, code: u8, state: u16) -> (u32, Option<char>) {
        let group = usize::from((state >> 13) & 3);
        let mut base = if group > 0 && self.sym(code, 2) != 0 { 2 } else { 0 };
        if state & LEVEL3 != 0 && self.sym(code, 4) != 0 {
            base = 4;
        }
        let (first, second) = (self.sym(code, base), self.sym(code, base + 1));
        let (shift, lock) = (state & SHIFT != 0, state & LOCK != 0);
        if (0xff80..=0xffbd).contains(&second) {
            let sym = if (state & self.num_lock != 0) != shift { second } else { first };
            return (sym, char_from_keysym(sym));
        }
        let sym = if shift && second != 0 { second } else { first };
        let mut c = char_from_keysym(sym);
        // A key with one symbol has its upper case under Shift; Caps Lock turns letters over.
        let upper = (shift && second == 0) != (lock && c.is_some_and(char::is_alphabetic) && (second == 0 || !shift));
        let lower = lock && shift && second != 0;
        if let Some(ch) = c {
            if lower {
                c = ch.to_lowercase().next();
            } else if upper {
                c = ch.to_uppercase().next();
            }
        }
        (sym, c)
    }

    /// The modifier bit a modifier key holds, if the symbol is one.
    fn modifier_bit(&self, sym: u32) -> Option<u16> {
        match sym {
            0xffe1 | 0xffe2 => Some(SHIFT),
            0xffe3 | 0xffe4 => Some(CONTROL),
            0xffe7..=0xffea => Some(self.alt),
            0xffeb | 0xffec => Some(self.command),
            _ => None,
        }
    }
}

/// A key from an X keysym, or `None` for a key the contract has no name for.
/// Characters come out lower case. Symbols outside Latin-1 that are not
/// Unicode keysyms (the legacy national sets) have no name here.
fn key_from_keysym(sym: u32) -> Option<Key> {
    Some(match sym {
        0xff51 | 0xff96 => Key::Left,
        0xff52 | 0xff97 => Key::Up,
        0xff53 | 0xff98 => Key::Right,
        0xff54 | 0xff99 => Key::Down,
        0xff55 | 0xff9a => Key::PageUp,
        0xff56 | 0xff9b => Key::PageDown,
        0xff50 | 0xff95 => Key::Home,
        0xff57 | 0xff9c => Key::End,
        0xff09 | 0xfe20 | 0xff89 => Key::Tab,
        0xff0d | 0xff8d => Key::Return,
        0xff1b => Key::Escape,
        0xffff | 0xff9f => Key::Delete,
        0xff08 => Key::Backspace,
        0x20 | 0xff80 => Key::Space,
        // Keypad digits and operators are their ASCII characters plus 0xff80.
        0xffaa..=0xffb9 | 0xffbd => Key::Char(char::from_u32(sym - 0xff80)?),
        0x21..=0x7e | 0xa0..=0xff => lower(char::from_u32(sym)?),
        0x0100_0100..=0x0110_ffff => {
            let c = char::from_u32(sym - 0x0100_0000)?;
            if c.is_control() {
                return None;
            }
            lower(c)
        }
        _ => return None,
    })
}

/// The character a keysym types, if it types one: Latin-1, the keypad and
/// the Unicode keysyms. The legacy national sets type nothing here.
fn char_from_keysym(sym: u32) -> Option<char> {
    match sym {
        0x20..=0x7e | 0xa0..=0xff => char::from_u32(sym),
        0xff80 => Some(' '),
        0xffaa..=0xffb9 | 0xffbd => char::from_u32(sym - 0xff80),
        0x0100_0100..=0x0110_ffff => char::from_u32(sym - 0x0100_0000).filter(|c| !c.is_control()),
        _ => None,
    }
}

fn lower(c: char) -> Key {
    Key::Char(c.to_lowercase().next().unwrap_or(c))
}

/// The scale `Xft.dpi` asks for in an X resource database, if it has one.
fn scale_from_resources(resources: &[u8]) -> Option<f32> {
    let text = String::from_utf8_lossy(resources);
    let dpi = text.lines().find_map(|line| line.trim().strip_prefix("Xft.dpi:"))?.trim().parse::<f32>().ok()?;
    (dpi.is_finite() && dpi > 0.0).then(|| (dpi / 96.0).clamp(1.0, 3.0))
}

/// An editor length in device pixels.
/// The scale of a display: `Xft.dpi` in the root window's resources, or 1.0.
fn scale_of_display(conn: &RustConnection, root: Window) -> f32 {
    conn.get_property(false, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 20)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| scale_from_resources(&reply.value))
        .unwrap_or(1.0)
}

/// Device pixels per point on the display named by `DISPLAY`: the scale a
/// view made now would have ([`EditorView::scale`]). For a format bridge that
/// is asked for a size in pixels before it has a view (the VST3 shell's
/// `IPlugView::getSize`). `None` when there is no X server to connect to.
/// Connects and disconnects: ask once and keep the answer.
pub fn display_scale() -> Option<f32> {
    let (conn, screen_index) = RustConnection::connect(None).ok()?;
    let root = conn.setup().roots.get(screen_index)?.root;
    Some(scale_of_display(&conn, root))
}

fn device(points: u32, scale: f32) -> u32 {
    ((points as f32 * scale).round() as u32).clamp(1, 32767)
}

/// An [`Editor`] in an X11 window.
pub struct EditorView {
    editor: Box<dyn Editor>,
    poll: Option<Poll>,
    /// The editor's size, which mouse positions are mapped to.
    size: (u32, u32),
    last: Instant,
    conn: RustConnection,
    root: Window,
    visual: u32,
    window: Window,
    /// The server has been asked for the window (`attach`, `run_window`).
    created: bool,
    gc: u32,
    /// A colormap the view made for a visual that is not the root's.
    colormap: Option<u32>,
    format: PixelFormat,
    /// Device pixels per point.
    scale: f32,
    /// The window's size in device pixels.
    win: (u32, u32),
    /// The view's last own `ConfigureWindow`: a `ConfigureNotify` from before
    /// it tells of a size that is already gone.
    configured: Option<(u16, Instant)>,
    max_request: usize,
    buf: Vec<u8>,
    /// The window's parent, zero while it is a child of the root.
    parent: Window,
    /// The window is a top-level one of its own ([`run_window`]).
    top_level: bool,
    hidden: bool,
    /// The server still has the window.
    alive: bool,
    /// The connection is lost.
    dead: bool,
    /// The modifier keys the editor was last told about.
    modifiers: Modifiers,
    /// The editor is inside one of its calls.
    busy: bool,
    /// A resize that came in while `busy`: width, height, and whether the
    /// window must follow (it must not when the host sized the window).
    pending: Option<(u32, u32, bool)>,
    keymap: Keymap,
    /// Key events are selected on the window.
    keys_selected: bool,
    focused: bool,
    click: Option<Click>,
    /// The whole window must be presented.
    full: bool,
    /// Device rectangles to present: x, y, width, height.
    rects: Vec<(u32, u32, u32, u32)>,
    /// The editor took input since its damage was last asked for.
    touched: bool,
    wm_protocols: u32,
    wm_delete: u32,
    close_requested: bool,
    /// Server time of the last input event.
    last_time: u32,
    /// Draws the field, the menu and the tooltip.
    painter: popup::Painter,
    /// The open type-in field.
    field: Option<text_field::Field>,
    /// The size changed under the field: it closes at the next poll.
    field_cancel: bool,
    /// The open parameter menu.
    menu: Option<menu::Menu>,
    /// The parameter the menu was last asked for.
    menu_param: u32,
    menu_hook: Option<MenuHook>,
    clip: clipboard::Clipboard,
    tips: tooltip::Tips,
}

/// Run an X request; a lost connection marks the view dead.
macro_rules! x {
    ($view:expr, $request:expr) => {
        if $request.is_err() {
            $view.dead = true;
        }
    };
}

mod clipboard;
mod menu;
mod popup;
mod text_field;
mod tooltip;

impl EditorView {
    /// Connects to the X server named by `DISPLAY` and reserves the window's
    /// id; the window itself is made by [`EditorView::attach`], inside the
    /// parent (see the module notes). `poll` runs before every tick (see
    /// [`Poll`]). `None` when there is no X server to connect to.
    pub fn new(editor: Box<dyn Editor>, poll: Option<Poll>) -> Option<EditorView> {
        let (conn, screen_index) = RustConnection::connect(None).ok()?;
        let setup = conn.setup();
        let screen = setup.roots.get(screen_index)?;
        let root = screen.root;

        // The root's visual when it is TrueColor, else the first TrueColor one.
        let mut choice: Option<(u8, xproto::Visualtype)> = None;
        for depth in &screen.allowed_depths {
            for visual in &depth.visuals {
                if visual.class != VisualClass::TRUE_COLOR {
                    continue;
                }
                let better = match choice {
                    None => true,
                    Some((d, v)) => v.visual_id != screen.root_visual && d < 24 && depth.depth >= 24,
                };
                if visual.visual_id == screen.root_visual || better {
                    choice = Some((depth.depth, *visual));
                }
            }
        }
        let (depth, visual) = choice?;
        let layout = setup.pixmap_formats.iter().find(|f| f.depth == depth)?;
        let bpp = usize::from(layout.bits_per_pixel);
        let pad = usize::from(layout.scanline_pad);
        if !matches!(bpp, 16 | 24 | 32) || !matches!(pad, 8 | 16 | 32) {
            return None;
        }
        let masks = visual.red_mask | visual.green_mask | visual.blue_mask;
        let format = PixelFormat {
            depth,
            bpp,
            pad,
            msb: setup.image_byte_order == ImageOrder::MSB_FIRST,
            red: Channel::of(visual.red_mask),
            green: Channel::of(visual.green_mask),
            blue: Channel::of(visual.blue_mask),
            fill: if depth == 32 { !masks } else { 0 },
            direct: (visual.red_mask, visual.green_mask, visual.blue_mask) == (0xff_0000, 0xff00, 0xff),
        };

        let scale = scale_of_display(&conn, root);

        let size = editor.size();
        let win = (device(size.0, scale), device(size.1, scale));
        let window = conn.generate_id().ok()?;
        let gc = conn.generate_id().ok()?;
        let mut colormap = None;
        if visual.visual_id != screen.root_visual {
            let id = conn.generate_id().ok()?;
            conn.create_colormap(ColormapAlloc::NONE, id, root, visual.visual_id).ok()?;
            colormap = Some(id);
        }
        let visual = visual.visual_id;
        let max_request = conn.maximum_request_bytes().min(MAX_IMAGE_BYTES);
        // Ids left unused: see the module notes on Hyprland.
        for _ in 0..16 {
            conn.generate_id().ok()?;
        }
        let keymap = read_keymap(&conn)?;
        let clip = clipboard::Clipboard::new(&conn)?;
        conn.flush().ok()?;

        Some(EditorView {
            editor,
            poll,
            size,
            last: Instant::now(),
            conn,
            root,
            visual,
            window,
            created: false,
            gc,
            colormap,
            format,
            scale,
            win,
            configured: None,
            max_request,
            buf: Vec::new(),
            parent: 0,
            top_level: false,
            hidden: false,
            alive: true,
            dead: false,
            modifiers: Modifiers::default(),
            busy: false,
            pending: None,
            keymap,
            keys_selected: false,
            focused: false,
            click: None,
            full: true,
            rects: Vec::new(),
            touched: false,
            wm_protocols: 0,
            wm_delete: 0,
            close_requested: false,
            last_time: 0,
            painter: popup::Painter::new(),
            field: None,
            field_cancel: false,
            menu: None,
            menu_param: 0,
            menu_hook: None,
            clip,
            tips: tooltip::Tips::default(),
        })
    }

    /// The view's own window: its id from [`EditorView::new`] on, a window on
    /// the server once attached. Zero is never a window.
    pub fn raw(&self) -> Window {
        self.window
    }

    /// The editor's current size in points.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Device pixels per point: the window is [`EditorView::size`] times this.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The sizes the editor can take.
    pub fn size_range(&self) -> SizeRange {
        self.editor.size_range()
    }

    /// Move the editor and the window to `width` x `height` points, brought
    /// into the editor's range. Returns the size the editor takes. If the
    /// editor is inside one of its own calls, the change is made when that
    /// call ends and the size returned is the one asked for, in range. A
    /// fixed editor stays as it is.
    pub fn resize(&mut self, width: u32, height: u32) -> (u32, u32) {
        let (w, h) = self.size_range().clamp(width, height);
        if self.busy {
            self.pending = Some((w, h, true));
            return (w, h);
        }
        self.apply(w, h, true);
        self.flush();
        self.size
    }

    /// Put the window inside a host's parent window and show it. False when
    /// `parent` is zero or the server refuses.
    pub fn attach(&mut self, parent: Window) -> bool {
        if parent == 0 || self.dead || !self.alive {
            return false;
        }
        if self.created {
            self.services_hide();
        }
        let done = if self.created {
            // Attached before: move to the new parent.
            match self.conn.reparent_window(self.window, parent, 0, 0) {
                Ok(cookie) => cookie.check().is_ok(),
                Err(_) => {
                    self.dead = true;
                    false
                }
            }
        } else {
            self.create(parent)
        };
        if !done {
            return false;
        }
        self.parent = parent;
        self.full = true;
        if !self.hidden {
            x!(self, self.conn.map_window(self.window));
        }
        self.flush();
        !self.dead
    }

    /// Show or hide the window.
    pub fn set_hidden(&mut self, hidden: bool) {
        if hidden == self.hidden {
            return;
        }
        self.hidden = hidden;
        if !self.usable() {
            return;
        }
        if hidden {
            self.services_hide();
            x!(self, self.conn.unmap_window(self.window));
        } else {
            self.full = true;
            x!(self, self.conn.map_window(self.window));
        }
        self.settle();
    }

    /// Send what is queued and wait until the server has done it, so that
    /// another client (the host) sees it done when the call returns.
    fn settle(&mut self) {
        if !self.dead && self.conn.get_input_focus().map_err(|_| ()).and_then(|cookie| cookie.reply().map_err(|_| ())).is_err() {
            self.dead = true;
        }
    }

    /// The hosted editor.
    pub fn editor(&mut self) -> Option<&mut (dyn Editor + 'static)> {
        Some(self.editor.as_mut())
    }

    pub fn set_poll(&mut self, poll: Option<Poll>) {
        self.poll = poll;
    }

    /// The X connection's file descriptor, for a host that watches
    /// descriptors: call [`EditorView::pump`] when it is readable.
    pub fn fd(&self) -> RawFd {
        self.conn.stream().as_raw_fd()
    }

    /// Handle every X event that has arrived, then, if a tick is due
    /// ([`TICK_INTERVAL_MS`] since the last), run the poll hook and
    /// [`Editor::tick`], follow a size the editor changed itself, and present
    /// what the editor damaged. Never blocks. Calling it more often than the
    /// tick interval is fine: input is handled at once and ticks stay at 30 Hz.
    pub fn pump(&mut self) {
        // Sending a frame can read events into the connection's queue, where a
        // descriptor watcher cannot see them: go round until none is left.
        for pass in 0..4 {
            let handled = self.drain();
            if pass > 0 && !handled {
                break;
            }
            if pass == 0 {
                self.tick_if_due();
                self.service_timers();
            }
            if self.touched {
                self.touched = false;
                self.poll_requests();
                self.invalidate();
            }
            self.present();
            self.flush();
        }
    }

    /// The window can take requests.
    fn usable(&self) -> bool {
        self.created && self.alive && !self.dead
    }

    /// Make the window inside `parent`, unmapped. False when the server refuses.
    fn create(&mut self, parent: Window) -> bool {
        // No background: the server never paints over a frame before the next one.
        let mut aux = CreateWindowAux::new().border_pixel(0).event_mask(event_mask(self.keys_selected));
        if let Some(colormap) = self.colormap {
            aux = aux.colormap(colormap);
        }
        let (w, h) = (self.win.0 as u16, self.win.1 as u16);
        let made = self.conn.create_window(self.format.depth, self.window, parent, 0, 0, w, h, 0, WindowClass::INPUT_OUTPUT, self.visual, &aux);
        let made = match made {
            Ok(cookie) => cookie.check(),
            Err(e) => Err(e.into()),
        };
        match made {
            Ok(()) => {
                x!(self, self.conn.create_gc(self.gc, self.window, &xproto::CreateGCAux::new().graphics_exposures(0)));
                self.created = true;
                !self.dead
            }
            Err(x11rb::errors::ReplyError::ConnectionError(_)) => {
                self.dead = true;
                false
            }
            Err(x11rb::errors::ReplyError::X11Error(_)) => false,
        }
    }

    /// The window is on screen as far as the view knows.
    fn shown(&self) -> bool {
        self.usable() && !self.hidden && (self.top_level || self.parent != 0)
    }

    fn flush(&mut self) {
        if !self.dead {
            x!(self, self.conn.flush());
        }
    }

    /// Run an editor call. A resize that arrived meanwhile is applied after it.
    fn guarded<R>(&mut self, call: impl FnOnce(&mut Self) -> R) -> R {
        self.busy = true;
        let result = call(self);
        self.busy = false;
        if let Some((w, h, frame)) = self.pending.take() {
            self.apply(w, h, frame);
        }
        result
    }

    /// Give the editor a size inside its range, and follow the size it ends up with.
    fn apply(&mut self, width: u32, height: u32, frame: bool) {
        let (w, h) = self.editor.size_range().clamp(width, height);
        if (w, h) != self.editor.size() {
            self.editor.resize(w, h);
        }
        self.follow(frame);
        if frame {
            // The host asked for the size the editor has: the window may still differ.
            self.set_window_size((device(self.size.0, self.scale), device(self.size.1, self.scale)));
        }
    }

    /// Make the window the editor's size if the editor's size changed.
    fn follow(&mut self, frame: bool) {
        let now = self.editor.size();
        if now == self.size || now.0 == 0 || now.1 == 0 {
            return;
        }
        self.size = now;
        self.full = true;
        self.field_cancel = self.field.is_some();
        if frame {
            self.set_window_size((device(now.0, self.scale), device(now.1, self.scale)));
        }
    }

    fn set_window_size(&mut self, win: (u32, u32)) {
        if win == self.win {
            return;
        }
        self.win = win;
        self.full = true;
        self.field_cancel = self.field.is_some();
        if !self.usable() {
            return;
        }
        match self.conn.configure_window(self.window, &ConfigureWindowAux::new().width(win.0).height(win.1)) {
            Ok(cookie) => self.configured = Some((cookie.sequence_number() as u16, Instant::now())),
            Err(_) => self.dead = true,
        }
    }

    /// A size from outside: now, or after the editor's current call.
    fn resize_to(&mut self, width: u32, height: u32, frame: bool) {
        if self.busy {
            self.pending = Some((width, height, frame));
        } else {
            self.apply(width, height, frame);
        }
    }

    fn tick_if_due(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32();
        if dt * 1000.0 < TICK_INTERVAL_MS as f32 - TICK_SLACK_MS {
            return;
        }
        self.last = now;
        self.guarded(|v| {
            if let Some(poll) = v.poll.as_mut() {
                poll(v.editor.as_mut());
            }
            v.editor.tick(dt.min(0.1));
        });
        // An editor may change its own size (a poll hook resized it).
        self.follow(true);
        self.select_keys();
        self.touched = true;
    }

    /// Select key events while the editor wants keys, and only then, so the
    /// host keeps every key of an editor that wants none.
    fn select_keys(&mut self) {
        // The field and the menu take every key while they are open.
        let wants = self.editor.wants_keys() || self.field.is_some() || self.menu.is_some();
        if wants == self.keys_selected || !self.usable() {
            return;
        }
        self.keys_selected = wants;
        x!(self, self.conn.change_window_attributes(self.window, &ChangeWindowAttributesAux::new().event_mask(event_mask(wants))));
    }

    /// Handle the events that have arrived. True when there was one.
    fn drain(&mut self) -> bool {
        if self.dead {
            return false;
        }
        let mut events = Vec::new();
        loop {
            match self.conn.poll_for_event() {
                Ok(Some(event)) => events.push(event),
                Ok(None) => break,
                Err(_) => {
                    self.dead = true;
                    break;
                }
            }
        }
        for (i, event) in events.iter().enumerate() {
            self.handle(event, events.get(i + 1));
        }
        !events.is_empty()
    }

    fn handle(&mut self, event: &Event, next: Option<&Event>) {
        // An event from after the view's own resize: later sizes are news again.
        if let (Some((sequence, at)), Some(seen)) = (self.configured, event.wire_sequence_number()) {
            if (seen.wrapping_sub(sequence) as i16) >= 0 || at.elapsed() > Duration::from_secs(2) {
                self.configured = None;
            }
        }
        if self.service_event(event) {
            return;
        }
        match event {
            Event::Expose(e) if e.window == self.window => self.full = true,
            Event::ConfigureNotify(e) if e.window == self.window => {
                let win = (u32::from(e.width), u32::from(e.height));
                if self.configured.is_some() || win == self.win || win.0 == 0 || win.1 == 0 {
                    return;
                }
                // The host (or the window manager) sized the window: the editor follows.
                self.win = win;
                self.full = true;
                self.field_cancel = self.field.is_some();
                let points = |v: u32| ((v as f32 / self.scale).round() as u32).max(1);
                let (w, h) = (points(win.0), points(win.1));
                if (w, h) != self.size {
                    self.resize_to(w, h, false);
                }
            }
            Event::DestroyNotify(e) if e.window == self.window => self.alive = false,
            Event::ButtonPress(e) if e.event == self.window => {
                self.last_time = e.time;
                let (x, y) = self.point(e.event_x, e.event_y);
                let state = u16::from(e.state);
                match e.detail {
                    1 => {
                        let slop = (4.0 * self.scale) as i32;
                        let count = match self.click {
                            Some(c)
                                if c.window == self.window
                                    && e.time.wrapping_sub(c.time) <= DOUBLE_CLICK_MS
                                    && (i32::from(e.event_x) - i32::from(c.x)).abs() <= slop
                                    && (i32::from(e.event_y) - i32::from(c.y)).abs() <= slop =>
                            {
                                c.count + 1
                            }
                            _ => 1,
                        };
                        self.click = Some(Click { time: e.time, x: e.event_x, y: e.event_y, count, window: self.window });
                        // A click gives the editor the keyboard, when it wants keys.
                        if self.editor.wants_keys() && !self.focused && self.usable() {
                            x!(self, self.conn.set_input_focus(InputFocus::PARENT, self.window, e.time));
                            self.settle();
                        }
                        self.input(state, |v| v.editor.mouse_down(x, y, count));
                    }
                    3 => {
                        self.input(state, |v| v.editor.right_mouse_down(x, y));
                    }
                    4 => self.input(state, |v| v.editor.wheel(x, y, 1.0)),
                    5 => self.input(state, |v| v.editor.wheel(x, y, -1.0)),
                    _ => {}
                }
            }
            Event::ButtonRelease(e) if e.detail == 1 && e.event == self.window => {
                let (x, y) = self.point(e.event_x, e.event_y);
                self.input(u16::from(e.state), |v| v.editor.mouse_up(x, y));
            }
            Event::MotionNotify(e) if e.event == self.window => {
                // Only the last of a run of moves matters.
                if matches!(next, Some(Event::MotionNotify(_))) {
                    return;
                }
                let (x, y) = self.point(e.event_x, e.event_y);
                self.input(u16::from(e.state), |v| v.editor.mouse_move(x, y));
            }
            Event::EnterNotify(e) if e.event == self.window => self.input(u16::from(e.state), |_| ()),
            Event::KeyPress(e) => self.key(e, true),
            Event::KeyRelease(e) => {
                // Auto-repeat is a release and a press at the same instant: the
                // editor hears the press again and no release.
                if matches!(next, Some(Event::KeyPress(p)) if p.detail == e.detail && p.time == e.time) {
                    return;
                }
                self.key(e, false);
            }
            Event::FocusIn(e) => self.focus(e.mode, e.detail, true),
            Event::FocusOut(e) => self.focus(e.mode, e.detail, false),
            Event::MappingNotify(_) => {
                if let Some(keymap) = read_keymap(&self.conn) {
                    self.keymap = keymap;
                }
            }
            Event::ClientMessage(e) if self.wm_delete != 0 && e.type_ == self.wm_protocols && e.format == 32 && e.data.as_data32()[0] == self.wm_delete => {
                self.close_requested = true;
            }
            // X errors (a parent that is gone, a window the host destroyed) are not fatal.
            _ => {}
        }
    }

    /// The event position in editor points (a host may show the window at another size).
    fn point(&self, x: i16, y: i16) -> (f32, f32) {
        let ratio = |points: u32, pixels: u32| if pixels > 0 && points > 0 { points as f32 / pixels as f32 } else { 1.0 / self.scale };
        (f32::from(x) * ratio(self.size.0, self.win.0), f32::from(y) * ratio(self.size.1, self.win.1))
    }

    fn modifiers_from_state(&self, state: u16) -> Modifiers {
        Modifiers {
            shift: state & SHIFT != 0,
            ctrl: state & CONTROL != 0,
            alt: state & self.keymap.alt != 0,
            command: state & self.keymap.command != 0,
        }
    }

    /// An input call: the modifiers first when they changed, then the call,
    /// then a size the editor changed in it.
    fn input<R>(&mut self, state: u16, call: impl FnOnce(&mut Self) -> R) -> R {
        let now = self.modifiers_from_state(state);
        let result = self.guarded(|v| {
            if now != v.modifiers {
                v.modifiers = now;
                v.editor.modifiers_changed(now);
            }
            call(v)
        });
        self.follow(true);
        self.select_keys();
        self.touched = true;
        result
    }

    fn key(&mut self, e: &xproto::KeyPressEvent, down: bool) {
        self.last_time = e.time;
        let mut state = u16::from(e.state);
        let sym = self.keymap.lookup(e.detail, state);
        // The state of an event is the one before it: a modifier key's own
        // press or release is not in it yet.
        if let Some(bit) = self.keymap.modifier_bit(sym) {
            state = if down { state | bit } else { state & !bit };
        }
        let key = key_from_keysym(sym);
        let handled = self.input(state, |v| {
            let mods = v.modifiers;
            match key {
                Some(key) if v.editor.wants_keys() => {
                    if down {
                        v.editor.key_down(key, mods)
                    } else {
                        v.editor.key_up(key, mods)
                    }
                }
                _ => false,
            }
        });
        // A key the editor did not use goes on to the host's window.
        if !handled && self.parent != 0 && self.usable() {
            let mask = if down { EventMask::KEY_PRESS } else { EventMask::KEY_RELEASE };
            let forward = xproto::KeyPressEvent { event: self.parent, child: self.window, ..*e };
            x!(self, self.conn.send_event(true, self.parent, mask, forward));
            // The host has the key before it looks for it.
            self.settle();
        }
    }

    fn focus(&mut self, mode: NotifyMode, detail: NotifyDetail, focused: bool) {
        // A grab (a menu, the window manager) is not a focus change, and the
        // pointer merely being inside a focused ancestor is not focus.
        if mode == NotifyMode::GRAB || mode == NotifyMode::UNGRAB || detail == NotifyDetail::POINTER || focused == self.focused {
            return;
        }
        self.focused = focused;
        self.guarded(|v| v.editor.focus_changed(focused));
        self.touched = true;
        if !focused {
            // Focus left the view: the field commits what is valid, the menu goes.
            self.field_lost_focus();
            self.close_menu();
            self.hide_tip();
        }
    }

    /// Ask the editor what changed and queue that: nothing, some rectangles, or the whole window.
    fn invalidate(&mut self) {
        if !self.shown() || self.full {
            return;
        }
        let Some(rects) = self.guarded(|v| v.editor.damage()) else {
            self.full = true;
            return;
        };
        for r in &rects {
            match device_rect(r, self.size, self.win) {
                Some((_, _, 0, _)) | Some((_, _, _, 0)) => {}
                Some(rect) => self.rects.push(rect),
                None => {
                    self.full = true;
                    return;
                }
            }
        }
    }

    /// Render and send what is queued.
    fn present(&mut self) {
        if !self.shown() {
            self.rects.clear();
            return;
        }
        if !self.full && self.rects.is_empty() {
            return;
        }
        let mut rects = std::mem::take(&mut self.rects);
        if self.full {
            rects.clear();
            rects.push((0, 0, self.win.0, self.win.1));
        }
        self.full = false;
        let sent = self.guarded(|v| {
            let frame = v.editor.render(v.scale);
            if frame.width == 0 || frame.height == 0 || frame.pixels.len() < frame.width * frame.height {
                return true;
            }
            let (ww, wh) = (v.win.0 as usize, v.win.1 as usize);
            let stretched = (frame.width, frame.height) != (ww, wh);
            for &(x, y, w, h) in &rects {
                let (x, y, w, h) = (x as usize, y as usize, w as usize, h as usize);
                if w == 0 || h == 0 || x + w > ww || y + h > wh {
                    continue;
                }
                // Source columns of a stretched frame, nearest pixel.
                let columns: Vec<usize> = if stretched { (x..x + w).map(|c| (c * frame.width / ww).min(frame.width - 1)).collect() } else { Vec::new() };
                let stride = v.format.stride(w);
                let rows_per = (v.max_request.saturating_sub(64) / stride.max(1)).max(1);
                let mut row = 0;
                while row < h {
                    let rows = rows_per.min(h - row);
                    v.buf.clear();
                    v.buf.resize(stride * rows, 0);
                    for (n, line) in v.buf.chunks_exact_mut(stride).enumerate() {
                        let dy = y + row + n;
                        let sy = if stretched { (dy * frame.height / wh).min(frame.height - 1) } else { dy };
                        let source = &frame.pixels[sy * frame.width..(sy + 1) * frame.width];
                        if stretched {
                            v.format.write_row(columns.iter().map(|&c| source[c]), line);
                        } else {
                            v.format.write_row(source[x..x + w].iter().copied(), line);
                        }
                    }
                    let put = v.conn.put_image(ImageFormat::Z_PIXMAP, v.window, v.gc, w as u16, rows as u16, x as i16, (y + row) as i16, 0, v.format.depth, &v.buf);
                    if put.is_err() {
                        return false;
                    }
                    row += rows;
                }
            }
            true
        });
        if !sent {
            self.dead = true;
        }
    }

    /// Requests the editor made during its last call: the parameter menu, the
    /// type-in field. Never called inside an editor call.
    fn poll_requests(&mut self) {
        if std::mem::take(&mut self.field_cancel) {
            self.close_field(true);
        }
        let (edit, menu) = self.guarded(|v| (v.editor.take_text_edit(), v.editor.take_param_menu()));
        if let Some(menu) = menu {
            self.show_param_menu(menu);
        }
        if let Some(edit) = edit {
            self.open_field(edit);
        }
    }

    /// What waits on time, not on events: an unanswered clipboard request, a
    /// pointer at rest, a menu whose view is no longer on screen.
    fn service_timers(&mut self) {
        self.clipboard_timeout();
        if self.menu.is_some() && !self.shown() {
            self.close_menu();
        }
        self.tip_check();
    }

    /// The view goes off screen (hidden, unmapped, moved, destroyed): the
    /// field commits what is valid, the menu and the tooltip go.
    fn services_hide(&mut self) {
        self.field_lost_focus();
        self.close_menu();
        self.hide_tip();
    }

    /// Events of the field, the menu, the tooltip and the clipboard. True
    /// when the event was theirs alone.
    fn service_event(&mut self, event: &Event) -> bool {
        let is = |window: Option<Window>, w: Window| window == Some(w);
        let (field, menu, tip) = (self.field.as_ref().map(|f| f.window), self.menu.as_ref().map(|m| m.window), self.tips.window());
        match event {
            Event::Expose(e) if e.window != self.window => {
                if e.count == 0 {
                    if is(field, e.window) {
                        self.paint_field();
                    } else if is(menu, e.window) {
                        self.paint_menu();
                    } else if is(tip, e.window) {
                        self.paint_tip();
                    }
                }
                true
            }
            Event::ButtonPress(e) => {
                self.hide_tip();
                if is(field, e.event) {
                    self.field_click(e);
                    true
                } else if is(menu, e.event) {
                    self.menu_button(e, true);
                    true
                } else if e.event == self.window {
                    // A press that reached the view: the menu held no grab.
                    self.close_menu();
                    // A click on the view abandons the field, as the right button does.
                    if matches!(e.detail, 1 | 3) {
                        self.close_field(true);
                    }
                    false
                } else {
                    true
                }
            }
            Event::ButtonRelease(e) if e.event != self.window => {
                if is(menu, e.event) {
                    self.menu_button(e, false);
                }
                true
            }
            Event::MotionNotify(e) => {
                if is(menu, e.event) {
                    self.menu_motion(e);
                    true
                } else if e.event == self.window {
                    self.tip_motion(e);
                    false
                } else {
                    true
                }
            }
            Event::LeaveNotify(e) => {
                if e.event == self.window && e.mode == NotifyMode::NORMAL {
                    self.hide_tip();
                    self.tips.forget();
                }
                true
            }
            Event::KeyPress(e) => {
                self.hide_tip();
                self.tips.forget();
                if self.menu.is_some() {
                    self.menu_key(e);
                    true
                } else if self.field.is_some() {
                    self.last_time = e.time;
                    self.field_key(e);
                    true
                } else {
                    false
                }
            }
            Event::KeyRelease(_) => self.menu.is_some() || self.field.is_some(),
            Event::UnmapNotify(e) if e.window == self.window => {
                self.services_hide();
                false
            }
            Event::DestroyNotify(e) if e.window != self.window => {
                // A pop-up taken away from outside: forget it, and its grabs.
                if is(field, e.window) {
                    self.field = None;
                    self.guarded(|v| v.editor.text_edit_cancel());
                    self.select_keys();
                    self.touched = true;
                } else if is(menu, e.window) {
                    self.close_menu();
                } else if is(tip, e.window) {
                    self.hide_tip();
                }
                true
            }
            Event::DestroyNotify(_) => {
                self.services_hide();
                false
            }
            Event::SelectionRequest(e) => {
                self.selection_request(e);
                true
            }
            Event::SelectionClear(e) => {
                self.selection_clear(e);
                true
            }
            Event::SelectionNotify(e) => {
                self.selection_notify(e);
                true
            }
            _ => false,
        }
    }

    /// Make the window a top-level one with a title, a close button and size hints.
    fn make_top_level(&mut self, title: &str) -> Option<()> {
        if !self.create(self.root) {
            return None;
        }
        let atom = |name: &[u8]| Some(self.conn.intern_atom(false, name).ok()?.reply().ok()?.atom);
        let protocols = atom(b"WM_PROTOCOLS")?;
        let delete = atom(b"WM_DELETE_WINDOW")?;
        let net_name = atom(b"_NET_WM_NAME")?;
        let utf8 = atom(b"UTF8_STRING")?;
        let window = self.window;
        // WM_NAME is Latin-1 by the ICCCM; the full title is in _NET_WM_NAME.
        let latin: Vec<u8> = title.chars().map(|c| if (c as u32) < 256 && !c.is_control() { c as u32 as u8 } else { b'?' }).collect();
        self.conn.change_property8(PropMode::REPLACE, window, AtomEnum::WM_NAME, AtomEnum::STRING, &latin).ok()?;
        self.conn.change_property8(PropMode::REPLACE, window, net_name, utf8, title.as_bytes()).ok()?;
        self.conn.change_property8(PropMode::REPLACE, window, AtomEnum::WM_CLASS, AtomEnum::STRING, b"openplugin\0OpenPlugin\0").ok()?;
        self.conn.change_property32(PropMode::REPLACE, window, protocols, AtomEnum::ATOM, &[delete]).ok()?;
        // WM_HINTS: the input flag, input true (the window manager gives the window focus).
        self.conn.change_property32(PropMode::REPLACE, window, AtomEnum::WM_HINTS, AtomEnum::WM_HINTS, &[1, 1, 0, 0, 0, 0, 0, 0, 0]).ok()?;
        self.conn.change_property32(PropMode::REPLACE, window, AtomEnum::WM_NORMAL_HINTS, AtomEnum::WM_SIZE_HINTS, &self.size_hints()).ok()?;
        self.wm_protocols = protocols;
        self.wm_delete = delete;
        self.top_level = true;
        self.full = true;
        if !self.hidden {
            self.conn.map_window(window).ok()?;
        }
        self.conn.flush().ok()
    }

    /// `WM_NORMAL_HINTS` from the editor's [`SizeRange`], in device pixels.
    fn size_hints(&self) -> [u32; 18] {
        let range = self.editor.size_range();
        let px = |v: u32| device(v, self.scale);
        let (min, max) = if range.is_fixed() { (self.size, self.size) } else { (range.min, range.max) };
        // PSize | PMinSize | PMaxSize, and PAspect when the editor keeps one.
        let mut hints = [0u32; 18];
        hints[0] = 8 | 16 | 32;
        hints[3] = self.win.0;
        hints[4] = self.win.1;
        hints[5] = px(min.0);
        hints[6] = px(min.1);
        hints[7] = px(max.0);
        hints[8] = px(max.1);
        if let Some((w, h)) = range.aspect_ratio().filter(|_| !range.is_fixed()) {
            hints[0] |= 128;
            hints[11] = w;
            hints[12] = h;
            hints[13] = w;
            hints[14] = h;
        }
        hints
    }
}

impl Drop for EditorView {
    fn drop(&mut self) {
        if self.dead {
            return;
        }
        // The grabs go first, then the pop-up windows (the field goes with the window).
        self.close_menu();
        self.hide_tip();
        // Errors (the window went with its parent) come back unread and are dropped.
        if self.created {
            let _ = self.conn.free_gc(self.gc);
            if self.alive {
                let _ = self.conn.destroy_window(self.window);
            }
        }
        if let Some(colormap) = self.colormap {
            let _ = self.conn.free_colormap(colormap);
        }
        let _ = self.conn.flush();
    }
}

fn event_mask(keys: bool) -> EventMask {
    let mask = EventMask::EXPOSURE
        | EventMask::STRUCTURE_NOTIFY
        | EventMask::BUTTON_PRESS
        | EventMask::BUTTON_RELEASE
        | EventMask::POINTER_MOTION
        | EventMask::BUTTON_MOTION
        | EventMask::ENTER_WINDOW
        | EventMask::LEAVE_WINDOW
        | EventMask::FOCUS_CHANGE;
    if keys {
        mask | EventMask::KEY_PRESS | EventMask::KEY_RELEASE
    } else {
        mask
    }
}

/// The server's keyboard mapping, and the modifier bits of Alt, Super and Num Lock.
fn read_keymap(conn: &RustConnection) -> Option<Keymap> {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    if max < min {
        return None;
    }
    let mapping = conn.get_keyboard_mapping(min, max - min + 1).ok()?.reply().ok()?;
    let modifiers = conn.get_modifier_mapping().ok()?.reply().ok()?;
    let mut keymap = Keymap { min, per: usize::from(mapping.keysyms_per_keycode), syms: mapping.keysyms, alt: 0, command: 0, num_lock: 0 };
    let per_modifier = usize::from(modifiers.keycodes_per_modifier());
    if per_modifier > 0 {
        // Mod1 to Mod5 are rows 3 to 7; what they mean is in the keys they hold.
        for (row, codes) in modifiers.keycodes.chunks(per_modifier).enumerate().skip(3) {
            for &code in codes.iter().filter(|&&c| c != 0) {
                match keymap.sym(code, 0) {
                    0xffe7..=0xffea => keymap.alt |= 1 << row,
                    0xffeb | 0xffec => keymap.command |= 1 << row,
                    0xff7f => keymap.num_lock |= 1 << row,
                    _ => {}
                }
            }
        }
    }
    // The usual layout when the server names none: Mod1 is Alt, Mod4 is Super.
    if keymap.alt == 0 {
        keymap.alt = 8;
    }
    if keymap.command == 0 {
        keymap.command = 64 & !keymap.alt;
    }
    Some(keymap)
}

/// The window rectangle in device pixels (a pixel more on every side for
/// anti-aliasing and rounding) that shows an editor rectangle, or `None` when
/// it is not finite.
fn device_rect(r: &DamageRect, editor: (u32, u32), win: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    if !r.is_finite() || editor.0 == 0 || editor.1 == 0 {
        return None;
    }
    let (ww, wh) = (f64::from(win.0), f64::from(win.1));
    let (sx, sy) = (ww / f64::from(editor.0), wh / f64::from(editor.1));
    let x0 = ((f64::from(r.x) * sx).floor() - 1.0).clamp(0.0, ww);
    let y0 = ((f64::from(r.y) * sy).floor() - 1.0).clamp(0.0, wh);
    let x1 = (((f64::from(r.x) + f64::from(r.w)) * sx).ceil() + 1.0).clamp(0.0, ww);
    let y1 = (((f64::from(r.y) + f64::from(r.h)) * sy).ceil() + 1.0).clamp(0.0, wh);
    Some((x0 as u32, y0 as u32, (x1 - x0).max(0.0) as u32, (y1 - y0).max(0.0) as u32))
}

/// Show the editor in its own window and run until the window closes.
/// Returns the exit code. Main thread.
pub fn run_window(title: &str, editor: Box<dyn Editor>, poll: Option<Poll>) -> i32 {
    let Some(mut view) = EditorView::new(editor, poll) else {
        eprintln!("openplugin: no X11 display to open a window on");
        return 1;
    };
    if view.make_top_level(title).is_none() {
        eprintln!("openplugin: the X server refused the window");
        return 1;
    }
    loop {
        view.pump();
        if view.close_requested {
            return 0;
        }
        if !view.usable() {
            return 1;
        }
        // Sleep until the next tick or the next event, whichever is first.
        let since = view.last.elapsed().as_millis().min(u128::from(TICK_INTERVAL_MS)) as i64;
        let wait = rustix::event::Timespec { tv_sec: 0, tv_nsec: (i64::from(TICK_INTERVAL_MS) - since).max(1) * 1_000_000 };
        let mut fds = [rustix::event::PollFd::new(view.conn.stream(), rustix::event::PollFlags::IN)];
        // An interrupted wait is a short wait.
        let _ = rustix::event::poll(&mut fds, Some(&wait));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysyms_name_the_contract_keys() {
        assert_eq!(key_from_keysym(0xff51), Some(Key::Left));
        assert_eq!(key_from_keysym(0xff0d), Some(Key::Return));
        assert_eq!(key_from_keysym(0xff8d), Some(Key::Return));
        assert_eq!(key_from_keysym(0x20), Some(Key::Space));
        assert_eq!(key_from_keysym(0x41), Some(Key::Char('a')));
        assert_eq!(key_from_keysym(0xffb7), Some(Key::Char('7')));
        assert_eq!(key_from_keysym(0xe9), Some(Key::Char('\u{e9}')));
        assert_eq!(key_from_keysym(0x0100_0416), Some(Key::Char('\u{436}')));
        assert_eq!(key_from_keysym(0xffe1), None);
        assert_eq!(key_from_keysym(0xffbe), None);
        assert_eq!(key_from_keysym(0), None);
    }

    #[test]
    fn scale_comes_from_xft_dpi() {
        assert_eq!(scale_from_resources(b"Xcursor.size:\t24\nXft.dpi:\t192\n"), Some(2.0));
        assert_eq!(scale_from_resources(b"Xft.dpi: 96"), Some(1.0));
        assert_eq!(scale_from_resources(b"Xft.dpi: 48"), Some(1.0));
        assert_eq!(scale_from_resources(b"Xft.dpi: 960"), Some(3.0));
        assert_eq!(scale_from_resources(b"Xft.dpi: lots"), None);
        assert_eq!(scale_from_resources(b"Xft.antialias: 1"), None);
    }

    #[test]
    fn pixels_pack_through_the_visual() {
        let direct = PixelFormat { depth: 24, bpp: 32, pad: 32, msb: false, red: Channel::of(0xff_0000), green: Channel::of(0xff00), blue: Channel::of(0xff), fill: 0, direct: true };
        let mut line = [0u8; 8];
        direct.write_row([0xff11_2233, 0x0044_5566].into_iter(), &mut line);
        assert_eq!(line, [0x33, 0x22, 0x11, 0, 0x66, 0x55, 0x44, 0]);
        let argb = PixelFormat { depth: 32, msb: true, fill: 0xff00_0000, ..direct };
        argb.write_row([0x0011_2233].into_iter(), &mut line[..4]);
        assert_eq!(line[..4], [0xff, 0x11, 0x22, 0x33]);
        // BGR masks, 24 bits per pixel.
        let bgr = PixelFormat { bpp: 24, red: Channel::of(0xff), green: Channel::of(0xff00), blue: Channel::of(0xff_0000), direct: false, ..direct };
        bgr.write_row([0x0011_2233].into_iter(), &mut line[..3]);
        assert_eq!(line[..3], [0x11, 0x22, 0x33]);
        assert_eq!(bgr.stride(3), 12);
        // 5-6-5.
        let rgb565 = PixelFormat { depth: 16, bpp: 16, pad: 32, red: Channel::of(0xf800), green: Channel::of(0x07e0), blue: Channel::of(0x001f), direct: false, ..direct };
        rgb565.write_row([0x00ff_ffff, 0x00ff_0000].into_iter(), &mut line[..4]);
        assert_eq!(line[..4], [0xff, 0xff, 0x00, 0xf8]);
        assert_eq!(rgb565.stride(3), 8);
    }

    #[test]
    fn damage_maps_to_device_pixels() {
        let r = DamageRect::new(10.0, 10.0, 20.0, 5.0);
        assert_eq!(device_rect(&r, (100, 50), (200, 100)), Some((19, 19, 42, 12)));
        assert_eq!(device_rect(&DamageRect::new(-5.0, 0.0, 500.0, 500.0), (100, 50), (100, 50)), Some((0, 0, 100, 50)));
        assert_eq!(device_rect(&DamageRect::new(f32::NAN, 0.0, 1.0, 1.0), (100, 50), (100, 50)), None);
    }
}
