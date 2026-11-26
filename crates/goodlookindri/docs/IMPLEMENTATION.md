# Implementation plan

## Product requirements

Audience: developers of audio utilities and effects. Plugins live in their own repositories. A rack and console aesthetic combines generated hardware surfaces with analysis displays. Instruments and synths are outside the current scope.

Controls and panels are generated in code. No designer-produced image assets are required. Parameter changes reach DSP immediately even while the graphic is still moving.

Development builds can carry an in-plugin editor. Testers change style, label, colour, type size, and bounds. Developers keep automation ids and stored enum values stable when the presentation changes.

Release users can scale the interface. They cannot edit typography or layout. The accepted design is a CSV file embedded in the plugin.

GoodLookin DRI (the Design Rust Interface) is the Rust implementation of that model. [GoodLookinUI](https://github.com/MistaMin/GoodLookinUI) is the C++ implementation, with a JUCE painting adapter. Design files move between them. The plugin project chooses which one to link. The toolkit does not choose a plugin framework.

Target systems are whatever the host plugin already ships: Apple Silicon and Intel macOS, Windows, and Linux. Format wrappers remain the plugin's responsibility.

## 0.1 — foundation

Shipped in 0.1:

- Design CSV, motion, settings, look table, level tap, spectrum analysis.
- Drawing commands for 47 knob styles in the hardware and flat families, 40 plates and finishes, meters, switches, retro faces, sliders, lamps, and four spectrum looks.
- Gallery page layout and a developer editor state machine with undo, a colour wheel, and atomic autosave.
- Tests for the design fixture, FFT calibration, level tap, and a non-empty command list for every knob style.

Known limits of 0.1:

- Commands approximate a few JUCE gradients. Rim strokes are a single ellipse. Gloss is a diagonal fill. Grub screws are empty, matching the C++ stubs. Text is not rasterised here.
- There is no pixel comparison against `docs/images`.
- Hit-testing exists for the colour wheel only. Knobs, faders, and switches do not yet report drags.
- No window demo is included. The first host plugin is the proof of embedding.

## 0.2 — lit rendering (current)

Shipped in 0.2:

- Scene commands for multi-stop linear, radial and conic fills, feathered and additive fills, soft shadows, inner shadows and an image blit, with a shared light rig. `Cmd` is now `#[non_exhaustive]`; `Scene::lower_basic` rewrites the newer commands for hosts that only know the original set.
- A lit knob tier: `knob3d` shades each physical knob per pixel from a height profile, materials and a fixed studio light, and caches the result as a sprite. All 35 physical styles have a lit model and there are 15 presets. The classic and flat tiers are unchanged, and the 12 display and neon styles stay vector.
- Redesigned panels and finishes, screws, rack ears, printed and engraved legends, VU and PPM meters, lamps, LED ladders, faders, toggles, rockers, push buttons, neon and display parts, and the spectrum display.
- The shell crate's window, software rasteriser, text and widget set with keyboard focus, plus examples that write catalogue pages and a demo faceplate to image files.

Known limits of 0.2:

- Lit knobs need a host that can blit images; other hosts get the classic drawing. Hosts must report their backing scale and panel colour for sharp, correctly lit sprites.
- There is no pixel comparison against `docs/images` beyond one golden frame in the shell crate.
- Hit-testing in the core crate exists for the colour wheel only. Knobs, faders and switches do not yet report drags.
- The shell repaints the whole frame.

## 0.3 — interaction

Hit-testing and keyboard focus for knobs, faders, switches, and keys in the core crate (the shell's own widget set already has focus and keyboard input). Comparison images from the gallery renders that 0.2 can already write. Redo beside undo. Dragging a control updates the CSV through the existing autosave.

## 0.4 — host polish

Dirty-region redraw, cached static plates at the editor scale, and idle suspension when the editor is closed. Accessibility names taken from the design row. A documented replay example for one immediate-mode UI and one retained-mode canvas.

## 0.5 — qualification

Continuous tests on macOS, Windows, and Linux. Pixel thresholds against the gallery images. A second plugin, in its own repository, linking this crate without copying it. DAW smoke tests stay in those plugin repositories.

## Later

Menus, control creation, alignment, and snapping. A routing editor is a DSP feature and does not belong in this crate.
