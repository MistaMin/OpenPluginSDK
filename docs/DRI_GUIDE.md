# GoodLookin DRI (Design Rust Interface) Guide

## Summary

GoodLookin DRI draws through the `goodlookindri` crate. That crate is the JUCE-free port of the 47 knob styles, 40 faceplates, meters, faders, switches, and spectrum. `openplugin-dri` does not keep a second copy of those drawers.

`draw_knob` and `draw_faceplate` take any GoodLookin style code, for example `"Chk"` or `"Walnut"`. The codes are the one style system: there are no style enums.

This release follows GoodLookin DRI 0.2.

## Knob tiers

GoodLookin DRI 0.2 draws a knob in one of three tiers. `DrawList::set_knob_tier` picks one for every knob an editor draws:

| Tier | What it is | Default |
|---|---|---|
| `KnobTier::Lit` | Each of the 35 physical knob styles is shaded per pixel from a height profile, materials and a studio light. The result is cached as a sprite and drawn as one image. The 12 display and neon styles stay vector. | yes |
| `KnobTier::Classic` | The vector drawing of GoodLookin DRI 0.1 | |
| `KnobTier::Flat` | The 2D vector family | |

A style prefix picks the tier for one knob: `lit_Brit`, `flat_Brit`. The sweep is GoodLookin's ±0.8π (`knob_angle`), the same as design files use. A faceplate records its colour on the `DrawList`, and the lit knobs drawn after it reflect that colour.

## From scene to pixels

A GoodLookin DRI 0.2 scene has commands the 0.1 set did not: multi-stop linear, radial and conic fills, feathered and additive fills, soft shadows, inner shadows, and images. `SceneCanvas` rasterises all of them with `goodlookindri-shell`'s software renderer into the `0x00RRGGBB` pixels an `Editor` returns. It also tells GoodLookin DRI the backing scale, so lit sprites and panel grain are baked at device resolution.

```rust
struct MyEditor { canvas: SceneCanvas, gain: f32 }

impl Editor for MyEditor {
    fn render(&mut self, scale: f32) -> Frame<'_> {
        let gain = self.gain;
        self.canvas.frame_layered(
            scale,
            0,                                                      // key for the static layer
            |dl| draw_faceplate(dl, 240.0, 160.0, "Navy"),     // painted once per scale
            |dl| draw_knob(dl, 55.0, 68.0, 30.0, gain, "GAIN", "0 dB", "Brit", Color::RED),
        )
    }
    // ...
}
```

Use `frame_layered` for anything with a plate. A 0.2 plate is shaded per pixel and is the expensive part of a frame. `frame_layered` keeps it as pixels and repaints only what moves on top. The result is identical to painting everything each frame.

| `examples/editor`, 240 × 160 points at 2×, Apple Silicon, release | per frame |
|---|---|
| plate and parts every frame (`frame`) | 18.3 ms |
| plate cached (`frame_layered`) | 3.0 ms |
| a lit knob alone (sprite cached) | 0.23 ms |
| a plate alone at 1080 × 700 points | 310 ms |

The view ticks 30 times a second on the main thread, so an editor of any size needs the layer. Change the key when the static look changes, or call `invalidate_layer`.

### Repainting only what changed

`frame_damaged(scale, key, layer, paint, damage)` is `frame_layered` that also takes the rectangles that changed (`Option<&[DamageRect]>`, points). `None` paints everything, `Some(&[])` returns the last frame and runs neither closure, and `Some(rects)` restores the cached layer under each rectangle (rounded outwards to whole device pixels) and replays the `paint` scene clipped to it, leaving every other pixel alone. `paint` still runs once, so an editor can skip building a part that cannot reach a damaged rectangle. The pixels equal a full frame provided the rectangles cover everything that differs from the last frame; the canvas falls back to a full frame when it cannot know that (a new size, scale or key, an invalidated layer, a `frame` in between, more than six separate rectangles, or more than 60% of the frame).

```rust
fn render(&mut self, scale: f32) -> Frame<'_> {
    self.collect_damage();                         // controls.take_changed(), the meter, ...
    let damage = self.damage.pending();            // DamageTracker
    let frame = self.canvas.frame_damaged(scale, 0, plate, paint, damage);
    self.damage.clear();
    frame
}
fn damage(&mut self) -> Option<Vec<DamageRect>> {
    self.collect_damage();
    self.damage.pending().map(<[_]>::to_vec)
}
```

The clip is a rectangle a hundredth of a pixel wider than the damage on each side, which keeps edge pixels at full coverage whatever the rounding of `pixels / scale * scale`; the ring of pixels around a rectangle is put back as it was, so nothing outside is touched. `ControlSet::take_changed()` lists the controls whose value, hover, pressed or focus state changed since the last call (the first call lists all), and an editor turns each into the rectangle its drawing can change: the control's area plus whatever the style paints past it (a lit knob's legend and value text, a focus ring, a meter's bloom). Too small a rectangle shows as stale pixels, so the tests compare a damage-only session with full repaints, frame by frame, and fail when a margin is too tight.

Measured with `examples/editor/src/bin/frames.rs` (the view's loop without the CoreGraphics blit: tick, `damage`, `render` unless empty), 240 × 160 points at 2×, Apple M1 Max, macOS 15.7, rustc 1.98, `--release` (`opt-level = 3`, thin LTO, one codegen unit), mean of 600 frames, three runs each:

| situation | before (every frame full) | after |
|---|---|---|
| idle | 1.55 to 1.65 ms | 0 ms, nothing drawn |
| knob dragged (a new lit sprite per frame) | 6.8 to 7.2 ms | 6.5 to 6.6 ms |
| knob moved between eight cached values | 1.6 ms | 1.13 ms |
| meter level changing every frame | 2.0 ms | 0.30 ms (draws in 144 of 600 frames) |

A drag does not get faster because the lit knob is shaded per pixel for each new value (milliseconds, then cached by GoodLookin DRI); repainting less does not change that. At 1× the same runs give idle 0.49 ms to 0, a dragged knob 1.76 to 1.58 ms, cached knob values 0.48 to 0.34 ms and the meter 0.60 to 0.09 ms. The Cocoa side (a smaller CoreGraphics blit and compositing) is not in these numbers.

## Fonts

Text draws in Source Sans Pro Semibold unless a scene picks another face. `scene.font(id)` sets the face for the `text` calls after it in the same scene; each scene starts with the default. A `FontId` comes from one of two places, both in `openplugin_dri::text`:

- **A font that ships with the plugin.** `text::register(text::bundled::INTER_REGULAR)` adds one of the open-licence faces that come with GoodLookin DRI: Inter, Source Sans 3, JetBrains Mono (monospaced readouts), Barlow Condensed (narrow panel lettering) and DSEG7 / DSEG14 (segment displays). `register` takes the bytes of any TrueType or OpenType font, so a plugin can also embed its own with `include_bytes!`. A plugin carries only the faces it names; ship the licence of each (`crates/goodlookindri/shell/fonts/<font>/OFL.txt`) with the plugin.
- **A font installed on the user's computer.** `text::system("Helvetica Neue")` finds an installed font by full name, PostScript name, or family name for its regular style, in the system's font folders (macOS, Windows and Linux), and reads it there. Nothing is copied into the plugin, as with JUCE's system fonts. It returns `None` when the font is not installed, so fall back to a bundled face. The first lookup of a name scans the folders; call it when the editor opens, not from the audio thread.

```rust
use openplugin_dri::text;

let label = text::system("Helvetica Neue")
    .or_else(|| text::register(text::bundled::INTER_REGULAR))
    .unwrap_or_default();
scene.font(label);
scene.text(at, "GAIN", ink, 11.0, true);
```

`text::width_in`, `line_height_in`, `wrap_in` and `ellipsize_in` measure in a chosen face. Registered faces stay loaded for the life of the process, and registering the same bundled bytes again returns the same id.

## Resizable editors

`SceneCanvas::resize(width, height)` sets a new size in points. The next frame has the new pixel size, and the cached layer is dropped, so `frame_layered` paints the plate again at that size and then reuses it. The same size changes nothing, and going back to an earlier size repaints it once. The layer's cache key already includes the pixel size, so a canvas that is resized without `resize` repaints too; `resize` is the call that changes the size the editor draws for.

Draw from the editor's current size, not from constants: pass the width and height to `draw_faceplate`, and scale the positions, radii and text sizes of the parts by one factor. `examples/editor` has a `layout(width, height)` function that returns the scale and the hit areas, and uses the same function to draw and to hit-test, so a click lands on what is drawn. Painting the plate costs the most (310 ms at 1080 × 700 points above), and it happens once per size, so a host that sends many sizes during a drag pays for each one. Keep the size range modest, or draw the plate more cheaply while the size is changing. See [EDITOR.md](EDITOR.md) for the contract.

`DrawList::commands()` stays for hosts that only draw basic shapes. It is GoodLookin DRI's `Scene::lower_basic`, with each lit knob drawn again in the classic tier, because a basic host has no images.

`cargo run -p example-editor --bin snapshot -- out.ppm 2 6 1` writes the example editor to an image. With `OP_SNAPSHOT_FRAMES=60` set, it also prints the time per frame.

## Aesthetic System

### 1. Procedural Knobs
47 procedural styles, lit by default (see Knob tiers):
- `"Brit"`: Classic vintage British console fluted skirt with distinct colored caps (Red, Blue, Green, Cream).
- `"N"`: Knurled industrial discrete channel strip dials.
- `"A"`: Precision studio pointer knobs with radial markers.
- `"Bk"`: Heavy vintage fluted bakelite with deep drop-shadows and engraved white indicators.
- `"Mtl"` / `"Chr"`: Machined concentric metallic specular rings.
- `"Wd"`: Hand-finished studio mahogany and walnut knobs.
- `"Neo"` / `"Seg"`: Cyberpunk neon rings and high-contrast digital displays.

### 2. Faceplates
40 plate finishes generated without bitmap images:
- `"Silver"`: Horizontal anodized grain lines.
- `"Navy"`: Classic olive/navy console finish with recessed beveled boundary.
- `"Stealth"`: Matte black 19-inch rack unit aesthetic.
- `"Cyber"`: High-contrast dark chassis with cyan/magenta neon edge glows.

### 3. Ballistic Meters
- `MeterStyle::AnalogVu`: 300 ms ANSI C16.5 ballistic integration with illuminated warm yellow incandescent dial face.
- `MeterStyle::Ppm`: Fast peak programme meter with quick rise and slow fall times.

### 4. Studio Faders & Sliders (from SoundCraft Console UI)
- `draw_fader`: Authentic console mixer fader with calibrated physical scale markings (+12, +6, 0, -5, -10, -20, -30, -40, -60, -inf dB), vertical slot track, and realistic 3D metallic fader cap with center indicator groove. Includes piecewise log-linear taper conversion (`fader_pos_to_db`, `fader_db_to_pos`).
- `draw_slider`: Horizontal parameter slider with active fill track and rounded thumb handle.
- `draw_pan_knob`: Console rotary pan knob with illuminated bipolar arc meter track from center (<L100 .. <C> .. R100>).

## Controls and hit testing

`openplugin_dri::hit` is a registry that replaces hand-coded hit rectangles and drag code. An editor owns a `ControlSet` built with its `EditorHost`, adds controls, forwards its input, and iterates the set to draw.

| Control | Constructor | Input |
|---|---|---|
| Rotary knob | `Control::knob(area, param, &INFO)` | vertical drag, 200 points for the full range (`.drag_range(points)` changes it) |
| Fader | `fader_v`, `fader_h` | drag, 1:1 with the length of its area |
| Toggle | `toggle` | click flips it (`begin`, value, `end`) |
| Momentary button | `button` | begin and the maximum on press; the minimum and end on release, wherever the pointer is |
| Key bank | `key_bank(area, param, &INFO, vertical)` | click or drag across equal keys; for a choice parameter |

An `Area` is a half-open rectangle or a circle (rim included), in points. A later control is on top where areas overlap. A read-only parameter never reacts.

```rust
let controls = ControlSet::new(host)
    .with(Control::knob(Area::circle(55.0, 68.0, 30.0), 0, &PARAMS[0]))
    .with(Control::toggle(Area::rect(118.0, 40.0, 34.0, 56.0), 1, &PARAMS[1]));

// Editor::mouse_down / mouse_move / mouse_up / wheel / modifiers_changed:
controls.mouse_down(x, y, clicks);   // and the others, same arguments
// Editor::param_changed:
controls.param_changed(index, plain);

// render:
for c in controls.iter() {           // back to front
    let (x, y, w, h) = c.area.bounds();
    match c.kind {
        ControlKind::Knob => draw_knob(dl, x + w * 0.5, y + h * 0.5, w * 0.5, c.normalized(), "GAIN", &c.info.format(c.value()), "Brit", Color::RED),
        ControlKind::Toggle => { /* c.is_on(), c.hover(), c.pressed() */ }
        _ => {}
    }
}
```

Behaviour, in gestures:

- Drag: `Begin`, one `Perform` per change of value, `End`. A click without movement sends `Begin`, `End` and no value. The drag is relative, so a grab never jumps, and the travel is clamped to the range so reversing at an end responds at once.
- Fine: while `Modifiers::fine()` (Shift or Command) is held, drags and wheel steps count a tenth. Changing the modifier mid-drag does not jump the value.
- Double-click on a knob or fader: one `Begin`, `Perform(default)`, `End`.
- Wheel: a continuous control moves 2% of its range per unit, as one `Begin`, `Perform`, `End`. A stepped, toggle or choice parameter moves one position per whole unit of accumulated scroll, so a trackpad does not run away. Nothing is sent at an end of the range.
- Every value sent is `ParamInfo::clamp`ed, so it is in range and snapped for `with_steps`, `Toggle` and `Choice`; a repeated value is not sent again. NaN and infinite coordinates, wheel deltas and host values are dropped.
- `param_changed` stores the host's value (clamped) and sends nothing. While the user holds a control on that parameter it is ignored, so a late echo cannot make the control jump; the host's value arrives again after the release.
- `hovered()`, `pressed()`, `Control::hover()` and `Control::pressed()` give the state to draw. Call `mouse_exit()` when the pointer leaves the view.

Why 200 points: it is about the height of a small plugin window and one hand movement on a trackpad, and it keeps one point under 0.5% of the range. A 150 point range (the old example) still works through `.drag_range(150.0)`.

The modifier keys reach the editor through `Editor::modifiers_changed`, which the macOS view calls from the flags of every mouse, wheel and key event and from `flagsChanged:`. Forward it with `ControlSet::set_modifiers`. A view that does not report modifiers leaves them unset (none held).

## Keyboard and focus in the registry

The set keeps at most one focused control, readable for drawing with `ControlSet::focused()` (the id) and `Control::focused()`. The editor forwards four more calls:

```rust
fn wants_keys(&self) -> bool { self.controls.wants_keys() }        // true if any control takes input
fn key_down(&mut self, key: Key, m: Modifiers) -> bool { self.controls.key_down(key, m) }
fn key_up(&mut self, key: Key, m: Modifiers) -> bool { self.controls.key_up(key, m) }
fn focus_changed(&mut self, focused: bool) { if !focused { self.controls.release_keys(); } }

// render: a ring around the focused control
if let Some(c) = controls.focused().and_then(|id| controls.get(id)) { /* c.area.bounds() */ }
```

Both key calls return whether the set used the key. Everything it does not use returns false, which the view hands to the host, so with nothing focused no key is taken.

| Key | Effect on the focused control |
|---|---|
| Tab, Shift-Tab | next or previous interactive control, in the order they were added (read-only ones are skipped). From nothing, the first or the last. Past either end focus is cleared and the key is not handled, so the host can move on |
| click | focuses the clicked control; a click on bare plate or a read-only control clears focus |
| Up or Right, Down or Left | raise or lower a continuous control by 1% of its range; a stepped, toggle or choice control by one position. A vertical key bank counts from the top, so there Up and Left go to the previous key |
| Page Up, Page Down | 10% of the range; a tenth of the positions (at least one) for a stepped control |
| Shift or Command with the above | a tenth of the step (Shift or Command, as for the mouse) |
| Home, End | the minimum, the maximum (a toggle: off, on) |
| Return, Space | flips a toggle; presses a button (begin and the maximum) until the key is up (the minimum and end). Auto-repeat of the held key does nothing. Not handled on a knob, fader or key bank |
| Delete, Backspace | the default value. Not handled on a button |
| Escape | clears focus. Not handled when nothing is focused |
| any other key | not handled |

Every step is a whole `Begin`, `Perform`, `End` through the `EditorHost`, as for the wheel, and the value is `ParamInfo::clamp`ed. At an end of the range the key is handled and nothing is sent; a value equal to the current one is never sent. Home, End, Return, Space, Delete, Backspace, Escape and Tab with Command, Control or Alt held are not handled, so the host's shortcuts win; the arrows and Page keys step with any modifier. Keys are ignored while the mouse holds a control. `key_up` returns true for a key whose down was handled and false otherwise, so the host sees the up of exactly the keys it saw go down. `release_keys()` lets go of a held button and forgets held keys; call it when the view loses keyboard focus (focus itself stays). `set_focus(Option<ControlId>)` moves focus in code, for an editor that rebuilds its set (as `examples/editor` does on resize). `DesignEditor` forwards the keys to its set and draws the same ring, around the focused control while the view holds the keyboard.

## Screen readers in the registry

`ControlSet::accessibility_nodes()` lists the controls for VoiceOver: the parameter's name, its value as `ParamInfo::format` text, a role (slider for knobs, faders and key banks; switch for toggles; button for momentary buttons), the area and the focus; a node's index is its `ControlId`. `accessibility_action(id, AccessAction)` answers Increment and Decrement with one whole gesture of `ACCESS_STEP` (5%) of the travel along the taper, or one position for a stepped parameter, a key bank or a toggle (on, off), and Press with a flip (toggle) or begin, maximum, minimum, end (button). `accessibility_focus(id)` follows the reader's cursor. An editor forwards `Editor::accessibility_nodes`, `accessibility_action` and `accessibility_focus` to the set, as `DesignEditor` and `examples/editor` do; the view does the rest ([EDITOR.md](EDITOR.md#accessibility)).

## Editors from a design file

`openplugin_dri::design_editor::DesignEditor` builds a whole `Editor` from a GoodLookin design CSV (`id,parameter,label,style,colour,x,y,width,height,font size[,marks]`, read by `goodlookindri::design::read_design`). Each row becomes a control of the `hit` registry, bound to the plugin parameter whose id is in the `parameter` column, placed with `goodlookindri::host::map_rect`, and drawn through GoodLookin DRI. The plate and the labels are one cached layer (`frame_layered`); only the controls and the value text are painted each frame.

```rust
const DESIGN: &str = include_str!("../design.csv");

fn make_editor(host: EditorHost) -> Option<Box<dyn Editor>> {
    DesignEditor::from_csv(DESIGN, &INFO, host).map(|e| Box::new(e) as Box<dyn Editor>)
}
```

| Constructor | Use |
|---|---|
| `DesignEditor::from_csv(csv, &INFO, host)` | An editor factory: a bad design is logged and gives `None`, so the host shows its generic controls |
| `DesignEditor::try_new(csv, &INFO, host, &DesignOptions)` | The same with the error (`DesignEditorError`) and options |
| `DesignEditor::from_items(rows, &INFO, host, &DesignOptions)` | Rows you already read or built with `Item::new` |

The control follows the parameter, not the row:

| Parameter | Control | Drawn as |
|---|---|---|
| `Unit::Toggle` | toggle | `Lvr` a lever, `Mini` and `Tab` a mini toggle, any other style a bat; the row colour is the lamp |
| `Unit::Choice` | key bank | one latching key per choice, in a push style taken from the knob style (`Sq` square, `Rd` round, `Dm` dome, `Cyb` chamfer, `Neo` neon, and so on); vertical when the row is taller than wide |
| anything else | knob or fader | the row's knob style; a vertical fader when the row is at least twice as tall as wide, a horizontal one when at least twice as wide as tall |

GoodLookin DRI's loader accepts only knob styles in the `style` column, so a fader or a switch cannot be named there. The shape of the row picks the fader, and the knob style picks the look of the switch, the keys and the fader cap (console cap for `Brit`, `N`, `A`, `FS` and `Mtl`, oval for the others). `marks` is read and validated but not used yet: the text colours come from the plate.

The top of a row is the control and the hit area (a circle for a knob, the rectangle for the rest). Under it is a text strip as high as the row's font size asks: the label (the parameter's name when the label is empty) and, for knobs and faders, the value. The strip does not answer the mouse.

`DesignOptions` carries what the file does not: the faceplate (`.plate("Navy")`, the default, any of GoodLookin's 40 codes), `map_rect`'s `.scale()` and `.origin()`, an explicit `.size()` (by default the furthest edge of the rows plus 12 points) and `.strict()`.

Rows that cannot bind are reported, not dropped silently. A row with an empty `parameter` or an unknown id is not a control and is not drawn; `DesignEditor::issues()` lists it (`RowIssue`), and a warning goes to the log. With `.strict(true)` the same rows make `try_new` fail with `DesignEditorError::Unbound`. A malformed CSV, an invalid row, a duplicate id, an unknown faceplate or a bad option are always errors.

`bindings()` gives each bound row, its parameter index and its control; `controls()` the `ControlSet` with the hit areas and values; `control_for("gain")` the control of a parameter.

`examples/design_editor` is a working plugin: five rows of CSV give two knobs, a fader, a lever switch and a three-key bank. `cargo run -p example-design-editor --bin snapshot -- out.ppm 2` writes its editor to an image.
