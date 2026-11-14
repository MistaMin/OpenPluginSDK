# Editor

## Summary

A plugin writes one editor and the SDK hosts it in every format: CLAP (`clap.gui`), LV2 (`ui:CocoaUI`), and the VST3, Audio Unit and AAX shells (`op_editor_*`). The editor and its hosting are Rust. The view is an `NSView` made through the Objective-C runtime (`openplugin_dri::cocoa`), so the editor is macOS only in this release. No JUCE code or JUCE dependency is involved.

The working example is [`examples/editor`](../examples/editor): a fader, a switch, a meter, two text presets, and every format exported.

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
