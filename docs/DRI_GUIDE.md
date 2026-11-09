# GoodLookin DRI (Direct Rendering Interface) Guide

## Summary

GoodLookin DRI draws through the GoodLookinDri crate. That crate is the JUCE-free port of the 47 knob styles, 40 faceplates, meters, faders, switches, and spectrum. `openplugin-dri` does not keep a second copy of those drawers.

`draw_knob` and `draw_faceplate` accept the short style enums and call GoodLookinDri. `draw_knob_code` and `draw_faceplate_code` take any GoodLookin style code, for example `"Chk"` or `"Walnut"`.

## Aesthetic System

### 1. Procedural Knobs
47 procedural styles rendered through vector geometry and lighting:
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
