# Editor

## Summary

A plugin writes one editor and the SDK hosts it in every format: CLAP (`clap.gui`), LV2 (`ui:CocoaUI`), and the VST3, Audio Unit and AAX shells (`op_editor_*`). The editor and its hosting are Rust. The view is an `NSView` made through the Objective-C runtime (`openplugin_dri::cocoa`), so the editor is macOS only in this release. No JUCE code or JUCE dependency is involved.

The working example is [`examples/editor`](../examples/editor): a Navy plate, a lit Brit knob, a bat toggle, an LED ladder, two text presets, and every format exported. It resizes from 180 x 120 to 480 x 320 points, keeping its 3:2 shape. It is drawn with GoodLookin DRI 0.2 and rasterised by `openplugin_dri::SceneCanvas` ([DRI_GUIDE.md](DRI_GUIDE.md)). An editor may also fill its pixels any other way.

## The contract

| Piece | Where | Does |
|---|---|---|
| `Editor` | `openplugin_core::editor` | size, size range, `resize`, `bind`, `param_changed`, `tick`, `render`, mouse input, modifiers, keys and focus |
| `Key`, `Modifiers` | the same | the keys an editor can ask for, and the held modifier keys |
| `SizeRange` | the same | the sizes an editor takes: minimum, maximum and an optional aspect, with `clamp` |
| `EditorHost` | the same | where gestures go: `begin`, `perform`, `end`, or `set` for all three; `request_resize` asks the host for a size |
| `EditorFactory` | the same | `fn(EditorHost) -> Option<Box<dyn Editor>>`, passed to the export macros |
| `Plugin::editor_state` | `openplugin_core::Plugin` | what the engine shares with its editor (meters, an analyser ring) |
| `EditorView` | `openplugin_dri::cocoa` | the `NSView`, its 30 Hz timer, the CoreGraphics blit, and `run_window` for a standalone |

```rust
fn make_editor(host: EditorHost) -> Option<Box<dyn Editor>> {
    Some(Box::new(MyEditor::new(host)))
}

openplugin::targets::export_clap!(MyPlugin, editor = make_editor);
openplugin::targets::export_lv2!(MyPlugin, editor = make_editor);
openplugin::targets::export_shell!(MyPlugin, editor = make_editor);
```

The rules are the same in every format:

- Every `Editor` call is on the main thread.
- The editor never writes to the engine. It sends a gesture through `EditorHost` in plain units. The host returns the value to the engine as an ordinary parameter change.
- Values that change elsewhere (automation, a preset, a state load) reach the editor through `param_changed`, only when they change.
- An editor can exist without an engine and can be re-bound while open. `bind` gets the engine's `EditorState` or `None`. The editor downcasts it to the plugin's own type.
- `render` returns pixels as `0x00RRGGBB` words, top row first, at `scale` device pixels per point.

## Size

An editor is fixed-size unless it says otherwise. The contract has three parts, all additive, so an editor written before them keeps its one size in every format.

| Call | Meaning |
|---|---|
| `Editor::size()` | the current size in points. A fixed editor never changes it |
| `Editor::size_range()` | a `SizeRange { min, max, aspect }`. The default is `SizeRange::fixed(size())` |
| `Editor::resize(w, h)` | the host gives the editor a new size. Only sizes inside `size_range()` arrive. Afterwards `size()` is that size and `render` draws at it. The default ignores it |
| `EditorHost::request_resize(&range, w, h)` | the editor asks the host for a size. It returns whether the host agreed. The editor is resized later through `resize`, never inside this call |

`SizeRange::clamp(w, h)` is the one rule every format uses to bring a request into range. Without an aspect, each side is held to its bounds. With an aspect, the result has that shape (to a point) and the area nearest to the request, inside both bounds. Clamping a result again changes nothing. A `min` above `max` is read as a fixed size, and sizes below 1 are read as 1. `contains(w, h)` checks the bounds only: the aspect is a hint, and a size inside the bounds with another shape is accepted.

An editor that resizes overrides `size_range` and `resize`, and draws from its current size. The example does it in a few lines:

```rust
fn size(&self) -> (u32, u32) { self.size }
fn size_range(&self) -> SizeRange { SizeRange::between((180, 120), (480, 320)).with_aspect(240, 160) }
fn resize(&mut self, w: u32, h: u32) {
    self.size = self.size_range().clamp(w, h);
    self.canvas.resize(self.size.0 as f32, self.size.1 as f32); // drops the cached plate
}
```

The view (`EditorView`) follows the editor. `EditorView::resize(w, h)` clamps, calls `Editor::resize`, and sets the view's frame to the editor's new size. If a host sets the view's frame itself (`setFrameSize:`), the view hands that size to the editor, and the frame stays as the host gave it. Mouse positions are always mapped to editor points, so a click lands on the control that is drawn there, even for the moment the frame and the editor differ. A resize that arrives while the editor is inside one of its own calls (a click that asked the host for a size, answered at once) is applied when that call returns. `run_window` makes the standalone window resizable, with minimum, maximum and aspect, when the editor's range is not fixed.

## Keyboard and focus

Five optional methods on `Editor`, all with defaults that change nothing, so an editor written before them behaves as it did: it never takes the keyboard, and every key stays with the host.

| Call | Meaning |
|---|---|
| `modifiers_changed(Modifiers)` | the held Shift, Control, Alt and Command changed. Called before the mouse or key call it belongs to, never when nothing changed |
| `wants_keys() -> bool` | the editor wants key events. Default `false`. The view takes keyboard focus only while this is true |
| `key_down(Key, Modifiers) -> bool` | a key went down (auto-repeat sends it again). Return true if the editor used it |
| `key_up(Key, Modifiers) -> bool` | the same for the key going up |
| `focus_changed(bool)` | the view gained or lost keyboard focus. On a loss, forget keys held |

`Key` is `Left`, `Right`, `Up`, `Down`, `PageUp`, `PageDown`, `Home`, `End`, `Tab`, `Return`, `Escape`, `Delete`, `Backspace`, `Space` and `Char(char)`.

The host keeps what the editor does not take. A key for which `key_down` or `key_up` returns false (or that has no `Key`, such as a function key) is passed to the view's next responder, so the host's transport keys and shortcuts work with the editor focused. The editor should return false for everything it does not use, and the registry does (below). Command-key shortcuts also go through the host's own key-equivalent handling before the view sees them.

Cocoa mapping, in `openplugin_dri::cocoa`:

| Event | Result |
|---|---|
| any mouse, wheel or key event | its `modifierFlags` (Shift bit 17, Control 18, Alt 19, Command 20) are compared with the last ones and `modifiers_changed` is called on a difference |
| `flagsChanged:` | the same, and the event goes on to the next responder |
| `keyDown:`, `keyUp:` | key codes 123 to 126 are the arrows, 116 and 121 Page Up and Down, 115 and 119 Home and End, 48 Tab, 36 and 76 (keypad Enter) Return, 53 Escape, 51 Backspace, 117 Delete (forward), 49 Space. Any other key is `Char(c)`, the character it types without modifiers (lower case); keys that type only control or private-use characters have no `Key` and are passed on |
| `acceptsFirstResponder` | `Editor::wants_keys()` |
| a click | the view becomes first responder first, when the editor wants keys (acceptsFirstMouse stays true) |
| `becomeFirstResponder`, `resignFirstResponder` | `focus_changed(true)` and `focus_changed(false)` |

The CLAP, LV2 and shell hosts reach the editor through this same view, so none of them needed a change. How a given host's window routes keys to a plugin view (some forward them to the plugin's view only when it is first responder, and some keep them) differs per host and is not claimed here.

## Per format

| Format | Gestures | Values back to the editor | Engine binding |
|---|---|---|---|
| CLAP | The value goes into the parameter plane at once. Begin, value and end events go to the host from the next `process` or `flush`, after `request_flush` | The view polls the plane on its timer | The instance's `editor_state` |
| LV2 | Control port writes, with `ui:touch` grabs for begin and end | `port_event` | `instance-access`, checked against this binary's instances |
| VST3, AU, AAX | `OpEditorHost` callbacks into the C++ shell, which calls the host | `op_editor_set_plain` from the shell | `op_editor_bind`, checked against the live-instance table |

### Resizing per format

| Format | Host resizes the editor | Editor asks the host | Description |
|---|---|---|---|
| CLAP | `can_resize` is true unless the range is fixed. `get_resize_hints` gives the axes and the aspect. `adjust_size` clamps in place. `set_size` accepts any size inside the bounds (the aspect is a hint) and refuses others, a fixed editor accepting only its own size | `clap_host_gui::request_resize`, looked up from the host when the GUI is created. A host that answers `true` calls `set_size`, possibly inside the request | |
| LV2 | the UI offers `ui:resize` through `extension_data`. `ui_resize(handle, w, h)` returns 0 for a size inside the bounds and non-zero otherwise | the host's `ui:resize` feature. When it returns 0 the editor takes the size on the view's next tick | `lv2:extensionData ui:resize` for a resizable editor; `ui:noUserResize` only for a fixed one. `lv2::write_bundle_for_editor` asks the editor, and `plugin_ttl_sized` writes either form |
| VST3, AU, AAX | `op_editor_resize`, `op_editor_adjust_size`, `op_editor_get_size` and `op_editor_size_range` in the `OP_EDITOR_RESIZE` block of `op_editor.h`. The Rust side exports them; the C++ shells do not use them yet | `op_editor_set_request_resize(editor, context, callback)`. `OpEditorHost` is unchanged, so existing shells build as before | |

## Why the shells are C++

The VST3, Audio Unit and AAX entry points stay C++ (`native/`):

- AAX has no C interface. Its SDK is C++ classes that a plugin subclasses, so the shell has to be C++.
- The VST3 interfaces are C++ COM-style classes. They could be laid out by hand in Rust, but the shell is already proven and the SDK headers are the reference.
- The Audio Unit shell builds on Apple's AudioUnitSDK, which is C++.

Everything those shells call is Rust: the audio contract (`op_shell_*`), the tail, the AAX IDs, and the editor (`op_editor_*`, written by `export_shell!` over `EditorView`). The only Objective-C++ file added is the `VST3EditorProof` test host, because it must call the C++ VST3 interfaces.

## Tests

- `cargo test -p example-editor`: a test with no harness that runs on the main thread. It opens the editor through the shell ABI, CLAP and LV2. It clicks it, checks the gestures, and reads pixels back from the view. It then resizes the editor through each of the three (`op_editor_resize`, CLAP `adjust_size` and `set_size`, LV2 `ui:resize`), and checks that the view's size, the drawn pixels and the click positions follow: a click at the old position of the Warm switch hits nothing, a click at the scaled one does. It also has the editor ask for a size (a double-click on the bare plate) through `op_editor_set_request_resize`, `clap_host_gui::request_resize` (the host calls `set_size` inside the request) and the host's `ui:resize`. A last case sends real `NSEvent`s to the shell view with a recording responder standing in for the host: modifier flags from mouse events and `flagsChanged:` (a Shift arrow steps a tenth as far), a handled arrow (a whole gesture, nothing for the next responder), unhandled keys and the up of a key the editor did not take (both reach the next responder), Tab, Space and Escape, and the focus ring in the pixels. `flagsChanged:` is exercised with a key event that carries the flags, because `NSEvent` cannot build a flags-changed event directly. No window is used, so first-responder handoff in a real window is not exercised.
- `cargo test -p openplugin-core`: the clamping rules of `SizeRange` (bounds, aspect, a result clamps to itself).
- `VST3EditorProof` (`native/vst3/editor_host.mm`, built with `-DOP_EDITOR=ON`): loads the VST3 bundle in process and opens the view. It re-creates the engine with the view open. If `OP_EDITOR_PROOF_CLICK` is `x;y;param`, it clicks there and checks the paired edits.

None of these is a session in Logic, Cubase, Pro Tools, Bitwig or Reaper.

## Hit testing

An editor does not have to hand-code hit rectangles. `openplugin_dri::ControlSet` holds knobs, faders, toggles, momentary buttons and key banks, each with a hit area, a parameter index and its `ParamInfo`. The editor forwards `mouse_down`, `mouse_move`, `mouse_up`, `wheel`, `modifiers_changed` and `param_changed` to it. The set turns the mouse into `begin`, `perform` and `end` gestures through the `EditorHost`: relative drags, a fine adjustment with Shift or Command, double-click to reset, wheel steps, snapping for stepped and choice parameters, and a clamp so that only in-range values reach the host. A value from the host updates a control without sending a gesture back. The set also keeps hover and pressed state, and iterating it gives what to draw. `examples/editor` is written this way; see [DRI_GUIDE.md](DRI_GUIDE.md#controls-and-hit-testing) for the rules.

`Editor::modifiers_changed(Modifiers)` is optional and empty by default. The macOS view calls it (see below), so Shift or Command gives the fine adjustment on macOS.

`ControlSet` handles the keys for an editor: it keeps one focused control, moves focus with Tab, and turns the keys into gestures. `examples/editor` forwards `wants_keys`, `key_down`, `key_up` and `focus_changed` to it and draws a ring around the focused control. See [DRI_GUIDE.md](DRI_GUIDE.md#keyboard-and-focus-in-the-registry).

## Editors from a design file

An editor does not have to be written by hand. `openplugin_dri::DesignEditor` implements `Editor` from a GoodLookin design CSV and the plugin's `PluginInfo`: each row is a control bound to the parameter named in its `parameter` column, so the layout lives in a text file and the plugin has no drawing or hit-testing code.

```rust
fn make_editor(host: EditorHost) -> Option<Box<dyn Editor>> {
    DesignEditor::from_csv(include_str!("../design.csv"), &INFO, host).map(|e| Box::new(e) as Box<dyn Editor>)
}

openplugin::targets::export_clap!(MyPlugin, editor = make_editor);
```

It goes through the same contract as any editor: gestures through `EditorHost`, host values through `param_changed`, pixels from `SceneCanvas` with the plate cached. It works in every format that takes an `EditorFactory`. A design with a malformed row makes `from_csv` return `None`, and the host shows its generic controls. Rows that name no parameter, or one the plugin does not have, are listed by `issues()`. The kinds of control, the options and the rules are in [DRI_GUIDE.md](DRI_GUIDE.md#editors-from-a-design-file). The working example is [`examples/design_editor`](../examples/design_editor), exported as CLAP; its tests load the CSV, check that every parameter has a control, and render a frame. It has not been opened in a host.
