# Editor

## Summary

A plugin writes one editor and the SDK hosts it in every format: CLAP (`clap.gui`), LV2 (`ui:CocoaUI`), and the VST3, Audio Unit and AAX shells (`op_editor_*`). The editor and its hosting are Rust. The view is an `NSView` made through the Objective-C runtime (`openplugin_dri::cocoa`), so the editor is macOS only in this release. No JUCE code or JUCE dependency is involved.

The working example is [`examples/editor`](../examples/editor): a Navy plate, a lit Brit knob, a bat toggle, an LED ladder, two text presets, and every format exported. It resizes from 180 x 120 to 480 x 320 points, keeping its 3:2 shape. It is drawn with GoodLookin DRI 0.2 and rasterised by `openplugin_dri::SceneCanvas` ([DRI_GUIDE.md](DRI_GUIDE.md)). An editor may also fill its pixels any other way.

## The contract

| Piece | Where | Does |
|---|---|---|
| `Editor` | `openplugin_core::editor` | size, size range, `resize`, `bind`, `param_changed`, `tick`, `render`, mouse input, modifiers, keys and focus |
| `Key`, `Modifiers` | the same | the keys an editor can ask for, and the held modifier keys |
| `DamageRect`, `DamageTracker`, `Editor::damage` | the same | what changed on screen, so the view repaints only that (below) |
| `SizeRange` | the same | the sizes an editor takes: minimum, maximum and an optional aspect, with `clamp` |
| `EditorHost` | the same | where gestures go: `begin`, `perform`, `end`, or `set` for all three; `request_resize` asks the host for a size |
| `EditorFactory` | the same | `fn(EditorHost) -> Option<Box<dyn Editor>>`, passed to the export macros |
| `Plugin::editor_state` | `openplugin_core::Plugin` | what the engine shares with its editor (meters, an analyser ring) |
| `AccessNode`, `AccessRole`, `AccessAction` | the same | what a screen reader sees and does (below) |
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

## Accessibility

The editor is a picture, so to VoiceOver it is one empty box until the editor says what is in it. Three optional methods on `Editor`, all with defaults that change nothing (no nodes, no actions), so an editor written before them behaves as it did:

| Call | Meaning |
|---|---|
| `accessibility_nodes() -> Vec<AccessNode>` | one node per control, in reading order: `label` (the parameter's name), `value` (the text of `ParamInfo::format`), `role` (`Slider`, `Switch` or `Button`), `on`, `rect` in editor points, `focused`, `enabled` |
| `accessibility_action(node, AccessAction) -> bool` | `Increment`, `Decrement` or `Press` on a node. The editor sends the gestures the mouse would, through its `EditorHost`, and returns whether it did anything |
| `accessibility_focus(node) -> bool` | VoiceOver's cursor arrived on the node: take keyboard focus there |

An editor built on `ControlSet` (`examples/editor`, `DesignEditor`) forwards the three calls to the set and has nothing else to write; `DesignEditor` already does. A custom editor that draws its own controls returns its own nodes. `ControlSet` gives:

| Control | Role | Value | Increment, Decrement | Press |
|---|---|---|---|---|
| knob, fader | slider | `ParamInfo::format` | 5% of the travel along the taper (`ACCESS_STEP`); one position for a stepped parameter. One whole begin, perform, end | none |
| key bank | slider | the choice's name | the next or previous key | none |
| toggle | check box | 0 or 1 (`On` or `Off` as the value description) | on, off | flips: begin, perform, end |
| momentary button | button | none | none | begin, maximum, minimum, end |

A read-only control is listed with `enabled` false and takes no action. Nothing is sent when the value would not change (at the end of the range), when the mouse or a key holds a control, or for a pair that means nothing (Press on a slider). Values sent are clamped like every other gesture.

The macOS view (`openplugin_dri::cocoa`) is an accessibility group (`AXGroup`) whose `accessibilityChildren` are one `NSAccessibilityElement` subclass per node. The elements keep no copy of the node: role, label, value, value description, enabled and focused are asked of the editor on every call, so they are always current. `accessibilityFrame` is worked out on every call too, from the node's rectangle scaled to the view's bounds, then `convertRect:toView:` and `convertRectToScreen:`, so a resize, a moved window or another display needs no update. `accessibilityPerformIncrement`, `Decrement` and `Press` become `accessibility_action`; `setAccessibilityFocused:` makes the view first responder (when the editor wants keys) and calls `accessibility_focus`; the view answers `accessibilityFocusedUIElement` with the element of the focused node and `accessibilityHitTest:` with the element under a screen point.

After every tick and every input call the view compares the nodes with the last ones it saw and posts: `NSAccessibilityValueChangedNotification` on an element whose value text changed (a drag, a step, host automation, a preset), `NSAccessibilityFocusedUIElementChangedNotification` when keyboard focus moved, and `NSAccessibilityLayoutChangedNotification` on the view when the number of nodes or the editor's size changed. When the view goes away, elements that the host still holds stop answering.

The proof is `cargo test -p example-editor --test a11y` and `cargo test -p example-design-editor --test a11y`: main-thread tests with the editor in an `EditorView` in a borderless window that is never shown. They read `accessibilityChildren`, role, label, value and frame from the view (the frames checked against the window's screen position computed separately), perform increment, decrement and press on the elements, check the exact gestures that reach a recording `EditorHost`, change a value from the host side and read it back, move focus and resize. What they cannot show: that VoiceOver itself speaks the tree, or that it receives the notifications (the notification pass is tested as a pure comparison, `ax_diff`, and run in the tests without error). A manual check with VoiceOver on a real host is still owed.

## Typing a value, tooltips and files

Three groups of optional `Editor` methods, all with defaults that change nothing, so an editor written before them behaves as it did. `ControlSet` (so `DesignEditor` and `examples/editor`) implements the first two; the third is the editor's own.

**Typing a value.** `take_text_edit() -> Option<TextEdit>` is polled by the view after every input call and tick; the editor returns the request once (`rect` in editor points, `text`, `label`). The view puts a small native `NSTextField` over the rectangle (at least 72 points wide, 22 high, centred on it), prefilled with the text, selected and focused. Return and keypad Enter call `text_edit_commit(text) -> bool`; Escape, a click on the view itself, or a resize of the editor call `text_edit_cancel()` and send nothing. Losing focus (a click elsewhere in the window, the window resigning key, the view leaving its window) commits the text when `text_edit_commit` accepts it and otherwise discards it silently, by `text_edit_cancel()`, with no beep and no red field left behind.

**Hosts that take Return.** Some hosts handle Return and Enter as their own commands (Pro Tools adds a marker) before a focused text field sees them. While the field is open the view therefore holds an `NSEvent` local monitor for key downs. When the field's own editor is the window's first responder, Return (key code 36) and keypad Enter (76) commit and Escape (53) cancels, and the event is swallowed; every other key, and these keys when the field does not have the keyboard, pass unchanged. The monitor is removed when the field closes, when the view is about to leave its window and when the view is dropped (`openplugin_dri::cocoa::key_monitor_count()` is zero when no field is open). Invalid text on Return stays in the field, red, with a beep, as before. `ControlSet` answers with `ParamInfo::format` for the text and, on commit, `ParamInfo::parse`, `clamp`, and one whole begin, perform, end through the `EditorHost`.

- What opens it: a double-click on a knob or fader, and Return on a focused knob, fader or key bank (a key bank accepts a choice name or number). Toggles and buttons keep Return as press, and read-only controls never open it.
- The default for the double-click is a choice and is documented here: **a double-click opens the field** (`DoubleClick::EditText`), because typing a value is what most plugin users expect of it. **Option-double-click resets to the default** in one gesture. `ControlSet::set_double_click(DoubleClick::Reset)` (or `DesignEditor::set_double_click`) turns it round: double-click resets, Option-double-click types. Delete and Backspace on a focused control reset in either mode. This changes what a double-click did before 0.7 (it reset); an editor that wants the old behaviour sets `Reset`.
- Invalid text (it does not parse) is refused: the field stays open, its text turns red, the system beeps, and no gesture is sent. Valid text outside the range is clamped, like every value.
- Keys typed while the field is open belong to the field. The view's `keyDown:` and `keyUp:` return at once, so they reach neither the editor nor the host (no transport keys, no arrow steps).

**Tooltips.** `tooltips() -> Vec<Tooltip>` lists `rect` and `text` pairs. The view registers one NSView tooltip rectangle per entry (`addToolTipRect:owner:userData:`, the view as owner), and compares the rectangles after every tick and input call, so a resize or a new layout re-registers them. The text is not stored: AppKit asks `view:stringForToolTip:point:userData:` each time a tip is about to show, and the view asks `tooltips()` again, so the value shown is current. `ControlSet::tooltips()` gives every control "Name: formatted value", or the string given with `Control::tooltip("...")`.

**Files.**

- Dropped files: `wants_drops() -> bool` (asked when the view is made and every tick) decides whether the view registers as a drag destination for file URLs at all, so an editor that says no leaves drops to the host. While a drag is over the view, `accepts_drop(x, y) -> bool` is asked with the position in editor points (top left origin) and true shows the copy cursor. `files_dropped(&[PathBuf], x, y) -> bool` receives the paths.
- Open-file dialog: the editor returns a `FileDialog { tag, title, extensions, multiple }` once from `take_file_dialog()`. The view builds an `NSOpenPanel` (files only, the extensions as allowed types, a leading dot ignored) and shows it as a sheet on the view's window, or as a window of its own when the view has none. The completion block queues the answer, and the next tick calls `file_dialog_done(tag, Some(paths))`, or `None` for a cancel, so the answer never arrives inside the call that asked. `examples/editor` asks for it with Command-O, takes dropped files too, and shows the last name in the meter's tooltip.

The proofs are `cargo test -p example-editor --test typing` and `cargo test -p example-design-editor --test typing`, main-thread tests with no harness and a window that is never shown. They send `NSEvent`s (a double-click, Return, an arrow, Backspace) to the view, feed the field's delegate calls (`control:textView:doCommandBySelector:` with `insertNewline:` and `cancelOperation:`, `controlTextDidEndEditing:`), post Return, keypad Enter, Escape and another key through `-[NSApplication sendEvent:]` to exercise the key monitor (its count is read back to zero after close, view removal and drop), post `NSWindowDidResignKeyNotification`, and compare the gestures at a recording `EditorHost`; they read the tooltip rectangles and the text the view answers for them through the same method AppKit calls; and they call the drag destination methods (`draggingEntered:`, `draggingUpdated:`, `prepareForDragOperation:`, `performDragOperation:`) with a stand-in dragging info whose pasteboard holds file URLs. The file dialog is tested three ways: a hook on the view (`EditorView::set_file_dialog_hook`) stands in for the panel and answers; `EditorView::open_panel_for` builds the real `NSOpenPanel` (never shown) and the test reads back its multiple-selection flag, message and allowed types; and `EditorView::simulate_panel_response` runs the real completion block on such a panel with a cancel. What they cannot show: the sheet or panel on screen and a user choosing a file, the tooltip bubble appearing after the pointer rests, the cursor during a real drag, and the field's look and caret. Those need a person at a display.

## The host's parameter menu and the track

Right-click on a parameter control (or Control-click, which is a right click on a Mac) asks the host for that parameter's context menu: automation, MIDI learn, whatever the host adds. All of it is additive and defaulted, so an editor written before it behaves as it did.

- `EditorHost::context_menu(param_index, x, y) -> bool` asks the format's host. `x` and `y` are editor points, origin top left. True means the host showed a menu; false means it has none, cannot show one now, or declined. A host with no handler (a preview, a test, `EditorHost::detached()`) answers false. The format bridge installs the handler with `EditorHost::with_context_menu`.
- `Editor::right_mouse_down(x, y) -> bool` is what the view calls for a right click. Return true when the click was on a parameter control (the view then does not pass it to the host), false otherwise. `ControlSet` implements it as `context_click`, so `DesignEditor` and `examples/editor` have it: on a control it asks the host, and when the host shows no menu it queues the SDK's own. A click on the bare plate, or while the pointer holds a control, returns false.
- The SDK's own menu is `Editor::take_param_menu() -> Option<ParamMenu>` (polled after every input call, like the text field; `ParamMenu` has the parameter, the click point and the parameter's name) and `Editor::param_menu_action(param, ParamMenuAction) -> Option<String>`. The Cocoa view pops up an `NSMenu` with four items, `openplugin_dri::cocoa::PARAM_MENU_ITEMS`: **Reset to default** (one whole begin, perform, end gesture), **Copy value** (the formatted value, `ParamInfo::format`, to the general pasteboard), **Paste value** (the pasteboard text through `ParamInfo::parse`, clamped, one whole gesture; text that is not a value changes nothing) and **Type value** (opens the text field of the previous section over the control). Reset, paste and type are for interactive controls only.

Per format:

| Format | The host's menu |
|---|---|
| CLAP | `clap.context-menu/1` (and the draft id `clap.context-menu.draft/0`). The editor's host calls `clap_host_context_menu::can_popup` and then `popup` with a `CLAP_CONTEXT_MENU_TARGET_KIND_PARAM` target (the parameter id is its descriptor index), screen 0, and the point relative to the plugin's window. The plugin side of the extension is offered with no items of its own (`populate` succeeds, `perform` does nothing) |
| VST3 | `IComponentHandler3::createContextMenu(view, &paramID)`, then `IContextMenu::popup(x, y)`, from the editor view. The shell reaches it through `op_editor_set_context_menu` (below); no items are added to the host's menu |
| LV2, Audio Unit, AAX | The format has no such menu to ask (the Audio Unit and AAX shells do not set the callback, LV2 has no host call), so the SDK's own menu shows |

The shell ABI addition is in `native/include/op_editor.h`, in an `OP_EDITOR_MENU` block (a shell that does not define it builds as before, and `OpEditorHost` is unchanged): `op_editor_set_context_menu(editor, context, callback)` with `uint32_t (*)(void* context, uint32_t index, int32_t x, int32_t y)` returning non-zero when the host showed a menu, and `op_editor_set_track_info(editor, name_utf8, rgba, flags)`.

**Track name and colour.** The host's track reaches the plugin and the editor as a `TrackInfo { name: Option<String>, colour: Option<u32>, kind: u32 }` (`openplugin_core`). The name is `None` when the host gave none or an empty one; the colour is `0xRRGGBBAA`, `None` when unknown (`TrackInfo::rgb()` drops the alpha, giving the editor's `0x00RRGGBB` pixel format); `kind` holds `TrackInfo::IS_RETURN`, `IS_BUS` and `IS_MASTER` (CLAP reports them; VST3's channel context has no such flags, so it is 0). `Plugin::track_info_changed(&TrackInfo)` and `Editor::track_info(&TrackInfo)` are both defaulted to nothing and both run on the main thread, never on the audio thread: the bridges take the plugin object between blocks, as for a state call (a block that starts meanwhile is passed through dry, and a call that cannot get the plugin in time gives up and delivers nothing). They are called when the host first knows (the CLAP entry reads it in `init`, and again when the editor is made; the VST3, Audio Unit and AAX shells when an engine exists, and when the view opens) and whenever the host changes it. A format gives what it has: CLAP all three, VST3 the name and the colour, the Audio Unit and AAX the name alone (`colour` is `None` and `kind` is 0), LV2 nothing. A call that carries the same information as the last is not repeated.

| Format | Where it comes from |
|---|---|
| CLAP | `clap.track-info/1` (or the draft id): the host's `get` is read at `init`, and again whenever the plugin's `changed` is called; name and colour only when the host flags them |
| VST3 | `Vst::ChannelContext::IInfoListener::setChannelContextInfos` on the edit controller: `kChannelNameKey` (UTF-16, to UTF-8) and `kChannelColorKey` (ARGB, to `0xRRGGBBAA`). A call that lists only one of them keeps the other. The controller sends it to the processor as an `IMessage` (the engine lives there), which calls `op_shell_set_track_info` once an engine exists and again for each new engine, and to an open editor with `op_editor_set_track_info`; what a host says before it connects the two halves is sent when it does |
| Audio Unit | `kAudioUnitProperty_ContextName`, the CFString a host sets on the global scope to name the track (Logic and GarageBand do). The name only: the Audio Unit API has no property for a track's colour or kind. The shell reads it on the thread that set the property and calls `op_shell_set_track_info` on the main thread, inside the host's call when that is the main thread and from its run-loop timer (30 ms) otherwise; a host that removes the property is seen by the same timer and reaches the plugin as no name. A new engine (`Initialize`) is told again, and the Cocoa view tells its editor when it opens and from its own timer |
| AAX | `AAX_eNotificationEvent_TrackNameChanged` (Pro Tools 11.2 and later; Media Composer does not send it), whose payload is an `AAX_IString`. The name only: the AAX SDK (checked through 2.10) has no notification or call for a track's colour or kind. AAX lets the host send a notification on any thread, so the data model keeps the name at once and calls `op_shell_set_track_info` on the main thread: inside the notification when it arrives there, and from the main dispatch queue otherwise (macOS; a delivery still waiting when the effect is destroyed is dropped). On Windows the call is made on the notification's thread, which is never the algorithm's (audio) thread. A new engine (`EffectInit`) is told what was said before it. The view reads the name from the data model when it opens and from the host's GUI timer, which AAX runs on the main thread, so `Editor::track_info` is on the main thread; a notification Pro Tools sends to the view is handed to the data model. On Windows the AAX view does not tell the editor yet |
| LV2 | Not delivered. No published LV2 extension carries a track's name or colour (the LV2 1.18.10 specifications were searched): `ui:foregroundColor` and `ui:backgroundColor` are the colours of the host's own interface, not of a track, and no host option or feature names the track. The entry reads nothing, and `track_info_changed` is never called |

The proofs: `cargo test -p openplugin-dri` (the control set's right click and the four items), `cargo test -p example-editor --test menu` (main thread; a right-click event and a Control-click event reach a host handler with the parameter and the point; with no host menu the SDK's menu request reaches a hook in place of the `NSMenu`, and the four items are run by calling the view's methods and its real `openpluginMenuItem:` action; the shell's editor ABI carries a menu callback and the track), the CLAP tests in `openplugin-targets` (a fake host with `context-menu` and `track-info`), `vst3_track_proof` (a fake host sending the channel context through `IInfoListener`), `au_track_proof` (a host that sets and removes `kAudioUnitProperty_ContextName`, from the main thread and from another, with a render thread running) and `aax_track_proof` (a host that sends `AAX_eNotificationEvent_TrackNameChanged` to the data model and to the view), both over a plugin that records `op_shell_set_track_info` and, with `-DOP_EDITOR=ON`, over an editor that records `op_editor_set_track_info` behind the shell's real view, each with a unit that is never named as the negative control, and `vst3_editor_proof` (a fake `IComponentHandler3`). What none of them show: the `NSMenu` on screen and a menu choice by a person, the pasteboard round trip (the tests pass the text in so the user's clipboard is never touched), a real DAW's automation or MIDI-learn menu, and a track named by Logic or Pro Tools.

## Repaint

The Cocoa view used to redraw the whole frame 30 times a second. One optional method now lets an editor say what changed:

```rust
fn damage(&mut self) -> Option<Vec<DamageRect>> { None }   // the default
```

`None` means everything, which is the default, so an editor written before it is redrawn in full as it always was. `Some(rects)` is what changed since the last call, in points with the origin at the top left; `Some(vec![])` means nothing. The rectangles must cover every pixel that differs from the frame `render` last returned, anti-aliased edges, shadows and glows included.

The view calls `damage` after each tick and after each mouse or key call. It invalidates only those rectangles (`setNeedsDisplayInRect:`, a point larger on each side), and when the list is empty it invalidates nothing, so an idle editor costs the timer tick. A view that does not ask (a test, the snapshot tool, a host that calls `render` itself) still gets correct frames: `render` collects whatever is pending on its own, and the editor clears it once a frame has drawn it. A new size, scale or static layer makes the canvas paint the whole frame whatever the list says. When the window moves to a display of another scale the view invalidates the whole view.

`DamageTracker` keeps the pending list for an editor: `add(rect)`, `add_all()` after a resize, `pending()` for `render` and `damage`, `clear()` after a frame. It starts as "everything", so the first frame is full, and more than 16 rectangles become "everything".

`examples/editor` and `DesignEditor` report the area of a control when its value, hover, pressed or focus state changes (`ControlSet::take_changed`), the focused control when the view gains or loses the keyboard, and the meter only when its picture changes: the 16-segment ladder shows 16 lit-or-not states, so a level change that lights the same segments draws nothing. `SceneCanvas::frame_damaged` does the painting ([DRI_GUIDE.md](DRI_GUIDE.md#repainting-only-what-changed)).

## Per format

| Format | Gestures | Values back to the editor | Engine binding |
|---|---|---|---|
| CLAP | The value goes into the parameter plane at once. Begin, value and end events go to the host from the next `process` or `flush`, after `request_flush` | The view polls the plane on its timer | The instance's `editor_state` |
| LV2 | Control port writes, with `ui:touch` grabs for begin and end | `port_event` | `instance-access`, checked against this binary's instances |
| VST3, AU, AAX | `OpEditorHost` callbacks into the C++ shell, which calls the host | `op_editor_set_plain` from the shell | `op_editor_bind`, checked against the live-instance table |

The host's parameter menu and the track have their own table above.

### Resizing per format

| Format | Host resizes the editor | Editor asks the host | Description |
|---|---|---|---|
| CLAP | `can_resize` is true unless the range is fixed. `get_resize_hints` gives the axes and the aspect. `adjust_size` clamps in place. `set_size` accepts any size inside the bounds (the aspect is a hint) and refuses others, a fixed editor accepting only its own size | `clap_host_gui::request_resize`, looked up from the host when the GUI is created. A host that answers `true` calls `set_size`, possibly inside the request | |
| LV2 | the UI offers `ui:resize` through `extension_data`. `ui_resize(handle, w, h)` returns 0 for a size inside the bounds and non-zero otherwise | the host's `ui:resize` feature. When it returns 0 the editor takes the size on the view's next tick | `lv2:extensionData ui:resize` for a resizable editor; `ui:noUserResize` only for a fixed one. `lv2::write_bundle_for_editor` asks the editor, and `plugin_ttl_sized` writes either form |
| VST3, AU, AAX | `op_editor_resize`, `op_editor_adjust_size`, `op_editor_get_size` and `op_editor_size_range` in the `OP_EDITOR_RESIZE` block of `op_editor.h`. The Rust side exports them, and the VST3 (`IPlugView` resizing, `IPlugFrame::resizeView`), Audio Unit (the view follows its frame) and AAX (`GetViewSize`, `AAX_IViewContainer::SetViewSize`) shells use them with `-DOP_EDITOR=ON` | `op_editor_set_request_resize(editor, context, callback)`. `OpEditorHost` is unchanged, so existing shells build as before | |

## Why the shells are C++

The VST3, Audio Unit and AAX entry points stay C++ (`native/`):

- AAX has no C interface. Its SDK is C++ classes that a plugin subclasses, so the shell has to be C++.
- The VST3 interfaces are C++ COM-style classes. They could be laid out by hand in Rust, but the shell is already proven and the SDK headers are the reference.
- The Audio Unit shell builds on Apple's AudioUnitSDK, which is C++.

Everything those shells call is Rust: the audio contract (`op_shell_*`), the tail, the AAX IDs, and the editor (`op_editor_*`, written by `export_shell!` over `EditorView`). The only Objective-C++ file added is the `VST3EditorProof` test host, because it must call the C++ VST3 interfaces.

## Tests

- `cargo test -p example-editor`: a test with no harness that runs on the main thread. It opens the editor through the shell ABI, CLAP and LV2. It clicks it, checks the gestures, and reads pixels back from the view. It then resizes the editor through each of the three (`op_editor_resize`, CLAP `adjust_size` and `set_size`, LV2 `ui:resize`), and checks that the view's size, the drawn pixels and the click positions follow: a click at the old position of the Warm switch hits nothing, a click at the scaled one does. It also has the editor ask for a size (a double-click on the bare plate) through `op_editor_set_request_resize`, `clap_host_gui::request_resize` (the host calls `set_size` inside the request) and the host's `ui:resize`. The editor's own tests (`cargo test -p example-editor --lib`) drive a session through two editors fed the same events, one repainting only its damage and one repainting everything, at scales 1, 1.5 and 2, and compare every frame pixel for pixel; `cargo test -p openplugin-dri` does the same for `SceneCanvas` and `DesignEditor`. A last case sends real `NSEvent`s to the shell view with a recording responder standing in for the host: modifier flags from mouse events and `flagsChanged:` (a Shift arrow steps a tenth as far), a handled arrow (a whole gesture, nothing for the next responder), unhandled keys and the up of a key the editor did not take (both reach the next responder), Tab, Space and Escape, and the focus ring in the pixels. `flagsChanged:` is exercised with a key event that carries the flags, because `NSEvent` cannot build a flags-changed event directly. No window is used, so first-responder handoff in a real window is not exercised.
- `cargo test -p openplugin-core`: the clamping rules of `SizeRange` (bounds, aspect, a result clamps to itself).
- `VST3EditorProof` (`native/vst3/editor_host.mm`, built with `-DOP_EDITOR=ON`): loads the VST3 bundle in process and opens the view. It re-creates the engine with the view open. If `OP_EDITOR_PROOF_CLICK` is `x;y;param`, it clicks there and checks the paired edits.

- `cargo test -p example-editor --test a11y` and `cargo test -p example-design-editor --test a11y`: the accessibility tree and its actions, as in the Accessibility section.
- `cargo test -p example-editor --test menu`: the parameter menu and the track, as in their section.
- `cargo test -p example-editor --test typing` and `cargo test -p example-design-editor --test typing`: the text field, tooltips, dropped files and the file dialog, as in their section.

None of these is a session in Logic, Cubase, Pro Tools, Bitwig or Reaper.

## Hit testing

An editor does not have to hand-code hit rectangles. `openplugin_dri::ControlSet` holds knobs, faders, toggles, momentary buttons and key banks, each with a hit area, a parameter index and its `ParamInfo`. The editor forwards `mouse_down`, `mouse_move`, `mouse_up`, `wheel`, `modifiers_changed` and `param_changed` to it. The set turns the mouse into `begin`, `perform` and `end` gestures through the `EditorHost`: relative drags, a fine adjustment with Shift or Command, double-click to type a value (Option-double-click to reset; configurable), wheel steps, snapping for stepped and choice parameters, and a clamp so that only in-range values reach the host. A value from the host updates a control without sending a gesture back. The set also keeps hover and pressed state, and iterating it gives what to draw. `examples/editor` is written this way; see [DRI_GUIDE.md](DRI_GUIDE.md#controls-and-hit-testing) for the rules.

`Editor::modifiers_changed(Modifiers)` is optional and empty by default. The macOS view calls it (see below), so Shift or Command gives the fine adjustment on macOS.

`ControlSet` handles the keys for an editor: it keeps one focused control, moves focus with Tab, and turns the keys into gestures. `examples/editor` and `DesignEditor` forward `wants_keys`, `key_down`, `key_up` and `focus_changed` to it and draw a ring around the focused control. See [DRI_GUIDE.md](DRI_GUIDE.md#keyboard-and-focus-in-the-registry).

## Editors from a design file

An editor does not have to be written by hand. `openplugin_dri::DesignEditor` implements `Editor` from a GoodLookin design CSV and the plugin's `PluginInfo`: each row is a control bound to the parameter named in its `parameter` column, so the layout lives in a text file and the plugin has no drawing or hit-testing code.

```rust
fn make_editor(host: EditorHost) -> Option<Box<dyn Editor>> {
    DesignEditor::from_csv(include_str!("../design.csv"), &INFO, host).map(|e| Box::new(e) as Box<dyn Editor>)
}

openplugin::targets::export_clap!(MyPlugin, editor = make_editor);
```

It goes through the same contract as any editor: gestures through `EditorHost`, host values through `param_changed`, pixels from `SceneCanvas` with the plate cached. It works in every format that takes an `EditorFactory`. A design with a malformed row makes `from_csv` return `None`, and the host shows its generic controls. Rows that name no parameter, or one the plugin does not have, are listed by `issues()`. The kinds of control, the options and the rules are in [DRI_GUIDE.md](DRI_GUIDE.md#editors-from-a-design-file). The working example is [`examples/design_editor`](../examples/design_editor), exported as CLAP; its tests load the CSV, check that every parameter has a control, and render a frame. It has not been opened in a host.
