# goodlookindri-shell

Window, input, outline text and a software rasteriser for GoodLookin DRI scenes. A tool that is
not a plugin (a manager, an installer, a standalone utility) uses it to show a `Scene` on screen
and to hold a few controls.

```rust
use goodlookindri::scene::Scene;
use goodlookindri_shell::{run, App, Config, Event};

struct MyApp;
impl App for MyApp {
    fn event(&mut self, e: Event) -> bool { false }              // true = repaint
    fn paint(&mut self, scene: &mut Scene, w: f32, h: f32) { /* draw with goodlookindri */ }
}
std::process::exit(run(Config { title: "Tool".into(), width: 560, height: 300, smoke_ms: None }, Box::new(MyApp)));
```

`render_offscreen` draws one frame without a window, for tests and machines with no display.

## Systems

| Lane | Systems | How |
| --- | --- | --- |
| Windows | 7 SP1 to 11, x64 and x86 | `*-win7-windows-msvc` targets, GDI. Arm64 from Windows 10 on Arm. |
| macOS modern | 10.12 to current, Intel and Apple silicon | stable Rust, Cocoa, one universal binary |
| macOS legacy | 10.6.8 to 10.11, Intel | pinned nightly, custom target, see `legacy/README.md` |

No GPU API is used. The window blits a software-rendered buffer (GDI `StretchDIBits`,
CoreGraphics `CGImage`). Nothing uses thread-local storage, so the legacy lane can load.

## Drawing and text

- Replay of every `Cmd`: gradients, ellipses, round rects, arcs, polygons, strokes, clips.
  One pixel of anti-aliasing from signed distance fields.
- Text is outline text in Source Sans Pro Regular (SIL OFL 1.1, `fonts/OFL.txt`), rasterised by
  `fontdue` with a glyph cache. `text::width`, `text::wrap` and `text::ellipsize` measure and
  fit labels in the same units as the scene.

## Input

`Event` carries mouse move, down and up, wheel (`dy` positive when content moves up), keys
(`Key::{Tab, Enter, Space, Up, Down, Escape}` with a shift flag) and a 16 ms tick.

- Windows: `WM_MOUSEWHEEL`, `WM_KEYDOWN`, and `SetFocus` at start-up.
- macOS: `scrollWheel:` (deltaY is negated), `keyDown:` (key codes, shift from `modifierFlags`).

## Widgets (`shell::ui`)

A `Ui` owns a list of `Widget { id, bounds, kind }` and routes events to them. It keeps focus,
hover and press state, and returns a `Response` with `redraw` and an optional `Action`.

| Kind | Paints with | Input |
| --- | --- | --- |
| `Button` | `switches::draw_push` | press and release on it, or Enter/Space when focused. Focusable. |
| `Lamp` | `meters::draw_lamp` | none |
| `Bar` | a plate and a level fill | none |
| `List` | a plate, selection, clipped rows, scroll thumb | click selects; wheel scrolls; Up/Down move selection; focusable |
| `Text` | wrapped lines, clipped at the bottom | none |
| `Readout` | `meters::draw_seven_segment` | none |

Tab and Shift-Tab cycle focus through buttons and lists. Escape clears focus. The app changes
content through `Ui::kind_mut(id)`. See `hello/src/bin/widgets.rs` for a complete example.

## Checks

- `cargo test --workspace`: rasteriser, text measuring and wrapping, event routing (`shell/tests/ui.rs`),
  and a golden frame (`shell/tests/golden.rs`) that is compared with `shell/tests/golden/widgets_240x120.ppm`
  within a channel tolerance of 3. Regenerate with `GLDRI_UPDATE_GOLDEN=1` after a deliberate change.
- `gldri-hello --smoke` and `gldri-widgets --smoke` open a window, draw, and exit 0 only if the frame
  has colour in it. CI runs these on each lane.
- The legacy 10.6 build of both samples has no thread-local or newer libSystem imports, and renders
  the same bytes as the modern build.

## Known limits

- The rasteriser is simple and not fast on large, busy scenes. Dirty regions and cached plates are
  in the 0.4 milestone. The `Text` widget re-wraps its text every frame.
- Keys are limited to the six in `Key`. Letters, digits and text entry are not done.
- DPI is system-wide on Windows. Per-monitor DPI on Windows 8.1 and later is not done.
- The golden frame is not compared with the GoodLookinUI gallery images in `docs/images`. Those are
  full app screenshots with page tabs and cell cards that `gallery::paint_page` does not draw, so a
  pixel diff would test the layout, not the rasteriser. The comparison needs that chrome first.
- Nothing has run on a real Windows 7 machine, a real macOS 10.6 to 10.11 machine, or a Windows
  machine at all. The Windows input path is compile-checked only. The legacy build has run under
  Rosetta on a current Mac only.
