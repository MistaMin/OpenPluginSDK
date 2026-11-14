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
