# Editor

## Summary

A plugin writes one editor and the SDK hosts it in every format: CLAP (`clap.gui`), LV2 (`ui:CocoaUI`), and the VST3, Audio Unit and AAX shells (`op_editor_*`). The editor and its hosting are Rust. The view is an `NSView` made through the Objective-C runtime (`openplugin_dri::cocoa`), so the editor is macOS only in this release. No JUCE code or JUCE dependency is involved.

The working example is [`examples/editor`](../examples/editor): a Navy plate, a lit Brit knob, a bat toggle, an LED ladder, two text presets, and every format exported. It is drawn with GoodLookin DRI 0.2 and rasterised by `openplugin_dri::SceneCanvas` ([DRI_GUIDE.md](DRI_GUIDE.md)). An editor may also fill its pixels any other way.

## The contract

| Piece | Where | Does |
|---|---|---|
| `Editor` | `openplugin_core::editor` | size, `bind`, `param_changed`, `tick`, `render`, mouse input |
| `EditorHost` | the same | where gestures go: `begin`, `perform`, `end`, or `set` for all three |
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

## Per format

| Format | Gestures | Values back to the editor | Engine binding |
|---|---|---|---|
| CLAP | The value goes into the parameter plane at once. Begin, value and end events go to the host from the next `process` or `flush`, after `request_flush` | The view polls the plane on its timer | The instance's `editor_state` |
| LV2 | Control port writes, with `ui:touch` grabs for begin and end | `port_event` | `instance-access`, checked against this binary's instances |
| VST3, AU, AAX | `OpEditorHost` callbacks into the C++ shell, which calls the host | `op_editor_set_plain` from the shell | `op_editor_bind`, checked against the live-instance table |

## Why the shells are C++

The VST3, Audio Unit and AAX entry points stay C++ (`native/`):

- AAX has no C interface. Its SDK is C++ classes that a plugin subclasses, so the shell has to be C++.
- The VST3 interfaces are C++ COM-style classes. They could be laid out by hand in Rust, but the shell is already proven and the SDK headers are the reference.
- The Audio Unit shell builds on Apple's AudioUnitSDK, which is C++.

Everything those shells call is Rust: the audio contract (`op_shell_*`), the tail, the AAX IDs, and the editor (`op_editor_*`, written by `export_shell!` over `EditorView`). The only Objective-C++ file added is the `VST3EditorProof` test host, because it must call the C++ VST3 interfaces.

## Tests

- `cargo test -p example-editor`: a test with no harness that runs on the main thread. It opens the editor through the shell ABI, CLAP and LV2. It clicks it, checks the gestures, and reads pixels back from the view.
- `VST3EditorProof` (`native/vst3/editor_host.mm`, built with `-DOP_EDITOR=ON`): loads the VST3 bundle in process and opens the view. It re-creates the engine with the view open. If `OP_EDITOR_PROOF_CLICK` is `x;y;param`, it clicks there and checks the paired edits.

None of these is a session in Logic, Cubase, Pro Tools, Bitwig or Reaper.

## Hit testing

An editor does not have to hand-code hit rectangles. `openplugin_dri::ControlSet` holds knobs, faders, toggles, momentary buttons and key banks, each with a hit area, a parameter index and its `ParamInfo`. The editor forwards `mouse_down`, `mouse_move`, `mouse_up`, `wheel`, `modifiers_changed` and `param_changed` to it. The set turns the mouse into `begin`, `perform` and `end` gestures through the `EditorHost`: relative drags, a fine adjustment with Shift or Command, double-click to reset, wheel steps, snapping for stepped and choice parameters, and a clamp so that only in-range values reach the host. A value from the host updates a control without sending a gesture back. The set also keeps hover and pressed state, and iterating it gives what to draw. `examples/editor` is written this way; see [DRI_GUIDE.md](DRI_GUIDE.md#controls-and-hit-testing) for the rules.

`Editor::modifiers_changed(Modifiers)` is the one addition to the contract: optional, empty by default, so existing editors are unaffected. The macOS view does not call it yet.

## Editors from a design file

An editor does not have to be written by hand. `openplugin_dri::DesignEditor` implements `Editor` from a GoodLookin design CSV and the plugin's `PluginInfo`: each row is a control bound to the parameter named in its `parameter` column, so the layout lives in a text file and the plugin has no drawing or hit-testing code.

```rust
fn make_editor(host: EditorHost) -> Option<Box<dyn Editor>> {
    DesignEditor::from_csv(include_str!("../design.csv"), &INFO, host).map(|e| Box::new(e) as Box<dyn Editor>)
}

openplugin::targets::export_clap!(MyPlugin, editor = make_editor);
```

It goes through the same contract as any editor: gestures through `EditorHost`, host values through `param_changed`, pixels from `SceneCanvas` with the plate cached. It works in every format that takes an `EditorFactory`. A design with a malformed row makes `from_csv` return `None`, and the host shows its generic controls. Rows that name no parameter, or one the plugin does not have, are listed by `issues()`. The kinds of control, the options and the rules are in [DRI_GUIDE.md](DRI_GUIDE.md#editors-from-a-design-file). The working example is [`examples/design_editor`](../examples/design_editor), exported as CLAP; its tests load the CSV, check that every parameter has a control, and render a frame. It has not been opened in a host.
