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

## Resizable editors

`SceneCanvas::resize(width, height)` sets a new size in points. The next frame has the new pixel size, and the cached layer is dropped, so `frame_layered` paints the plate again at that size and then reuses it. The same size changes nothing, and going back to an earlier size repaints it once. The layer's cache key already includes the pixel size, so a canvas that is resized without `resize` repaints too; `resize` is the call that changes the size the editor draws for.

Draw from the editor's current size, not from constants: pass the width and height to `draw_faceplate_code`, and scale the positions, radii and text sizes of the parts by one factor. `examples/editor` has a `layout(width, height)` function that returns the scale and the hit areas, and uses the same function to draw and to hit-test, so a click lands on what is drawn. Painting the plate costs the most (310 ms at 1080 × 700 points above), and it happens once per size, so a host that sends many sizes during a drag pays for each one. Keep the size range modest, or draw the plate more cheaply while the size is changing. See [EDITOR.md](EDITOR.md) for the contract.

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
