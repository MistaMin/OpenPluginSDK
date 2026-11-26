# Integration guide

GoodLookin DRI is a library. The plugin owns the window, the audio device callbacks, the parameter table, and the format wrapper. This crate owns the design file, the display motion, the analysis used by meters and the spectrum, and the list of drawing commands.

## Add the crate

```toml
[dependencies]
goodlookindri = { git = "https://github.com/MistaMin/GoodLookinDri" }
```

Rust 2021 is enough. There are no native dependencies.

## Design files

A design is plain CSV. The first line is `GoodLookinUI design version 1`. Columns are:

`id,parameter,label,style,colour,x,y,width,height,font size`

and, when any control sets it, `marks`.

Use unique ids, single-line text, and `#RRGGBB` colours. Quoted commas and escaped quotes are supported. The loader checks style, geometry, numbers, marks, and duplicate ids. Coordinates are relative to the control's parent. The original `HardwareUI design version 1` header still loads. Files written here load in GoodLookinUI, and the other way around.

```rust
use goodlookindri::design::{write_design, read_design, Item};

let gain = Item::new("input-gain", "inputGain", "Input", "console", "#B69B73",
                      12.0, 52.0, 150.0, 98.0, 11.0);
let text = write_design(&[gain]).unwrap();
let items = read_design(&text).unwrap();
```

Embed the accepted CSV in the plugin build with `include_str!`. Release users do not need an editable file on disk.

## Audio thread and GUI thread

Call only these from the audio callback. Both are wait-free for a fixed channel count:

```rust
levels.push(&[left, right]);   // goodlookindri::level::LevelTracker
input.push(&[left, right]);    // goodlookindri::spectrum::SampleRing
```

On the GUI thread, a few dozen times a second:

```rust
let reading = levels.read();          // peak and mean square since the last read
motion.step(normalized, dt, 18.0);    // display only
view.update(&input, dt, sample_rate); // FFT, fall, peak hold
```

Send parameter edits to the DSP immediately. Do not read or write CSV, and do not run the FFT, on the audio thread.

`Motion::step` is the same critically damped step as GoodLookinUI. `frequency` defaults to 18.

## Painting

`draw::knob` appends commands for one design row. `proportion` is 0 to 1. The pointer sweep is ±0.8π, zero straight up.

```rust
use goodlookindri::draw;
use goodlookindri::hardware::{draw_panel, use_dark_marks};
use goodlookindri::scene::Scene;

let mut scene = Scene::new();
draw_panel(&mut scene, panel_bounds, top, bottom, faceplate.finish);
let dark = use_dark_marks(&item.marks, backdrop);
draw::knob(&mut scene, knob_bounds, motion.position, &item, enabled, dark);
```

Replay `scene.cmds`. Each command is an ellipse, round rectangle, line, polygon, arc, clip, or a text run. Text is a string, a size, and a colour. The host draws it with its own font. Clips may be ignored by a backend that cannot clip.

`draw::FLAT_KNOBS` selects the flat family for every knob. A style of `flat_Brit` selects it for that knob only.

### Lit knobs

`draw::set_knob_mode(KnobMode::Lit)` selects the lit tier for every knob, and a style of `lit_Brit` selects it for one knob. A lit knob is a per-pixel shaded sprite from `knob3d`, cached per style, size, angle and colour, and emitted as one `Cmd::Image`. Tell the crate the backing scale and the panel colour so sprites are sharp and reflect the right surface:

```rust
draw::set_knob_scale(scale);          // device pixels per logical unit, default 2
draw::set_knob_floor(panel_colour);   // colour the knob stands on
hardware::set_device_scale(scale);    // panel grain follows the same scale
```

Styles without a lit model, and hosts that cannot blit images, get the classic drawing.

### Newer commands

`Cmd` is `#[non_exhaustive]`. Besides the original shapes it carries `Fill` (multi-stop linear, radial and conic paints, feathered edges, additive blends), `Shadow`, `InnerShadow` and `Image` (premultiplied sRGB pixels). A host that matches on `Cmd` needs a `_` arm. A host that only knows the original commands can replay `scene.lower_basic()`, which rewrites the newer ones into approximations and drops images.

Meters, switches, sliders, screws and the spectrum view are the same idea: pass a `Scene` and the current levels. `SpectrumView::paint` draws Standard, Multicolour, Bars, or Spectrogram from `SpectrumSettings`. `gallery::paint_page` draws one of the six catalogue pages when you want a browser inside a tool.

## Parameters

Implement `host::ParamView` on your parameter, or pass any slice of types that expose `id()` and `normalized()`.

```rust
let value = goodlookindri::host::value_for(&item, &params, 0.0);
let rect = goodlookindri::host::map_rect(&item, origin_x, origin_y, scale);
```

Changing a label or a colour does not change the parameter id. Store that id in the `parameter` column and keep it stable.

## Development inspector

`editor::Editor` holds the design, an undo stack, and the colour-wheel state. Call `set_style`, `set_label`, `set_colour_hex`, `set_font`, `set_bounds`, `set_marks`, `undo`, `to_csv`, and `load_csv`. `paint_wheel` and `wheel_mouse` cover the hue ring and the saturation square. Your editor draws the buttons.

Wire `autosave::DesignAutosave` to the folder you want in git. Call `request` when the CSV changes, `poll(dt_ms)` from the UI timer, and `flush` when the editor closes. Identical text is not rewritten.

Leave the inspector out of release builds. Ship the CSV you already saved.

## A five-call canvas

Some hosts already implement five primitives: gradient ellipse, stroke ellipse, line, gradient panel, stroke panel. `water::brit_knob` and `water::panel` target that set and return `UnsupportedStyle` for every other knob. Use `Scene` for the full catalogue.

## What this crate will not do

It will not open a window, create a plugin instance, attach to a host parameter object, or choose CLAP, VST3, AU, LV2, or AAX. Those stay in the plugin. A second plugin should be able to depend on this crate without taking any code from the first.
