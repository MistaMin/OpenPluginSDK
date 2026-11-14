# GoodLookin DRI (Design Rust Interface) Guide

## Summary

GoodLookin DRI draws through the `goodlookindri` crate. That crate is the JUCE-free port of the 47 knob styles, 40 faceplates, meters, faders, switches, and spectrum. `openplugin-dri` does not keep a second copy of those drawers.

`draw_knob` and `draw_faceplate` accept the short style enums and call GoodLookin DRI. `draw_knob_code` and `draw_faceplate_code` take any GoodLookin style code, for example `"Chk"` or `"Walnut"`.

This release follows GoodLookin DRI 0.2.

## Knob tiers

GoodLookin DRI 0.2 draws a knob in one of three tiers. `DrawList::knob_tier` picks one for every knob an editor draws:

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
            |dl| draw_faceplate_code(dl, 240.0, 160.0, "Navy"),     // painted once per scale
            |dl| draw_knob_code(dl, 55.0, 68.0, 30.0, gain, "GAIN", "0 dB", "Brit", Color::BRIT_RED),
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

The view repaints 30 times a second on the main thread, so an editor of any size needs the layer. Change the key when the static look changes, or call `invalidate_layer`.

`DrawList::commands` stays for hosts that only draw basic shapes. It is GoodLookin DRI's `Scene::lower_basic`, with each lit knob drawn again in the classic tier, because a basic host has no images.

`cargo run -p example-editor --bin snapshot -- out.ppm 2 6 1` writes the example editor to an image. With `OP_SNAPSHOT_FRAMES=60` set, it also prints the time per frame.

## Aesthetic System

### 1. Procedural Knobs
47 procedural styles, lit by default (see Knob tiers):
- `KnobStyle::Brit`: Classic vintage British console fluted skirt with distinct colored caps (Red, Blue, Green, Cream).
- `KnobStyle::N`: Knurled industrial discrete channel strip dials.
- `KnobStyle::A`: Precision studio pointer knobs with radial markers.
- `KnobStyle::Bakelite`: Heavy vintage fluted bakelite with deep drop-shadows and engraved white indicators.
- `KnobStyle::Metal` / `Chrome`: Machined concentric metallic specular rings.
- `KnobStyle::Wood`: Hand-finished studio mahogany and walnut knobs.
- `KnobStyle::Neon` / `Digital`: Cyberpunk neon rings and high-contrast digital displays.

### 2. Faceplates
40 plate finishes generated without bitmap images:
- `FaceplateStyle::BrushedAluminum`: Horizontal anodized grain lines.
- `FaceplateStyle::BritishConsole`: Classic olive/navy console finish with recessed beveled boundary.
- `FaceplateStyle::DarkAnodized`: Matte black 19-inch rack unit aesthetic.
- `FaceplateStyle::Cyberpunk`: High-contrast dark chassis with cyan/magenta neon edge glows.

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
        ControlKind::Knob => draw_knob_code(dl, x + w * 0.5, y + h * 0.5, w * 0.5, c.normalized(), "GAIN", &c.info.format(c.value()), "Brit", Color::BRIT_RED),
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

The modifier keys reach the editor through `Editor::modifiers_changed`, an optional method with an empty default. The macOS view does not call it yet, so on macOS today the fine adjustment is available only to a host that reports modifiers some other way. Call `ControlSet::set_modifiers` yourself in a standalone window.

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
